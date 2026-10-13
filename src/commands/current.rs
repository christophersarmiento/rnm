//! `rnm current`: print the active version, like `nvm current`.

use std::io::Write;

use super::{Context, Failure, NONE_LABEL, SYSTEM_LABEL, say};
use crate::multishell::Target;
use crate::nvm_dir::{NODE_EXECUTABLE, version_dir_of};

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
      | Some(Target::Version(dir)) => return Ok(label_for_version_dir(ctx, &dir)),
      | Some(Target::System) => {
        let label = if ctx.system_node().is_some() {
          SYSTEM_LABEL
        } else {
          NONE_LABEL
        };
        return Ok(label.to_owned());
      },
      | None => {},
    }
  }
  Ok(from_path(ctx))
}

/// The version in `dir`, which may also be a link to a version directory
/// (`rnm default --persist`'s `<root>/default`).
fn label_for_version_dir(ctx: &Context, dir: &std::path::Path) -> String {
  ctx
    .nvm_dir
    .installed_at(dir)
    .or_else(|| {
      let target = crate::fs::read_dir_link(dir).ok()?;
      ctx.nvm_dir.installed_at(&target)
    })
    .map_or_else(|| NONE_LABEL.to_owned(), |installed| installed.to_string())
}

fn from_path(ctx: &Context) -> String {
  let Some(node) = ctx
    .path
    .iter()
    .map(|dir| dir.join(NODE_EXECUTABLE))
    .find(|p| p.is_file())
  else {
    return NONE_LABEL.to_owned();
  };
  if !node.starts_with(ctx.nvm_dir.root()) {
    return SYSTEM_LABEL.to_owned();
  }
  version_dir_of(&node).map_or_else(
    || NONE_LABEL.to_owned(),
    |dir| label_for_version_dir(ctx, dir),
  )
}
