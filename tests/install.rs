//! `rnm install` and `rnm uninstall` against a local mirror that serves
//! `index.json`, `SHASUMS256.txt`, and a tiny fake Node.js archive.

mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use ring::digest;
use rnm::install::Sha256;
use rnm::nvm_dir::NvmDir;
use rnm::platform::Platform;
use support::{Reply, Seen, serve};

const RNM: &str = env!("CARGO_BIN_EXE_rnm");

const TOKEN: &str = "Bearer s3cret";

const VERSION: &str = "v20.0.0";

/// What the fake mirror serves; tests change it between runs.
#[derive(Clone)]
struct Mirror {
  /// The `files` list in `index.json`.
  files: Vec<String>,
  /// The archive's bytes and its name.
  archive: Vec<u8>,
  archive_name: String,
  /// The `SHASUMS256.txt` body.
  shasums: String,
}

impl Mirror {
  /// A valid release of `VERSION` for this platform.
  fn valid() -> Self {
    let slug = Platform::detect()
      .unwrap()
      .artifact(&VERSION.parse().unwrap())
      .slug;
    let (archive, extension) = fake_archive(&slug);
    let archive_name = format!("{slug}.{extension}");
    let shasums = format!("{}  {archive_name}\n", sha256_hex(&archive));
    Self {
      files: Vec::new(),
      archive,
      archive_name,
      shasums,
    }
  }

  fn index(&self) -> String {
    let files: Vec<String> = self.files.iter().map(|f| format!("\"{f}\"")).collect();
    format!(
      r#"[{{"version":"{VERSION}","date":"2023-04-18","files":[{}],"lts":false,"security":false}}]"#,
      files.join(",")
    )
  }
}

fn sha256_hex(bytes: &[u8]) -> String {
  let mut digest_bytes = [0; 32];
  digest_bytes.copy_from_slice(digest::digest(&digest::SHA256, bytes).as_ref());
  Sha256(digest_bytes).to_string()
}

/// A Node.js-shaped archive: `<slug>/bin/node` (or `<slug>/node.exe` on
/// Windows) that prints the version.
#[cfg(unix)]
fn fake_archive(slug: &str) -> (Vec<u8>, &'static str) {
  let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
  let mut builder = tar::Builder::new(encoder);
  let script = format!("#!/bin/sh\necho {VERSION}\n");
  let mut header = tar::Header::new_gnu();
  header.set_size(u64::try_from(script.len()).unwrap());
  header.set_mode(0o755);
  header.set_cksum();
  builder
    .append_data(&mut header, format!("{slug}/bin/node"), script.as_bytes())
    .unwrap();
  (builder.into_inner().unwrap().finish().unwrap(), "tar.gz")
}

#[cfg(windows)]
fn fake_archive(slug: &str) -> (Vec<u8>, &'static str) {
  use std::io::Write as _;
  let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
  let options =
    zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
  writer
    .start_file(format!("{slug}/node.exe"), options)
    .unwrap();
  writer.write_all(b"fake").unwrap();
  (writer.finish().unwrap().into_inner(), "zip")
}

/// Serves `mirror` (shared, so tests can change it) under `/dist`.
fn serve_mirror(mirror: &Arc<Mutex<Mirror>>) -> (String, Seen) {
  let mirror = Arc::clone(mirror);
  let (base, seen) = serve(move |request| {
    let mirror = mirror.lock().unwrap().clone();
    let release = format!("/dist/{VERSION}/");
    match request.path.as_str() {
      | "/dist/index.json" => Reply::ok(&mirror.index()),
      | path if path == format!("{release}SHASUMS256.txt") => Reply::ok(&mirror.shasums),
      | path if path == format!("{release}{}", mirror.archive_name) => Reply::file(&mirror.archive),
      | _ => Reply::status(404),
    }
  });
  (format!("{base}/dist"), seen)
}

struct Tree {
  _tmp: tempfile::TempDir,
  root: PathBuf,
  mirror_url: String,
}

impl Tree {
  fn new(mirror_url: &str) -> Self {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("nvm");
    std::fs::create_dir(&root).unwrap();
    Self {
      root,
      _tmp: tmp,
      mirror_url: mirror_url.to_owned(),
    }
  }

  fn rnm(&self, args: &[&str]) -> Output {
    self.rnm_with(args, &[])
  }

  fn rnm_with(&self, args: &[&str], envs: &[(&str, &Path)]) -> Output {
    let mut command = Command::new(RNM);
    command.env_clear();
    if let Some(system_root) = std::env::var_os("SystemRoot") {
      command.env("SystemRoot", system_root);
    }
    command
      .args(args)
      .env("RNM_DIR", &self.root)
      .env("NVM_DIR", &self.root)
      .env("RNM_NODE_MIRROR", &self.mirror_url)
      .env("RNM_AUTH_HEADER", TOKEN)
      .env("NVM_INSTALL_LOCK_TIMEOUT", "0")
      .envs(envs.iter().copied())
      .current_dir(&self.root)
      .output()
      .unwrap()
  }

  fn dir(&self) -> NvmDir {
    NvmDir::new(&self.root)
  }

  fn version_dir(&self) -> PathBuf {
    self.dir().node_versions_dir().join(VERSION)
  }
}

fn text(bytes: &[u8]) -> String {
  String::from_utf8_lossy(bytes).into_owned()
}

/// Fails the test with rnm's output when `output` did not succeed.
fn assert_success(output: &Output) {
  assert!(
    output.status.success(),
    "status {:?}\nstdout: {}\nstderr: {}",
    output.status.code(),
    text(&output.stdout),
    text(&output.stderr)
  );
}

/// `NVM_DIR` paths are compared after canonicalisation (macOS temp dirs are
/// behind a `/var` → `/private/var` symlink).
fn canonical(path: &Path) -> PathBuf {
  std::fs::canonicalize(path).unwrap()
}

#[test]
fn installs_verifies_and_caches() {
  let mirror = Arc::new(Mutex::new(Mirror::valid()));
  let (url, seen) = serve_mirror(&mirror);
  let tree = Tree::new(&url);

  let output = tree.rnm(&["install", "20"]);
  assert_success(&output);
  let stdout = text(&output.stdout);
  assert!(stdout.contains(&format!("Downloading and installing node {VERSION}...")));
  assert!(
    stdout.contains(&format!(
      "Creating default alias: default -> 20 (-> {VERSION})"
    )),
    "{stdout}"
  );
  assert!(text(&output.stderr).contains("Checksums matched!"));

  let installed = tree.dir().installed_versions().unwrap();
  assert_eq!(installed.len(), 1);
  assert_eq!(
    canonical(&installed[0].path),
    canonical(&tree.version_dir())
  );
  assert_eq!(
    tree.dir().aliases().get("default").unwrap().as_deref(),
    Some("20")
  );

  let archive_name = mirror.lock().unwrap().archive_name.clone();
  let slug_dir = archive_name
    .trim_end_matches(".tar.gz")
    .trim_end_matches(".zip")
    .to_owned();
  let cached = tree
    .dir()
    .cache_dir()
    .join("bin")
    .join(&slug_dir)
    .join(&archive_name);
  assert!(cached.is_file(), "{} missing", cached.display());
  let leftovers: Vec<String> = std::fs::read_dir(tree.dir().node_versions_dir())
    .unwrap()
    .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
    .collect();
  assert_eq!(leftovers, [VERSION]);

  let requests = seen.lock().unwrap();
  assert!(
    requests
      .iter()
      .all(|request| request.header("authorization") == Some(TOKEN))
  );
}

#[test]
fn exact_installed_version_needs_no_network() {
  let mirror = Arc::new(Mutex::new(Mirror::valid()));
  let (url, seen) = serve_mirror(&mirror);
  let tree = Tree::new(&url);
  assert_success(&tree.rnm(&["install", VERSION]));
  let requests_after_install = seen.lock().unwrap().len();

  let output = tree.rnm(&["install", "20.0.0"]);
  assert_success(&output);
  assert!(text(&output.stderr).contains("is already installed."));
  assert_eq!(seen.lock().unwrap().len(), requests_after_install);
}

/// The `--use-on-cd` hook with `--install-if-missing` installs the version
/// file's version and switches the session to it; without the flag it only
/// reports the missing version.
#[test]
fn use_installs_a_missing_version_on_request() {
  let mirror = Arc::new(Mutex::new(Mirror::valid()));
  let (url, _seen) = serve_mirror(&mirror);
  let tree = Tree::new(&url);
  std::fs::write(tree.root.join(".nvmrc"), "20\n").unwrap();
  let links = tree.root.join("multishells");
  std::fs::create_dir(&links).unwrap();
  let session = links.join("1_2");
  let envs = [("RNM_MULTISHELL_PATH", session.as_path())];

  let output = tree.rnm_with(&["use", "--from-hook"], &envs);
  assert_eq!(output.status.code(), Some(3));
  assert!(text(&output.stderr).contains("rnm install"));
  assert_eq!(tree.dir().installed_versions().unwrap().len(), 0);

  let output = tree.rnm_with(&["use", "--from-hook", "--install-if-missing"], &envs);
  assert_success(&output);
  let stdout = text(&output.stdout);
  assert!(
    stdout.contains(&format!("Downloading and installing node {VERSION}...")),
    "{stdout}"
  );
  assert!(
    stdout.ends_with(&format!("Now using node {VERSION}\n")),
    "{stdout}"
  );
  assert_eq!(
    canonical(&session),
    canonical(&tree.version_dir()),
    "the session link points at the new version"
  );
}

#[test]
fn reuses_the_cached_archive() {
  let mirror = Arc::new(Mutex::new(Mirror::valid()));
  let (url, seen) = serve_mirror(&mirror);
  let tree = Tree::new(&url);
  assert_success(&tree.rnm(&["install", "20"]));
  assert_success(&tree.rnm(&["uninstall", "20"]));
  assert!(!tree.version_dir().exists());

  let output = tree.rnm(&["install", "20"]);
  assert_success(&output);
  assert!(text(&output.stderr).contains("Using the verified archive in the cache"));
  let archive_name = mirror.lock().unwrap().archive_name.clone();
  let downloads = seen
    .lock()
    .unwrap()
    .iter()
    .filter(|request| request.path.ends_with(&archive_name))
    .count();
  assert_eq!(downloads, 1);
}

#[test]
fn refuses_unverified_or_missing_binaries() {
  let mirror = Arc::new(Mutex::new(Mirror::valid()));
  let (url, _) = serve_mirror(&mirror);
  let tree = Tree::new(&url);

  let good_shasums = mirror.lock().unwrap().shasums.clone();
  let tampered = format!("{}{}", "0".repeat(64), &good_shasums[64..]);
  mirror.lock().unwrap().shasums = tampered;
  let output = tree.rnm(&["install", "20"]);
  assert_eq!(output.status.code(), Some(1));
  assert!(
    text(&output.stderr).contains("checksum mismatch"),
    "{}",
    text(&output.stderr)
  );
  assert!(!tree.version_dir().exists());
  assert_eq!(tree.dir().installed_versions().unwrap().len(), 0);

  mirror.lock().unwrap().shasums = "0000  other-file.tar.gz\n".to_owned();
  let output = tree.rnm(&["install", "20"]);
  assert_eq!(output.status.code(), Some(1));
  assert!(
    text(&output.stderr).contains("refusing to install an unverified download"),
    "{}",
    text(&output.stderr)
  );

  let mut wrong_platform = Mirror::valid();
  wrong_platform.files = vec!["sunos-x86".to_owned()];
  *mirror.lock().unwrap() = wrong_platform;
  let output = tree.rnm(&["install", "20"]);
  assert_eq!(output.status.code(), Some(1));
  assert!(
    text(&output.stderr).contains("has no prebuilt binary for this platform"),
    "{}",
    text(&output.stderr)
  );

  let output = tree.rnm(&["install", "21"]);
  assert_eq!(output.status.code(), Some(3));
  assert!(text(&output.stderr).contains("Version '21' not found"));
}

#[test]
fn uninstall_removes_exact_aliases_only() {
  let mirror = Arc::new(Mutex::new(Mirror::valid()));
  let (url, _) = serve_mirror(&mirror);
  let tree = Tree::new(&url);
  assert_success(&tree.rnm(&["install", VERSION, "--alias=pinned"]));
  let aliases = tree.dir().aliases();
  aliases.set("bare", "20.0.0").unwrap();
  aliases.set("partial", "20").unwrap();
  aliases.set("rc", "v20.0.0-rc.1").unwrap();

  let output = tree.rnm(&["uninstall", "20"]);
  assert_success(&output);
  let stdout = text(&output.stdout);
  assert!(
    stdout.contains(&format!("Uninstalled node {VERSION}")),
    "{stdout}"
  );
  let remaining: Vec<String> = aliases
    .list(false)
    .unwrap()
    .into_iter()
    .map(|(name, _)| name)
    .collect();
  assert_eq!(remaining, ["partial", "rc"]);

  let output = tree.rnm(&["uninstall", "20"]);
  assert_success(&output);
  assert!(text(&output.stderr).contains("Version '20' is not installed."));
}

/// Windows cannot move a directory while a program holds one of its files
/// open, as a running `node.exe` does. Uninstall then fails without deleting
/// anything, and works once the file is closed.
#[cfg(windows)]
#[test]
fn uninstall_reports_a_version_in_use() {
  use std::os::windows::fs::OpenOptionsExt;
  /// Others may read the file, but not delete or rename it.
  const FILE_SHARE_READ: u32 = 1;

  let mirror = Arc::new(Mutex::new(Mirror::valid()));
  let (url, _) = serve_mirror(&mirror);
  let tree = Tree::new(&url);
  assert_success(&tree.rnm(&["install", VERSION]));
  let node = tree.version_dir().join("node.exe");
  let running = std::fs::OpenOptions::new()
    .read(true)
    .share_mode(FILE_SHARE_READ)
    .open(&node)
    .unwrap();

  let output = tree.rnm(&["uninstall", VERSION]);
  assert_eq!(output.status.code(), Some(1), "{}", text(&output.stderr));
  assert!(
    text(&output.stderr).contains("is in use"),
    "{}",
    text(&output.stderr)
  );
  assert!(node.is_file(), "nothing is deleted");
  assert_eq!(tree.dir().installed_versions().unwrap().len(), 1);

  drop(running);
  assert_success(&tree.rnm(&["uninstall", VERSION]));
  assert_eq!(tree.dir().installed_versions().unwrap().len(), 0);
}
