//! nvm's LTS alias files, rewritten from the index after every fetch so that
//! `lts/*`, `lts/<codename>`, and `lts/-N` resolve offline, in rnm and nvm.

use super::index::RemoteIndex;
use crate::nvm_dir::{AliasError, Aliases};
use crate::version::{LATEST_LTS, LTS_PREFIX};

/// Points `alias/lts/<codename>` at the newest `vX.Y.Z` of each LTS line and
/// `alias/lts/*` at `lts/<newest codename>`, as `nvm ls-remote` does.
/// Files that already hold the right target are not rewritten, and aliases
/// for lines missing from the index are kept.
///
/// # Errors
///
/// Fails if an alias file cannot be read or written.
pub fn refresh_lts_aliases(aliases: &Aliases, index: &RemoteIndex) -> Result<(), AliasError> {
  let lines = index.lts_lines();
  for (name, release) in &lines {
    set_if_changed(
      aliases,
      &format!("{LTS_PREFIX}{name}"),
      &release.version.to_string(),
    )?;
  }
  if let Some((newest, _)) = lines.first() {
    set_if_changed(
      aliases,
      &format!("{LTS_PREFIX}{LATEST_LTS}"),
      &format!("{LTS_PREFIX}{newest}"),
    )?;
  }
  Ok(())
}

fn set_if_changed(aliases: &Aliases, name: &str, target: &str) -> Result<(), AliasError> {
  if aliases.get(name)?.as_deref() == Some(target) {
    return Ok(());
  }
  aliases.set(name, target)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::remote::Release;

  fn release(version: &str, lts: Option<&str>) -> Release {
    Release {
      version: version.parse().unwrap(),
      date: String::new(),
      files: Vec::new(),
      npm: None,
      lts: lts.map(str::to_owned),
      security: false,
    }
  }

  #[test]
  fn writes_nvm_lts_aliases() {
    let tmp = tempfile::tempdir().unwrap();
    let aliases = Aliases::new(tmp.path().join("alias"));
    aliases.set("lts/argon", "v4.9.1").unwrap();
    aliases.set("lts/iron", "v20.0.0").unwrap();
    let index = RemoteIndex::from_releases(vec![
      release("20.19.4", Some("Iron")),
      release("20.19.5", Some("Iron")),
      release("22.20.0", Some("Jod")),
      release("24.9.0", None),
    ]);

    refresh_lts_aliases(&aliases, &index).unwrap();

    let read = |name: &str| aliases.get(name).unwrap().unwrap();
    assert_eq!(read("lts/iron"), "v20.19.5");
    assert_eq!(read("lts/jod"), "v22.20.0");
    assert_eq!(read("lts/*"), "lts/jod");
    assert_eq!(read("lts/argon"), "v4.9.1");
  }

  #[test]
  fn leaves_aliases_alone_without_lts_releases() {
    let tmp = tempfile::tempdir().unwrap();
    let aliases = Aliases::new(tmp.path().join("alias"));
    refresh_lts_aliases(&aliases, &RemoteIndex::default()).unwrap();
    assert!(!aliases.dir().exists());
  }
}
