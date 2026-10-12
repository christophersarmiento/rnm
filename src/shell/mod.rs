//! Shell code generation for `rnm env`.
//!
//! Supported shells: bash, zsh, fish, Nushell, and PowerShell (pwsh 7 on any
//! OS, and Windows PowerShell 5.1). Every value passes through the shell's
//! own quoting function; nothing is formatted with Rust's `{:?}`.
//!
//! Nushell cannot `eval` text, so `rnm env --shell nu` prints a static script
//! that runs `rnm env --json` at startup (see `docs/README.md` decision 11).
//!
//! With `--use-on-cd`, the output also installs a hook that runs
//! `rnm use --from-hook` whenever the nearest `.nvmrc` or `.node-version`
//! changes. The hook finds that file in shell code, so changing directory
//! inside a project (or between directories without one) starts no process.
//! Running the setup code again replaces the hook instead of adding a second
//! one.

use std::fmt::{self, Write as _};

use crate::sys::{self, Process};

/// How many ancestors to search for the calling shell. Covers wrappers such
/// as `sudo`, `env`, terminal multiplexers, and IDE launchers.
const SHELL_SEARCH_DEPTH: usize = 8;

/// A shell rnm can generate code for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, clap::ValueEnum)]
pub enum ShellKind {
  Bash,
  Zsh,
  Fish,
  #[value(name = "nu", alias = "nushell")]
  Nu,
  #[value(name = "powershell", alias = "pwsh")]
  PowerShell,
}

impl fmt::Display for ShellKind {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(match self {
      | Self::Bash => "bash",
      | Self::Zsh => "zsh",
      | Self::Fish => "fish",
      | Self::Nu => "nu",
      | Self::PowerShell => "powershell",
    })
  }
}

/// The result of looking for the calling shell in the process tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inferred {
  /// A supported shell.
  Supported(ShellKind),
  /// A shell rnm does not support yet, by process name.
  Unsupported(String),
  /// No shell found.
  Unknown,
}

/// Process names of shells rnm recognizes but does not support yet. Seeing
/// one stops the search, so `dash` started from `bash` is not mistaken for
/// bash.
const UNSUPPORTED: &[&str] = &[
  "sh", "dash", "ash", "busybox", "ksh", "ksh93", "mksh", "lksh", "pdksh", "oksh", "csh", "tcsh",
  "xonsh", "elvish", "cmd", "psh",
];

impl ShellKind {
  /// Every supported shell.
  pub const ALL: [Self; 5] = [
    Self::Bash,
    Self::Zsh,
    Self::Fish,
    Self::Nu,
    Self::PowerShell,
  ];

  /// The supported shells' names, for messages: `bash, zsh, fish, nu,
  /// powershell`.
  #[must_use]
  pub fn names() -> String {
    Self::ALL.map(|shell| shell.to_string()).join(", ")
  }

  /// Finds the calling shell by walking up the process tree.
  #[must_use]
  pub fn detect() -> Inferred {
    Self::infer(&sys::ancestors(SHELL_SEARCH_DEPTH))
  }

  /// Finds the nearest shell among `ancestors` (nearest first).
  #[must_use]
  pub fn infer(ancestors: &[Process]) -> Inferred {
    for process in ancestors {
      let name = process.name.trim_start_matches('-').to_ascii_lowercase();
      let kind = match name.as_str() {
        | "bash" => Self::Bash,
        | "zsh" => Self::Zsh,
        | "fish" => Self::Fish,
        | "nu" => Self::Nu,
        | "pwsh" | "powershell" => Self::PowerShell,
        | other if UNSUPPORTED.contains(&other) => return Inferred::Unsupported(name),
        | _ => continue,
      };
      return Inferred::Supported(kind);
    }
    Inferred::Unknown
  }

  /// The line users put in their shell's startup file.
  #[must_use]
  pub const fn setup_hint(self) -> &'static str {
    match self {
      | Self::Bash => "eval \"$(rnm env --shell bash)\"  # in ~/.bashrc",
      | Self::Zsh => "eval \"$(rnm env --shell zsh)\"  # in ~/.zshrc",
      | Self::Fish => "rnm env --shell fish | source  # in ~/.config/fish/config.fish",
      | Self::Nu => {
        "rnm env --shell nu | save -f ($nu.data-dir | path join vendor/autoload/rnm.nu)  # run once"
      },
      | Self::PowerShell => {
        "rnm env --shell powershell | Out-String | Invoke-Expression  # in $PROFILE"
      },
    }
  }
}

/// What `rnm env` sets up, independent of the shell.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnvPlan {
  /// Directories to put at the front of `PATH`, in order.
  pub path_prepend: Vec<String>,
  /// Variables to export, in order.
  pub set: Vec<(String, String)>,
  /// Install the hook that switches versions after a directory change.
  pub use_on_cd: Option<CdHook>,
}

/// How the `--use-on-cd` hook runs `rnm use`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CdHook {
  /// Install a missing version instead of reporting it.
  pub install_if_missing: bool,
}

impl CdHook {
  /// The flags for `rnm use` and `rnm env --json` that select this hook
  /// behaviour, each with a leading space.
  fn flags(self) -> &'static str {
    if self.install_if_missing {
      " --install-if-missing"
    } else {
      ""
    }
  }

  /// `template` with this hook's flags after `--from-hook`.
  fn apply(self, template: &str) -> String {
    template.replace(HOOK_FLAG, &format!("{HOOK_FLAG}{}", self.flags()))
  }
}

/// The flag every hook template passes to `rnm use`.
const HOOK_FLAG: &str = "--from-hook";

/// The `--use-on-cd` hooks, one per shell.
const BASH_HOOK: &str = include_str!("use_on_cd/hook.bash");
const ZSH_HOOK: &str = include_str!("use_on_cd/hook.zsh");
const FISH_HOOK: &str = include_str!("use_on_cd/hook.fish");
const NU_HOOK: &str = include_str!("use_on_cd/hook.nu");
const POWERSHELL_HOOK: &str = include_str!("use_on_cd/hook.ps1");

impl EnvPlan {
  /// Renders the plan for `shell`. For Nushell this is the static loader
  /// script, which only reads `use_on_cd`; see [`EnvPlan::render_json`].
  #[must_use]
  pub fn render(&self, shell: ShellKind) -> String {
    let mut out = String::new();
    let hook = match shell {
      | ShellKind::Nu => return nu_loader(self.use_on_cd),
      | ShellKind::Bash | ShellKind::Zsh => {
        for dir in &self.path_prepend {
          let _ = writeln!(out, "export PATH={}:\"$PATH\"", quote_posix(dir));
        }
        for (key, value) in &self.set {
          let _ = writeln!(out, "export {key}={}", quote_posix(value));
        }
        if shell == ShellKind::Bash {
          BASH_HOOK
        } else {
          ZSH_HOOK
        }
      },
      | ShellKind::Fish => {
        for dir in &self.path_prepend {
          let _ = writeln!(out, "set -gx PATH {} $PATH", quote_fish(dir));
        }
        for (key, value) in &self.set {
          let _ = writeln!(out, "set -gx {key} {}", quote_fish(value));
        }
        FISH_HOOK
      },
      | ShellKind::PowerShell => {
        for dir in &self.path_prepend {
          let _ = writeln!(
            out,
            "$env:PATH = {} + [IO.Path]::PathSeparator + $env:PATH",
            quote_powershell(dir)
          );
        }
        for (key, value) in &self.set {
          let _ = writeln!(out, "$env:{key} = {}", quote_powershell(value));
        }
        POWERSHELL_HOOK
      },
    };
    if let Some(cd_hook) = self.use_on_cd {
      out.push_str(&cd_hook.apply(hook));
    }
    out
  }

  /// The variables as a JSON object, for `rnm env --json` (used by the
  /// Nushell script with `load-env`). `PATH` is not included; consumers
  /// prepend `NVM_BIN` themselves, which keeps Nushell's list semantics.
  #[must_use]
  pub fn render_json(&self) -> String {
    let fields: Vec<String> = self
      .set
      .iter()
      .map(|(key, value)| format!("{}:{}", quote_json(key), quote_json(value)))
      .collect();
    format!("{{{}}}\n", fields.join(","))
  }
}

/// The static Nushell loader. It works whether `PATH` is a list (normal) or
/// still a string, and `uniq` keeps re-sourcing from adding duplicates.
fn nu_loader(use_on_cd: Option<CdHook>) -> String {
  let (flag, hook) = match use_on_cd {
    | Some(cd_hook) => (
      format!(" --use-on-cd{}", cd_hook.flags()),
      cd_hook.apply(NU_HOOK),
    ),
    | None => (String::new(), String::new()),
  };
  format!(
    "# Generated by `rnm env --shell nu{flag}`. Static: save it once, for example:\n\
     #   rnm env --shell nu{flag} | save -f ($nu.data-dir | path join vendor/autoload/rnm.nu)\n\
     ^rnm env --json{flag} | from json | load-env\n\
     {NU_PATH}{hook}"
  )
}

const NU_PATH: &str = r"$env.PATH = (
  (if ($env.PATH | describe) == 'string' { $env.PATH | split row (char esep) } else { $env.PATH })
  | prepend $env.NVM_BIN
  | uniq
)
";

/// POSIX single quotes: everything is literal; `'` becomes `'\''`.
#[must_use]
pub fn quote_posix(value: &str) -> String {
  format!("'{}'", value.replace('\'', r"'\''"))
}

/// fish single quotes: only `\` and `'` need escaping.
#[must_use]
pub fn quote_fish(value: &str) -> String {
  format!("'{}'", value.replace('\\', r"\\").replace('\'', r"\'"))
}

/// PowerShell single quotes, with ASCII-only output.
///
/// `'` becomes `''`. Non-ASCII characters are emitted as `[char]0xNNNN`
/// (one per UTF-16 unit) and concatenated, because Windows PowerShell 5.1
/// decodes a native command's output with the OEM code page. This also covers
/// the curly quotes U+2018 to U+201B, which PowerShell treats as `'`. The
/// result is wrapped in parentheses when concatenation is needed, so it is
/// always a single expression.
#[must_use]
pub fn quote_powershell(value: &str) -> String {
  if value.is_ascii() {
    return powershell_ascii_literal(value);
  }
  // Start with '' so `+` is string concatenation even if the first part is a
  // [char].
  let mut parts = vec!["''".to_owned()];
  let mut ascii = String::new();
  for c in value.chars() {
    if c.is_ascii() {
      ascii.push(c);
      continue;
    }
    if !ascii.is_empty() {
      parts.push(powershell_ascii_literal(&ascii));
      ascii.clear();
    }
    let mut units = [0u16; char::MAX_LEN_UTF16];
    for unit in c.encode_utf16(&mut units) {
      parts.push(format!("[char]0x{unit:04X}"));
    }
  }
  if !ascii.is_empty() {
    parts.push(powershell_ascii_literal(&ascii));
  }
  format!("({})", parts.join(" + "))
}

fn powershell_ascii_literal(ascii: &str) -> String {
  format!("'{}'", ascii.replace('\'', "''"))
}

/// A JSON string literal.
#[must_use]
pub fn quote_json(value: &str) -> String {
  let mut out = String::with_capacity(value.len());
  out.push('"');
  for c in value.chars() {
    match c {
      | '"' => out.push_str("\\\""),
      | '\\' => out.push_str("\\\\"),
      | '\n' => out.push_str("\\n"),
      | '\r' => out.push_str("\\r"),
      | '\t' => out.push_str("\\t"),
      | c if c.is_ascii_control() => {
        let _ = write!(out, "\\u{:04x}", u32::from(c));
      },
      | c => out.push(c),
    }
  }
  out.push('"');
  out
}

#[cfg(test)]
mod tests {
  use super::*;

  fn process(name: &str) -> Process {
    Process {
      pid: 1,
      name: name.to_owned(),
    }
  }

  fn plan() -> EnvPlan {
    EnvPlan {
      path_prepend: vec!["/s/ms/1_2/bin".to_owned()],
      set: vec![
        ("RNM_MULTISHELL_PATH".to_owned(), "/s/ms/1_2".to_owned()),
        ("NVM_BIN".to_owned(), "/s/ms/1_2/bin".to_owned()),
      ],
      use_on_cd: None,
    }
  }

  fn plan_with_hook(install_if_missing: bool) -> EnvPlan {
    EnvPlan {
      use_on_cd: Some(CdHook { install_if_missing }),
      ..plan()
    }
  }

  #[test]
  fn infers_nearest_shell() {
    let infer =
      |names: &[&str]| ShellKind::infer(&names.iter().map(|n| process(n)).collect::<Vec<_>>());
    assert_eq!(infer(&["zsh"]), Inferred::Supported(ShellKind::Zsh));
    assert_eq!(infer(&["-zsh"]), Inferred::Supported(ShellKind::Zsh));
    assert_eq!(
      infer(&["sudo", "env", "fish", "bash"]),
      Inferred::Supported(ShellKind::Fish)
    );
    assert_eq!(infer(&["pwsh"]), Inferred::Supported(ShellKind::PowerShell));
    assert_eq!(
      infer(&["PowerShell"]),
      Inferred::Supported(ShellKind::PowerShell)
    );
    assert_eq!(infer(&["nu"]), Inferred::Supported(ShellKind::Nu));
    assert_eq!(
      infer(&["dash", "bash"]),
      Inferred::Unsupported("dash".to_owned())
    );
    assert_eq!(
      infer(&["sh", "zsh"]),
      Inferred::Unsupported("sh".to_owned())
    );
    assert_eq!(infer(&["tmux", "launchd"]), Inferred::Unknown);
    assert_eq!(infer(&[]), Inferred::Unknown);
  }

  #[test]
  fn posix_quoting() {
    assert_eq!(quote_posix("/a b/c"), "'/a b/c'");
    assert_eq!(quote_posix("it's $HOME `x`"), r"'it'\''s $HOME `x`'");
  }

  #[test]
  fn fish_quoting() {
    assert_eq!(quote_fish(r"C:\a 'b'"), r"'C:\\a \'b\''");
  }

  #[test]
  fn powershell_quoting() {
    assert_eq!(
      quote_powershell(r"C:\Users\O'Brien"),
      r"'C:\Users\O''Brien'"
    );
    assert_eq!(quote_powershell("$env:x `n"), "'$env:x `n'");
    assert_eq!(quote_powershell("José"), "('' + 'Jos' + [char]0x00E9)");
    assert_eq!(quote_powershell("\u{2019}x"), "('' + [char]0x2019 + 'x')");
    // A non-BMP character becomes a surrogate pair.
    assert_eq!(quote_powershell("😀"), "('' + [char]0xD83D + [char]0xDE00)");
  }

  #[test]
  fn json_quoting() {
    assert_eq!(quote_json("a\"b\\c\nd\u{1}é"), r#""a\"b\\c\nd\u0001é""#);
  }

  #[test]
  fn renders_bash_and_zsh() {
    let expected = "export PATH='/s/ms/1_2/bin':\"$PATH\"\n\
                    export RNM_MULTISHELL_PATH='/s/ms/1_2'\n\
                    export NVM_BIN='/s/ms/1_2/bin'\n";
    assert_eq!(plan().render(ShellKind::Bash), expected);
    assert_eq!(plan().render(ShellKind::Zsh), expected);
  }

  #[test]
  fn renders_fish() {
    assert_eq!(
      plan().render(ShellKind::Fish),
      "set -gx PATH '/s/ms/1_2/bin' $PATH\n\
       set -gx RNM_MULTISHELL_PATH '/s/ms/1_2'\n\
       set -gx NVM_BIN '/s/ms/1_2/bin'\n"
    );
  }

  #[test]
  fn renders_powershell() {
    assert_eq!(
      plan().render(ShellKind::PowerShell),
      "$env:PATH = '/s/ms/1_2/bin' + [IO.Path]::PathSeparator + $env:PATH\n\
       $env:RNM_MULTISHELL_PATH = '/s/ms/1_2'\n\
       $env:NVM_BIN = '/s/ms/1_2/bin'\n"
    );
    assert!(plan().render(ShellKind::PowerShell).is_ascii());
  }

  #[test]
  fn renders_nu_and_json() {
    let loader = plan().render(ShellKind::Nu);
    assert!(loader.contains("^rnm env --json | from json | load-env\n"));
    assert!(!loader.contains("__rnm_use_on_cd"));
    assert!(
      !loader.contains("/s/ms"),
      "the loader must not contain session paths"
    );
    assert_eq!(
      plan().render_json(),
      "{\"RNM_MULTISHELL_PATH\":\"/s/ms/1_2\",\"NVM_BIN\":\"/s/ms/1_2/bin\"}\n"
    );
  }

  #[test]
  fn appends_the_use_on_cd_hook() {
    for shell in [
      ShellKind::Bash,
      ShellKind::Zsh,
      ShellKind::Fish,
      ShellKind::PowerShell,
    ] {
      let without = plan().render(shell);
      let with = plan_with_hook(false).render(shell);
      let hook = with
        .strip_prefix(&without)
        .expect("the hook comes after the variables");
      assert_eq!(hook.matches("rnm use --from-hook").count(), 1, "{shell}");
      assert!(!hook.contains("--install-if-missing"), "{shell}");
      let installing = plan_with_hook(true).render(shell);
      assert_eq!(
        installing
          .matches("rnm use --from-hook --install-if-missing")
          .count(),
        1,
        "{shell}"
      );
    }
    assert!(
      plan_with_hook(true)
        .render(ShellKind::PowerShell)
        .is_ascii()
    );
  }

  #[test]
  fn nu_loader_passes_hook_flags_through() {
    let loader = plan_with_hook(false).render(ShellKind::Nu);
    assert!(loader.contains("^rnm env --json --use-on-cd | from json | load-env\n"));
    assert!(loader.contains("^rnm use --from-hook }"));
    let loader = plan_with_hook(true).render(ShellKind::Nu);
    assert!(loader.contains("^rnm env --json --use-on-cd --install-if-missing | from json"));
    assert!(loader.contains("^rnm use --from-hook --install-if-missing }"));
  }
}
