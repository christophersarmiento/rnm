//! Installing a Node.js release into the tree, the way nvm lays it out.
//!
//! 1. Check the index's `files` list for a binary for this platform.
//! 2. Take nvm's per-version lock.
//! 3. Fetch `SHASUMS256.txt` and pick the best archive it lists (`.tar.xz`,
//!    else `.tar.gz`; `.zip` on Windows). A missing entry is an error: rnm
//!    never installs unverified files.
//! 4. Download into nvm's cache, `.cache/bin/<slug>/<slug>.<ext>`, hashing
//!    while writing. A cached archive with the right digest is reused.
//! 5. Unpack into a hidden staging directory next to the final one, so the
//!    last step is a rename on one file system, then rename it into place.
//!    Readers never see a half-installed version.

mod archive;
mod checksums;
mod lock;

use std::fmt;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use ring::digest;

pub use archive::ArchiveKind;
pub use checksums::{Sha256, find_checksum};
pub use lock::{InstallLock, LockError, LockSettings};

use crate::http::{Client, HttpError, InvalidUrl, Mirror, Url};
use crate::nvm_dir::{Flavor, InstalledVersion, NvmDir, node_binary};
use crate::platform::{Artifact, Platform};
use crate::remote::Release;
use crate::version::Version;

/// The checksum file in each release directory.
const SHASUMS_FILE: &str = "SHASUMS256.txt";

/// Today's files are about 30 KB.
const MAX_SHASUMS_BYTES: u64 = 4 * 1024 * 1024;

/// Copy buffer for downloads and hashing.
const BUFFER_SIZE: usize = 64 * 1024;

/// Hidden prefixes for work directories next to version directories. They
/// start with `.`, so version listings (rnm's and nvm's) skip them.
const STAGING_PREFIX: &str = ".rnm-staging-";
const REMOVING_PREFIX: &str = ".rnm-removing-";

/// What an install is doing, for progress output.
#[derive(Debug)]
pub enum Event<'a> {
  /// Another install of this version holds the lock.
  WaitingForLock,
  /// Starting a download of this many bytes, if known.
  Downloading { url: &'a Url, size: Option<u64> },
  /// Bytes received so far in the current download.
  Progress { received: u64, size: Option<u64> },
  /// A previously downloaded archive passed verification and is reused.
  UsingCached { path: &'a Path },
  /// The archive matched its SHA-256 from `SHASUMS256.txt`.
  ChecksumMatched,
}

/// Why an install failed.
#[derive(Debug)]
pub enum InstallError {
  /// The release lists binaries, but none for this platform.
  NoBinary {
    version: Version,
    file_id: String,
  },
  /// `SHASUMS256.txt` lists none of the archive names rnm can unpack.
  NoChecksum {
    url: String,
    names: Vec<String>,
  },
  /// The download does not match its listed SHA-256.
  ChecksumMismatch {
    url: String,
    expected: Sha256,
    actual: Sha256,
  },
  /// The archive does not contain a single top-level directory with a
  /// usable `node` binary.
  BadArchive {
    path: PathBuf,
    reason: String,
  },
  Url(InvalidUrl),
  Http(HttpError),
  Lock(LockError),
  Io {
    path: PathBuf,
    source: io::Error,
  },
}

impl fmt::Display for InstallError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      | Self::NoBinary { version, file_id } => {
        write!(
          f,
          "node {version} has no prebuilt binary for this platform ({file_id})"
        )?;
        if file_id.ends_with("-musl") {
          f.write_str(
            "; musl builds are published on https://unofficial-builds.nodejs.org: set \
             RNM_NODE_MIRROR=https://unofficial-builds.nodejs.org/download/release",
          )?;
        }
        Ok(())
      },
      | Self::NoChecksum { url, names } => write!(
        f,
        "{url} lists no checksum for {}; refusing to install an unverified download",
        names.join(" or ")
      ),
      | Self::ChecksumMismatch {
        url,
        expected,
        actual,
      } => write!(
        f,
        "checksum mismatch for {url}: expected {expected}, got {actual}; the download is \
         corrupt or was tampered with"
      ),
      | Self::BadArchive { path, reason } => {
        write!(f, "{}: unexpected archive layout: {reason}", path.display())
      },
      | Self::Url(e) => e.fmt(f),
      | Self::Http(e) => e.fmt(f),
      | Self::Lock(e) => e.fmt(f),
      | Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
    }
  }
}

impl std::error::Error for InstallError {}

impl From<HttpError> for InstallError {
  fn from(e: HttpError) -> Self {
    Self::Http(e)
  }
}

impl From<InvalidUrl> for InstallError {
  fn from(e: InvalidUrl) -> Self {
    Self::Url(e)
  }
}

impl From<LockError> for InstallError {
  fn from(e: LockError) -> Self {
    Self::Lock(e)
  }
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> InstallError + '_ {
  move |source| InstallError::Io {
    path: path.to_path_buf(),
    source,
  }
}

/// Installs releases from one mirror into one tree.
pub struct Installer<'a> {
  pub nvm_dir: &'a NvmDir,
  pub mirror: &'a Mirror,
  pub client: &'a Client,
  pub platform: &'a Platform,
  pub lock: LockSettings,
}

/// An archive chosen from `SHASUMS256.txt`.
struct Chosen {
  kind: ArchiveKind,
  file_name: String,
  url: Url,
  expected: Sha256,
}

impl Installer<'_> {
  /// Installs `release`, or returns the existing install if it is already
  /// there (perhaps installed by a concurrent process while waiting for the
  /// lock).
  ///
  /// # Errors
  ///
  /// See [`InstallError`].
  pub fn install(
    &self,
    release: &Release,
    events: &mut dyn FnMut(Event<'_>),
  ) -> Result<InstalledVersion, InstallError> {
    let version = &release.version;
    let artifact = self.platform.artifact(version);
    if let Some(file_id) = &artifact.file_id {
      let is_announced = release.files.is_empty() || release.files.contains(file_id);
      if !is_announced {
        return Err(InstallError::NoBinary {
          version: version.clone(),
          file_id: file_id.clone(),
        });
      }
    }

    let locks_dir = self.nvm_dir.cache_dir().join("locks");
    let _lock = InstallLock::acquire(&locks_dir, &version.to_string(), &self.lock, || {
      events(Event::WaitingForLock);
    })?;
    let version_dir = self.nvm_dir.version_dir(Flavor::Node, version);
    if let Some(installed) = self.nvm_dir.installed_at(&version_dir) {
      return Ok(installed);
    }

    let chosen = self.choose_archive(version, &artifact)?;
    let archive = self.fetch_archive(&artifact, &chosen, events)?;
    self.unpack_into_place(&chosen, &archive, &version_dir)?;
    self
      .nvm_dir
      .installed_at(&version_dir)
      .ok_or_else(|| InstallError::BadArchive {
        path: archive,
        reason: "the installed node binary is not usable".to_owned(),
      })
  }

  fn choose_archive(&self, version: &Version, artifact: &Artifact) -> Result<Chosen, InstallError> {
    let release_dir = self.mirror.url.child(&version.to_string())?;
    let shasums_url = release_dir.child(SHASUMS_FILE)?;
    let mut shasums = String::new();
    self
      .client
      .get(&shasums_url)?
      .body
      .take(MAX_SHASUMS_BYTES)
      .read_to_string(&mut shasums)
      .map_err(|source| {
        InstallError::Http(HttpError::Transport {
          url: shasums_url.to_string(),
          message: source.to_string(),
        })
      })?;

    let candidates: Vec<(ArchiveKind, String)> = ArchiveKind::preferred()
      .iter()
      .map(|kind| (*kind, format!("{}.{}", artifact.slug, kind.extension())))
      .collect();
    for (kind, file_name) in &candidates {
      if let Some(expected) = find_checksum(&shasums, file_name) {
        return Ok(Chosen {
          kind: *kind,
          url: release_dir.child(file_name)?,
          file_name: file_name.clone(),
          expected,
        });
      }
    }
    Err(InstallError::NoChecksum {
      url: shasums_url.to_string(),
      names: candidates.into_iter().map(|(_, name)| name).collect(),
    })
  }

  /// The verified archive in nvm's cache, downloading it if needed.
  fn fetch_archive(
    &self,
    artifact: &Artifact,
    chosen: &Chosen,
    events: &mut dyn FnMut(Event<'_>),
  ) -> Result<PathBuf, InstallError> {
    let cache_dir = self.nvm_dir.cache_dir().join("bin").join(&artifact.slug);
    let cached = cache_dir.join(&chosen.file_name);
    let cached_digest = File::open(&cached)
      .ok()
      .and_then(|file| hash(file, None, &mut |_| {}).ok());
    if cached_digest == Some(chosen.expected) {
      events(Event::UsingCached { path: &cached });
      return Ok(cached);
    }

    std::fs::create_dir_all(&cache_dir).map_err(io_error(&cache_dir))?;
    let partial = cache_dir.join(format!(".{}.part-{}", chosen.file_name, std::process::id()));
    let result = self
      .download(chosen, &partial, events)
      .and_then(|()| std::fs::rename(&partial, &cached).map_err(io_error(&cached)));
    if result.is_err() {
      let _ = std::fs::remove_file(&partial);
    }
    result.map(|()| cached)
  }

  fn download(
    &self,
    chosen: &Chosen,
    partial: &Path,
    events: &mut dyn FnMut(Event<'_>),
  ) -> Result<(), InstallError> {
    let response = self.client.get(&chosen.url)?;
    let size = response.content_length;
    events(Event::Downloading {
      url: &chosen.url,
      size,
    });
    let mut file = File::create(partial).map_err(io_error(partial))?;
    let actual = hash(response.body, Some(&mut file), &mut |received| {
      events(Event::Progress { received, size });
    })
    .map_err(|source| {
      InstallError::Http(HttpError::Transport {
        url: chosen.url.to_string(),
        message: source.to_string(),
      })
    })?;
    file.sync_all().map_err(io_error(partial))?;
    if actual != chosen.expected {
      return Err(InstallError::ChecksumMismatch {
        url: chosen.url.to_string(),
        expected: chosen.expected,
        actual,
      });
    }
    events(Event::ChecksumMatched);
    Ok(())
  }

  /// Unpacks next to `version_dir` and renames the archive's single
  /// top-level directory into place, replacing a broken earlier install.
  fn unpack_into_place(
    &self,
    chosen: &Chosen,
    archive: &Path,
    version_dir: &Path,
  ) -> Result<(), InstallError> {
    let parent = version_dir.parent().unwrap_or(self.nvm_dir.root());
    std::fs::create_dir_all(parent).map_err(io_error(parent))?;
    let name = version_dir
      .file_name()
      .map(|name| name.to_string_lossy().into_owned())
      .unwrap_or_default();
    remove_leftovers(parent, &format!("{STAGING_PREFIX}{name}-"));
    let staging = parent.join(format!("{STAGING_PREFIX}{name}-{}", std::process::id()));
    std::fs::create_dir(&staging).map_err(io_error(&staging))?;

    let result = archive::unpack(chosen.kind, archive, &staging)
      .map_err(|e| InstallError::BadArchive {
        path: archive.to_path_buf(),
        reason: with_causes(&e),
      })
      .and_then(|()| single_top_dir(&staging, archive))
      .and_then(|top| {
        if !crate::nvm_dir::is_executable(&node_binary(&top)) {
          return Err(InstallError::BadArchive {
            path: archive.to_path_buf(),
            reason: format!("no node binary at {}", node_binary(&top).display()),
          });
        }
        if version_dir.exists() {
          remove_dir_safely(version_dir)?;
        }
        std::fs::rename(&top, version_dir).map_err(io_error(version_dir))
      });
    let _ = std::fs::remove_dir_all(&staging);
    result
  }
}

/// Removes a version directory by first renaming it to a hidden name, so it
/// stops counting as installed at once, even if deleting the files fails
/// half way.
///
/// # Errors
///
/// Fails if the directory cannot be renamed or deleted.
pub fn remove_dir_safely(dir: &Path) -> Result<(), InstallError> {
  let parent = dir.parent().unwrap_or(dir);
  let name = dir
    .file_name()
    .map(|name| name.to_string_lossy().into_owned())
    .unwrap_or_default();
  remove_leftovers(parent, &format!("{REMOVING_PREFIX}{name}-"));
  let doomed = parent.join(format!("{REMOVING_PREFIX}{name}-{}", std::process::id()));
  std::fs::rename(dir, &doomed).map_err(io_error(dir))?;
  std::fs::remove_dir_all(&doomed).map_err(io_error(&doomed))
}

/// The error and its causes, so the root cause (such as "No space left on
/// device") is not hidden behind the archive library's summary.
fn with_causes(error: &dyn std::error::Error) -> String {
  let mut text = error.to_string();
  let mut source = error.source();
  while let Some(cause) = source {
    let cause_text = cause.to_string();
    if !text.contains(&cause_text) {
      text.push_str(": ");
      text.push_str(&cause_text);
    }
    source = cause.source();
  }
  text
}

/// Deletes work directories named `<prefix>*` left by interrupted runs.
fn remove_leftovers(parent: &Path, prefix: &str) {
  let Ok(entries) = std::fs::read_dir(parent) else {
    return;
  };
  for entry in entries.flatten() {
    if entry.file_name().to_string_lossy().starts_with(prefix) {
      let _ = std::fs::remove_dir_all(entry.path());
    }
  }
}

fn single_top_dir(staging: &Path, archive: &Path) -> Result<PathBuf, InstallError> {
  let bad = |reason: &str| InstallError::BadArchive {
    path: archive.to_path_buf(),
    reason: reason.to_owned(),
  };
  let mut entries = std::fs::read_dir(staging)
    .map_err(io_error(staging))?
    .flatten();
  let first = entries.next().ok_or_else(|| bad("the archive is empty"))?;
  if entries.next().is_some() {
    return Err(bad("expected one top-level directory"));
  }
  let is_dir = first.file_type().is_ok_and(|kind| kind.is_dir());
  if !is_dir {
    return Err(bad("the top-level entry is not a directory"));
  }
  Ok(first.path())
}

/// Hashes everything `reader` yields, copying it to `copy` when given and
/// reporting the running byte count.
fn hash(
  mut reader: impl Read,
  mut copy: Option<&mut File>,
  on_progress: &mut dyn FnMut(u64),
) -> io::Result<Sha256> {
  let mut context = digest::Context::new(&digest::SHA256);
  let mut buffer = vec![0; BUFFER_SIZE];
  let mut received: u64 = 0;
  loop {
    let read = match reader.read(&mut buffer) {
      | Ok(0) => break,
      | Ok(read) => read,
      | Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
      | Err(e) => return Err(e),
    };
    let chunk = &buffer[..read];
    context.update(chunk);
    if let Some(file) = copy.as_deref_mut() {
      file.write_all(chunk)?;
    }
    received += u64::try_from(read).unwrap_or(u64::MAX);
    on_progress(received);
  }
  let mut bytes = [0; checksums::SHA256_LEN];
  bytes.copy_from_slice(context.finish().as_ref());
  Ok(Sha256(bytes))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn hashes_like_sha256sum() {
    let digest = hash(&b"abc"[..], None, &mut |_| {}).unwrap();
    assert_eq!(
      digest.to_string(),
      "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
  }

  #[test]
  fn shows_root_causes() {
    let root = io::Error::new(io::ErrorKind::StorageFull, "No space left on device");
    let outer = io::Error::new(io::ErrorKind::StorageFull, Wrapped(root));
    assert_eq!(
      with_causes(&outer),
      "failed to unpack bin/node: No space left on device"
    );
  }

  #[derive(Debug)]
  struct Wrapped(io::Error);

  impl fmt::Display for Wrapped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
      f.write_str("failed to unpack bin/node")
    }
  }

  impl std::error::Error for Wrapped {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
      Some(&self.0)
    }
  }

  #[test]
  fn removes_directories_safely() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("v20.0.0");
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    std::fs::create_dir(tmp.path().join(format!("{REMOVING_PREFIX}v20.0.0-1"))).unwrap();
    remove_dir_safely(&dir).unwrap();
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
  }
}
