//! `rnm default`: show or set the `default` alias, and (on Windows) make it
//! visible to programs that never run a shell startup file
//! (`docs/research/nvm-windows.md` §10.5).
//!
//! `--persist` creates the junction `<root>\default`, pointing at the
//! default version, and adds it once to the user's `Path` in the registry.
//! IDEs and other programs started from Explorer then find that `node`.
//! Later changes to the `default` alias only re-point the junction.

use std::io::Write;
use std::path::{Path, PathBuf};

use super::{Context, DEFAULT_ALIAS, Failure, alias, exit, say};
use crate::nvm_dir::Resolved;
use crate::version::VersionSpec;

/// The name of the persistent link in the install root.
const PERSISTENT_LINK: &str = "default";

/// Separates `Path` entries in the Windows registry.
const PATH_SEPARATOR: char = ';';

/// What `rnm default` should do besides showing or setting the alias.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Persistence {
  #[default]
  Unchanged,
  /// `--persist`.
  Persist,
  /// `--unpersist`.
  Unpersist,
}

/// Shows the `default` alias, or points it at `version`; then applies
/// `persistence`.
///
/// # Errors
///
/// The errors of `rnm alias`, and exit code 1 when `--persist` cannot
/// update the link or the registry, or is used outside Windows.
pub fn default_version(
  ctx: &Context,
  version: Option<&str>,
  persistence: Persistence,
  terminal: bool,
  out: &mut dyn Write,
  err: &mut dyn Write,
) -> Result<(), Failure> {
  let shows_alias = version.is_some() || persistence == Persistence::Unchanged;
  if shows_alias {
    alias(ctx, Some(DEFAULT_ALIAS), version, false, terminal, out, err)?;
  }
  match persistence {
    | Persistence::Unchanged => {
      refresh_persistent_default(ctx, err);
      Ok(())
    },
    | Persistence::Persist => persist(ctx, out),
    | Persistence::Unpersist => unpersist(ctx, out),
  }
}

fn persistent_link(ctx: &Context) -> PathBuf {
  ctx.nvm_dir.root().join(PERSISTENT_LINK)
}

/// The directory of the version `default` resolves to.
fn default_dir(ctx: &Context) -> Result<PathBuf, Failure> {
  let missing = || {
    Failure::new(
      exit::NOT_AVAILABLE,
      "The default alias does not name an installed version; set it with `rnm default \
       <version>`.",
    )
  };
  let spec: VersionSpec = DEFAULT_ALIAS.parse().map_err(|_| missing())?;
  match ctx.nvm_dir.resolve(&spec) {
    | Ok(Resolved::Installed(installed)) => Ok(installed.path),
    | _ => Err(missing()),
  }
}

/// Re-points the persistent link at the current `default`, if `--persist`
/// created one. Problems are warnings: the alias itself is already saved.
pub(crate) fn refresh_persistent_default(ctx: &Context, err: &mut dyn Write) {
  let link = persistent_link(ctx);
  if !crate::fs::is_link(&link) {
    return;
  }
  let result = default_dir(ctx).and_then(|target| point_link(&link, &target));
  if let Err(failure) = result {
    say(
      err,
      format_args!(
        "warning: cannot update {}: {}",
        link.display(),
        failure.message
      ),
    );
  }
}

fn point_link(link: &Path, target: &Path) -> Result<(), Failure> {
  let unchanged = crate::fs::read_dir_link(link).is_ok_and(|current| current == target);
  if unchanged {
    return Ok(());
  }
  let io_failure =
    |e: std::io::Error| Failure::new(exit::ERROR, format!("{}: {e}", link.display()));
  if crate::fs::is_link(link) {
    crate::fs::remove_dir_link(link).map_err(io_failure)?;
  }
  crate::fs::link_dir(target, link).map_err(io_failure)
}

#[cfg(windows)]
mod user_path {
  use crate::sys::{
    Hive, RawRegistryString, broadcast_environment_change, registry_raw_string, set_registry_string,
  };

  const ENVIRONMENT_KEY: &str = "Environment";
  const PATH_VALUE: &str = "Path";

  /// Applies `change` to the user's `Path` and tells running programs.
  /// Returns whether anything changed.
  pub fn update(change: impl Fn(&str) -> Option<String>) -> std::io::Result<bool> {
    let current = registry_raw_string(Hive::CurrentUser, ENVIRONMENT_KEY, PATH_VALUE).unwrap_or(
      RawRegistryString {
        value: String::new(),
        expandable: true,
      },
    );
    let Some(value) = change(&current.value) else {
      return Ok(false);
    };
    set_registry_string(
      Hive::CurrentUser,
      ENVIRONMENT_KEY,
      PATH_VALUE,
      &RawRegistryString {
        value,
        expandable: current.expandable,
      },
    )?;
    broadcast_environment_change();
    Ok(true)
  }
}

#[cfg(windows)]
fn persist(ctx: &Context, out: &mut dyn Write) -> Result<(), Failure> {
  let link = persistent_link(ctx);
  let target = default_dir(ctx)?;
  point_link(&link, &target)?;
  let entry = link.to_string_lossy().into_owned();
  let added = user_path::update(|path| with_entry_first(path, &entry))
    .map_err(|e| Failure::new(exit::ERROR, format!("cannot update your Path: {e}")))?;
  say(
    out,
    format_args!("{} -> {}", link.display(), target.display()),
  );
  if added {
    say(
      out,
      format_args!(
        "Added {entry} to your user Path. Programs started from now on find this node; \
         restart open editors and terminals."
      ),
    );
  } else {
    say(out, "Your user Path already contains it.");
  }
  Ok(())
}

#[cfg(windows)]
fn unpersist(ctx: &Context, out: &mut dyn Write) -> Result<(), Failure> {
  let link = persistent_link(ctx);
  if crate::fs::is_link(&link) {
    crate::fs::remove_dir_link(&link)
      .map_err(|e| Failure::new(exit::ERROR, format!("{}: {e}", link.display())))?;
  }
  let entry = link.to_string_lossy().into_owned();
  user_path::update(|path| without_entry(path, &entry))
    .map_err(|e| Failure::new(exit::ERROR, format!("cannot update your Path: {e}")))?;
  say(
    out,
    format_args!("Removed {entry} and its entry in your user Path."),
  );
  Ok(())
}

#[cfg(not(windows))]
fn persist(_: &Context, _: &mut dyn Write) -> Result<(), Failure> {
  Err(not_windows())
}

#[cfg(not(windows))]
fn unpersist(_: &Context, _: &mut dyn Write) -> Result<(), Failure> {
  Err(not_windows())
}

#[cfg(not(windows))]
fn not_windows() -> Failure {
  Failure::new(
    exit::ERROR,
    "--persist and --unpersist are for Windows, where programs started from Explorer never \
     run a shell startup file. Elsewhere, set NVM_SYMLINK_CURRENT=true to keep \
     $NVM_DIR/current pointing at the active version.",
  )
}

/// Two `Path` entries name the same directory: Windows compares paths
/// case-insensitively, and a trailing `\` does not matter.
fn same_entry(a: &str, b: &str) -> bool {
  let normalize = |entry: &str| entry.trim().trim_end_matches(['\\', '/']).to_lowercase();
  normalize(a) == normalize(b)
}

/// `path` with `entry` in front, or `None` when it already contains it.
#[cfg_attr(
  not(windows),
  allow(dead_code, reason = "used on Windows; tested everywhere")
)]
fn with_entry_first(path: &str, entry: &str) -> Option<String> {
  let present = path.split(PATH_SEPARATOR).any(|e| same_entry(e, entry));
  if present {
    return None;
  }
  let rest = path.trim_start_matches(PATH_SEPARATOR);
  Some(if rest.is_empty() {
    entry.to_owned()
  } else {
    format!("{entry}{PATH_SEPARATOR}{rest}")
  })
}

/// `path` without `entry`, or `None` when it does not contain it.
#[cfg_attr(
  not(windows),
  allow(dead_code, reason = "used on Windows; tested everywhere")
)]
fn without_entry(path: &str, entry: &str) -> Option<String> {
  let entries: Vec<&str> = path.split(PATH_SEPARATOR).collect();
  let kept: Vec<&str> = entries
    .iter()
    .copied()
    .filter(|e| !same_entry(e, entry))
    .collect();
  (kept.len() != entries.len()).then(|| kept.join(&PATH_SEPARATOR.to_string()))
}

#[cfg(test)]
mod tests {
  use super::*;

  const LINK: &str = r"C:\Users\u\AppData\Local\rnm\default";

  #[test]
  fn adds_the_entry_once_in_front() {
    assert_eq!(
      with_entry_first(r"%USERPROFILE%\bin;C:\x", LINK).as_deref(),
      Some(r"C:\Users\u\AppData\Local\rnm\default;%USERPROFILE%\bin;C:\x")
    );
    assert_eq!(with_entry_first("", LINK).as_deref(), Some(LINK));
    assert_eq!(
      with_entry_first(r"C:\x;c:\users\U\appdata\local\RNM\default\", LINK),
      None
    );
  }

  #[test]
  fn removes_every_copy_of_the_entry() {
    assert_eq!(
      without_entry(&format!(r"{LINK};C:\x;{LINK}\"), LINK).as_deref(),
      Some(r"C:\x")
    );
    assert_eq!(without_entry(r"C:\x", LINK), None);
  }
}
