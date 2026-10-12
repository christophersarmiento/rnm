//! Differential tests against real nvm.
//!
//! These run only when `RNM_NVM_SH` points to an `nvm.sh` (for example
//! `RNM_NVM_SH=reference/nvm/nvm.sh cargo test --test nvm_compat`). Each test
//! builds a fake install tree, then checks that rnm resolves every spec to the
//! same version as nvm's `nvm_version`, or prints the same `ls-remote`
//! output from the same local mirror.

#![cfg(unix)]

mod support;

use std::fmt::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use rnm::nvm_dir::{NvmDir, Resolved};
use rnm::version::VersionSpec;
use support::{Reply, serve};

const RNM: &str = env!("CARGO_BIN_EXE_rnm");

/// `(version, lts codename)` for the fake mirror, newest first.
const RELEASES: [(&str, Option<&str>); 9] = [
  ("v24.9.0", None),
  ("v23.11.1", None),
  ("v22.20.0", Some("Jod")),
  ("v22.19.0", Some("Jod")),
  ("v20.19.5", Some("Iron")),
  ("v20.19.4", Some("Iron")),
  ("v18.20.8", Some("Hydrogen")),
  ("v4.9.1", Some("Argon")),
  ("v0.12.18", None),
];

fn nvm_sh() -> Option<String> {
  std::env::var("RNM_NVM_SH")
    .ok()
    .filter(|path| !path.is_empty())
}

fn fake_install(dir: &Path) {
  let bin = dir.join("bin");
  std::fs::create_dir_all(&bin).unwrap();
  let node = bin.join("node");
  std::fs::write(&node, "#!/bin/sh\n").unwrap();
  std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// What nvm's `nvm_version <spec>` prints, with `N/A` for failures.
fn nvm_version(nvm_sh: &str, root: &Path, spec: &str) -> String {
  let output = Command::new("bash")
    .arg("-c")
    .arg(r#". "$NVM_SH" --no-use >/dev/null 2>&1; nvm_version "$1" 2>/dev/null"#)
    .arg("bash")
    .arg(spec)
    .env_clear()
    .env("PATH", "/usr/bin:/bin")
    .env("HOME", root)
    .env("NVM_DIR", root)
    .env("NVM_SH", nvm_sh)
    .output()
    .unwrap();
  let out = String::from_utf8_lossy(&output.stdout).trim().to_owned();
  if out.is_empty() || out == "∞" {
    "N/A".to_owned()
  } else {
    out
  }
}

fn rnm_version(dir: &NvmDir, spec: &str) -> String {
  let Ok(spec) = spec.parse::<VersionSpec>() else {
    return "N/A".to_owned();
  };
  match dir.resolve(&spec) {
    | Ok(Resolved::Installed(installed)) => installed.to_string(),
    | Ok(Resolved::System) => "system".to_owned(),
    | Ok(Resolved::Current) => "current".to_owned(),
    | Err(_) => "N/A".to_owned(),
  }
}

#[test]
fn resolution_matches_nvm() {
  let Some(nvm_sh) = nvm_sh() else {
    eprintln!("skipped: set RNM_NVM_SH to run");
    return;
  };
  let tmp = tempfile::tempdir().unwrap();
  let root = tmp.path();
  let dir = NvmDir::new(root);

  for v in [
    "v18.19.0", "v18.20.4", "v18.3.0", "v20.11.1", "v20.19.0", "v22.20.0", "v1.8.4",
  ] {
    fake_install(&dir.node_versions_dir().join(v));
  }
  for v in ["v0.10.48", "v0.11.16"] {
    fake_install(&root.join(v));
  }
  for v in ["v2.5.0", "v3.3.1"] {
    fake_install(&dir.iojs_versions_dir().join(v));
  }
  // An incomplete install that nvm still lists but cannot use.
  std::fs::create_dir_all(dir.node_versions_dir().join("v23.0.0")).unwrap();

  let aliases = dir.aliases();
  for (name, target) in [
    ("default", "work"),
    ("work", "18"),
    ("pinned", "v20.11.1"),
    ("loop-a", "loop-b"),
    ("loop-b", "loop-a"),
    ("lts/hydrogen", "v18.20.4"),
    ("lts/iron", "v20.19.0"),
    ("lts/jod", "v22.20.0"),
    ("lts/*", "lts/jod"),
    ("to-lts", "lts/*"),
    ("missing-target", "v16.0.0"),
  ] {
    aliases.set(name, target).unwrap();
  }
  std::fs::write(
    aliases.dir().join("commented"),
    "# note\n\n  20 # trailing\n",
  )
  .unwrap();

  let specs = [
    "18",
    "v18",
    "18.",
    "18.3",
    "v18.3.0",
    "18.19.0",
    "18.19.1",
    "20",
    "20.11",
    "22",
    "1",
    "19",
    "0.10",
    "0.11",
    "node",
    "stable",
    "unstable",
    "iojs",
    "iojs-v2",
    "iojs-v3.3.1",
    "3.3.1",
    "default",
    "work",
    "pinned",
    "commented",
    "to-lts",
    "missing-target",
    "nope",
    "lts/*",
    "lts/iron",
    "lts/hydrogen",
    "lts/-1",
    "lts/-2",
    "lts/-3",
    "lts/argon",
    "loop-a",
    "23",
  ];

  // Deliberate differences: (spec, nvm result, rnm result).
  let expected_differences = [
    // nvm picks the incomplete v23.0.0 dir (no bin/node), then fails on use.
    // rnm only considers usable installs.
    ("node", "v23.0.0", "v22.20.0"),
    ("stable", "v23.0.0", "v22.20.0"),
    ("23", "v23.0.0", "N/A"),
    // nvm keeps leading whitespace in alias targets ("  20") and fails.
    // rnm trims it.
    ("commented", "N/A", "v20.19.0"),
  ];

  let mismatches: Vec<String> = specs
    .iter()
    .filter_map(|spec| {
      let nvm = nvm_version(&nvm_sh, root, spec);
      let rnm = rnm_version(&dir, spec);
      let expected = expected_differences.iter().find(|(s, ..)| s == spec);
      let ok = match expected {
        | Some((_, want_nvm, want_rnm)) => nvm == *want_nvm && rnm == *want_rnm,
        | None => nvm == rnm,
      };
      (!ok).then(|| format!("{spec:>16}: nvm={nvm} rnm={rnm}"))
    })
    .collect();
  assert!(
    mismatches.is_empty(),
    "rnm and nvm disagree:\n{}",
    mismatches.join("\n")
  );
}

/// The fake mirror's `index.json` (for rnm) and `index.tab` (for nvm).
fn mirror_files() -> (String, String) {
  let mut json = Vec::new();
  let mut tab =
    "version\tdate\tfiles\tnpm\tv8\tuv\tzlib\topenssl\tmodules\tlts\tsecurity\n".to_owned();
  for (version, lts) in RELEASES {
    let lts_json = lts.map_or("false".to_owned(), |name| format!("\"{name}\""));
    json.push(format!(
      r#"{{"version":"{version}","date":"2025-01-01","files":["linux-x64"],"lts":{lts_json},"security":false}}"#
    ));
    let _ = writeln!(
      tab,
      "{version}\t2025-01-01\tlinux-x64\t-\t-\t-\t-\t-\t-\t{}\tfalse",
      lts.unwrap_or("-")
    );
  }
  (format!("[{}]", json.join(",")), tab)
}

/// stdout and exit code of a command line run against `root`.
fn run(command: &mut Command, root: &Path, mirror: &str) -> (String, Option<i32>) {
  let output = command
    .env_clear()
    .env("PATH", "/usr/bin:/bin")
    .env("HOME", root)
    .env("NVM_DIR", root)
    .env("NVM_NODEJS_ORG_MIRROR", mirror)
    .env("NVM_IOJS_ORG_MIRROR", format!("{mirror}/no-iojs"))
    .output()
    .unwrap();
  (
    String::from_utf8_lossy(&output.stdout).into_owned(),
    output.status.code(),
  )
}

#[test]
fn ls_remote_matches_nvm() {
  let Some(nvm_sh) = nvm_sh() else {
    eprintln!("skipped: set RNM_NVM_SH to run");
    return;
  };
  let (json, tab) = mirror_files();
  let (base, _) = serve(move |request| match request.path.as_str() {
    | "/dist/index.json" => Reply::ok(&json),
    | "/dist/index.tab" => Reply::ok(&tab),
    | _ => Reply::status(404),
  });
  let mirror = format!("{base}/dist");
  let nvm_tmp = tempfile::tempdir().unwrap();
  let rnm_tmp = tempfile::tempdir().unwrap();
  for root in [nvm_tmp.path(), rnm_tmp.path()] {
    fake_install(&NvmDir::new(root).node_versions_dir().join("v20.19.4"));
  }

  let cases: [(&[&str], bool); 9] = [
    (&[], false),
    (&["--lts"], true),
    (&["--lts=iron"], true),
    (&["lts/jod"], true),
    (&["--lts=-1"], true),
    (&["20"], false),
    (&["v22.19"], false),
    (&["19"], true),
    (&["--lts=nope"], true),
  ];
  for (args, compare_exit_code) in cases {
    let nvm = run(
      Command::new("bash")
        .arg("-c")
        .arg(
          r#"nvm_sh="$1"; shift; . "$nvm_sh" --no-use >/dev/null 2>&1; nvm ls-remote --no-colors "$@" 2>/dev/null"#,
        )
        .arg("bash")
        .arg(&nvm_sh)
        .args(args),
      nvm_tmp.path(),
      &mirror,
    );
    let rnm = run(
      Command::new(RNM).arg("ls-remote").args(args),
      rnm_tmp.path(),
      &mirror,
    );
    assert_eq!(nvm.0, rnm.0, "ls-remote {args:?}");
    // nvm also returns its (empty) io.js listing's exit code 3 whenever io.js
    // is listed, even when node versions matched; rnm does not list io.js.
    if compare_exit_code {
      assert_eq!(nvm.1, rnm.1, "exit code of ls-remote {args:?}");
    }
  }

  let lts_aliases = |root: &Path| NvmDir::new(root).aliases().list(true).unwrap();
  assert_eq!(lts_aliases(nvm_tmp.path()), lts_aliases(rnm_tmp.path()));
}
