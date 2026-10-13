//! nvm's alias files: `alias/<name>` holds one target line.
//!
//! Reading follows `nvm_print_alias_file`: strip `#` comments, skip blank
//! lines, and use the first remaining line. Writing produces `target\n`, as
//! `nvm alias` does. Names may contain `/` (for example `lts/iron`), but never
//! a `.` or `..` component.

use std::fmt::{self, Write as _};
use std::io;
use std::path::{Path, PathBuf};

use crate::fs::write_atomic;
use crate::version::{LATEST_LTS, LTS_PREFIX, is_valid_alias_component};

/// The subdirectory that holds `lts/<codename>` aliases.
const LTS_DIR: &str = "lts";

/// Characters Windows forbids in file names, plus `%`, the escape character
/// of [`encode_component`].
const WINDOWS_RESERVED: [char; 8] = ['<', '>', ':', '"', '|', '?', '*', '%'];

/// `%` followed by two hex digits.
const ESCAPE_LEN: usize = 3;

/// The alias store under `<root>/alias`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Aliases {
  dir: PathBuf,
}

/// Why an alias operation failed.
#[derive(Debug)]
pub enum AliasError {
  /// The name is empty, absolute, or has an empty, `.`, or `..` component,
  /// or a character that cannot appear in an alias file name.
  InvalidName(String),
  /// The target is empty or spans several lines.
  InvalidTarget(String),
  /// A file-system error.
  Io { path: PathBuf, source: io::Error },
}

impl fmt::Display for AliasError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      | Self::InvalidName(name) => write!(f, "invalid alias name: '{name}'"),
      | Self::InvalidTarget(target) => write!(f, "invalid alias target: '{target}'"),
      | Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
    }
  }
}

impl std::error::Error for AliasError {
  fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    match self {
      | Self::Io { source, .. } => Some(source),
      | _ => None,
    }
  }
}

impl Aliases {
  /// Uses `dir` (normally `<root>/alias`) as the store.
  #[must_use]
  pub fn new(dir: impl Into<PathBuf>) -> Self {
    Self { dir: dir.into() }
  }

  /// The alias directory.
  #[must_use]
  pub fn dir(&self) -> &Path {
    &self.dir
  }

  /// Reads the target of alias `name`.
  ///
  /// Returns `Ok(None)` if the alias does not exist, or if the file has no
  /// target line (nvm treats an empty alias file as missing).
  ///
  /// # Errors
  ///
  /// Returns an error for an invalid name, or if the file exists but cannot
  /// be read.
  pub fn get(&self, name: &str) -> Result<Option<String>, AliasError> {
    let path = self.path(name)?;
    let bytes = match std::fs::read(&path) {
      | Ok(bytes) => bytes,
      | Err(e) if is_missing(&e) => return Ok(None),
      | Err(source) => return Err(AliasError::Io { path, source }),
    };
    Ok(parse_alias_file(&String::from_utf8_lossy(&bytes)))
  }

  /// Writes alias `name` with `target`, creating directories as needed. The
  /// file is replaced atomically.
  ///
  /// # Errors
  ///
  /// Returns an error for an invalid name or target, or on I/O failure.
  pub fn set(&self, name: &str, target: &str) -> Result<(), AliasError> {
    let path = self.path(name)?;
    let target = target.trim();
    let is_single_line = !target.contains(['\n', '\r']);
    let has_comment = target.contains('#');
    if target.is_empty() || !is_single_line || has_comment {
      return Err(AliasError::InvalidTarget(target.to_owned()));
    }
    let io_error = |source| AliasError::Io {
      path: path.clone(),
      source,
    };
    std::fs::create_dir_all(path.parent().unwrap_or(&self.dir)).map_err(io_error)?;
    write_atomic(&path, &format!("{target}\n")).map_err(io_error)
  }

  /// Removes alias `name`. Returns `false` if it did not exist.
  ///
  /// # Errors
  ///
  /// Returns an error for an invalid name, or on I/O failure.
  pub fn remove(&self, name: &str) -> Result<bool, AliasError> {
    let path = self.path(name)?;
    match std::fs::remove_file(&path) {
      | Ok(()) => Ok(true),
      | Err(e) if is_missing(&e) => Ok(false),
      | Err(source) => Err(AliasError::Io { path, source }),
    }
  }

  /// Lists `(name, target)` pairs, sorted by name. User aliases come from
  /// `alias/`; LTS aliases from `alias/lts/` are included as `lts/<name>`
  /// when `include_lts` is set. Files without a target line are skipped.
  ///
  /// # Errors
  ///
  /// Returns an error if a directory exists but cannot be read.
  pub fn list(&self, include_lts: bool) -> Result<Vec<(String, String)>, AliasError> {
    let mut out = Vec::new();
    for name in file_names(&self.dir)? {
      if let Some(target) = self.get(&name)? {
        out.push((name, target));
      }
    }
    if include_lts {
      for name in file_names(&self.dir.join(LTS_DIR))? {
        let name = format!("{LTS_PREFIX}{name}");
        if let Some(target) = self.get(&name)? {
          out.push((name, target));
        }
      }
    }
    out.sort();
    Ok(out)
  }

  /// LTS codenames with an alias file under `alias/lts/`, in alphabetical
  /// order, excluding the `*` entry.
  ///
  /// # Errors
  ///
  /// Returns an error if the directory exists but cannot be read.
  pub fn lts_names(&self) -> Result<Vec<String>, AliasError> {
    let mut names = file_names(&self.dir.join(LTS_DIR))?;
    names.retain(|name| name != LATEST_LTS);
    names.sort();
    Ok(names)
  }

  /// The file for alias `name`, after validating it.
  fn path(&self, name: &str) -> Result<PathBuf, AliasError> {
    let invalid = || AliasError::InvalidName(name.to_owned());
    if name.is_empty() {
      return Err(invalid());
    }
    let mut path = self.dir.clone();
    for component in name.split('/') {
      if !is_valid_alias_component(component) {
        return Err(invalid());
      }
      path.push(encode_component(component).as_ref());
    }
    Ok(path)
  }
}

/// The decoded names of regular files directly inside `dir`, skipping hidden
/// files (rnm's temp files start with `.`).
fn file_names(dir: &Path) -> Result<Vec<String>, AliasError> {
  let io_error = |source| AliasError::Io {
    path: dir.to_path_buf(),
    source,
  };
  let entries = match std::fs::read_dir(dir) {
    | Ok(entries) => entries,
    | Err(e) if is_missing(&e) => return Ok(Vec::new()),
    | Err(e) => return Err(io_error(e)),
  };
  let mut names = Vec::new();
  for entry in entries {
    let entry = entry.map_err(io_error)?;
    if !entry.file_type().map_err(io_error)?.is_file() {
      continue;
    }
    if let Some(name) = entry.file_name().to_str()
      && !name.starts_with('.')
    {
      names.push(decode_component(name));
    }
  }
  Ok(names)
}

/// The first target line of an alias file: strip `#` comments, skip blank
/// lines, trim whitespace.
fn parse_alias_file(content: &str) -> Option<String> {
  content
    .lines()
    .map(|line| {
      line
        .split_once('#')
        .map_or(line, |(before, _)| before)
        .trim()
    })
    .find(|line| !line.is_empty())
    .map(str::to_owned)
}

fn is_missing(e: &io::Error) -> bool {
  // `NotADirectory` covers an alias like `a/b` where `a` is a file.
  matches!(
    e.kind(),
    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
  )
}

/// Windows file names cannot contain `<>:"|?*`, but nvm's tree has a file
/// named `*` (`alias/lts/*`). The Windows tree is rnm's own, so those
/// characters (and `%`, the escape character) are percent-encoded there.
/// On unix, names are used as-is, matching nvm.
fn encode_component(component: &str) -> std::borrow::Cow<'_, str> {
  if cfg!(windows) && component.contains(WINDOWS_RESERVED) {
    let mut out = String::with_capacity(component.len());
    for c in component.chars() {
      if WINDOWS_RESERVED.contains(&c) {
        let _ = write!(out, "%{:02X}", u32::from(c));
      } else {
        out.push(c);
      }
    }
    out.into()
  } else {
    component.into()
  }
}

fn decode_component(name: &str) -> String {
  if !cfg!(windows) || !name.contains('%') {
    return name.to_owned();
  }
  let mut out = String::with_capacity(name.len());
  let mut rest = name;
  while let Some(index) = rest.find('%') {
    out.push_str(&rest[..index]);
    let hex = rest.get(index + 1..index + ESCAPE_LEN);
    match hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
      | Some(byte) => {
        out.push(char::from(byte));
        rest = &rest[index + ESCAPE_LEN..];
      },
      | None => {
        out.push('%');
        rest = &rest[index + 1..];
      },
    }
  }
  out.push_str(rest);
  out
}

#[cfg(test)]
mod tests {
  use super::*;

  fn store() -> (tempfile::TempDir, Aliases) {
    let tmp = tempfile::tempdir().unwrap();
    let aliases = Aliases::new(tmp.path().join("alias"));
    (tmp, aliases)
  }

  #[test]
  fn parses_like_nvm() {
    assert_eq!(parse_alias_file("v20.11.1\n"), Some("v20.11.1".to_owned()));
    assert_eq!(
      parse_alias_file("# note\n\n  lts/iron  # newest\nignored\n"),
      Some("lts/iron".to_owned())
    );
    assert_eq!(parse_alias_file("18\r\n"), Some("18".to_owned()));
    assert_eq!(parse_alias_file(""), None);
    assert_eq!(parse_alias_file("# only\n \n"), None);
  }

  #[test]
  fn set_get_remove() {
    let (_tmp, aliases) = store();
    assert_eq!(aliases.get("default").unwrap(), None);
    aliases.set("default", "lts/*").unwrap();
    assert_eq!(aliases.get("default").unwrap(), Some("lts/*".to_owned()));
    assert_eq!(
      std::fs::read_to_string(aliases.dir().join("default")).unwrap(),
      "lts/*\n"
    );
    aliases.set("default", " 20 ").unwrap();
    assert_eq!(aliases.get("default").unwrap(), Some("20".to_owned()));
    assert!(aliases.remove("default").unwrap());
    assert!(!aliases.remove("default").unwrap());
    assert_eq!(aliases.get("default").unwrap(), None);
  }

  #[test]
  fn nested_names_and_listing() {
    let (_tmp, aliases) = store();
    aliases.set("lts/iron", "v20.19.0").unwrap();
    aliases.set("lts/jod", "v22.20.0").unwrap();
    aliases.set("lts/*", "lts/jod").unwrap();
    aliases.set("default", "lts/*").unwrap();
    aliases.set("work", "18").unwrap();
    std::fs::write(aliases.dir().join("empty"), "# nothing\n").unwrap();

    assert_eq!(aliases.get("lts/*").unwrap(), Some("lts/jod".to_owned()));
    assert_eq!(aliases.lts_names().unwrap(), ["iron", "jod"]);
    assert_eq!(
      aliases.list(false).unwrap(),
      [
        ("default".to_owned(), "lts/*".to_owned()),
        ("work".to_owned(), "18".to_owned())
      ]
    );
    let names: Vec<String> = aliases
      .list(true)
      .unwrap()
      .into_iter()
      .map(|(n, _)| n)
      .collect();
    assert_eq!(names, ["default", "lts/*", "lts/iron", "lts/jod", "work"]);
  }

  #[test]
  fn rejects_bad_names_and_targets() {
    let (_tmp, aliases) = store();
    for bad in [
      "", "/abs", "a//b", "a/", "..", "a/../b", ".", "a b", "a#b", "a\\b",
    ] {
      assert!(
        matches!(aliases.set(bad, "18"), Err(AliasError::InvalidName(_))),
        "{bad:?}"
      );
      assert!(
        matches!(aliases.get(bad), Err(AliasError::InvalidName(_))),
        "{bad:?}"
      );
    }
    for bad in ["", "  ", "18\n20", "18 # x"] {
      assert!(
        matches!(aliases.set("x", bad), Err(AliasError::InvalidTarget(_))),
        "{bad:?}"
      );
    }
  }

  #[test]
  fn alias_under_a_file_is_missing() {
    let (_tmp, aliases) = store();
    aliases.set("default", "18").unwrap();
    assert_eq!(aliases.get("default/x").unwrap(), None);
  }

  #[cfg(windows)]
  #[test]
  fn encodes_reserved_characters_on_windows() {
    let (_tmp, aliases) = store();
    aliases.set("lts/*", "lts/jod").unwrap();
    assert!(aliases.dir().join("lts").join("%2A").is_file());
    assert_eq!(aliases.get("lts/*").unwrap(), Some("lts/jod".to_owned()));
    let names: Vec<String> = aliases
      .list(true)
      .unwrap()
      .into_iter()
      .map(|(n, _)| n)
      .collect();
    assert_eq!(names, ["lts/*"]);
    assert_eq!(decode_component("a%25b%2"), "a%b%2");
  }
}
