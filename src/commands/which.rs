//! `rnm which`: print the path of a version's `node`, like `nvm which`.

use std::io::Write;

use super::use_version::{resolve_failure, version_file};
use super::{Context, Failure, exit, say};
use crate::nvm_dir::{ResolveError, Resolved, node_binary};
use crate::version::{SpecError, VersionSpec};

/// Prints the `node` binary of `spec`, or of the nearest version file.
/// `current` means the active version and `system` the Node.js outside rnm.
///
/// # Errors
///
/// Exit code 127 when no version is given or found, or `system` has no
/// Node.js; 8 for an alias loop; 1 when the version is not installed.
pub fn which(
  ctx: &Context,
  spec: Option<&str>,
  silent: bool,
  out: &mut dyn Write,
) -> Result<(), Failure> {
  let requested = match spec {
    | Some(spec) => spec.to_owned(),
    | None => {
      let file = version_file(ctx).map_err(|_| usage())?;
      if !silent {
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
  let parsed: VersionSpec = requested
    .parse()
    .map_err(|e: SpecError| Failure::new(exit::NOT_AVAILABLE, e.to_string()))?;
  let path = match ctx.nvm_dir.resolve(&parsed) {
    | Ok(Resolved::Installed(installed)) => node_binary(&installed.path),
    | Ok(Resolved::System) => ctx
      .system_node()
      .ok_or_else(|| Failure::new(exit::NO_VERSION, "System version of node not found."))?,
    | Ok(Resolved::Current) => {
      let current = super::current::active(ctx)?;
      return which(ctx, Some(&current), silent, out);
    },
    | Err(ResolveError::AliasCycle(_)) => {
      return Err(Failure::new(
        exit::ALIAS_LOOP,
        format!("The alias \"{requested}\" leads to an infinite loop. Aborting."),
      ));
    },
    // nvm's `which` exits 1 here, where `use` exits 3.
    | Err(e) => {
      let failure = resolve_failure(&parsed, &e);
      return Err(Failure::new(exit::ERROR, failure.message));
    },
  };
  say(out, path.display());
  Ok(())
}

fn usage() -> Failure {
  Failure::new(
    exit::NO_VERSION,
    "Usage: rnm which [current | <version>]\n  Provide a <version>, or run from a directory \
     containing an .nvmrc file.",
  )
}
