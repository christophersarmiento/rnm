//! `rnm self-update` against a local server that answers like GitHub's API
//! and release downloads. The "new release" is a shell script that prints
//! `rnm <version>`, so these tests run on unix only.
#![cfg(unix)]

mod support;

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use ring::digest;
use support::{Reply, serve};

const RNM: &str = env!("CARGO_BIN_EXE_rnm");
const RELEASES: &str = "/repos/christophersarmiento/rnm/releases";
const DOWNLOADS: &str = "/christophersarmiento/rnm/releases/download/";

/// Every unix archive name the update may ask for.
const ARCHIVES: [&str; 4] = [
  "rnm-x86_64-unknown-linux-musl.tar.gz",
  "rnm-aarch64-unknown-linux-musl.tar.gz",
  "rnm-x86_64-apple-darwin.tar.gz",
  "rnm-aarch64-apple-darwin.tar.gz",
];

/// A release archive whose `rnm` prints `rnm <version>`.
fn archive(version: &str) -> Vec<u8> {
  let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
  let mut builder = tar::Builder::new(encoder);
  let script = format!("#!/bin/sh\necho 'rnm {version}'\n");
  let mut header = tar::Header::new_gnu();
  header.set_size(u64::try_from(script.len()).unwrap());
  header.set_mode(0o755);
  header.set_cksum();
  builder
    .append_data(&mut header, "rnm", script.as_bytes())
    .unwrap();
  builder.into_inner().unwrap().finish().unwrap()
}

fn sha256_hex(bytes: &[u8]) -> String {
  let mut digest_bytes = [0; 32];
  digest_bytes.copy_from_slice(digest::digest(&digest::SHA256, bytes).as_ref());
  rnm::install::Sha256(digest_bytes).to_string()
}

/// What the fake GitHub serves; tests change it.
#[derive(Clone)]
struct Server {
  stable: String,
  nightly: String,
  corrupt_checksums: bool,
}

fn start(state: &Arc<Mutex<Server>>) -> String {
  let state = Arc::clone(state);
  let (base, _) = serve(move |request| {
    let state = state.lock().unwrap().clone();
    let path = request.path.as_str();
    if path == format!("{RELEASES}/latest") {
      return Reply::ok(&format!(r#"{{"tag_name":"v{}"}}"#, state.stable));
    }
    if path == format!("{RELEASES}/tags/nightly") {
      return Reply::ok(&format!(r#"{{"name":"Nightly {}"}}"#, state.nightly));
    }
    if let Some(tag) = path.strip_prefix(&format!("{RELEASES}/tags/")) {
      return Reply::ok(&format!(r#"{{"tag_name":"{tag}"}}"#));
    }
    let Some(rest) = path.strip_prefix(DOWNLOADS) else {
      return Reply::status(404);
    };
    let (tag, file) = rest.split_once('/').unwrap();
    let version = if tag == "nightly" {
      state.nightly.clone()
    } else {
      tag.trim_start_matches('v').to_owned()
    };
    let bytes = archive(&version);
    if file == "SHA256SUMS" {
      let hash = if state.corrupt_checksums {
        "0".repeat(64)
      } else {
        sha256_hex(&bytes)
      };
      let sums = ARCHIVES.iter().fold(String::new(), |mut sums, name| {
        let _ = writeln!(sums, "{hash}  {name}");
        sums
      });
      return Reply::ok(&sums);
    }
    if ARCHIVES.contains(&file) {
      return Reply::file(&bytes);
    }
    Reply::status(404)
  });
  base
}

struct Install {
  _tmp: tempfile::TempDir,
  bin: PathBuf,
  base: String,
}

impl Install {
  /// A copy of rnm in its own directory, with an install marker unless
  /// `marker` is `None`.
  fn new(base: &str, marker: Option<&str>) -> Self {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::copy(RNM, bin.join("rnm")).unwrap();
    if let Some(marker) = marker {
      std::fs::write(bin.join("install.json"), marker).unwrap();
    }
    Self {
      _tmp: tmp,
      bin,
      base: base.to_owned(),
    }
  }

  fn rnm(&self, args: &[&str]) -> Output {
    let root = self.bin.parent().unwrap();
    let mut command = Command::new(self.bin.join("rnm"));
    command
      .env_clear()
      .args(args)
      .env("HOME", root)
      .env("RNM_DIR", root.join("nvm"))
      .env("RNM_GITHUB_API_URL", &self.base)
      .env("RNM_GITHUB_URL", &self.base)
      .current_dir(root);
    // On Linux, a child forked by a parallel test can briefly inherit the
    // write handle of this freshly copied binary, so exec fails with
    // ETXTBSY until that child execs. Retry for a moment.
    for _ in 0..50 {
      match command.output() {
        | Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
          std::thread::sleep(std::time::Duration::from_millis(20));
        },
        | result => return result.unwrap(),
      }
    }
    command.output().unwrap()
  }

  fn version(&self) -> String {
    text(&self.rnm(&["--version"]).stdout).trim().to_owned()
  }

  fn marker(&self) -> String {
    std::fs::read_to_string(self.bin.join("install.json")).unwrap()
  }

  fn leftovers(&self) -> Vec<String> {
    std::fs::read_dir(&self.bin)
      .unwrap()
      .flatten()
      .map(|e| e.file_name().to_string_lossy().into_owned())
      .filter(|name| name.starts_with('.'))
      .collect()
  }
}

fn text(bytes: &[u8]) -> String {
  String::from_utf8_lossy(bytes).into_owned()
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

const SCRIPT_MARKER: &str = r#"{"source":"install.sh","channel":"stable"}"#;

fn state() -> Arc<Mutex<Server>> {
  Arc::new(Mutex::new(Server {
    stable: "9.9.9".to_owned(),
    nightly: "9.9.10-nightly.20991231+abc1234".to_owned(),
    corrupt_checksums: false,
  }))
}

#[test]
fn checks_and_updates_on_the_stable_channel() {
  let state = state();
  let install = Install::new(&start(&state), Some(SCRIPT_MARKER));

  let output = install.rnm(&["self-update", "--check"]);
  assert_success(&output);
  assert_eq!(
    text(&output.stdout),
    format!(
      "rnm 9.9.9 is available (installed: {}). Update with: rnm self-update\n",
      rnm::VERSION
    )
  );
  assert_eq!(install.version(), format!("rnm {}", rnm::VERSION));

  let output = install.rnm(&["self-update"]);
  assert_success(&output);
  assert!(text(&output.stdout).ends_with("Updated to rnm 9.9.9.\n"));
  assert_eq!(install.version(), "rnm 9.9.9");
  assert_eq!(install.leftovers(), Vec::<String>::new());
}

#[test]
fn switches_channels_and_installs_specific_releases() {
  let state = state();
  let install = Install::new(&start(&state), Some(SCRIPT_MARKER));

  assert_success(&install.rnm(&["self-update", "--nightly"]));
  assert_eq!(install.version(), "rnm 9.9.10-nightly.20991231+abc1234");
  assert!(install.marker().contains(r#""channel":"nightly""#));

  // The fake binary cannot update itself, so put rnm back for the next step.
  std::fs::copy(RNM, install.bin.join("rnm")).unwrap();
  assert_success(&install.rnm(&["self-update", "--version", "0.0.1"]));
  assert_eq!(install.version(), "rnm 0.0.1");
  assert!(install.marker().contains(r#""channel":"nightly""#));
}

#[test]
fn refuses_copies_it_did_not_install() {
  let state = state();
  let install = Install::new(&start(&state), None);
  let output = install.rnm(&["self-update"]);
  assert_eq!(output.status.code(), Some(1));
  assert!(
    text(&output.stderr).contains("was not installed by rnm's install script"),
    "{}",
    text(&output.stderr)
  );
  assert_eq!(install.version(), format!("rnm {}", rnm::VERSION));

  let output = install.rnm(&["self-update", "--check"]);
  assert_success(&output);
  assert!(text(&output.stdout).contains("Update with: run the install script again"));
}

#[test]
fn keeps_the_old_binary_on_a_bad_checksum() {
  let state = state();
  state.lock().unwrap().corrupt_checksums = true;
  let install = Install::new(&start(&state), Some(SCRIPT_MARKER));
  let output = install.rnm(&["self-update"]);
  assert_eq!(output.status.code(), Some(1));
  assert!(text(&output.stderr).contains("checksum mismatch"));
  assert_eq!(install.version(), format!("rnm {}", rnm::VERSION));
  assert_eq!(install.leftovers(), Vec::<String>::new());
}

#[test]
fn reports_when_up_to_date() {
  let state = state();
  state.lock().unwrap().stable = rnm::VERSION.to_owned();
  let install = Install::new(&start(&state), Some(SCRIPT_MARKER));
  let output = install.rnm(&["self-update"]);
  assert_success(&output);
  assert_eq!(
    text(&output.stdout),
    format!("rnm {} is up to date (stable channel).\n", rnm::VERSION)
  );
}
