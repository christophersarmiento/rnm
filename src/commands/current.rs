//! `rnm current`: print the active version, like `nvm current`.

use std::io::Write;

use super::{Context, Failure, say};
use crate::multishell::Target;
use crate::nvm_dir::NvmDir;

/// Prints `vX.Y.Z`, `iojs-vX.Y.Z`, `system`, or `none`.
///
/// With an rnm session, the session link decides. Without one, the first
/// `node` on `PATH` decides: inside the install tree it maps back to a
/// version, elsewhere it is `system`.
///
/// # Errors
///
/// Fails if the session link exists but cannot be read.
pub fn current(ctx: &Context, out: &mut dyn Write) -> Result<(), Failure> {
  say(out, active(ctx)?);
  Ok(())
}

/// The active version as `nvm current` prints it.
///
/// # Errors
///
/// Fails if the session link exists but cannot be read.
pub fn active(ctx: &Context) -> Result<String, Failure> {
  if let Some(session) = &ctx.session {
    match session.target()? {
      | Some(Target::Version(dir)) => {
        return Ok(
          ctx
            .nvm_dir
            .installed_at(&dir)
            .map_or("none".to_owned(), |i| i.to_string()),
        );
      },
      | Some(Target::System) => {
        return Ok(
          if ctx.system_node().is_some() {
            "system"
          } else {
            "none"
          }
          .to_owned(),
        );
      },
      | None => {},
    }
  }
  Ok(from_path(ctx, &ctx.nvm_dir))
}

fn from_path(ctx: &Context, nvm_dir: &NvmDir) -> String {
  let name = if cfg!(windows) { "node.exe" } else { "node" };
  let Some(node) = ctx
    .path
    .iter()
    .map(|dir| dir.join(name))
    .find(|p| p.is_file())
  else {
    return "none".to_owned();
  };
  if !node.starts_with(nvm_dir.root()) {
    return "system".to_owned();
  }
  // <version dir>/bin/node on unix, <version dir>/node.exe on Windows.
  let version_dir = if cfg!(windows) {
    node.parent()
  } else {
    node.parent().and_then(|bin| bin.parent())
  };
  version_dir
    .and_then(|dir| nvm_dir.installed_at(dir))
    .map_or("none".to_owned(), |i| i.to_string())
}
