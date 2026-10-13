//! `rnm doctor` on a healthy setup and on one with every problem it knows.

mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use rnm::nvm_dir::{NODE_EXECUTABLE, NvmDir, bin_dir};
use support::{Reply, serve};

const RNM: &str = env!("CARGO_BIN_EXE_rnm");

const TOKEN: &str = "Bearer s3cret";

const INDEX: &str =
  r#"[{"version":"v20.0.0","date":"2023-04-18","files":[],"lts":false,"security":false}]"#;

/// An executable `node` in `dir`.
fn fake_node(dir: &Path) {
  std::fs::create_dir_all(dir).unwrap();
  let node = dir.join(NODE_EXECUTABLE);
  std::fs::write(&node, "#!/bin/sh\n").unwrap();
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o755)).unwrap();
  }
}

struct Setup {
  _tmp: tempfile::TempDir,
  root: PathBuf,
  home: PathBuf,
}

impl Setup {
  /// A tree with v20.0.0 installed and `default` pointing at it.
  fn new() -> Self {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("nvm");
    let dir = NvmDir::new(&root);
    fake_node(&bin_dir(&dir.node_versions_dir().join("v20.0.0")));
    dir.aliases().set("default", "20").unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    Self {
      root,
      home,
      _tmp: tmp,
    }
  }

  fn command(&self, mirror: &str) -> Command {
    let mut command = Command::new(RNM);
    command.env_clear();
    for key in ["SystemRoot", "ComSpec", "PATHEXT"] {
      if let Some(value) = std::env::var_os(key) {
        command.env(key, value);
      }
    }
    command
      .env("RNM_DIR", &self.root)
      .env("NVM_DIR", &self.root)
      .env("XDG_STATE_HOME", self.root.join("state"))
      .env("HOME", &self.home)
      .env("USERPROFILE", &self.home)
      .env("RNM_NODE_MIRROR", mirror)
      .env("RNM_AUTH_HEADER", TOKEN)
      .env(
        "RNM_GITHUB_API_URL",
        format!("http://127.0.0.1:{}", support::unused_port()),
      )
      .current_dir(&self.root);
    command
  }

  /// Creates a session link with `rnm env --json`; returns the variables to
  /// pass on (`RNM_MULTISHELL_PATH`, `NVM_BIN`).
  fn session(&self, mirror: &str) -> Vec<(String, String)> {
    let output = self
      .command(mirror)
      .args(["env", "--json"])
      .output()
      .unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));
    let json: serde_json::Map<String, serde_json::Value> =
      serde_json::from_slice(&output.stdout).unwrap();
    json
      .into_iter()
      .map(|(key, value)| (key, value.as_str().unwrap().to_owned()))
      .collect()
  }

  fn doctor(&self, mirror: &str, path: &[PathBuf], envs: &[(String, String)]) -> Output {
    self
      .command(mirror)
      .arg("doctor")
      .env("PATH", std::env::join_paths(path).unwrap())
      .envs(envs.iter().map(|(k, v)| (k, v)))
      .output()
      .unwrap()
  }
}

fn text(bytes: &[u8]) -> String {
  String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}

fn mirror(status: Option<u16>) -> String {
  let (base, _) = serve(move |request| match (status, request.path.as_str()) {
    | (Some(status), _) => Reply::status(status),
    | (None, "/dist/index.json") if request.header("authorization") == Some(TOKEN) => {
      Reply::ok(INDEX)
    },
    | _ => Reply::status(401),
  });
  format!("{base}/dist")
}

#[test]
fn healthy_setup_passes() {
  let setup = Setup::new();
  let mirror = mirror(None);
  let session = setup.session(&mirror);
  let nvm_bin = session.iter().find(|(k, _)| k == "NVM_BIN").unwrap();
  let output = setup.doctor(&mirror, &[PathBuf::from(&nvm_bin.1)], &session);
  let stdout = text(&output.stdout);
  assert_eq!(output.status.code(), Some(0), "{stdout}");
  for expected in [
    "ok    install directory: ",
    "ok    shell: set up, using v20.0.0",
    "ok    default alias: 20 (v20.0.0)",
    "ok    npm prefix: not overridden",
    "ok    other version managers: none active",
    "ok    mirror: ",
  ] {
    assert!(
      stdout.contains(expected),
      "missing {expected:?} in:\n{stdout}"
    );
  }
  assert!(
    stdout.contains("credentials from RNM_AUTH_HEADER, 1 releases"),
    "{stdout}"
  );
}

#[test]
fn reports_problems() {
  let setup = Setup::new();
  let mirror = mirror(Some(401));
  let mut session = setup.session(&mirror);
  let nvm_bin = session
    .iter()
    .find(|(k, _)| k == "NVM_BIN")
    .unwrap()
    .1
    .clone();
  let other_node = setup.home.join("other");
  fake_node(&other_node);
  std::fs::write(setup.home.join(".npmrc"), "prefix=/somewhere\n").unwrap();
  std::fs::remove_file(setup.root.join("alias").join("default")).unwrap();
  session.push(("FNM_MULTISHELL_PATH".to_owned(), "/x".to_owned()));

  let output = setup.doctor(
    &mirror,
    &[other_node.clone(), PathBuf::from(nvm_bin)],
    &session,
  );
  let stdout = text(&output.stdout);
  assert_eq!(output.status.code(), Some(1), "{stdout}");
  for expected in [
    "fail  shell: ",
    "comes before rnm on PATH",
    "warn  default alias: none",
    "warn  npm prefix: ",
    "sets prefix=/somewhere",
    "warn  other version managers: fnm (FNM_MULTISHELL_PATH)",
    "fail  mirror: ",
    "401 Unauthorized",
    "2 problems, ",
  ] {
    assert!(
      stdout.contains(expected),
      "missing {expected:?} in:\n{stdout}"
    );
  }
}

#[test]
fn works_before_setup() {
  let setup = Setup::new();
  let mirror = mirror(None);
  let output = setup.doctor(&mirror, &[], &[]);
  let stdout = text(&output.stdout);
  assert_eq!(output.status.code(), Some(0), "{stdout}");
  assert!(
    stdout.contains("warn  shell: rnm is not set up in this shell"),
    "{stdout}"
  );
}
