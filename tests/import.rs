//! `rnm import nvm-windows` and `rnm default`. The import runs everywhere
//! with `--from`, so the link, move, and copy modes are tested on every
//! platform; registry discovery and `--persist` run only on Windows.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use rnm::nvm_dir::{NvmDir, bin_dir};

const RNM: &str = env!("CARGO_BIN_EXE_rnm");

/// A version directory, laid out as rnm expects on this platform.
fn fake_version(version_dir: &Path) {
  let bin = bin_dir(version_dir);
  std::fs::create_dir_all(&bin).unwrap();
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    let node = bin.join("node");
    std::fs::write(&node, "#!/bin/sh\necho fake\n").unwrap();
    std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o755)).unwrap();
  }
  #[cfg(windows)]
  std::fs::write(bin.join("node.exe"), "").unwrap();
}

struct Setup {
  _tmp: tempfile::TempDir,
  root: PathBuf,
  nvm_windows: PathBuf,
}

impl Setup {
  fn new() -> Self {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("rnm");
    let nvm_windows = tmp.path().join("nvm-windows");
    std::fs::create_dir_all(&root).unwrap();
    for version in ["v18.20.4", "v20.11.1"] {
      fake_version(&nvm_windows.join(version));
    }
    std::fs::create_dir_all(nvm_windows.join("not-a-version")).unwrap();
    std::fs::create_dir_all(nvm_windows.join("v16.0.0")).unwrap();
    Self {
      _tmp: tmp,
      root,
      nvm_windows,
    }
  }

  fn rnm(&self, args: &[&str]) -> Output {
    let mut command = Command::new(RNM);
    command.env_clear();
    for key in ["SystemRoot", "ComSpec", "PATHEXT"] {
      if let Some(value) = std::env::var_os(key) {
        command.env(key, value);
      }
    }
    command
      .args(args)
      .env("RNM_DIR", &self.root)
      .env("NVM_DIR", &self.root)
      .current_dir(&self.root)
      .output()
      .unwrap()
  }

  fn import(&self, mode: &str) -> Output {
    let from = self.nvm_windows.to_str().unwrap();
    let mut args = vec!["import", "nvm-windows", "--from", from];
    if !mode.is_empty() {
      args.push(mode);
    }
    self.rnm(&args)
  }

  fn version_dir(&self, version: &str) -> PathBuf {
    NvmDir::new(&self.root).node_versions_dir().join(version)
  }
}

fn text(bytes: &[u8]) -> String {
  String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}

fn assert_success(output: &Output) {
  assert!(
    output.status.success(),
    "status {:?}\nstdout: {}\nstderr: {}",
    output.status.code(),
    text(&output.stdout),
    text(&output.stderr)
  );
}

#[test]
fn links_versions_and_uninstall_removes_only_the_link() {
  let setup = Setup::new();
  let output = setup.import("");
  assert_success(&output);
  let stdout = text(&output.stdout);
  assert!(
    stdout.contains("Imported v18.20.4 (linked from"),
    "{stdout}"
  );
  assert!(stdout.contains("2 of 2 versions imported."), "{stdout}");
  assert!(rnm::fs::is_link(&setup.version_dir("v20.11.1")));

  let listed = text(&setup.rnm(&["ls", "--no-colors", "--no-alias"]).stdout);
  assert!(
    listed.contains("v18.20.4") && listed.contains("v20.11.1"),
    "{listed}"
  );

  let output = setup.import("");
  assert_success(&output);
  assert!(text(&output.stdout).contains("v18.20.4 is already installed; skipped."));

  assert_success(&setup.rnm(&["uninstall", "v20.11.1"]));
  assert!(!setup.version_dir("v20.11.1").exists());
  assert!(
    bin_dir(&setup.nvm_windows.join("v20.11.1")).is_dir(),
    "uninstall must not delete nvm-windows's files"
  );
}

#[test]
fn copies_or_moves_versions() {
  let setup = Setup::new();
  assert_success(&setup.import("--copy"));
  let copied = setup.version_dir("v18.20.4");
  assert!(copied.is_dir() && !rnm::fs::is_link(&copied));
  assert!(setup.nvm_windows.join("v18.20.4").is_dir());

  let setup = Setup::new();
  assert_success(&setup.import("--move"));
  assert!(setup.version_dir("v20.11.1").is_dir());
  assert!(!setup.nvm_windows.join("v20.11.1").exists());
}

#[test]
fn explains_when_nothing_is_found() {
  let setup = Setup::new();
  let empty = setup.root.join("empty");
  std::fs::create_dir_all(&empty).unwrap();
  let output = setup.rnm(&["import", "nvm-windows", "--from", empty.to_str().unwrap()]);
  assert_eq!(output.status.code(), Some(1));
  assert!(text(&output.stderr).contains("No nvm-windows versions found"));
  let output = setup.rnm(&["import", "nvm-windows", "--move", "--copy"]);
  assert_eq!(output.status.code(), Some(2), "the modes are exclusive");
}

#[test]
fn default_sets_the_alias() {
  let setup = Setup::new();
  assert_success(&setup.import(""));
  let output = setup.rnm(&["default", "18"]);
  assert_success(&output);
  assert_eq!(
    std::fs::read_to_string(setup.root.join("alias").join("default")).unwrap(),
    "18\n"
  );
  let output = setup.rnm(&["default"]);
  let stdout = text(&output.stdout);
  assert!(stdout.starts_with("default -> 18 (-> v18.20.4"), "{stdout}");
}

#[cfg(not(windows))]
#[test]
fn persist_is_windows_only() {
  let setup = Setup::new();
  let output = setup.rnm(&["default", "--persist"]);
  assert_eq!(output.status.code(), Some(1));
  assert!(text(&output.stderr).contains("for Windows"));
}

/// The persistent link follows every change of the `default` alias.
#[test]
fn persistent_link_follows_the_default_alias() {
  let setup = Setup::new();
  assert_success(&setup.import("--copy"));
  assert_success(&setup.rnm(&["default", "18"]));
  let link = setup.root.join("default");
  rnm::fs::link_dir(&setup.version_dir("v18.20.4"), &link).unwrap();

  assert_success(&setup.rnm(&["alias", "default", "20"]));
  assert_eq!(
    rnm::fs::read_dir_link(&link).unwrap(),
    setup.version_dir("v20.11.1")
  );
}
