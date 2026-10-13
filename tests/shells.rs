//! End-to-end tests in real shells: bash, zsh, fish, Nushell, pwsh, and (on
//! Windows) Windows PowerShell 5.1 and Git Bash.
//!
//! Each test runs only for shells it finds. In CI, set `RNM_REQUIRE_SHELLS`
//! (for example `bash,zsh,fish,nu,pwsh`) so a missing shell fails the test
//! instead of being skipped.
//!
//! The fixture's directories contain spaces, quotes, `$`, backticks, and
//! non-ASCII characters, so every path goes through the shells' quoting.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
  Shell {
    name: "powershell",
    extension: "ps1",
    kind: ShellKind::PowerShell,
    args: &[
      "-NoProfile",
      "-NonInteractive",
      "-ExecutionPolicy",
      "Bypass",
      "-File",
    ],
  },
];

/// `powershell` is Windows PowerShell 5.1, which exists only on Windows.
const WINDOWS_ONLY: &[&str] = &["powershell"];

fn find_on_path(name: &str) -> Option<PathBuf> {
  let names = [name.to_owned(), format!("{name}.exe")];
  std::env::split_paths(&std::env::var_os("PATH")?)
    .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
    .find(|p| p.is_file())
}

/// The shell's executable. On Windows, `bash` means Git Bash; the `bash.exe`
/// in System32 starts WSL instead.
fn locate(shell: &Shell) -> Option<PathBuf> {
  if WINDOWS_ONLY.contains(&shell.name) && !cfg!(windows) {
    return None;
  }
  if cfg!(windows) && shell.name == "bash" {
    let program_files = std::env::var_os("ProgramFiles")?;
    let git_bash = Path::new(&program_files).join(r"Git\bin\bash.exe");
    return git_bash.is_file().then_some(git_bash);
  }
  find_on_path(shell.name)
}

/// The shells to test. Fails if a shell listed in `RNM_REQUIRE_SHELLS` is
/// missing.
fn available() -> Vec<&'static Shell> {
  let required = std::env::var("RNM_REQUIRE_SHELLS").unwrap_or_default();
  let mut found = Vec::new();
  for shell in SHELLS {
    if locate(shell).is_some() {
      found.push(shell);
    } else if required.split(',').any(|name| name.trim() == shell.name) {
      panic!(
        "{} is required by RNM_REQUIRE_SHELLS but not found",
        shell.name
      );
    } else {
      eprintln!("skipping {}: not found", shell.name);
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
      let version_dir = nvm.join("versions").join("node").join(version);
      fake_node(&rnm::nvm_dir::bin_dir(&version_dir), version);
    }
    std::fs::create_dir_all(nvm.join("alias")).unwrap();
    std::fs::write(nvm.join("alias").join("default"), "20\n").unwrap();
    let project = root.join("project");
    std::fs::create_dir_all(project.join("sub")).unwrap();
    std::fs::write(project.join(".nvmrc"), "22 # comment\n").unwrap();
    std::fs::create_dir_all(project.join("old")).unwrap();
    std::fs::write(project.join("old").join(".node-version"), "18\n").unwrap();
    std::fs::create_dir_all(root.join("plain")).unwrap();
    Self { _tmp: tmp, root }
  }

  /// Runs `script` (saved to a file) with `shell` in the fixture's
  /// environment and returns stdout. Panics with stderr on failure.
  fn run(&self, shell: &Shell, script: &str) -> String {
    let file = self.root.join(format!("script.{}", shell.extension));
    std::fs::write(&file, script).unwrap();
    let mut command = self.command(shell);
    command.args(shell.args).arg(&file);
    Self::finish(shell, &mut command, None)
  }

  /// Feeds `script` to an interactive shell on stdin, so prompt hooks run
  /// before each line.
  fn run_interactive(&self, shell: &Shell, args: &[&str], script: &str) -> String {
    let mut command = self.command(shell);
    command.args(args);
    Self::finish(shell, &mut command, Some(script))
  }

  /// The shell, started in the fixture with only the fixture's install tree
  /// visible. On unix the environment is cleared. Windows shells need many
  /// system variables, so there only rnm's variables are removed.
  fn command(&self, shell: &Shell) -> Command {
    let program = locate(shell).unwrap();
    let rnm_dir = Path::new(RNM).parent().unwrap().to_path_buf();
    let mut command = Command::new(&program);
    let system_path: Vec<PathBuf> = if cfg!(windows) {
      std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect()
    } else {
      command.env_clear();
      ["/usr/local/bin", "/opt/homebrew/bin", "/usr/bin", "/bin"]
        .map(PathBuf::from)
        .into_iter()
        .chain(program.parent().map(Path::to_path_buf))
        .collect()
    };
    for (key, _) in std::env::vars_os() {
      let key_text = key.to_string_lossy().to_ascii_uppercase();
      if ["RNM_", "NVM_", "FNM_"]
        .iter()
        .any(|p| key_text.starts_with(p))
      {
        command.env_remove(&key);
      }
    }
    let path = std::env::join_paths(std::iter::once(rnm_dir).chain(system_path)).unwrap();
    command
      .current_dir(&self.root)
      .env("PATH", path)
      .env("HOME", &self.root)
      .env("RNM_DIR", self.root.join("nvm"))
      .env("NVM_DIR", self.root.join("nvm"))
      .env("XDG_STATE_HOME", self.root.join("state"))
      .env("PWD", &self.root);
    command
  }

  fn finish(shell: &Shell, command: &mut Command, stdin: Option<&str>) -> String {
    let output = match stdin {
      | None => command.output().unwrap(),
      | Some(input) => {
        let mut child = command
          .stdin(Stdio::piped())
          .stdout(Stdio::piped())
          .stderr(Stdio::piped())
          .spawn()
          .unwrap();
        child
          .stdin
          .take()
          .unwrap()
          .write_all(input.as_bytes())
          .unwrap();
        child.wait_with_output().unwrap()
      },
    };
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

/// A `node` in `bin` that prints `version`. On unix, a shell script. On
/// Windows, rnm only counts a version with a `node.exe`, so it is a copy of
/// the `fake_node` example, which prints its directory's name.
fn fake_node(bin: &Path, version: &str) {
  std::fs::create_dir_all(bin).unwrap();
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    let script = bin.join("node");
    std::fs::write(&script, format!("#!/bin/sh\necho {version}\n")).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
  }
  #[cfg(windows)]
  {
    assert_eq!(bin.file_name().unwrap(), version);
    let fake = Path::new(RNM)
      .with_file_name("examples")
      .join("fake_node.exe");
    std::fs::copy(&fake, bin.join("node.exe")).unwrap_or_else(|e| {
      panic!(
        "{}: {e} (build it with `cargo build --example fake_node`)",
        fake.display()
      )
    });
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

/// What [`use_on_cd_script`] prints: `node` after each step, plus the
/// hook's messages. As in nvm's README recipes, every directory change
/// applies the nearest version file (a parent's counts), or the `default`
/// alias (20) without one, so it also undoes a manual `rnm use`. Re-running
/// the setup code does not install the hook twice (the last message would
/// repeat).
const USE_ON_CD_OUTPUT: &str = "\
v22.20.0
Now using node v20.19.0
v20.19.0
Now using node v22.20.0
v22.20.0
v22.20.0
Now using node v22.20.0
v22.20.0
Now using node v18.20.4
v18.20.4
Now using node v20.19.0
v20.19.0
Now using node v20.19.0
v20.19.0
Now using node v22.20.0
v22.20.0
";

/// Starts in the project, then: leave it, `cd` back in and run `node` on the
/// same line, enter a subdirectory, `rnm use 20` and go up, `pushd` into a
/// directory with its own `.node-version`, leave the project, `rnm use 18`
/// and enter a directory without a version file, set up again, and enter
/// the project.
fn use_on_cd_script(shell: &Shell) -> String {
  match shell.kind {
    | ShellKind::Bash | ShellKind::Zsh => {
      // Quiet `pushd`: zsh's hook runs inside it, so redirecting its output
      // would hide the hook's message too, and `pushd -q` skips the hook.
      let pushd = if shell.kind == ShellKind::Zsh {
        "setopt pushd_silent; pushd old"
      } else {
        "pushd old >/dev/null"
      };
      format!(
        "cd project\neval \"$(rnm env --shell {name} --use-on-cd)\" || exit 1\nnode\ncd ..\nnode\n\
         cd project && node\ncd sub\nnode\nrnm use 20 >/dev/null\ncd ..\nnode\n{pushd}\nnode\n\
         cd ../..\nnode\nrnm use 18 >/dev/null\ncd plain\nnode\n\
         eval \"$(rnm env --shell {name} --use-on-cd)\"\ncd ../project\nnode\nexit\n",
        name = shell.name
      )
    },
    | ShellKind::Fish => {
      "cd project\nrnm env --shell fish --use-on-cd | source; or exit 1\nnode\ncd ..\nnode\n\
       cd project; and node\ncd sub\nnode\nrnm use 20 >/dev/null\ncd ..\nnode\npushd old\nnode\n\
       cd ../..\nnode\nrnm use 18 >/dev/null\ncd plain\nnode\n\
       rnm env --shell fish --use-on-cd | source\ncd ../project\nnode\n"
        .to_owned()
    },
    | ShellKind::Nu => {
      let loader = String::from_utf8(
        Command::new(RNM)
          .args(["env", "--shell", "nu", "--use-on-cd"])
          .output()
          .unwrap()
          .stdout,
      )
      .unwrap();
      // Nushell runs hooks only in its interactive REPL, so the script calls
      // the installed hook itself after each `cd`.
      format!(
        "cd project\n{loader}\n\
         def --env go [dir: string] {{\n\
           cd $dir\n\
           let hooks = ($env.config.hooks.env_change.PWD | where {{|h| ($h | describe | str starts-with 'record') and ('__rnm_use_on_cd' in ($h | columns)) }})\n\
           if ($hooks | length) != 1 {{ error make {{msg: $'expected one rnm hook, found ($hooks | length)'}} }}\n\
           do --env ($hooks | first).code null $env.PWD\n\
         }}\n\
         ^node\ngo ..\n^node\ngo project\n^node\ngo sub\n^node\n^rnm use 20 | ignore\ngo ..\n^node\n\
         go old\n^node\ngo ../..\n^node\n^rnm use 18 | ignore\ngo plain\n^node\n\
         {loader}\ngo ../project\n^node\n"
      )
    },
    | ShellKind::PowerShell => {
      // Windows PowerShell 5.1 has no LocationChangedAction, so the hook runs
      // from `prompt`, which a script never shows; `go` calls it there.
      let go = "function go($dir) {\n\
                  Set-Location -LiteralPath $dir\n\
                  if ($PSVersionTable.PSVersion -lt [version]'6.1') { $null = prompt }\n\
                }\n";
      format!(
        "$ErrorActionPreference = 'Stop'\n{go}Set-Location project\n\
         rnm env --shell powershell --use-on-cd | Out-String | Invoke-Expression\nnode\ngo ..\nnode\n\
         go project; node\ngo sub\nnode\nrnm use 20 | Out-Null\ngo ..\nnode\n\
         Push-Location old\nif ($PSVersionTable.PSVersion -lt [version]'6.1') {{ $null = prompt }}\nnode\n\
         go ../..\nnode\nrnm use 18 | Out-Null\ngo plain\nnode\n\
         rnm env --shell powershell --use-on-cd | Out-String | Invoke-Expression\ngo ../project\nnode\n"
      )
    },
  }
}

#[test]
fn use_on_cd_follows_version_files() {
  for shell in available() {
    let fixture = Fixture::new();
    let script = use_on_cd_script(shell);
    let output = if shell.kind == ShellKind::Bash {
      // Interactive, so the `cd` alias expands and `PROMPT_COMMAND` (which
      // catches `pushd`) runs.
      fixture.run_interactive(shell, &["--norc", "--noprofile", "-i"], &script)
    } else {
      fixture.run(shell, &script)
    };
    assert_eq!(
      output, USE_ON_CD_OUTPUT,
      "unexpected output from {}",
      shell.name
    );
  }
}

/// Without a `default` alias, leaving a project keeps the version; a version
/// file naming a missing version only prints an error; the prompt hook keeps
/// `$?`.
#[test]
fn use_on_cd_edge_cases_in_bash() {
  let Some(bash) = available().into_iter().find(|s| s.kind == ShellKind::Bash) else {
    return;
  };
  let fixture = Fixture::new();
  let script = "cd project\neval \"$(rnm env --shell bash --use-on-cd)\" || exit 1\n\
                rm \"$NVM_DIR/alias/default\"\ncd ..\nnode\n\
                mkdir missing && echo 99 > missing/.nvmrc\ncd missing\nnode\n\
                (exit 4)\necho $?\nexit\n";
  let output = fixture.run_interactive(bash, &["--norc", "--noprofile", "-i"], script);
  assert_eq!(output, "v22.20.0\nv22.20.0\n4\n");
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
        use_on_cd: None,
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
