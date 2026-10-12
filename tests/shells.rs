//! End-to-end tests in real shells: bash, zsh, fish, Nushell, and PowerShell.
//!
//! Each test runs only for shells found on `PATH`. In CI, set
//! `RNM_REQUIRE_SHELLS=bash,zsh,fish,nu,pwsh` so a missing shell fails the
//! test instead of being skipped.
//!
//! The fixture's directories contain spaces, quotes, `$`, backticks, and
//! non-ASCII characters, so every path goes through the shells' quoting.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use rnm::shell::{EnvPlan, ShellKind};

const RNM: &str = env!("CARGO_BIN_EXE_rnm");

/// A shell under test: its binary and how to run a script file with it.
struct Shell {
  name: &'static str,
  extension: &'static str,
  kind: ShellKind,
  args: &'static [&'static str],
}

const SHELLS: &[Shell] = &[
  Shell {
    name: "bash",
    extension: "sh",
    kind: ShellKind::Bash,
    args: &["--norc", "--noprofile"],
  },
  Shell {
    name: "zsh",
    extension: "zsh",
    kind: ShellKind::Zsh,
    args: &["-f"],
  },
  Shell {
    name: "fish",
    extension: "fish",
    kind: ShellKind::Fish,
    args: &["--no-config"],
  },
  Shell {
    name: "nu",
    extension: "nu",
    kind: ShellKind::Nu,
    args: &["-n"],
  },
  Shell {
    name: "pwsh",
    extension: "ps1",
    kind: ShellKind::PowerShell,
    args: &["-NoProfile", "-NonInteractive", "-File"],
  },
];

fn find_on_path(name: &str) -> Option<PathBuf> {
  std::env::split_paths(&std::env::var_os("PATH")?)
    .map(|dir| dir.join(name))
    .find(|p| p.is_file())
}

/// The shells to test. Fails if a shell listed in `RNM_REQUIRE_SHELLS` is
/// missing.
fn available() -> Vec<&'static Shell> {
  let required = std::env::var("RNM_REQUIRE_SHELLS").unwrap_or_default();
  let mut found = Vec::new();
  for shell in SHELLS {
    if find_on_path(shell.name).is_some() {
      found.push(shell);
    } else if required.split(',').any(|name| name.trim() == shell.name) {
      panic!(
        "{} is required by RNM_REQUIRE_SHELLS but not on PATH",
        shell.name
      );
    } else {
      eprintln!("skipping {}: not on PATH", shell.name);
    }
  }
  found
}

/// A fake install tree with three versions whose `node` prints its version.
struct Fixture {
  _tmp: tempfile::TempDir,
  root: PathBuf,
}

impl Fixture {
  fn new() -> Self {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("it's a $dir `x` é");
    let nvm = root.join("nvm");
    for version in ["v18.20.4", "v20.19.0", "v22.20.0"] {
      let bin = nvm.join("versions").join("node").join(version).join("bin");
      std::fs::create_dir_all(&bin).unwrap();
      let node = bin.join("node");
      std::fs::write(&node, format!("#!/bin/sh\necho {version}\n")).unwrap();
      std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::create_dir_all(nvm.join("alias")).unwrap();
    std::fs::write(nvm.join("alias").join("default"), "20\n").unwrap();
    let project = root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join(".nvmrc"), "22 # comment\n").unwrap();
    Self { _tmp: tmp, root }
  }

  /// Runs `script` (saved to a file) with `shell` in the fixture's
  /// environment and returns stdout. Panics with stderr on failure.
  fn run(&self, shell: &Shell, script: &str) -> String {
    let file = self.root.join(format!("script.{}", shell.extension));
    std::fs::write(&file, script).unwrap();
    let bin_dir = Path::new(RNM).parent().unwrap();
    let path = std::env::join_paths(
      std::iter::once(bin_dir.to_path_buf())
        .chain(["/usr/local/bin", "/opt/homebrew/bin", "/usr/bin", "/bin"].map(PathBuf::from))
        .chain(find_on_path(shell.name).and_then(|p| p.parent().map(Path::to_path_buf))),
    )
    .unwrap();
    let output = Command::new(shell.name)
      .args(shell.args)
      .arg(&file)
      .current_dir(&self.root)
      .env_clear()
      .env("PATH", path)
      .env("HOME", &self.root)
      .env("NVM_DIR", self.root.join("nvm"))
      .env("XDG_STATE_HOME", self.root.join("state"))
      .env("PWD", &self.root)
      .output()
      .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    assert!(
      output.status.success(),
      "{} failed ({}):\nstdout:\n{stdout}\nstderr:\n{}",
      shell.name,
      output.status,
      String::from_utf8_lossy(&output.stderr)
    );
    stdout
  }
}

/// The setup line, then node / use / node / current, per shell.
fn switch_script(shell: &Shell) -> String {
  let project = "project";
  match shell.kind {
    | ShellKind::Bash | ShellKind::Zsh => format!(
      "eval \"$(rnm env --shell {name})\" || exit 1\nnode\nrnm use 18 >/dev/null || exit 1\nnode\n\
       cd {project} && rnm use --silent || exit 1\nnode\nrnm current\n\
       eval \"$(rnm env --shell {name})\"\necho \"$PATH\" | tr ':' '\\n' | grep -c multishells\n",
      name = shell.name
    ),
    | ShellKind::Fish => format!(
      "rnm env --shell fish | source; or exit 1\nnode\nrnm use 18 >/dev/null; or exit 1\nnode\n\
       cd {project}; and rnm use --silent; or exit 1\nnode\nrnm current\n\
       rnm env --shell fish | source\nstring join \\n $PATH | grep -c multishells\n"
    ),
    | ShellKind::Nu => {
      let loader = Command::new(RNM)
        .args(["env", "--shell", "nu"])
        .output()
        .unwrap()
        .stdout;
      format!(
        "{}\n^node\n^rnm use 18 | ignore\n^node\ncd {project}\n^rnm use --silent\n^node\n^rnm current\n\
         $env.PATH | where {{|p| $p | str contains multishells }} | length\n",
        String::from_utf8(loader).unwrap()
      )
    },
    | ShellKind::PowerShell => format!(
      "$ErrorActionPreference = 'Stop'\n\
       rnm env --shell powershell | Out-String | Invoke-Expression\nnode\nrnm use 18 | Out-Null\nnode\n\
       Set-Location {project}; rnm use --silent\nnode\nrnm current\n\
       rnm env --shell powershell | Out-String | Invoke-Expression\n\
       @($env:PATH -split [IO.Path]::PathSeparator | Where-Object {{ $_ -like '*multishells*' }}).Count\n"
    ),
  }
}

#[test]
fn env_and_use_switch_versions() {
  for shell in available() {
    let fixture = Fixture::new();
    let output = fixture.run(shell, &switch_script(shell));
    assert_eq!(
      output, "v20.19.0\nv18.20.4\nv22.20.0\nv22.20.0\n1\n",
      "unexpected output from {}",
      shell.name
    );
  }
}

/// Values that break naive quoting.
const TRICKY: &[&str] = &[
  "plain",
  "with space",
  "it's",
  "double \" quote",
  "$HOME and ${HOME} and $(echo x)",
  "`backtick`",
  r"back\slash\\",
  "semi;colon & amp | pipe",
  "glob * ? [a]",
  "bang !x !!",
  "tilde ~ # hash",
  "é ü 日本",
  "curly \u{2018}single\u{2019} \u{201C}double\u{201D}",
  "emoji 😀",
  "tab\there",
  "new\nline",
];

/// Renders `RNM_T=<value>` with each shell's quoting and checks that the
/// shell reads back exactly the same bytes.
#[test]
fn quoting_round_trips() {
  for shell in available() {
    let fixture = Fixture::new();
    for value in TRICKY {
      let plan = EnvPlan {
        path_prepend: Vec::new(),
        set: vec![("RNM_T".to_owned(), (*value).to_owned())],
        use_on_cd: false,
      };
      let script = match shell.kind {
        | ShellKind::Bash | ShellKind::Zsh | ShellKind::Fish => {
          format!("{}printf %s \"$RNM_T\"\n", plan.render(shell.kind))
        },
        | ShellKind::PowerShell => format!(
          "[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)\n{}[Console]::Out.Write($env:RNM_T)\n",
          plan.render(shell.kind)
        ),
        | ShellKind::Nu => {
          let json = plan.render_json();
          let path = fixture.root.join("value.json");
          std::fs::write(&path, json).unwrap();
          // A Nushell raw string; the fixture path has `'` but not `'#`.
          format!(
            "open --raw r#'{}'# | from json | load-env\nprint -n $env.RNM_T\n",
            path.display()
          )
        },
      };
      let got = fixture.run(shell, &script);
      assert_eq!(got, *value, "{} mangled {value:?}", shell.name);
    }
  }
}
