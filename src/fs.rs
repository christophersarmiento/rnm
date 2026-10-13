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

/// Creates `link` as a link to the directory `target`: a symlink on unix, a
/// directory junction on Windows (which needs no admin rights or Developer
/// Mode).
///
/// # Errors
///
/// Returns an error if `link` exists or cannot be created.
pub fn link_dir(target: &Path, link: &Path) -> io::Result<()> {
  #[cfg(unix)]
  {
    std::os::unix::fs::symlink(target, link)
  }
  #[cfg(windows)]
  {
    junction::create(target, link)
  }
}

/// The directory a [`link_dir`] link points to.
///
/// # Errors
///
/// Returns an error if `link` is not such a link.
pub fn read_dir_link(link: &Path) -> io::Result<PathBuf> {
  #[cfg(unix)]
  {
    std::fs::read_link(link)
  }
  #[cfg(windows)]
  {
    junction::get_target(link)
  }
}

/// Removes a [`link_dir`] link, never the target's contents.
///
/// # Errors
///
/// Returns an error if `link` cannot be removed.
pub fn remove_dir_link(link: &Path) -> io::Result<()> {
  #[cfg(unix)]
  {
    std::fs::remove_file(link)
  }
  #[cfg(windows)]
  {
    std::fs::remove_dir(link)
  }
}

/// `path` is a link (symlink or junction) rather than a real directory.
#[must_use]
pub fn is_link(path: &Path) -> bool {
  std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// Copies the directory tree `from` to `to`, which must not exist. Symlinks
/// inside are recreated as symlinks on unix (npm's `bin` links) and copied as
/// files elsewhere.
///
/// # Errors
///
/// Returns the first error; `to` may then be partly written.
pub fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
  std::fs::create_dir(to)?;
  for entry in std::fs::read_dir(from)? {
    let entry = entry?;
    let source = entry.path();
    let destination = to.join(entry.file_name());
    let file_type = entry.file_type()?;
    #[cfg(unix)]
    if file_type.is_symlink() {
      std::os::unix::fs::symlink(std::fs::read_link(&source)?, &destination)?;
      continue;
    }
    if file_type.is_dir() {
      copy_dir(&source, &destination)?;
    } else {
      std::fs::copy(&source, &destination)?;
    }
  }
  Ok(())
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

  #[test]
  fn links_and_copies_directories() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    std::fs::create_dir_all(source.join("bin")).unwrap();
    std::fs::write(source.join("bin").join("node"), "x").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("node", source.join("bin").join("alias")).unwrap();

    let link = tmp.path().join("link");
    link_dir(&source, &link).unwrap();
    assert!(is_link(&link));
    assert_eq!(read_dir_link(&link).unwrap(), source);
    assert!(link.join("bin").join("node").is_file());
    remove_dir_link(&link).unwrap();
    assert!(source.join("bin").join("node").is_file());

    let copy = tmp.path().join("copy");
    copy_dir(&source, &copy).unwrap();
    assert!(!is_link(&copy));
    assert_eq!(
      std::fs::read_to_string(copy.join("bin").join("node")).unwrap(),
      "x"
    );
    #[cfg(unix)]
    assert_eq!(
      std::fs::read_link(copy.join("bin").join("alias")).unwrap(),
      Path::new("node")
    );
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
