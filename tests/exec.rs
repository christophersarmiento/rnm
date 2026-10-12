//! `rnm exec`: version selection, environment, and exit codes.

use std::path::Path;
use std::process::{Command, Output};

use rnm::nvm_dir::{NvmDir, bin_dir};

const RNM: &str = env!("CARGO_BIN_EXE_rnm");

/// A version directory whose `node` (on unix) prints the version.
fn fake_install(dir: &NvmDir, version: &str) {
  let version_dir = dir.node_versions_dir().join(version);
  let bin = bin_dir(&version_dir);
  std::fs::create_dir_all(&bin).unwrap();
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    let node = bin.join("node");
    std::fs::write(&node, format!("#!/bin/sh\necho {version}\n")).unwrap();
    std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o755)).unwrap();
  }
  #[cfg(windows)]
  std::fs::write(bin.join("node.exe"), "").unwrap();
}

fn rnm(root: &Path, cwd: &Path, args: &[&str]) -> Output {
  let mut command = Command::new(RNM);
  command.env_clear();
  for key in ["SystemRoot", "ComSpec", "PATHEXT"] {
    if let Some(value) = std::env::var_os(key) {
      command.env(key, value);
    }
  }
  let system_path = std::env::var_os("PATH").unwrap_or_default();
  command
    .args(args)
    .env("RNM_DIR", root)
    .env("NVM_DIR", root)
    .env("PATH", system_path)
    .current_dir(cwd)
    .output()
    .unwrap()
}

fn text(bytes: &[u8]) -> String {
  String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}

#[cfg(unix)]
#[test]
fn runs_the_requested_version() {
  let tmp = tempfile::tempdir().unwrap();
  let dir = NvmDir::new(tmp.path());
  fake_install(&dir, "v18.20.4");
  fake_install(&dir, "v20.11.1");

  let output = rnm(tmp.path(), tmp.path(), &["exec", "18", "node"]);
  assert_eq!(text(&output.stdout), "Running node v18.20.4\nv18.20.4\n");

  let output = rnm(
    tmp.path(),
    tmp.path(),
    &["exec", "--silent", "--using=20", "node"],
  );
  assert_eq!(text(&output.stdout), "v20.11.1\n");

  let script = r#"echo "$NVM_BIN"; exit 7"#;
  let output = rnm(
    tmp.path(),
    tmp.path(),
    &["exec", "--silent", "20", "sh", "-c", script],
  );
  assert_eq!(output.status.code(), Some(7));
  let bin = bin_dir(&dir.node_versions_dir().join("v20.11.1"));
  assert_eq!(text(&output.stdout), format!("{}\n", bin.display()));
}

#[cfg(unix)]
#[test]
fn falls_back_to_the_version_file() {
  let tmp = tempfile::tempdir().unwrap();
  let dir = NvmDir::new(tmp.path());
  fake_install(&dir, "v20.11.1");
  let project = tmp.path().join("project");
  std::fs::create_dir(&project).unwrap();
  std::fs::write(project.join(".nvmrc"), "20\n").unwrap();

  let output = rnm(tmp.path(), &project, &["exec", "--silent", "node"]);
  assert_eq!(text(&output.stdout), "v20.11.1\n");
}

#[test]
fn reports_errors() {
  let tmp = tempfile::tempdir().unwrap();
  let dir = NvmDir::new(tmp.path());
  fake_install(&dir, "v20.11.1");

  let output = rnm(tmp.path(), tmp.path(), &["exec", "--using=18", "node"]);
  assert_eq!(output.status.code(), Some(3), "{}", text(&output.stderr));

  let output = rnm(
    tmp.path(),
    tmp.path(),
    &["exec", "--silent", "20", "no-such-command-rnm"],
  );
  assert_eq!(output.status.code(), Some(126), "{}", text(&output.stderr));

  let output = rnm(tmp.path(), tmp.path(), &["exec", "node"]);
  assert_eq!(output.status.code(), Some(127), "{}", text(&output.stderr));
}

#[cfg(windows)]
#[test]
fn passes_exit_codes_through_on_windows() {
  let tmp = tempfile::tempdir().unwrap();
  let dir = NvmDir::new(tmp.path());
  fake_install(&dir, "v20.11.1");
  let output = rnm(
    tmp.path(),
    tmp.path(),
    &[
      "exec",
      "--silent",
      "20",
      "cmd",
      "/C",
      "echo %NVM_BIN%& exit 7",
    ],
  );
  assert_eq!(output.status.code(), Some(7), "{}", text(&output.stderr));
  let bin = bin_dir(&dir.node_versions_dir().join("v20.11.1"));
  assert_eq!(text(&output.stdout).trim(), bin.display().to_string());
}
