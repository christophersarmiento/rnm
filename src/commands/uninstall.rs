//! `rnm uninstall`: remove an installed version, like `nvm uninstall`.

use std::io::Write;

use super::current::active;
use super::{Context, Failure, exit, say};
use crate::install::{InstallLock, LockSettings, Removal, remove_dir_safely};
use crate::nvm_dir::{Flavor, InstalledVersion, ResolveError, Resolved};
use crate::version::{LTS_PREFIX, VersionSpec};

/// Removes the version that `spec` (or `--lts[=name]`) resolves to locally,
/// then every alias that points at exactly that version. The downloaded
/// archive stays in the cache, as in nvm.
///
/// Like nvm, a version that is not installed is reported on stderr with
/// exit code 0.
///
/// # Errors
///
/// Exit code 1 for the active version, a version a running program still
/// uses (Windows), or on file system errors; 8 for an alias loop.
pub fn uninstall(
  ctx: &Context,
  spec: Option<&str>,
  lts: Option<&str>,
  out: &mut dyn Write,
  err: &mut dyn Write,
) -> Result<(), Failure> {
  let requested = match (lts, spec) {
    | (Some(lts), _) => format!("{LTS_PREFIX}{lts}"),
    | (None, Some(spec)) => spec.to_owned(),
    | (None, None) => {
      return Err(Failure::new(
        exit::NO_VERSION,
        "Usage: rnm uninstall <version>",
      ));
    },
  };
  let not_installed = |err: &mut dyn Write| -> Result<(), Failure> {
    say(err, format_args!("Version '{requested}' is not installed."));
    Ok(())
  };
  let Ok(parsed) = requested.parse::<VersionSpec>() else {
    return not_installed(err);
  };
  let installed = match ctx.nvm_dir.resolve(&parsed) {
    | Ok(Resolved::Installed(installed)) => installed,
    | Ok(Resolved::Current) => {
      return Err(Failure::new(
        exit::ERROR,
        "rnm: Cannot uninstall the currently-active version.",
      ));
    },
    | Err(e @ ResolveError::AliasCycle(_)) => {
      return Err(Failure::new(exit::ALIAS_LOOP, e.to_string()));
    },
    | Ok(Resolved::System) | Err(_) => return not_installed(err),
  };

  if active(ctx)? == installed.to_string() {
    return Err(Failure::new(
      exit::ERROR,
      format!(
        "rnm: Cannot uninstall currently-active {} version, {installed} (inferred from {requested}).",
        flavor_name(&installed)
      ),
    ));
  }

  let locks_dir = ctx.nvm_dir.cache_dir().join("locks");
  let settings = LockSettings::from_env(|key| ctx.var_string(key));
  let _lock = InstallLock::acquire(&locks_dir, &installed.to_string(), &settings, || {
    say(
      err,
      format_args!("Waiting for another install of {installed} to finish..."),
    );
  })
  .map_err(|e| Failure::new(exit::ERROR, e.to_string()))?;
  let removal =
    remove_dir_safely(&installed.path).map_err(|e| Failure::new(exit::ERROR, e.to_string()))?;
  if let Removal::Partial { leftover, source } = removal {
    say(
      err,
      format_args!(
        "rnm: some files of {installed} are still in use ({source}); they stay in {} until \
         a later install or uninstall deletes them",
        leftover.display()
      ),
    );
  }
  say(
    out,
    format_args!(
      "Uninstalled {} {}",
      flavor_name(&installed),
      installed.version
    ),
  );
  remove_aliases_to(ctx, &installed, out)
}

fn flavor_name(installed: &InstalledVersion) -> &'static str {
  match installed.flavor {
    | Flavor::Node => "node",
    | Flavor::Iojs => "io.js",
  }
}

/// Removes aliases whose target is exactly this version (`v20.11.1` or
/// `20.11.1`). Unlike nvm's substring match, `v20.1.0` does not remove an
/// alias to `v20.1.0-rc.1`, and partial targets such as `20` stay.
fn remove_aliases_to(
  ctx: &Context,
  installed: &InstalledVersion,
  out: &mut dyn Write,
) -> Result<(), Failure> {
  let aliases = ctx.nvm_dir.aliases();
  let full = installed.to_string();
  let without_prefix = match installed.flavor {
    | Flavor::Node => full.strip_prefix('v').unwrap_or(&full).to_owned(),
    | Flavor::Iojs => full.clone(),
  };
  for (name, target) in aliases.list(false)? {
    if target == full || target == without_prefix {
      aliases.remove(&name)?;
      say(
        out,
        format_args!("Deleted alias {name} - restore it with `rnm alias \"{name}\" \"{target}\"`"),
      );
    }
  }
  Ok(())
}
