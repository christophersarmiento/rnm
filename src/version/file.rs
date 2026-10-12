//! Version files: `.nvmrc` and `.node-version`.
//!
//! `.nvmrc` parsing follows nvm's `nvm_process_nvmrc_content` exactly (see
//! `docs/research/nvm.md` §3.2). `.node-version` takes the first word of the
//! first non-comment line, as nodenv and fnm do.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use super::spec::{SpecError, VersionSpec};

/// The kinds of version file rnm reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VersionFileKind {
  /// `.nvmrc`, nvm's format.
  Nvmrc,
  /// `.node-version`, the format used by nodenv, fnm, and others.
  NodeVersion,
}

impl VersionFileKind {
  /// The default lookup order within one directory: `.nvmrc` wins over
  /// `.node-version`.
  pub const DEFAULT: &[Self] = &[Self::Nvmrc, Self::NodeVersion];

  /// The file name, for example `.nvmrc`.
  #[must_use]
  pub const fn file_name(self) -> &'static str {
    match self {
      | Self::Nvmrc => ".nvmrc",
      | Self::NodeVersion => ".node-version",
    }
  }
}

/// Why a version file could not be read or parsed.
#[derive(Debug)]
pub enum VersionFileError {
  /// The file could not be read.
  Io { path: PathBuf, source: io::Error },
  /// The bytes are not UTF-8 (with or without a BOM) or BOM-marked UTF-16.
  NotText,
  /// The file is empty or contains only whitespace.
  Empty,
  /// The file has content, but no version line (for example only comments or
  /// only `key=value` lines).
  NoVersion,
  /// More than one version line.
  MultipleVersions(Vec<String>),
  /// A `node=...` line, which nvm reserves and rejects.
  NodeKey,
  /// The same `key=value` key appears twice.
  DuplicateKey(String),
}

impl fmt::Display for VersionFileError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      | Self::Io { path, source } => write!(f, "cannot read '{}': {source}", path.display()),
      | Self::NotText => f.write_str("file is not UTF-8 or UTF-16 text"),
      | Self::Empty => f.write_str("file is empty"),
      | Self::NoVersion => f.write_str("file has no version line"),
      | Self::MultipleVersions(lines) => {
        write!(
          f,
          "file has more than one version line: {}",
          lines.join(", ")
        )
      },
      | Self::NodeKey => {
        f.write_str("'node' is not allowed as a key; put the version on its own line")
      },
      | Self::DuplicateKey(key) => write!(f, "key '{key}' appears more than once"),
    }
  }
}

impl std::error::Error for VersionFileError {
  fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    match self {
      | Self::Io { source, .. } => Some(source),
      | _ => None,
    }
  }
}

/// The parsed content of an `.nvmrc`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NvmrcContent {
  /// The version line, unvalidated. It can be anything in nvm's spec
  /// grammar, so parse it with [`VersionSpec`].
  pub version: String,
  /// `key=value` lines, in file order. nvm reserves these for future use and
  /// ignores them.
  pub options: Vec<(String, String)>,
}

/// A version file found on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionFile {
  pub path: PathBuf,
  pub kind: VersionFileKind,
  /// The raw version string from the file.
  pub version: String,
}

impl VersionFile {
  /// Reads and parses the version file at `path`.
  ///
  /// # Errors
  ///
  /// Returns an error if the file cannot be read, is not text, or does not
  /// contain exactly one version.
  pub fn load(path: &Path, kind: VersionFileKind) -> Result<Self, VersionFileError> {
    let bytes = std::fs::read(path).map_err(|source| VersionFileError::Io {
      path: path.to_path_buf(),
      source,
    })?;
    let text = decode_version_file(&bytes)?;
    let version = match kind {
      | VersionFileKind::Nvmrc => parse_nvmrc(&text)?.version,
      | VersionFileKind::NodeVersion => parse_node_version(&text)?,
    };
    Ok(Self {
      path: path.to_path_buf(),
      kind,
      version,
    })
  }

  /// Parses the version string as a [`VersionSpec`].
  ///
  /// # Errors
  ///
  /// Returns an error if the string is not in nvm's spec grammar.
  pub fn spec(&self) -> Result<VersionSpec, SpecError> {
    self.version.parse()
  }
}

/// Decodes version-file bytes to text.
///
/// Accepts UTF-8 with or without a BOM, and UTF-16 LE or BE with a BOM.
/// Windows PowerShell 5.1 writes UTF-16 LE with `>` (for example
/// `node -v > .nvmrc`).
///
/// # Errors
///
/// Returns [`VersionFileError::NotText`] for anything else.
pub fn decode_version_file(bytes: &[u8]) -> Result<String, VersionFileError> {
  let decode_utf16 = |rest: &[u8], from_bytes: fn([u8; 2]) -> u16| {
    let (pairs, remainder) = rest.as_chunks::<2>();
    if !remainder.is_empty() {
      return Err(VersionFileError::NotText);
    }
    let units = pairs.iter().map(|&pair| from_bytes(pair));
    char::decode_utf16(units)
      .collect::<Result<String, _>>()
      .map_err(|_| VersionFileError::NotText)
  };

  match bytes {
    | [0xEF, 0xBB, 0xBF, rest @ ..] => utf8(rest),
    | [0xFF, 0xFE, rest @ ..] => decode_utf16(rest, u16::from_le_bytes),
    | [0xFE, 0xFF, rest @ ..] => decode_utf16(rest, u16::from_be_bytes),
    | _ => utf8(bytes),
  }
}

fn utf8(bytes: &[u8]) -> Result<String, VersionFileError> {
  std::str::from_utf8(bytes)
    .map(str::to_owned)
    .map_err(|_| VersionFileError::NotText)
}

/// Trims POSIX `[[:space:]]`: space, tab, LF, VT, FF, CR.
fn trim_space(s: &str) -> &str {
  s.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\x0B' | '\x0C' | '\r'))
}

/// Removes a `#` comment and surrounding whitespace from one line.
fn strip_comment(line: &str) -> &str {
  trim_space(line.split_once('#').map_or(line, |(before, _)| before))
}

/// Parses `.nvmrc` content with nvm's rules.
///
/// 1. On every line, strip `#` and everything after it, then trim whitespace
///    (this also removes the CR of CRLF line endings). Drop empty lines.
/// 2. A line containing `=` is a `key=value` option, unless it starts with
///    `=`. Keys and values are trimmed. The key `node` and duplicate keys are
///    errors.
/// 3. Every other line is a version line. There must be exactly one.
///
/// # Errors
///
/// See [`VersionFileError`].
pub fn parse_nvmrc(content: &str) -> Result<NvmrcContent, VersionFileError> {
  if trim_space(content).is_empty() {
    return Err(VersionFileError::Empty);
  }

  let mut versions = Vec::new();
  let mut options: Vec<(String, String)> = Vec::new();

  for line in content
    .split('\n')
    .map(strip_comment)
    .filter(|line| !line.is_empty())
  {
    match line.split_once('=') {
      // nvm counts a line that starts with `=` as a version line.
      | Some((key, value)) if !line.starts_with('=') => {
        let key = trim_space(key);
        if key == "node" {
          return Err(VersionFileError::NodeKey);
        }
        if options.iter().any(|(existing, _)| existing == key) {
          return Err(VersionFileError::DuplicateKey(key.to_owned()));
        }
        options.push((key.to_owned(), trim_space(value).to_owned()));
      },
      | _ => versions.push(line.to_owned()),
    }
  }

  match versions.len() {
    | 0 => Err(VersionFileError::NoVersion),
    | 1 => Ok(NvmrcContent {
      version: versions.remove(0),
      options,
    }),
    | _ => Err(VersionFileError::MultipleVersions(versions)),
  }
}

/// Parses `.node-version` content: the first word of the first line that is
/// not empty after removing `#` comments.
///
/// # Errors
///
/// Returns [`VersionFileError::Empty`] or [`VersionFileError::NoVersion`] if
/// there is no such word.
pub fn parse_node_version(content: &str) -> Result<String, VersionFileError> {
  if trim_space(content).is_empty() {
    return Err(VersionFileError::Empty);
  }
  content
    .split('\n')
    .map(strip_comment)
    .find(|line| !line.is_empty())
    .and_then(|line| line.split_ascii_whitespace().next())
    .map(str::to_owned)
    .ok_or(VersionFileError::NoVersion)
}

/// Finds the nearest version file, walking up from `start` to the root.
///
/// In each directory, `kinds` are checked in order, so with
/// [`VersionFileKind::DEFAULT`] an `.nvmrc` wins over a `.node-version` in the
/// same directory, while a `.node-version` in a closer directory wins over an
/// `.nvmrc` further up.
///
/// Like nvm, the walk uses the *logical* path it is given (pass `$PWD`, not a
/// canonicalized path, so symlinked directories are walked by their link
/// path). It does not stop at `$HOME` or a repository root, and it checks the
/// root directory itself. `start` should be absolute.
#[must_use]
pub fn find_version_file(
  start: &Path,
  kinds: &[VersionFileKind],
) -> Option<(PathBuf, VersionFileKind)> {
  start
    .ancestors()
    .filter(|dir| !dir.as_os_str().is_empty())
    .find_map(|dir| {
      kinds.iter().find_map(|&kind| {
        let path = dir.join(kind.file_name());
        path.is_file().then_some((path, kind))
      })
    })
}

#[cfg(test)]
mod tests {
  use super::*;

  fn nvmrc(s: &str) -> String {
    parse_nvmrc(s)
      .unwrap_or_else(|e| panic!("{s:?}: {e}"))
      .version
  }

  #[test]
  fn nvmrc_simple() {
    assert_eq!(nvmrc("18"), "18");
    assert_eq!(nvmrc("v20.11.1\n"), "v20.11.1");
    assert_eq!(nvmrc("lts/*"), "lts/*");
  }

  #[test]
  fn nvmrc_whitespace_and_crlf() {
    assert_eq!(nvmrc("  18  \r\n"), "18");
    assert_eq!(nvmrc("\n\n\t lts/iron\t\r\n\r\n"), "lts/iron");
    assert_eq!(nvmrc("\x0B\x0C18\x0C"), "18");
  }

  #[test]
  fn nvmrc_comments() {
    assert_eq!(nvmrc("# pinned for CI\n20 # current LTS\n"), "20");
    assert_eq!(nvmrc("20#comment"), "20");
    assert_eq!(nvmrc("#only a comment\n18"), "18");
  }

  #[test]
  fn nvmrc_options() {
    let parsed = parse_nvmrc("18\nfoo = bar\nbaz=\n").unwrap();
    assert_eq!(parsed.version, "18");
    assert_eq!(
      parsed.options,
      vec![
        ("foo".to_owned(), "bar".to_owned()),
        ("baz".to_owned(), String::new())
      ]
    );
    // A line starting with `=` is a version line in nvm.
    assert_eq!(nvmrc("=x"), "=x");
    assert!(matches!(
      parse_nvmrc("18\n=x"),
      Err(VersionFileError::MultipleVersions(_))
    ));
  }

  #[test]
  fn nvmrc_errors() {
    assert!(matches!(parse_nvmrc(""), Err(VersionFileError::Empty)));
    assert!(matches!(
      parse_nvmrc(" \r\n\t"),
      Err(VersionFileError::Empty)
    ));
    assert!(matches!(
      parse_nvmrc("# nothing"),
      Err(VersionFileError::NoVersion)
    ));
    assert!(matches!(
      parse_nvmrc("foo=bar"),
      Err(VersionFileError::NoVersion)
    ));
    assert!(matches!(
      parse_nvmrc("node=18"),
      Err(VersionFileError::NodeKey)
    ));
    assert!(matches!(
      parse_nvmrc(" node = 18\n18"),
      Err(VersionFileError::NodeKey)
    ));
    assert!(matches!(
      parse_nvmrc("18\na=1\na=2"),
      Err(VersionFileError::DuplicateKey(k)) if k == "a"
    ));
    match parse_nvmrc("18\n20\n") {
      | Err(VersionFileError::MultipleVersions(lines)) => assert_eq!(lines, ["18", "20"]),
      | other => panic!("{other:?}"),
    }
  }

  #[test]
  fn node_version_file() {
    assert_eq!(parse_node_version("18.17.0\n").unwrap(), "18.17.0");
    assert_eq!(parse_node_version("  v20  \r\n").unwrap(), "v20");
    assert_eq!(
      parse_node_version("# comment\n\n22 extra words\n24\n").unwrap(),
      "22"
    );
    assert!(matches!(
      parse_node_version(" \n"),
      Err(VersionFileError::Empty)
    ));
    assert!(matches!(
      parse_node_version("# x\n"),
      Err(VersionFileError::NoVersion)
    ));
  }

  #[test]
  fn decodes_boms() {
    assert_eq!(decode_version_file(b"18\n").unwrap(), "18\n");
    assert_eq!(decode_version_file(b"\xEF\xBB\xBF18\n").unwrap(), "18\n");

    let utf16 = |s: &str, le: bool| -> Vec<u8> {
      let mut out = if le {
        vec![0xFF, 0xFE]
      } else {
        vec![0xFE, 0xFF]
      };
      for unit in s.encode_utf16() {
        out.extend(if le {
          unit.to_le_bytes()
        } else {
          unit.to_be_bytes()
        });
      }
      out
    };
    assert_eq!(
      decode_version_file(&utf16("v20.0.0\r\n", true)).unwrap(),
      "v20.0.0\r\n"
    );
    assert_eq!(
      decode_version_file(&utf16("lts/*", false)).unwrap(),
      "lts/*"
    );

    assert!(matches!(
      decode_version_file(b"\xFF\xFE1"),
      Err(VersionFileError::NotText)
    ));
    assert!(matches!(
      decode_version_file(b"\xC3\x28"),
      Err(VersionFileError::NotText)
    ));
  }

  #[test]
  fn finds_nearest_file_walking_up() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    let nested = project.join("a").join("b");
    std::fs::create_dir_all(&nested).unwrap();

    assert_eq!(find_version_file(&nested, VersionFileKind::DEFAULT), None);

    std::fs::write(root.path().join(".nvmrc"), "18").unwrap();
    assert_eq!(
      find_version_file(&nested, VersionFileKind::DEFAULT),
      Some((root.path().join(".nvmrc"), VersionFileKind::Nvmrc))
    );

    // A closer `.node-version` wins over a farther `.nvmrc`.
    std::fs::write(project.join(".node-version"), "20").unwrap();
    assert_eq!(
      find_version_file(&nested, VersionFileKind::DEFAULT),
      Some((project.join(".node-version"), VersionFileKind::NodeVersion))
    );

    // In the same directory, `.nvmrc` wins.
    std::fs::write(project.join(".nvmrc"), "22").unwrap();
    assert_eq!(
      find_version_file(&nested, VersionFileKind::DEFAULT),
      Some((project.join(".nvmrc"), VersionFileKind::Nvmrc))
    );

    // Restricting kinds skips `.nvmrc`.
    assert_eq!(
      find_version_file(&nested, &[VersionFileKind::NodeVersion]),
      Some((project.join(".node-version"), VersionFileKind::NodeVersion))
    );
  }

  #[test]
  fn ignores_directories_named_like_version_files() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".nvmrc")).unwrap();
    std::fs::write(root.path().join(".node-version"), "20").unwrap();
    assert_eq!(
      find_version_file(root.path(), VersionFileKind::DEFAULT),
      Some((
        root.path().join(".node-version"),
        VersionFileKind::NodeVersion
      ))
    );
  }

  #[test]
  fn loads_and_parses_spec() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(".nvmrc");
    std::fs::write(&path, "\u{FEFF}lts/iron # LTS\r\n").unwrap();
    let file = VersionFile::load(&path, VersionFileKind::Nvmrc).unwrap();
    assert_eq!(file.version, "lts/iron");
    assert_eq!(file.spec().unwrap().to_string(), "lts/iron");

    let missing = VersionFile::load(&root.path().join("nope"), VersionFileKind::Nvmrc);
    assert!(matches!(missing, Err(VersionFileError::Io { .. })));
  }
}
