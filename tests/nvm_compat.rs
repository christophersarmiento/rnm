//! Differential tests against real nvm.
//!
//! These run only when `RNM_NVM_SH` points to an `nvm.sh` (for example
//! `RNM_NVM_SH=reference/nvm/nvm.sh cargo test --test nvm_compat`). Each test
//! builds a fake install tree, then checks that rnm resolves every spec to the
//! same version as nvm's `nvm_version`.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use rnm::nvm_dir::{NvmDir, Resolved};
use rnm::version::VersionSpec;

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
