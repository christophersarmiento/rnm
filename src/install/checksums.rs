//! `SHASUMS256.txt`: one `<sha256 hex>  <file name>` line per artifact.

use std::fmt;

/// The length of a SHA-256 digest in bytes.
pub const SHA256_LEN: usize = 32;

/// A SHA-256 digest.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Sha256(pub [u8; SHA256_LEN]);

impl Sha256 {
  /// Parses 64 hex digits, in either case.
  #[must_use]
  pub fn from_hex(hex: &str) -> Option<Self> {
    let has_expected_length = hex.len() == SHA256_LEN * 2;
    if !has_expected_length || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
      return None;
    }
    let mut digest = [0; SHA256_LEN];
    let (pairs, _) = hex.as_bytes().as_chunks::<2>();
    for (byte, pair) in digest.iter_mut().zip(pairs) {
      let pair = std::str::from_utf8(pair).ok()?;
      *byte = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(Self(digest))
  }
}

impl fmt::Display for Sha256 {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
  }
}

impl fmt::Debug for Sha256 {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    fmt::Display::fmt(self, f)
  }
}

/// The digest listed for `file_name`. A leading `*` on the name (the
/// "binary mode" marker of `sha256sum`) is ignored.
#[must_use]
pub fn find_checksum(shasums: &str, file_name: &str) -> Option<Sha256> {
  shasums.lines().find_map(|line| {
    let mut fields = line.split_whitespace();
    let digest = fields.next()?;
    let name = fields.next()?;
    let is_only_name = fields.next().is_none();
    let names_the_file = name.strip_prefix('*').unwrap_or(name) == file_name;
    (is_only_name && names_the_file)
      .then(|| Sha256::from_hex(digest))
      .flatten()
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  const XZ: &str = "4f0bbd9a0f5d9b7a1e1c9d1a0c2a2b5c3e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b";
  const GZ: &str = "AAAABBBBCCCCDDDDEEEEFFFF0000111122223333444455556666777788889999";

  #[test]
  fn finds_the_named_file() {
    let shasums = format!(
      "{XZ}  node-v20.0.0-linux-x64.tar.xz\n{GZ} *node-v20.0.0-linux-x64.tar.gz\n\
       {XZ}  win-x64/node.exe\n"
    );
    assert_eq!(
      find_checksum(&shasums, "node-v20.0.0-linux-x64.tar.xz")
        .unwrap()
        .to_string(),
      XZ
    );
    assert_eq!(
      find_checksum(&shasums, "node-v20.0.0-linux-x64.tar.gz")
        .unwrap()
        .to_string(),
      GZ.to_ascii_lowercase()
    );
    assert_eq!(find_checksum(&shasums, "node-v20.0.0-linux-x64"), None);
    assert_eq!(find_checksum(&shasums, "node.exe"), None);
  }

  #[test]
  fn rejects_malformed_lines() {
    assert_eq!(find_checksum("abc  f\n", "f"), None);
    assert_eq!(find_checksum(&format!("{XZ}  f extra\n"), "f"), None);
    assert_eq!(find_checksum(&format!("{}g  f\n", &XZ[..63]), "f"), None);
    assert_eq!(find_checksum("", "f"), None);
    assert_eq!(find_checksum(&format!("+{}  f\n", &XZ[..63]), "f"), None);
  }
}
