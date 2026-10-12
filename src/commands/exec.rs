//! `rnm exec`: run a command with a version's `node` first on `PATH`, like
//! `nvm exec`, without changing the shell.

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

use super::use_version::{resolve_failure, version_file};
use super::{Context, Failure, exit, npm_version, say};
use crate::nvm_dir::{InstalledVersion, Resolved, bin_dir};
use crate::version::{SpecError, VersionSpec};

/// Exit code when the command cannot be started (the shell convention).
const CANNOT_EXECUTE: u8 = 126;

/// Options for [`exec`].
#[derive(Clone, Debug, Default)]
pub struct ExecOptions {
  /// `--using=<version>` (fnm's spelling). Without it, the first argument
  /// is the version when it names an installed one.
  pub using: Option<String>,
  /// The version (optional) followed by the command and its arguments.
  pub args: Vec<String>,
  /// `--silent`: do not print `Running node …`.
  pub silent: bool,
}

/// Runs the command with the version's `bin` directory first on `PATH`,
/// and `NVM_BIN`/`NVM_INC` pointing at it. On unix the command replaces
/// rnm; on Windows rnm waits and exits with the command's code.
///
/// # Errors
///
/// Exit code 127 when no command or version is given; 3 when the version is
/// not installed; 126 when the command cannot be started; otherwise the
/// command's own exit code (Windows).
pub fn exec(ctx: &Context, opts: &ExecOptions, out: &mut dyn Write) -> Result<(), Failure> {
  let (requested, command) = split_version(ctx, opts)?;
  let Some((program, args)) = command.split_first() else {
    return Err(Failure::new(
      exit::NO_VERSION,
      "Usage: rnm exec [--silent] [<version>] <command> [<args>...]",
    ));
  };
  let spec: VersionSpec = requested
    .parse()
    .map_err(|e: SpecError| Failure::new(exit::NOT_AVAILABLE, e.to_string()))?;
  let version = match ctx.nvm_dir.resolve(&spec) {
    | Ok(Resolved::Installed(installed)) => Some(installed),
    | Ok(Resolved::System | Resolved::Current) => None,
    | Err(e) => return Err(resolve_failure(&spec, &e)),
  };
  if !opts.silent {
    say(out, running_message(version.as_ref()));
  }
  let _ = out.flush();

  let mut child = Command::new(program);
  child
    .args(args)
    .env("PATH", search_path(ctx, version.as_ref())?);
  if let Some(installed) = &version {
    let bin = bin_dir(&installed.path);
    child
      .env("NVM_BIN", &bin)
      .env("NVM_INC", installed.path.join("include").join("node"));
  }
  run(child, program)
}

/// Splits the version from the command: `--using`, else the first argument
/// when it resolves to something installed and a command follows it, else
/// the version file. The "command follows" rule keeps `rnm exec node` running
/// `node`, where nvm would read `node` as the version and find no command.
fn split_version(ctx: &Context, opts: &ExecOptions) -> Result<(String, Vec<String>), Failure> {
  if let Some(using) = &opts.using {
    return Ok((using.clone(), opts.args.clone()));
  }
  if let Some((first, rest)) = opts.args.split_first()
    && !rest.is_empty()
  {
    let names_a_version = first
      .parse::<VersionSpec>()
      .is_ok_and(|spec| ctx.nvm_dir.resolve(&spec).is_ok());
    if names_a_version {
      return Ok((first.clone(), rest.to_vec()));
    }
  }
  let file = version_file(ctx)?;
  Ok((file.version, opts.args.clone()))
}

fn running_message(version: Option<&InstalledVersion>) -> String {
  let Some(installed) = version else {
    return "Running system node".to_owned();
  };
  match npm_version(&installed.path) {
    | Some(npm) => format!("Running node {installed} (npm v{npm})"),
    | None => format!("Running node {installed}"),
  }
}

/// `PATH` without rnm's and nvm's directories, with the version's `bin`
/// directory in front (none for `system`).
fn search_path(ctx: &Context, version: Option<&InstalledVersion>) -> Result<OsString, Failure> {
  let unmanaged = ctx.path.iter().filter(|dir| !ctx.is_managed(dir)).cloned();
  let dirs: Vec<PathBuf> = version
    .map(|installed| bin_dir(&installed.path))
    .into_iter()
    .chain(unmanaged)
    .collect();
  std::env::join_paths(dirs).map_err(|e| Failure::new(exit::ERROR, e.to_string()))
}

#[cfg(unix)]
fn run(mut child: Command, program: &str) -> Result<(), Failure> {
  use std::os::unix::process::CommandExt;
  let error = child.exec();
  Err(Failure::new(CANNOT_EXECUTE, format!("{program}: {error}")))
}

#[cfg(not(unix))]
fn run(mut child: Command, program: &str) -> Result<(), Failure> {
  let status = child
    .status()
    .map_err(|e| Failure::new(CANNOT_EXECUTE, format!("{program}: {e}")))?;
  match status.code() {
    | Some(0) => Ok(()),
    | Some(code) => Err(Failure::new(u8::try_from(code).unwrap_or(exit::ERROR), "")),
    | None => Err(Failure::new(exit::ERROR, "")),
  }
}
