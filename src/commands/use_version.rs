//! `rnm use`: switch this shell session to another version.

use std::io::Write;
use std::path::Path;

use super::{Context, Failure, exit, npm_version, say};
use crate::multishell::Target;
use crate::nvm_dir::{InstalledVersion, ResolveError, Resolved};
use crate::shell::{Inferred, ShellKind};
use crate::sys;
use crate::version::{VersionFile, VersionFileKind, VersionSpec, find_version_file};

/// Options for [`use_version`].
#[derive(Clone, Debug, Default)]
pub struct UseOptions {
  /// The version spec. When `None`, the nearest version file is used.
  pub version: Option<String>,
  /// `--lts[=name]`: `Some("*")` for the newest LTS.
  pub lts: Option<String>,
  /// `--silent`: print nothing on success.
  pub silent: bool,
  /// `--silent-if-unchanged`: print nothing when already on that version.
  pub silent_if_unchanged: bool,
  /// `--save`: write the resolved version to `./.nvmrc`.
  pub save: bool,
}

/// Re-points this session's link at the requested version.
///
/// # Errors
///
/// Fails with nvm's exit codes: 3 when the version is not installed, 8 for
/// an alias loop, 127 when no version is given or found, and 1 when rnm is
/// not set up in this shell.
pub fn use_version(ctx: &Context, opts: &UseOptions, out: &mut dyn Write) -> Result<(), Failure> {
  let spec_text = match (&opts.lts, &opts.version) {
    | (Some(lts), _) => format!("lts/{lts}"),
    | (None, Some(version)) => version.clone(),
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
  let spec: VersionSpec = spec_text
    .parse()
    .map_err(|e| Failure::new(exit::NOT_AVAILABLE, format!("{e}")))?;

  let session = ctx.session.as_ref().ok_or_else(not_set_up)?;
  let resolved = ctx
    .nvm_dir
    .resolve(&spec)
    .map_err(|e| resolve_failure(&spec, &e))?;
  let current = session.target()?;

  let (message, saved) = match resolved {
    | Resolved::Installed(installed) => {
      let unchanged = current == Some(Target::Version(installed.path.clone()));
      session.point_to(Some(&installed.path))?;
      update_current_symlink(ctx, &installed.path);
      let message = (!(opts.silent_if_unchanged && unchanged)).then(|| now_using(&installed));
      (message, installed.to_string())
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
      let message = (!(opts.silent_if_unchanged && unchanged))
        .then(|| "Now using system version of node.".to_owned());
      (message, "system".to_owned())
    },
    | Resolved::Current => (None, String::new()),
  };

  if let Some(message) = message
    && !opts.silent
  {
    say(out, message);
  }
  if opts.save && !saved.is_empty() {
    std::fs::write(ctx.cwd.join(".nvmrc"), format!("{saved}\n"))?;
  }
  Ok(())
}

/// The nearest `.nvmrc` or `.node-version`, parsed.
fn version_file(ctx: &Context) -> Result<VersionFile, Failure> {
  let (path, kind) = find_version_file(&ctx.cwd, VersionFileKind::DEFAULT).ok_or_else(|| {
    Failure::new(
      exit::NO_VERSION,
      "No version provided and no .nvmrc or .node-version file found",
    )
  })?;
  VersionFile::load(&path, kind)
    .map_err(|e| Failure::new(exit::ERROR, format!("invalid {}: {e}", path.display())))
}

fn now_using(installed: &InstalledVersion) -> String {
  match npm_version(&installed.path) {
    | Some(npm) => format!("Now using node {installed} (npm v{npm})"),
    | None => format!("Now using node {installed}"),
  }
}

fn resolve_failure(spec: &VersionSpec, e: &ResolveError) -> Failure {
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
  let hint = match ShellKind::infer(&sys::ancestors(8)) {
    | Inferred::Supported(shell) => format!("\n\n  {}", shell.setup_hint()),
    | _ => {
      let all = [
        ShellKind::Bash,
        ShellKind::Zsh,
        ShellKind::Fish,
        ShellKind::Nu,
        ShellKind::PowerShell,
      ];
      all.iter().fold(String::new(), |mut hint, shell| {
        hint.push_str("\n  ");
        hint.push_str(shell.setup_hint());
        hint
      })
    },
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
    let link = ctx.nvm_dir.root().join("current");
    let tmp = ctx
      .nvm_dir
      .root()
      .join(format!(".current.tmp-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    if std::os::unix::fs::symlink(version_dir, &tmp).is_ok()
      && std::fs::rename(&tmp, &link).is_err()
    {
      let _ = std::fs::remove_file(&tmp);
    }
  }
  #[cfg(not(unix))]
  let _ = (ctx, version_dir);
}
