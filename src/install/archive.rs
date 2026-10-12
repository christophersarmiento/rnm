//! Unpacking Node.js archives: `.tar.xz` or `.tar.gz` on unix, `.zip` on
//! Windows. Both archive libraries refuse entries that would land outside
//! the destination (absolute paths, `..`), and file ownership is never
//! restored, like `tar --no-same-owner`.

use std::fs::File;
use std::io::{self, BufReader};
use std::path::Path;

/// An archive format Node.js publishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveKind {
  /// About half the size of `.tar.gz`; every release since Node.js 4.
  TarXz,
  TarGz,
  /// The Windows format; it has no `bin/` directory.
  Zip,
}

impl ArchiveKind {
  /// The formats to try, best first.
  #[must_use]
  pub fn preferred() -> &'static [Self] {
    if cfg!(windows) {
      &[Self::Zip]
    } else {
      &[Self::TarXz, Self::TarGz]
    }
  }

  /// The file extension, without the leading dot.
  #[must_use]
  pub fn extension(self) -> &'static str {
    match self {
      | Self::TarXz => "tar.xz",
      | Self::TarGz => "tar.gz",
      | Self::Zip => "zip",
    }
  }
}

/// Unpacks `archive` into the existing directory `destination`.
///
/// # Errors
///
/// Fails on I/O errors and on corrupt or unsafe archives.
pub fn unpack(kind: ArchiveKind, archive: &Path, destination: &Path) -> io::Result<()> {
  let file = BufReader::new(File::open(archive)?);
  match kind {
    | ArchiveKind::TarXz => unpack_tar(lzma_rust2::XzReader::new(file, true), destination),
    | ArchiveKind::TarGz => unpack_tar(flate2::bufread::GzDecoder::new(file), destination),
    | ArchiveKind::Zip => unpack_zip(file, destination),
  }
}

fn unpack_tar(decompressed: impl io::Read, destination: &Path) -> io::Result<()> {
  let mut archive = tar::Archive::new(decompressed);
  archive.set_preserve_permissions(true);
  archive.set_preserve_mtime(true);
  archive.set_overwrite(true);
  archive.unpack(destination)
}

#[cfg(windows)]
fn unpack_zip(file: BufReader<File>, destination: &Path) -> io::Result<()> {
  zip::ZipArchive::new(file)
    .and_then(|mut archive| archive.extract(destination))
    .map_err(io::Error::other)
}

#[cfg(not(windows))]
fn unpack_zip(_: BufReader<File>, _: &Path) -> io::Result<()> {
  Err(io::Error::new(
    io::ErrorKind::Unsupported,
    "zip archives are only used on Windows",
  ))
}

#[cfg(test)]
mod tests {
  use super::*;

  /// A `.tar.gz` with `top/bin/node` (mode 755) and `top/bin/npm`, a
  /// symlink into `lib/`.
  #[cfg(unix)]
  fn tar_gz(path: &Path) {
    let file = File::create(path).unwrap();
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    let mut header = tar::Header::new_gnu();
    header.set_size(4);
    header.set_mode(0o755);
    header.set_cksum();
    builder
      .append_data(&mut header, "top/bin/node", &b"node"[..])
      .unwrap();
    let mut link = tar::Header::new_gnu();
    link.set_entry_type(tar::EntryType::Symlink);
    link.set_size(0);
    link.set_mode(0o777);
    builder
      .append_link(
        &mut link,
        "top/bin/npm",
        "../lib/node_modules/npm/bin/npm-cli.js",
      )
      .unwrap();
    builder.into_inner().unwrap().finish().unwrap();
  }

  #[cfg(unix)]
  #[test]
  fn unpacks_tar_gz_with_modes_and_symlinks() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("a.tar.gz");
    tar_gz(&archive);
    let out = tmp.path().join("out");
    std::fs::create_dir(&out).unwrap();
    unpack(ArchiveKind::TarGz, &archive, &out).unwrap();
    let node = out.join("top/bin/node");
    assert_eq!(std::fs::read(&node).unwrap(), b"node");
    assert_eq!(
      std::fs::metadata(&node).unwrap().permissions().mode() & 0o777,
      0o755
    );
    assert_eq!(
      std::fs::read_link(out.join("top/bin/npm")).unwrap(),
      Path::new("../lib/node_modules/npm/bin/npm-cli.js")
    );
  }

  #[test]
  fn rejects_corrupt_archives() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("a.tar.xz");
    std::fs::write(&archive, b"not xz").unwrap();
    let out = tmp.path().join("out");
    std::fs::create_dir(&out).unwrap();
    assert!(unpack(ArchiveKind::TarXz, &archive, &out).is_err());
  }
}
