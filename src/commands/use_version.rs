//! `rnm use`: switch this shell session to another version.

use std::io::Write;
use std::path::Path;

use super::install::{InstallOptions, Progress, install};
use super::{Context, DEFAULT_ALIAS, Failure, SYSTEM_LABEL, exit, npm_version, say};
use crate::multishell::{Session, Target};
use crate::nvm_dir::{InstalledVersion, ResolveError, Resolved};
use crate::shell::{Inferred, ShellKind};
use crate::version::{LTS_PREFIX, VersionFile, VersionFileKind, VersionSpec, find_version_file};

/// Options for [`use_version`].
#[derive(Clone, Debug, Default)]
#[allow(
  clippy::struct_excessive_bools,
  reason = "each bool is one command-line flag"
)]
pub struct UseOptions {
  /// The version spec. When `None`, the nearest version file is used.
  pub version: Option<String>,
  /// `--lts[=name]`: an LTS codename, or [`crate::version::LATEST_LTS`] for
  /// the newest LTS.
  pub lts: Option<String>,
  /// `--silent`: print nothing on success.
  pub silent: bool,
  /// `--silent-if-unchanged`: print nothing when already on that version.
  pub silent_if_unchanged: bool,
  /// `--save`: write the resolved version to `./.nvmrc`.
  pub save: bool,
  /// `--from-hook`: run by the `--use-on-cd` hook after a directory change.
  /// Implies `silent_if_unchanged`. Without a version file, switches to the
  /// `default` alias, or does nothing when there is none.
  pub from_hook: bool,
  /// `--install-if-missing`: install the version when it is not installed.
  pub install_if_missing: bool,
  /// Download progress output for `install_if_missing`.
  pub progress: Progress,
}

impl UseOptions {
  /// The options of the `--use-on-cd` hook.
  #[must_use]
  pub fn hook(install_if_missing: bool, progress: Progress) -> Self {
    Self {
      from_hook: true,
      install_if_missing,
      progress,
      ..Self::default()
    }
  }
}

/// Re-points this session's link at the requested version.
///
/// # Errors
///
/// Fails with nvm's exit codes: 3 when the version is not installed, 8 for
/// an alias loop, 127 when no version is given or found, and 1 when rnm is
/// not set up in this shell. With `install_if_missing`, also the exit codes
/// of [`install`].
pub fn use_version(
  ctx: &Context,
  opts: &UseOptions,
  out: &mut dyn Write,
  err: &mut dyn Write,
) -> Result<(), Failure> {
  let Some(spec) = requested_spec(ctx, opts, out)? else {
    return Ok(());
  };
  let session = ctx.session.as_ref().ok_or_else(not_set_up)?;
  switch_session(ctx, session, &spec, opts, out, err)
}

/// The version to switch to: the explicit one, or the nearest version
/// file's. `None` when a hook finds neither a version file nor a `default`
/// alias, so there is nothing to do.
pub(crate) fn requested_spec(
  ctx: &Context,
  opts: &UseOptions,
  out: &mut dyn Write,
) -> Result<Option<VersionSpec>, Failure> {
  let spec_text = match (&opts.lts, &opts.version) {
    | (Some(lts), _) => format!("{LTS_PREFIX}{lts}"),
    | (None, Some(version)) => version.clone(),
    | (None, None) if opts.from_hook => match find_version_file(&ctx.cwd, ctx.version_file_kinds())
    {
      | Some((path, kind)) => load_version_file(&path, kind)?.version,
      | None if has_default_alias(ctx) => DEFAULT_ALIAS.to_owned(),
      | None => return Ok(None),
    },
    | (None, None) => {
      let file = version_file(ctx)?;
      if !opts.silent && !opts.silent_if_unchanged {
        say(
          out,
          format_args!(
            "Found '{}' with version <{}>",
            file.path.display(),
            file.version
          ),
        );
      }
      file.version
    },
  };
  spec_text
    .parse()
    .map(Some)
    .map_err(|e| Failure::new(exit::NOT_AVAILABLE, format!("{e}")))
}

fn has_default_alias(ctx: &Context) -> bool {
  ctx
    .nvm_dir
    .aliases()
    .get(DEFAULT_ALIAS)
    .ok()
    .flatten()
    .is_some()
}

/// Re-points `session` at `spec` (installing it first with
/// `install_if_missing`) and prints the result.
pub(crate) fn switch_session(
  ctx: &Context,
  session: &Session,
  spec: &VersionSpec,
  opts: &UseOptions,
  out: &mut dyn Write,
  err: &mut dyn Write,
) -> Result<(), Failure> {
  let resolved = match ctx.nvm_dir.resolve(spec) {
    | Err(ResolveError::NotInstalled(_)) if opts.install_if_missing => {
      let install_opts = InstallOptions {
        version: Some(spec.to_string()),
        no_use: true,
        progress: opts.progress,
        ..InstallOptions::default()
      };
      install(ctx, &install_opts, out, err)?;
      ctx.nvm_dir.resolve(spec)
    },
    | resolved => resolved,
  }
  .map_err(|e| resolve_failure(spec, &e))?;
  let current = session.target()?;

  let switched = match resolved {
    | Resolved::Installed(installed) => {
      let unchanged = current == Some(Target::Version(installed.path.clone()));
      session.point_to(Some(&installed.path))?;
      update_current_symlink(ctx, &installed.path);
      Some(Switched {
        unchanged,
        message: now_using(&installed),
        saved: installed.to_string(),
      })
    },
    | Resolved::System => {
      if ctx.system_node().is_none() {
        return Err(Failure::new(
          exit::NO_VERSION,
          "System version of node not found.",
        ));
      }
      let unchanged = current == Some(Target::System);
      session.point_to(None)?;
      Some(Switched {
        unchanged,
        message: "Now using system version of node.".to_owned(),
        saved: SYSTEM_LABEL.to_owned(),
      })
    },
    | Resolved::Current => None,
  };

  let Some(switched) = switched else {
    return Ok(());
  };
  let silent_if_unchanged = opts.silent_if_unchanged || opts.from_hook;
  let quiet = opts.silent || (silent_if_unchanged && switched.unchanged);
  if !quiet {
    say(out, &switched.message);
  }
  if opts.save {
    std::fs::write(
      ctx.cwd.join(VersionFileKind::Nvmrc.file_name()),
      format!("{}\n", switched.saved),
    )?;
  }
  Ok(())
}

/// The outcome of re-pointing the session link.
struct Switched {
  /// The link already pointed there.
  unchanged: bool,
  message: String,
  /// What `--save` writes to `.nvmrc`.
  saved: String,
}

/// The nearest version file, parsed.
pub(crate) fn version_file(ctx: &Context) -> Result<VersionFile, Failure> {
  let kinds = ctx.version_file_kinds();
  let (path, kind) = find_version_file(&ctx.cwd, kinds).ok_or_else(|| {
    let names: Vec<&str> = kinds.iter().map(|kind| kind.file_name()).collect();
    Failure::new(
      exit::NO_VERSION,
      format!(
        "No version provided and no {} file found",
        names.join(" or ")
      ),
    )
  })?;
  load_version_file(&path, kind)
}

fn load_version_file(path: &Path, kind: VersionFileKind) -> Result<VersionFile, Failure> {
  VersionFile::load(path, kind)
    .map_err(|e| Failure::new(exit::ERROR, format!("invalid {}: {e}", path.display())))
}

fn now_using(installed: &InstalledVersion) -> String {
  match npm_version(&installed.path) {
    | Some(npm) => format!("Now using node {installed} (npm v{npm})"),
    | None => format!("Now using node {installed}"),
  }
}

pub(crate) fn resolve_failure(spec: &VersionSpec, e: &ResolveError) -> Failure {
  match e {
    | ResolveError::NotInstalled(_) => Failure::new(
      exit::NOT_AVAILABLE,
      format!(
        "N/A: version \"{spec}\" is not yet installed.\n\n\
         You need to run `rnm install {spec}` to install it before using it."
      ),
    ),
    | ResolveError::AliasCycle(_) => Failure::new(exit::ALIAS_LOOP, e.to_string()),
    | _ => Failure::new(exit::NOT_AVAILABLE, e.to_string()),
  }
}

fn not_set_up() -> Failure {
  let hint = match ShellKind::detect() {
    | Inferred::Supported(shell) => format!("\n\n  {}", shell.setup_hint()),
    | _ => ShellKind::ALL
      .iter()
      .fold(String::new(), |mut hint, shell| {
        hint.push_str("\n  ");
        hint.push_str(shell.setup_hint());
        hint
      }),
  };
  Failure::new(
    exit::ERROR,
    format!(
      "rnm is not set up in this shell. Add this to your shell's startup file and restart it:{hint}"
    ),
  )
}

/// nvm's `NVM_SYMLINK_CURRENT=true`: keep `$NVM_DIR/current` pointing at the
/// active version. Unix only; failures are ignored, as in nvm.
fn update_current_symlink(ctx: &Context, version_dir: &Path) {
  #[cfg(unix)]
  if ctx.var("NVM_SYMLINK_CURRENT").is_some_and(|v| v == "true") {
    let _ = crate::fs::replace_symlink(version_dir, &ctx.nvm_dir.root().join("current"));
  }
  #[cfg(not(unix))]
  let _ = (ctx, version_dir);
}
