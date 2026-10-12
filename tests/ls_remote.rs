//! `rnm ls-remote` against a local mirror: output, LTS alias refresh, the
//! `ETag` cache, and the offline fallback.

mod support;

use std::path::Path;
use std::process::{Command, Output};

use rnm::nvm_dir::NvmDir;
use support::{Reply, Seen, serve, serve_count, unused_port};

const RNM: &str = env!("CARGO_BIN_EXE_rnm");

const TOKEN: &str = "Bearer s3cret";

const INDEX: &str = r#"[
  {"version":"v24.9.0","date":"2025-09-25","files":["linux-x64"],"npm":"11.6.0","lts":false,"security":false},
  {"version":"v22.20.0","date":"2025-09-24","files":["linux-x64"],"npm":"10.9.3","lts":"Jod","security":false},
  {"version":"v22.19.0","date":"2025-08-28","files":["linux-x64"],"npm":"10.9.3","lts":"Jod","security":false},
  {"version":"v20.19.5","date":"2025-09-03","files":["linux-x64"],"npm":"10.8.2","lts":"Iron","security":true},
  {"version":"v0.12.18","date":"2017-02-22","files":["src"],"lts":false,"security":false}
]"#;

/// Serves `INDEX` at `/dist/index.json`, answering 304 to its own `ETag`.
fn mirror() -> (String, Seen) {
  let (base, seen) = serve(|request| match request.path.as_str() {
    | "/dist/index.json" if request.header("if-none-match") == Some("\"v1\"") => Reply::status(304),
    | "/dist/index.json" => Reply::ok(INDEX),
    | _ => Reply::status(404),
  });
  (format!("{base}/dist"), seen)
}

fn rnm(root: &Path, mirror: &str, args: &[&str]) -> Output {
  let mut command = Command::new(RNM);
  command.env_clear();
  // Windows sockets need SystemRoot.
  if let Some(system_root) = std::env::var_os("SystemRoot") {
    command.env("SystemRoot", system_root);
  }
  command
    .arg("ls-remote")
    .args(args)
    .env("RNM_DIR", root)
    .env("RNM_NODE_MIRROR", mirror)
    .env("RNM_AUTH_HEADER", TOKEN)
    .current_dir(root)
    .output()
    .unwrap()
}

fn stdout(output: &Output) -> String {
  String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
  String::from_utf8(output.stderr.clone()).unwrap()
}

#[test]
fn lists_versions_like_nvm() {
  let tmp = tempfile::tempdir().unwrap();
  let (mirror, seen) = mirror();
  let output = rnm(tmp.path(), &mirror, &[]);
  assert!(output.status.success(), "{}", stderr(&output));
  assert_eq!(
    stdout(&output),
    "       v0.12.18\n       v20.19.5   (Latest LTS: Iron)\n       v22.19.0   (LTS: Jod)\n       \
     v22.20.0   (Latest LTS: Jod)\n        v24.9.0\n"
  );
  assert_eq!(seen.lock().unwrap()[0].header("authorization"), Some(TOKEN));

  let output = rnm(tmp.path(), &mirror, &["--lts=jod"]);
  assert_eq!(
    stdout(&output),
    "       v22.19.0   (LTS: Jod)\n       v22.20.0   (Latest LTS: Jod)\n"
  );

  let output = rnm(tmp.path(), &mirror, &["18"]);
  assert_eq!(output.status.code(), Some(3));
  assert_eq!(stdout(&output), "            N/A\n");
  assert_eq!(stderr(&output), "");
}

#[test]
fn refreshes_lts_aliases() {
  let tmp = tempfile::tempdir().unwrap();
  let (mirror, _) = mirror();
  let aliases = NvmDir::new(tmp.path()).aliases();
  aliases.set("lts/argon", "v4.9.1").unwrap();

  assert!(rnm(tmp.path(), &mirror, &["--lts"]).status.success());

  let alias = |name: &str| aliases.get(name).unwrap();
  assert_eq!(alias("lts/*").as_deref(), Some("lts/jod"));
  assert_eq!(alias("lts/jod").as_deref(), Some("v22.20.0"));
  assert_eq!(alias("lts/iron").as_deref(), Some("v20.19.5"));
  assert_eq!(alias("lts/argon").as_deref(), Some("v4.9.1"));
}

#[test]
fn revalidates_the_cache() {
  let tmp = tempfile::tempdir().unwrap();
  let (mirror, seen) = mirror();
  let first = rnm(tmp.path(), &mirror, &[]);
  let second = rnm(tmp.path(), &mirror, &[]);
  assert!(second.status.success(), "{}", stderr(&second));
  assert_eq!(stdout(&first), stdout(&second));
  let seen = seen.lock().unwrap();
  assert_eq!(seen.len(), 2);
  assert_eq!(seen[0].header("if-none-match"), None);
  assert_eq!(seen[1].header("if-none-match"), Some("\"v1\""));
}

#[test]
fn falls_back_to_the_cache_when_offline() {
  let tmp = tempfile::tempdir().unwrap();
  let (base, _, server) = serve_count(1, |_| Reply::ok(INDEX));
  let mirror = format!("{base}/dist");
  assert!(rnm(tmp.path(), &mirror, &[]).status.success());
  server.join().unwrap();

  let output = rnm(tmp.path(), &mirror, &["--lts=iron"]);
  assert!(output.status.success(), "{}", stderr(&output));
  assert_eq!(stdout(&output), "       v20.19.5   (Latest LTS: Iron)\n");
  assert!(
    stderr(&output).contains("using the cached version list"),
    "{}",
    stderr(&output)
  );

  let other_mirror = format!("http://127.0.0.1:{}/dist", unused_port());
  let output = rnm(tmp.path(), &other_mirror, &[]);
  assert_eq!(output.status.code(), Some(1), "the cache is per mirror");
}

#[test]
fn reports_mirror_failures() {
  let tmp = tempfile::tempdir().unwrap();
  let (base, _) = serve(|request| match request.header("authorization") {
    | Some(TOKEN) => Reply::ok("<html>not json</html>"),
    | _ => Reply::status(401),
  });
  let output = rnm(tmp.path(), &format!("{base}/dist"), &[]);
  assert_eq!(output.status.code(), Some(1));
  assert!(
    stderr(&output).contains("not a Node.js version index"),
    "{}",
    stderr(&output)
  );
}
