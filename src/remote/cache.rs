//! Fetching `index.json` with an on-disk copy that is revalidated by `ETag`.
//!
//! The copy lives in `<root>/.cache/rnm/`, one file per mirror, so switching
//! mirrors never mixes their release lists. Each fetch sends
//! `If-None-Match`; a 304 costs one round trip and no download.

use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use super::index::RemoteIndex;
use crate::fs::write_atomic;
use crate::http::{Client, Conditional, HttpError, Mirror, Url};

/// The index file under the mirror URL.
const INDEX_FILE: &str = "index.json";

/// The subdirectory of `.cache` that holds rnm's own files. nvm only uses
/// `bin/`, `src/`, and `locks/`.
const CACHE_SUBDIR: &str = "rnm";

/// HTTP 304, which only answers requests that carried an `ETag`.
const NOT_MODIFIED: u16 = 304;

/// About 50 times the size of today's official index.
const MAX_INDEX_BYTES: u64 = 16 * 1024 * 1024;

/// FNV-1a 64-bit parameters (<http://www.isthe.com/chongo/tech/comp/fnv/>).
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// A loaded index and anything the user should hear about.
#[derive(Debug)]
pub struct Loaded {
  pub index: RemoteIndex,
  pub warnings: Vec<IndexWarning>,
  /// The position, in the mirror list, of the mirror that served the
  /// index.
  pub mirror: usize,
}

/// A problem that did not stop the index from loading.
#[derive(Debug)]
pub enum IndexWarning {
  /// The mirror was unreachable, so the cached copy was used.
  UsedCachedCopy(HttpError),
  /// The fresh index could not be saved for next time.
  CacheNotSaved { path: PathBuf, source: io::Error },
  /// A mirror failed, so the next one in the list was tried.
  TriedNextMirror(String),
}

impl fmt::Display for IndexWarning {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      | Self::UsedCachedCopy(error) => {
        write!(f, "warning: {error}; using the cached version list")
      },
      | Self::TriedNextMirror(error) => write!(f, "warning: {error}; trying the next mirror"),
      | Self::CacheNotSaved { path, source } => {
        write!(
          f,
          "warning: cannot cache the version list in {}: {source}",
          path.display()
        )
      },
    }
  }
}

/// Why the index could not be loaded.
#[derive(Debug)]
pub enum IndexError {
  /// The mirror URL plus `index.json` is not a valid URL.
  Url(String),
  /// The request failed and there is no cached copy.
  Http(HttpError),
  /// The response body could not be read.
  Read { url: String, source: io::Error },
  /// The response is larger than [`MAX_INDEX_BYTES`].
  TooLarge { url: String },
  /// The response is not a valid index.
  Parse {
    url: String,
    source: serde_json::Error,
  },
}

impl fmt::Display for IndexError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      | Self::Url(url) => write!(f, "invalid index URL: {url}"),
      | Self::Http(error) => error.fmt(f),
      | Self::Read { url, source } => write!(f, "{url}: {source}"),
      | Self::TooLarge { url } => {
        write!(
          f,
          "{url}: larger than {MAX_INDEX_BYTES} bytes; is this a Node.js mirror?"
        )
      },
      | Self::Parse { url, source } => {
        write!(f, "{url}: not a Node.js version index: {source}")
      },
    }
  }
}

impl std::error::Error for IndexError {}

impl IndexError {
  /// Another mirror might do better: this one is unreachable, overloaded,
  /// or lacks a usable index. Credential errors are not in this group.
  #[must_use]
  pub fn allows_fallback(&self) -> bool {
    match self {
      | Self::Http(error) => error.allows_fallback(),
      | Self::Read { .. } | Self::TooLarge { .. } | Self::Parse { .. } => true,
      | Self::Url(_) => false,
    }
  }

  fn is_unreachable(&self) -> bool {
    matches!(self, Self::Http(HttpError::Transport { .. }))
  }
}

/// The cached copy of one mirror's index.
struct CachedIndex {
  body: PathBuf,
  etag: PathBuf,
}

impl CachedIndex {
  fn for_mirror(cache_dir: &Path, mirror: &Url) -> Self {
    let dir = cache_dir.join(CACHE_SUBDIR);
    let stem = format!("index-{:016x}", fnv1a(mirror.to_string().as_bytes()));
    Self {
      body: dir.join(format!("{stem}.json")),
      etag: dir.join(format!("{stem}.etag")),
    }
  }

  /// The cached index and its `ETag`, if the copy is present and valid.
  fn read(&self) -> Option<(RemoteIndex, Option<String>)> {
    let body = std::fs::read_to_string(&self.body).ok()?;
    let index = RemoteIndex::parse(&body).ok()?;
    let etag = std::fs::read_to_string(&self.etag)
      .ok()
      .map(|etag| etag.trim().to_owned())
      .filter(|etag| !etag.is_empty());
    Some((index, etag))
  }

  /// Saves the body first: a crash between the writes leaves an old `ETag`
  /// next to a newer body, which only costs one full download later.
  fn write(&self, body: &str, etag: Option<&str>) -> Result<(), IndexWarning> {
    let not_saved = |path: &Path| {
      let path = path.to_path_buf();
      move |source| IndexWarning::CacheNotSaved { path, source }
    };
    if let Some(dir) = self.body.parent() {
      std::fs::create_dir_all(dir).map_err(not_saved(dir))?;
    }
    write_atomic(&self.body, body).map_err(not_saved(&self.body))?;
    match etag {
      | Some(etag) => write_atomic(&self.etag, etag).map_err(not_saved(&self.etag)),
      | None => match std::fs::remove_file(&self.etag) {
        | Err(source) if source.kind() != io::ErrorKind::NotFound => {
          Err(not_saved(&self.etag)(source))
        },
        | _ => Ok(()),
      },
    }
  }
}

/// Loads the index from the first mirror that serves one, revalidating
/// the cached copy in `cache_dir` (normally `<root>/.cache`).
///
/// A mirror that cannot be reached, answers 404, 429, or 5xx, or serves
/// something that is not an index is skipped with a warning, and the next
/// one is tried. Credential problems (401, 403) stop the search: they need
/// fixing, not hiding. If no mirror could be reached at all (network, DNS,
/// TLS, or proxy failures only), the cached copy of the first mirror that
/// has one is used, with a warning.
///
/// # Errors
///
/// See [`IndexError`]. With several mirrors, the first mirror's error.
pub fn load_index(
  client: &Client,
  mirrors: &[Mirror],
  cache_dir: &Path,
) -> Result<Loaded, IndexError> {
  let mut skipped: Vec<IndexWarning> = Vec::new();
  let mut first_error: Option<IndexError> = None;
  let mut only_unreachable = true;
  for (position, mirror) in mirrors.iter().enumerate() {
    let is_last = position + 1 == mirrors.len();
    match load_fresh(client, mirror, cache_dir) {
      | Ok(mut loaded) => {
        loaded.mirror = position;
        skipped.append(&mut loaded.warnings);
        loaded.warnings = skipped;
        return Ok(loaded);
      },
      | Err(error) if error.allows_fallback() => {
        only_unreachable &= error.is_unreachable();
        if !is_last {
          skipped.push(IndexWarning::TriedNextMirror(error.to_string()));
        }
        first_error.get_or_insert(error);
      },
      | Err(error) => return Err(error),
    }
  }
  let error = first_error.unwrap_or_else(|| IndexError::Url("no mirror is configured".into()));
  let cached = mirrors.iter().enumerate().find_map(|(position, mirror)| {
    let (index, _) = CachedIndex::for_mirror(cache_dir, &mirror.url).read()?;
    Some((position, index))
  });
  match (cached, error) {
    | (Some((position, index)), IndexError::Http(http)) if only_unreachable => {
      skipped.retain(|warning| !matches!(warning, IndexWarning::TriedNextMirror(_)));
      skipped.push(IndexWarning::UsedCachedCopy(http));
      Ok(Loaded {
        index,
        warnings: skipped,
        mirror: position,
      })
    },
    | (_, error) => Err(error),
  }
}

/// Loads one mirror's index from the network (a 304 reuses the cached
/// copy).
fn load_fresh(client: &Client, mirror: &Mirror, cache_dir: &Path) -> Result<Loaded, IndexError> {
  let url = mirror
    .url
    .child(INDEX_FILE)
    .map_err(|_| IndexError::Url(format!("{}/{INDEX_FILE}", mirror.url)))?;
  let cache = CachedIndex::for_mirror(cache_dir, &mirror.url);
  let cached = cache.read();
  let etag = cached.as_ref().and_then(|(_, etag)| etag.clone());

  let response = match (client.get_if_none_match(&url, etag.as_deref()), cached) {
    | (Ok(Conditional::Modified(response)), _) => response,
    | (Ok(Conditional::NotModified), Some((index, _))) => {
      return Ok(Loaded {
        index,
        warnings: Vec::new(),
        mirror: 0,
      });
    },
    | (Ok(Conditional::NotModified), None) => {
      return Err(IndexError::Http(HttpError::Status {
        url: url.to_string(),
        status: NOT_MODIFIED,
      }));
    },
    | (Err(error), _) => return Err(IndexError::Http(error)),
  };

  let url = response.url.to_string();
  let body = read_limited(response.body, &url)?;
  let index = RemoteIndex::parse(&body).map_err(|source| IndexError::Parse {
    url: url.clone(),
    source,
  })?;
  let warnings = cache
    .write(&body, response.etag.as_deref())
    .err()
    .into_iter()
    .collect();
  Ok(Loaded {
    index,
    warnings,
    mirror: 0,
  })
}

fn read_limited(body: Box<dyn Read + Send>, url: &str) -> Result<String, IndexError> {
  let mut text = String::new();
  body
    .take(MAX_INDEX_BYTES + 1)
    .read_to_string(&mut text)
    .map_err(|source| IndexError::Read {
      url: url.to_owned(),
      source,
    })?;
  let is_too_large = u64::try_from(text.len()).is_ok_and(|len| len > MAX_INDEX_BYTES);
  if is_too_large {
    return Err(IndexError::TooLarge {
      url: url.to_owned(),
    });
  }
  Ok(text)
}

/// A stable, dependency-free hash for cache file names. It only has to
/// separate mirrors, not resist attackers.
fn fnv1a(bytes: &[u8]) -> u64 {
  bytes.iter().fold(FNV_OFFSET_BASIS, |hash, &byte| {
    (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME)
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn fnv1a_reference_values() {
    assert_eq!(fnv1a(b""), FNV_OFFSET_BASIS);
    assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
  }

  #[test]
  fn one_cache_file_per_mirror() {
    let url = |text: &str| Url::parse(text).unwrap();
    let a = CachedIndex::for_mirror(Path::new("/c"), &url("https://nodejs.org/dist"));
    let b = CachedIndex::for_mirror(Path::new("/c"), &url("https://mirror.corp/dist"));
    assert_ne!(a.body, b.body);
    assert!(a.body.starts_with(Path::new("/c").join(CACHE_SUBDIR)));
    assert_eq!(a.body.with_extension("etag"), a.etag);
  }

  #[test]
  fn round_trips_through_the_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = CachedIndex::for_mirror(tmp.path(), &Url::parse("https://h/d").unwrap());
    assert!(cache.read().is_none());
    let body = r#"[{"version":"v20.0.0","date":"2023-04-18","files":[],"lts":false}]"#;
    cache.write(body, Some("\"abc\"")).unwrap();
    let (index, etag) = cache.read().unwrap();
    assert_eq!(index.releases().len(), 1);
    assert_eq!(etag.as_deref(), Some("\"abc\""));
    cache.write(body, None).unwrap();
    assert_eq!(cache.read().unwrap().1, None);
  }

  #[test]
  fn ignores_a_corrupt_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = CachedIndex::for_mirror(tmp.path(), &Url::parse("https://h/d").unwrap());
    std::fs::create_dir_all(cache.body.parent().unwrap()).unwrap();
    std::fs::write(&cache.body, "[{").unwrap();
    std::fs::write(&cache.etag, "\"abc\"").unwrap();
    assert!(cache.read().is_none());
  }
}
