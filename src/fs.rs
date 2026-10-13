//! Crash-safe file replacement.
//!
//! Each function writes to a hidden temporary sibling and renames it over the
//! destination. A rename within one directory is atomic, so readers see either
//! the old or the new file, never a partial one.

use std::io;
use std::path::{Path, PathBuf};

/// Replaces `path` with a file containing `contents`.
///
/// # Errors
///
/// Returns an error if the temporary file cannot be written or renamed. The
/// temporary file is removed on failure.
pub fn write_atomic(path: &Path, contents: &str) -> io::Result<()> {
  let tmp = temp_sibling(path);
  std::fs::write(&tmp, contents)?;
  rename_or_clean_up(&tmp, path)
}

/// Replaces `link` with a symbolic link to `target`.
///
/// # Errors
///
/// Returns an error if the temporary link cannot be created or renamed. The
/// temporary link is removed on failure.
#[cfg(unix)]
pub fn replace_symlink(target: &Path, link: &Path) -> io::Result<()> {
  let tmp = temp_sibling(link);
  let _ = std::fs::remove_file(&tmp);
  std::os::unix::fs::symlink(target, &tmp)?;
  rename_or_clean_up(&tmp, link)
}

/// Other users can read the file at `path` (unix permission bits). Always
/// `false` on Windows, where files in the profile are private by default.
#[must_use]
pub fn is_readable_by_others(path: &Path) -> bool {
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    const GROUP_OR_OTHER: u32 = 0o077;
    std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & GROUP_OR_OTHER != 0)
  }
  #[cfg(not(unix))]
  {
    let _ = path;
    false
  }
}

/// A hidden name next to `path`, unique to this process, so concurrent rnm
/// processes never share a temporary file.
fn temp_sibling(path: &Path) -> PathBuf {
  let name = path
    .file_name()
    .map(|name| name.to_string_lossy())
    .unwrap_or_default();
  path.with_file_name(format!(".{name}.tmp-{}", std::process::id()))
}

fn rename_or_clean_up(tmp: &Path, destination: &Path) -> io::Result<()> {
  std::fs::rename(tmp, destination).inspect_err(|_| {
    let _ = std::fs::remove_file(tmp);
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn writes_and_replaces() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("file");
    write_atomic(&path, "one").unwrap();
    write_atomic(&path, "two").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
  }

  #[cfg(unix)]
  #[test]
  fn replaces_symlinks() {
    let tmp = tempfile::tempdir().unwrap();
    let link = tmp.path().join("link");
    replace_symlink(Path::new("/a"), &link).unwrap();
    replace_symlink(Path::new("/b"), &link).unwrap();
    assert_eq!(std::fs::read_link(&link).unwrap(), Path::new("/b"));
  }
}
