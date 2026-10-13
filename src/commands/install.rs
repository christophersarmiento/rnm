//! `rnm install`: download and install a version, like `nvm install`.

use std::io::Write;

use super::use_version::version_file;
use super::{
  Context, DEFAULT_ALIAS, Failure, Remote, UseOptions, exit, remote_index, say, use_version,
};
use crate::install::{Event, Installer, LockSettings, cached_versions, install_from_cache};
use crate::nvm_dir::{Flavor, InstalledVersion, Resolved};
use crate::platform::Platform;
use crate::remote::{Release, RemoteResolveError};
use crate::version::{LTS_PREFIX, VersionFileKind, VersionReq, VersionSpec};

/// The width of the download progress bar, as curl's `--progress-bar`.
const PROGRESS_BAR_WIDTH: u64 = 72;

/// Progress is redrawn in steps of 0.1 %.
const PER_MILLE: u64 = 1000;

/// Options for [`install`].
#[derive(Clone, Debug, Default)]
#[allow(
  clippy::struct_excessive_bools,
  reason = "each bool is one command-line flag"
)]
pub struct InstallOptions {
  /// The version spec. When `None` (and no `--lts`), the nearest version
  /// file is used.
  pub version: Option<String>,
  /// `--lts[=name]`.
  pub lts: Option<String>,
  /// `--default`: point the `default` alias at the requested spec.
  pub default: bool,
  /// `--alias=<name>`: point this alias at the requested spec.
  pub alias: Option<String>,
  /// `--save`: write the installed version to `./.nvmrc`.
  pub save: bool,
  /// `--no-use`: do not switch this shell to the installed version.
  pub no_use: bool,
  /// Whether to draw a download progress bar. `NVM_NO_PROGRESS` turns it
  /// off as well.
  pub progress: Progress,
  /// `--offline`: choose among installed and cached versions, and install
  /// from the download cache without network access.
  pub offline: bool,
}

/// Download progress output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Progress {
  /// A bar that redraws in place; only for terminals.
  Bar,
  /// Status lines only (`--no-progress`, or not a terminal).
  #[default]
  Hidden,
}

/// Installs a version from the mirror, then (like nvm) switches this shell
/// to it, creates the `default` alias if there is none, and applies
/// `--default`, `--alias`, and `--save`.
///
/// An exact version that is already installed is used without contacting
/// the mirror.
///
/// # Errors
///
/// Exit code 3 when the version does not exist on the mirror, 127 when no
/// version is given or found, and 1 for download, verification, or file
/// system failures.
pub fn install(
  ctx: &Context,
  opts: &InstallOptions,
  out: &mut dyn Write,
  err: &mut dyn Write,
) -> Result<(), Failure> {
  let requested = requested_spec(ctx, opts, out)?;
  let spec: VersionSpec = requested.parse().map_err(|_| not_found(&requested))?;

  let installed = match installed_exact_version(ctx, &spec) {
    | Some(installed) => {
      say(err, format_args!("{installed} is already installed."));
      installed
    },
    | None if opts.offline => install_offline(ctx, &requested, &spec, out, err)?,
    | None => {
      let remote = remote_index(ctx, err)?;
      let release = remote
        .index
        .resolve(&spec)
        .map_err(|e| remote_failure(&requested, &e))?;
      let version_dir = ctx.nvm_dir.version_dir(Flavor::Node, &release.version);
      if let Some(installed) = ctx.nvm_dir.installed_at(&version_dir) {
        say(err, format_args!("{installed} is already installed."));
        installed
      } else {
        say(
          out,
          format_args!("Downloading and installing node {}...", release.version),
        );
        download(ctx, opts, &remote, release, err)?
      }
    },
  };
  finish(ctx, opts, &requested, &installed, out, err)
}

/// The spec as the user gave it: `--lts`, the argument, or the version file.
/// nvm stores this text, not the resolved version, in new aliases.
fn requested_spec(
  ctx: &Context,
  opts: &InstallOptions,
  out: &mut dyn Write,
) -> Result<String, Failure> {
  if let Some(lts) = &opts.lts {
    return Ok(format!("{LTS_PREFIX}{lts}"));
  }
  if let Some(version) = &opts.version {
    return Ok(version.clone());
  }
  let file = version_file(ctx)?;
  say(
    out,
    format_args!(
      "Found '{}' with version <{}>",
      file.path.display(),
      file.version
    ),
  );
  Ok(file.version)
}

/// nvm's `--offline`: the newest installed or cached version that matches,
/// installed from the cache if needed. Aliases such as `lts/*` resolve
/// through their alias files, as they do for `use`.
fn install_offline(
  ctx: &Context,
  requested: &str,
  spec: &VersionSpec,
  out: &mut dyn Write,
  err: &mut dyn Write,
) -> Result<InstalledVersion, Failure> {
  let failure = |e: &dyn std::fmt::Display| Failure::new(exit::ERROR, e.to_string());
  let platform = Platform::detect().map_err(|e| failure(&e))?;
  let mut candidates = ctx.nvm_dir.installed_versions()?;
  for version in cached_versions(&ctx.nvm_dir, &platform) {
    let already_listed = candidates
      .iter()
      .any(|c| c.flavor == Flavor::Node && c.version == version);
    if !already_listed {
      candidates.push(InstalledVersion {
        flavor: Flavor::Node,
        path: ctx.nvm_dir.version_dir(Flavor::Node, &version),
        version,
      });
    }
  }
  candidates.sort_by(|a, b| a.version.cmp(&b.version));
  let not_available = || {
    Failure::new(
      exit::NOT_AVAILABLE,
      format!(
        "Version '{requested}' is neither installed nor in the download cache; \
         run without --offline to download it."
      ),
    )
  };
  let Ok(Resolved::Installed(chosen)) = ctx.nvm_dir.resolve_among(spec, &candidates) else {
    return Err(not_available());
  };
  if let Some(installed) = ctx.nvm_dir.installed_at(&chosen.path) {
    say(err, format_args!("{installed} is already installed."));
    return Ok(installed);
  }
  say(
    out,
    format_args!(
      "Installing node {} from the download cache...",
      chosen.version
    ),
  );
  let mut reporter = Reporter {
    err,
    version: chosen.version.to_string(),
    draws_progress: false,
    drawn_per_mille: None,
  };
  install_from_cache(
    &ctx.nvm_dir,
    &platform,
    &LockSettings::from_env(|key| ctx.var_string(key)),
    &chosen.version,
    &mut |event| reporter.report(&event),
  )
  .map_err(|e| failure(&e))
}

fn installed_exact_version(ctx: &Context, spec: &VersionSpec) -> Option<InstalledVersion> {
  let VersionSpec::Version(VersionReq::Exact(version)) = spec else {
    return None;
  };
  ctx
    .nvm_dir
    .installed_at(&ctx.nvm_dir.version_dir(Flavor::Node, version))
}

fn download(
  ctx: &Context,
  opts: &InstallOptions,
  remote: &Remote,
  release: &Release,
  err: &mut dyn Write,
) -> Result<InstalledVersion, Failure> {
  let failure = |e: &dyn std::fmt::Display| Failure::new(exit::ERROR, e.to_string());
  let platform = Platform::detect().map_err(|e| failure(&e))?;
  let installer = Installer {
    nvm_dir: &ctx.nvm_dir,
    mirrors: remote.download_mirrors(),
    client: &remote.network.client,
    platform: &platform,
    lock: LockSettings::from_env(|key| ctx.var_string(key)),
  };
  let draws_progress = opts.progress == Progress::Bar && ctx.var("NVM_NO_PROGRESS").is_none();
  let mut reporter = Reporter {
    err,
    version: release.version.to_string(),
    draws_progress,
    drawn_per_mille: None,
  };
  installer
    .install(release, &mut |event| reporter.report(&event))
    .map_err(|e| failure(&e))
}

/// Prints install events to stderr, as nvm does.
struct Reporter<'a> {
  err: &'a mut dyn Write,
  version: String,
  draws_progress: bool,
  /// The progress last drawn, so the bar is redrawn only when it changes.
  drawn_per_mille: Option<u64>,
}

impl Reporter<'_> {
  fn report(&mut self, event: &Event<'_>) {
    match event {
      | Event::WaitingForLock => say(
        self.err,
        format_args!(
          "Waiting for another install of {} to finish...",
          self.version
        ),
      ),
      | Event::Downloading { url, .. } => {
        say(self.err, format_args!("Downloading {url}..."));
        self.drawn_per_mille = None;
      },
      | Event::Progress {
        received,
        size: Some(size),
      } if self.draws_progress && *size > 0 => self.draw(*received, *size),
      | Event::Progress { .. } => {},
      | Event::UsingCached { path } => say(
        self.err,
        format_args!(
          "Using the verified archive in the cache: {}",
          path.display()
        ),
      ),
      | Event::ChecksumMatched => say(self.err, "Checksums matched!"),
      | Event::TryingNextMirror { error } => {
        say(
          self.err,
          format_args!("warning: {error}; trying the next mirror"),
        );
      },
      | Event::UsingCachedOffline { path } => say(
        self.err,
        format_args!(
          "Using the archive in the cache (verified when it was downloaded): {}",
          path.display()
        ),
      ),
    }
  }

  /// Draws `####…   42.0%` in place, ending the line at 100 %.
  fn draw(&mut self, received: u64, size: u64) {
    let per_mille = received.min(size) * PER_MILLE / size;
    if self.drawn_per_mille == Some(per_mille) {
      return;
    }
    self.drawn_per_mille = Some(per_mille);
    let filled = usize::try_from(per_mille * PROGRESS_BAR_WIDTH / PER_MILLE).unwrap_or(0);
    let width = usize::try_from(PROGRESS_BAR_WIDTH).unwrap_or(0);
    let _ = write!(
      self.err,
      "\r{:<width$} {:>3}.{}%",
      "#".repeat(filled),
      per_mille / 10,
      per_mille % 10
    );
    if per_mille == PER_MILLE {
      let _ = writeln!(self.err);
    }
  }
}

/// The steps after the version is on disk.
fn finish(
  ctx: &Context,
  opts: &InstallOptions,
  requested: &str,
  installed: &InstalledVersion,
  out: &mut dyn Write,
  err: &mut dyn Write,
) -> Result<(), Failure> {
  if ctx.session.is_some() && !opts.no_use {
    let use_opts = UseOptions {
      version: Some(installed.to_string()),
      ..UseOptions::default()
    };
    use_version(ctx, &use_opts, out, err)?;
  }

  let aliases = ctx.nvm_dir.aliases();
  let has_default = aliases.get(DEFAULT_ALIAS)?.is_some();
  if !has_default {
    aliases.set(DEFAULT_ALIAS, requested)?;
    say(
      out,
      format_args!(
        "Creating default alias: {}",
        alias_line(DEFAULT_ALIAS, requested, installed)
      ),
    );
  }
  let requested_alias = if opts.default {
    Some(DEFAULT_ALIAS)
  } else {
    opts.alias.as_deref()
  };
  if let Some(name) = requested_alias
    && (has_default || name != DEFAULT_ALIAS)
  {
    aliases.set(name, requested)?;
    say(out, alias_line(name, requested, installed));
  }

  if opts.save {
    std::fs::write(
      ctx.cwd.join(VersionFileKind::Nvmrc.file_name()),
      format!("{installed}\n"),
    )?;
  }
  Ok(())
}

/// nvm's alias display: `default -> 20 (-> v20.11.1)`, or
/// `default -> v20.11.1` when the target is the version itself.
fn alias_line(name: &str, target: &str, installed: &InstalledVersion) -> String {
  let version = installed.to_string();
  if target == version {
    format!("{name} -> {target}")
  } else {
    format!("{name} -> {target} (-> {version})")
  }
}

fn not_found(requested: &str) -> Failure {
  Failure::new(
    exit::NOT_AVAILABLE,
    format!("Version '{requested}' not found - try `rnm ls-remote` to browse available versions."),
  )
}

fn remote_failure(requested: &str, e: &RemoteResolveError) -> Failure {
  match e {
    | RemoteResolveError::NotFound(_) => not_found(requested),
    | _ => Failure::new(exit::NOT_AVAILABLE, e.to_string()),
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::path::PathBuf;

  fn installed(version: &str) -> InstalledVersion {
    InstalledVersion {
      flavor: Flavor::Node,
      version: version.parse().unwrap(),
      path: PathBuf::from("/x"),
    }
  }

  #[test]
  fn formats_aliases_like_nvm() {
    let v = installed("20.11.1");
    assert_eq!(
      alias_line("default", "20", &v),
      "default -> 20 (-> v20.11.1)"
    );
    assert_eq!(alias_line("default", "v20.11.1", &v), "default -> v20.11.1");
  }

  #[test]
  fn draws_progress_bar() {
    let mut err = Vec::new();
    let mut reporter = Reporter {
      err: &mut err,
      version: "v20.0.0".to_owned(),
      draws_progress: true,
      drawn_per_mille: None,
    };
    reporter.draw(1, 2);
    reporter.draw(1, 2);
    reporter.draw(2, 2);
    let text = String::from_utf8(err).unwrap();
    let frames: Vec<&str> = text.split('\r').filter(|s| !s.is_empty()).collect();
    assert_eq!(frames.len(), 2);
    assert_eq!(
      frames[0],
      format!("{}{}  50.0%", "#".repeat(36), " ".repeat(36))
    );
    assert_eq!(frames[1], format!("{} 100.0%\n", "#".repeat(72)));
  }
}
