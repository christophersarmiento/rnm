//! `rnm env`: set up rnm in the current shell session.

use std::io::Write;
use std::path::{Path, PathBuf};

use super::install::Progress;
use super::use_version::{UseOptions, requested_spec, switch_session};
use super::{Context, DEFAULT_ALIAS, Failure, exit, infer_shell, say};
use crate::multishell::{self, Session};
use crate::nvm_dir::Resolved;
use crate::shell::{CdHook, EnvPlan, ShellKind};
use crate::sys;
use crate::version::VersionSpec;

/// nvm's variable for the active version's `bin` directory.
const NVM_BIN: &str = "NVM_BIN";

/// nvm's variable for the active version's headers (`include/node`).
const NVM_INC: &str = "NVM_INC";

/// Enough ancestors to see the shell and, when `rnm env` runs inside `$(…)`,
/// the subshell in front of it. See [`owner_pid`].
const PARENT_AND_GRANDPARENT: usize = 2;

/// Options for [`env`].
#[derive(Clone, Copy, Debug, Default)]
pub struct EnvOptions {
  /// The shell to generate code for. Detected from the process tree when
  /// `None`, unless `json` is set.
  pub shell: Option<ShellKind>,
  /// `--json`: print the variables as JSON instead of shell code.
  pub json: bool,
  /// `--use-on-cd`: switch to the nearest version file's version now, and
  /// install a hook that does it again after every directory change.
  pub use_on_cd: Option<CdHook>,
  /// Download progress output when the hook installs a version at startup.
  pub progress: Progress,
}

/// Prints the shell code (or JSON) that puts this session's link on `PATH`.
///
/// - A new link is created for the calling shell, pointing at the `default`
///   alias (or at `system` when there is none). Dead sessions' links are
///   removed first.
/// - Running it again in the same shell (re-sourcing the startup file) reuses
///   the existing link and does not add `PATH` again.
/// - `--shell nu` (without `--json`) prints a static loader script and
///   creates nothing.
/// - `--use-on-cd` switches the link to the nearest version file's version
///   in-process, so a new shell starts one process, not two. Problems with
///   that file are reported on `err` and do not fail the command.
///
/// # Errors
///
/// Fails if the shell cannot be determined or the link cannot be created.
pub fn env(
  ctx: &Context,
  opts: &EnvOptions,
  out: &mut dyn Write,
  err: &mut dyn Write,
) -> Result<(), Failure> {
  let json = opts.json;
  let shell = match (opts.shell, json) {
    | (Some(shell), _) => Some(shell),
    | (None, true) => None,
    | (None, false) => Some(infer_shell()?),
  };
  let wants_nu_loader = shell == Some(ShellKind::Nu) && !json;
  if wants_nu_loader {
    let loader = EnvPlan {
      use_on_cd: opts.use_on_cd,
      ..EnvPlan::default()
    };
    let _ = out.write_all(loader.render(ShellKind::Nu).as_bytes());
    return Ok(());
  }

  let _ = ctx.multishells.collect_garbage(sys::is_alive);
  let owner = owner_pid(&sys::ancestors(PARENT_AND_GRANDPARENT))
    .or_else(sys::parent_pid)
    .ok_or_else(|| Failure::new(exit::ERROR, "cannot find the shell that runs rnm env"))?;
  let session = match reusable_session(ctx, owner) {
    | Some(session) => session,
    | None => ctx
      .multishells
      .create(owner, default_target(ctx, err).as_deref())?,
  };

  if let Some(cd_hook) = opts.use_on_cd {
    let hook = UseOptions::hook(cd_hook.install_if_missing, opts.progress);
    use_version_file(ctx, &session, &hook, err);
  }

  let bin = session.bin_dir();
  let mut plan = EnvPlan {
    use_on_cd: opts.use_on_cd,
    ..EnvPlan::default()
  };
  if !ctx.path.iter().any(|dir| dir == &bin) {
    plan.path_prepend.push(path_for_shell(ctx, shell, &bin));
  }
  plan
    .set
    .push((multishell::ENV_VAR.to_owned(), display(session.path())));
  plan.set.push((NVM_BIN.to_owned(), display(&bin)));
  plan
    .set
    .push((NVM_INC.to_owned(), display(&session.include_dir())));

  let text = match shell {
    | Some(shell) if !json => plan.render(shell),
    | _ => plan.render_json(),
  };
  let _ = out.write_all(text.as_bytes());
  Ok(())
}

/// Does what the `--use-on-cd` hook would do in this directory. Messages go
/// to `err`, because `out` is evaluated by the shell.
fn use_version_file(ctx: &Context, session: &Session, hook: &UseOptions, err: &mut dyn Write) {
  let mut messages = Vec::new();
  let result = requested_spec(ctx, hook, err).and_then(|spec| match spec {
    | Some(spec) => switch_session(ctx, session, &spec, hook, &mut messages, err),
    | None => Ok(()),
  });
  let _ = err.write_all(&messages);
  if let Err(failure) = result {
    say(err, failure.message);
  }
}

/// The PID of the shell that runs `rnm env`, which owns the session link.
///
/// Usually that is the parent process. But bash runs `$(rnm env)` in a forked
/// subshell that exits right away, and a link owned by it would be garbage
/// collected while the real shell still uses it. A subshell has the same
/// name as its parent, so when parent and grandparent share a name, the
/// grandparent owns the link. If that guess is wrong (a nested shell of the
/// same kind), the owner is the outer shell, which lives longer: the link is
/// kept longer than needed, but never removed too early.
fn owner_pid(ancestors: &[sys::Process]) -> Option<u32> {
  let name = |p: &sys::Process| p.name.trim_start_matches('-').to_ascii_lowercase();
  match ancestors {
    | [parent, grandparent, ..] if name(parent) == name(grandparent) => Some(grandparent.pid),
    | [parent, ..] => Some(parent.pid),
    | [] => None,
  }
}

/// The existing session link, when `rnm env` runs again in the shell that
/// owns it.
fn reusable_session(ctx: &Context, owner: u32) -> Option<Session> {
  let session = ctx.session.as_ref()?;
  let owned_by_this_shell = session.owner_pid() == Some(owner);
  let in_links_dir = session.path().parent() == Some(ctx.multishells.dir());
  let still_exists = session.target().ok().flatten().is_some();
  (owned_by_this_shell && in_links_dir && still_exists).then(|| session.clone())
}

/// The version directory for a new session: the `default` alias, resolved
/// locally. `None` (system) when there is no default or it cannot be used.
fn default_target(ctx: &Context, err: &mut dyn Write) -> Option<PathBuf> {
  let target = ctx.nvm_dir.aliases().get(DEFAULT_ALIAS).ok().flatten()?;
  let resolved = target
    .parse::<VersionSpec>()
    .map_err(|e| e.to_string())
    .and_then(|spec| ctx.nvm_dir.resolve(&spec).map_err(|e| e.to_string()));
  match resolved {
    | Ok(Resolved::Installed(installed)) => Some(installed.path),
    | Ok(Resolved::System | Resolved::Current) => None,
    | Err(e) => {
      say(err, format_args!("rnm: default alias '{target}': {e}"));
      None
    },
  }
}

/// The `PATH` entry as the shell should see it. In Git Bash and MSYS2 on
/// Windows, bash needs `/c/...` paths instead of `C:\...`.
fn path_for_shell(ctx: &Context, shell: Option<ShellKind>, dir: &Path) -> String {
  let is_msys_bash =
    cfg!(windows) && shell == Some(ShellKind::Bash) && ctx.var("MSYSTEM").is_some();
  if is_msys_bash {
    return to_msys_path(&display(dir));
  }
  display(dir)
}

fn display(path: &Path) -> String {
  path.to_string_lossy().into_owned()
}

/// `C:\Users\A B\x` → `/c/Users/A B/x`. Other paths only get `/` separators.
fn to_msys_path(path: &str) -> String {
  let forward_slashes = path.replace('\\', "/");
  match split_drive(&forward_slashes) {
    | Some((drive, rest)) => format!("/{}{rest}", drive.to_ascii_lowercase()),
    | None => forward_slashes,
  }
}

/// `C:/x` → `('C', "/x")`.
fn split_drive(path: &str) -> Option<(char, &str)> {
  let mut chars = path.chars();
  let drive = chars.next().filter(char::is_ascii_alphabetic)?;
  let rest = chars.as_str().strip_prefix(':')?;
  Some((drive, rest))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn owner_skips_one_subshell() {
    let p = |pid, name: &str| sys::Process {
      pid,
      name: name.to_owned(),
    };
    assert_eq!(owner_pid(&[p(10, "bash"), p(9, "-bash")]), Some(9));
    assert_eq!(owner_pid(&[p(10, "zsh"), p(9, "tmux")]), Some(10));
    assert_eq!(owner_pid(&[p(10, "fish")]), Some(10));
    assert_eq!(owner_pid(&[]), None);
  }

  #[test]
  fn msys_paths() {
    assert_eq!(to_msys_path(r"C:\Users\A B\rnm"), "/c/Users/A B/rnm");
    assert_eq!(to_msys_path(r"d:\x"), "/d/x");
    assert_eq!(to_msys_path(r"\\server\share"), "//server/share");
  }
}
