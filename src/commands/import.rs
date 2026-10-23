//! `rnm import nvm-windows`: adopt the versions nvm-windows installed
//! (`docs/research/nvm-windows.md` §3.4 and §10.6).
//!
//! By default each version directory is linked with a junction, so nothing
//! is copied and nvm-windows keeps working. `--move` renames the
//! directories into rnm's tree (fast on one volume, but nvm-windows loses
//! them); `--copy` duplicates them. nvm-windows's registry settings, PATH,
//! and files are never changed.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::{Context, DEFAULT_ALIAS, Failure, exit, say};
use crate::nvm_dir::{Flavor, is_executable, node_binary};
use crate::style;
use crate::version::{Version, VersionSpec, looks_like_range};

/// How imported versions get into rnm's tree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImportMode {
  /// A junction (a symlink on unix) to nvm-windows's directory.
  #[default]
  Link,
  /// Rename the directory into rnm's tree.
  Move,
  /// Copy the directory into rnm's tree.
  Copy,
}

impl ImportMode {
  fn past_tense(self) -> &'static str {
    match self {
      | Self::Link => "linked",
      | Self::Move => "moved",
      | Self::Copy => "copied",
    }
  }
}

/// Options for [`import_nvm_windows`].
#[derive(Clone, Debug, Default)]
pub struct ImportOptions {
  /// nvm-windows's install root. Found automatically on Windows.
  pub from: Option<PathBuf>,
  pub mode: ImportMode,
}

/// nvm-windows v2 keeps its settings here, under `HKEY_CURRENT_USER`.
#[cfg_attr(not(windows), allow(dead_code, reason = "read only on Windows"))]
const SETTINGS_KEY: &str = r"Software\Author Software\Preferences\nvm";

/// Prefix of the temporary directory a copy is written to before it is
/// renamed into place. Starts with `.`, so version listings skip it.
const IMPORTING_PREFIX: &str = ".rnm-importing-";

/// What nvm-windows's settings say, read from the registry on Windows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct NvmWindowsSettings {
  /// v2's `InstallRoot`.
  install_root: Option<String>,
  /// `name=version` pairs from `Aliases`.
  aliases: Vec<(String, String)>,
  /// `ActiveVersion`, which becomes rnm's `default` alias.
  active_version: Option<String>,
}

#[cfg(windows)]
fn registry_settings() -> NvmWindowsSettings {
  use crate::sys::{Hive, registry_string, registry_strings};
  let read =
    |name| registry_string(Hive::CurrentUser, SETTINGS_KEY, name).filter(|v| !v.is_empty());
  let aliases = registry_strings(Hive::CurrentUser, SETTINGS_KEY, "Aliases")
    .unwrap_or_default()
    .iter()
    .filter_map(|entry| {
      let (name, version) = entry.split_once('=')?;
      Some((name.trim().to_owned(), version.trim().to_owned()))
    })
    .collect();
  NvmWindowsSettings {
    install_root: read("InstallRoot"),
    aliases,
    active_version: read("ActiveVersion"),
  }
}

#[cfg(not(windows))]
fn registry_settings() -> NvmWindowsSettings {
  NvmWindowsSettings::default()
}

/// Imports nvm-windows's versions, then its aliases and active version
/// (as `default`) where rnm has none of the same name.
///
/// # Errors
///
/// Exit code 1 when no nvm-windows install is found or a version cannot be
/// imported.
pub fn import_nvm_windows(
  ctx: &Context,
  opts: &ImportOptions,
  out: &mut dyn Write,
  err: &mut dyn Write,
) -> Result<(), Failure> {
  let settings = registry_settings();
  let candidates = match &opts.from {
    | Some(from) => vec![from.clone()],
    | None => candidate_roots(|key| ctx.var(key), settings.install_root.as_deref()),
  };
  let Some((root, versions)) = candidates.iter().find_map(|root| {
    let versions = versions_in(root);
    (!versions.is_empty()).then(|| (root.clone(), versions))
  }) else {
    return Err(no_install_found(&candidates));
  };

  say(
    out,
    format_args!("Importing from nvm-windows in {}", root.display()),
  );
  let mut imported = 0;
  for (version, source) in &versions {
    let target = ctx.nvm_dir.version_dir(Flavor::Node, version);
    if target.exists() || crate::fs::is_link(&target) {
      say(
        out,
        format_args!("{version} is already installed; skipped."),
      );
      continue;
    }
    bring_in(source, &target, opts.mode).map_err(|e| {
      Failure::new(
        exit::ERROR,
        format!(
          "cannot import {version} from {}: {e}{}",
          source.display(),
          move_hint(opts.mode)
        ),
      )
    })?;
    imported += 1;
    say(
      out,
      format_args!(
        "Imported {version} ({} from {}).",
        opts.mode.past_tense(),
        source.display()
      ),
    );
  }
  import_aliases(ctx, &settings, out, err)?;
  super::default_version::refresh_persistent_default(ctx, err);
  say(
    out,
    format_args!(
      "{imported} of {} versions imported. nvm-windows itself was not changed.",
      versions.len()
    ),
  );
  Ok(())
}

fn move_hint(mode: ImportMode) -> &'static str {
  match mode {
    | ImportMode::Move => " (a move only works on one drive; try --copy)",
    | _ => "",
  }
}

fn no_install_found(searched: &[PathBuf]) -> Failure {
  let message = if searched.is_empty() {
    "No nvm-windows install found. nvm-windows runs only on Windows; pass --from <dir> to \
     import from a copy of its install directory."
      .to_owned()
  } else {
    let list: Vec<String> = searched
      .iter()
      .map(|path| format!("  {}", path.display()))
      .collect();
    format!(
      "No nvm-windows versions found. Searched:\n{}\nPass --from <dir> if nvm-windows keeps \
       its versions elsewhere.",
      list.join("\n")
    )
  };
  Failure::new(exit::ERROR, message)
}

/// Where nvm-windows may keep its versions, most specific first: v2's
/// registry `InstallRoot`, v1's `root:` in `%NVM_HOME%\settings.txt`, then
/// the default locations of both versions.
fn candidate_roots(
  env: impl Fn(&str) -> Option<OsString>,
  registry_install_root: Option<&str>,
) -> Vec<PathBuf> {
  let var = |key: &str| env(key).map(PathBuf::from);
  let mut roots: Vec<PathBuf> = Vec::new();
  roots.extend(registry_install_root.map(PathBuf::from));
  if let Some(home) = var("NVM_HOME") {
    roots.extend(settings_txt_root(&home.join("settings.txt")));
    roots.push(home);
  }
  if let Some(local) = var("LOCALAPPDATA") {
    roots.push(local.join("Author Software").join("nvm").join("installs"));
    roots.push(local.join("nvm"));
  }
  roots.extend(var("APPDATA").map(|roaming| roaming.join("nvm")));
  let mut unique = Vec::new();
  for root in roots {
    if !unique.contains(&root) {
      unique.push(root);
    }
  }
  unique
}

/// v1's `root:` setting, parsed as nvm-windows's installer does: the key is
/// matched case-insensitively and split at the first `:`.
fn settings_txt_root(file: &Path) -> Option<PathBuf> {
  let text = std::fs::read_to_string(file).ok()?;
  text.lines().find_map(|line| {
    let (key, value) = line.split_once(':')?;
    let value = value.trim();
    (key.trim().eq_ignore_ascii_case("root") && !value.is_empty()).then(|| PathBuf::from(value))
  })
}

/// The `v<version>` directories with a `node` binary, oldest first.
fn versions_in(root: &Path) -> Vec<(Version, PathBuf)> {
  let Ok(entries) = std::fs::read_dir(root) else {
    return Vec::new();
  };
  let mut versions: Vec<(Version, PathBuf)> = entries
    .flatten()
    .filter_map(|entry| {
      let name = entry.file_name().to_string_lossy().into_owned();
      let version: Version = name.strip_prefix('v')?.parse().ok()?;
      let path = entry.path();
      is_executable(&node_binary(&path)).then_some((version, path))
    })
    .collect();
  versions.sort();
  versions
}

fn bring_in(source: &Path, target: &Path, mode: ImportMode) -> std::io::Result<()> {
  if let Some(parent) = target.parent() {
    std::fs::create_dir_all(parent)?;
  }
  match mode {
    | ImportMode::Link => crate::fs::link_dir(source, target),
    | ImportMode::Move => std::fs::rename(source, target),
    | ImportMode::Copy => {
      let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
      let staging =
        target.with_file_name(format!("{IMPORTING_PREFIX}{name}-{}", std::process::id()));
      let result =
        crate::fs::copy_dir(source, &staging).and_then(|()| std::fs::rename(&staging, target));
      if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
      }
      result
    },
  }
}

/// nvm-windows's aliases, and its active version as `default`, for names
/// rnm does not use yet.
fn import_aliases(
  ctx: &Context,
  settings: &NvmWindowsSettings,
  out: &mut dyn Write,
  err: &mut dyn Write,
) -> Result<(), Failure> {
  let aliases = ctx.nvm_dir.aliases();
  let active = settings
    .active_version
    .iter()
    .map(|version| (DEFAULT_ALIAS.to_owned(), version.clone()));
  for (name, target) in settings.aliases.iter().cloned().chain(active) {
    let usable = name
      .parse::<VersionSpec>()
      .is_ok_and(|spec| matches!(spec, VersionSpec::Alias(_)))
      && target.parse::<VersionSpec>().is_ok()
      && !looks_like_range(&target);
    if !usable {
      say(
        err,
        style::warning(format_args!(
          "warning: skipped the nvm-windows alias {name}={target}"
        )),
      );
      continue;
    }
    if aliases.get(&name)?.is_some() {
      continue;
    }
    aliases.set(&name, &target)?;
    say(out, format_args!("Imported alias {name} -> {target}"));
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn finds_candidate_roots_in_nvm_windows_order() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("nvm-home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
      home.join("settings.txt"),
      "arch: 64\nROOT: C:\\nvm\\versions\nproxy: none\n",
    )
    .unwrap();
    let vars = [
      ("NVM_HOME", home.clone().into_os_string()),
      ("LOCALAPPDATA", OsString::from("L")),
      ("APPDATA", OsString::from("R")),
    ];
    let roots = candidate_roots(
      |key| vars.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone()),
      Some("I"),
    );
    assert_eq!(
      roots,
      [
        PathBuf::from("I"),
        PathBuf::from("C:\\nvm\\versions"),
        home,
        Path::new("L")
          .join("Author Software")
          .join("nvm")
          .join("installs"),
        Path::new("L").join("nvm"),
        Path::new("R").join("nvm"),
      ]
    );
  }
}
