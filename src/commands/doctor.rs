//! `rnm doctor`: check the setup and explain what to fix.

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::{Context, DEFAULT_ALIAS, Failure, exit, network, say};
use crate::multishell::Target;
use crate::nvm_dir::{NODE_EXECUTABLE, Resolved, is_executable};
use crate::platform::Platform;
use crate::remote::load_index;
use crate::shell::{Inferred, ShellKind};
use crate::version::VersionSpec;

/// How a check turned out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
  Ok,
  /// Works, but something is likely to surprise the user.
  Warn,
  /// Something rnm needs is broken.
  Fail,
}

impl fmt::Display for Status {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.pad(match self {
      | Self::Ok => "ok",
      | Self::Warn => "warn",
      | Self::Fail => "fail",
    })
  }
}

/// The result of one check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
  pub status: Status,
  pub topic: &'static str,
  pub message: String,
}

impl Finding {
  fn new(status: Status, topic: &'static str, message: impl Into<String>) -> Self {
    Self {
      status,
      topic,
      message: message.into(),
    }
  }
}

/// The width of the status column (`warn`, `fail`).
const STATUS_WIDTH: usize = 4;

/// Runs every check, prints one line each, and ends with a summary. The
/// output is plain ASCII, so it reads the same in any console.
///
/// The mirror check downloads (or revalidates) the version index; all other
/// checks are local.
///
/// # Errors
///
/// Exit code 1 when any check fails. Warnings alone exit 0.
pub fn doctor(ctx: &Context, out: &mut dyn Write) -> Result<(), Failure> {
  let mut findings = vec![
    platform(),
    install_dir(ctx),
    shell_setup(ctx),
    default_alias(ctx),
    npm_prefix(ctx),
    other_managers(ctx),
    mirror(ctx),
  ];
  #[cfg(windows)]
  findings.push(execution_policy());
  findings.retain(|finding| !finding.message.is_empty());

  for finding in &findings {
    say(
      out,
      format_args!(
        "{:<STATUS_WIDTH$}  {}: {}",
        finding.status, finding.topic, finding.message
      ),
    );
  }
  let count = |status| findings.iter().filter(|f| f.status == status).count();
  let (failures, warnings) = (count(Status::Fail), count(Status::Warn));
  say(out, "");
  say(out, summary(failures, warnings));
  if failures > 0 {
    return Err(Failure::new(exit::ERROR, ""));
  }
  Ok(())
}

fn summary(failures: usize, warnings: usize) -> String {
  let plural = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
  match (failures, warnings) {
    | (0, 0) => "No problems found.".to_owned(),
    | (0, w) => format!("{}, no problems.", plural(w, "warning")),
    | (f, w) => format!("{}, {}.", plural(f, "problem"), plural(w, "warning")),
  }
}

fn platform() -> Finding {
  match Platform::detect() {
    | Ok(platform) => {
      let libc = if platform.musl { "-musl" } else { "" };
      Finding::new(
        Status::Ok,
        "platform",
        format!(
          "{}-{}{libc}, rnm {}",
          platform.os.dist_name(),
          platform.arch,
          env!("CARGO_PKG_VERSION")
        ),
      )
    },
    | Err(e) => Finding::new(Status::Fail, "platform", e.to_string()),
  }
}

/// The tree exists and is writable, or can be created.
fn install_dir(ctx: &Context) -> Finding {
  const TOPIC: &str = "install directory";
  let root = ctx.nvm_dir.root();
  if !root.exists() {
    return Finding::new(
      Status::Ok,
      TOPIC,
      format!("{} (created by the first install)", root.display()),
    );
  }
  let probe = root.join(format!(".rnm-doctor-{}", std::process::id()));
  let writable = std::fs::write(&probe, "").is_ok();
  let _ = std::fs::remove_file(&probe);
  if !writable {
    return Finding::new(
      Status::Fail,
      TOPIC,
      format!("{} is not writable", root.display()),
    );
  }
  let count = ctx.nvm_dir.installed_versions().map_or(0, |v| v.len());
  Finding::new(
    Status::Ok,
    TOPIC,
    format!("{} ({count} installed)", root.display()),
  )
}

/// This shell runs `rnm env`, its link is on `PATH`, and no other `node`
/// comes first.
fn shell_setup(ctx: &Context) -> Finding {
  const TOPIC: &str = "shell";
  let Some(session) = &ctx.session else {
    let hint = match ShellKind::detect() {
      | Inferred::Supported(shell) => {
        format!("; add this to its startup file: {}", shell.setup_hint())
      },
      | _ => "; see `rnm env --help`".to_owned(),
    };
    return Finding::new(
      Status::Warn,
      TOPIC,
      format!("rnm is not set up in this shell{hint}"),
    );
  };
  let bin = session.bin_dir();
  let Some(position) = ctx.path.iter().position(|dir| same_dir(dir, &bin)) else {
    return Finding::new(
      Status::Fail,
      TOPIC,
      format!(
        "this shell's link {} is not on PATH; run the `rnm env` line again",
        bin.display()
      ),
    );
  };
  let shadowing = ctx.path[..position]
    .iter()
    .map(|dir| dir.join(NODE_EXECUTABLE))
    .find(|node| is_executable(node));
  if let Some(node) = shadowing {
    return Finding::new(
      Status::Fail,
      TOPIC,
      format!(
        "{} comes before rnm on PATH, so `node` runs it instead of the version rnm selects; \
         load rnm after whatever adds it",
        node.display()
      ),
    );
  }
  let using = match session.target() {
    | Ok(Some(Target::Version(dir))) => ctx
      .nvm_dir
      .installed_at(&dir)
      .map_or_else(|| dir.display().to_string(), |v| v.to_string()),
    | Ok(Some(Target::System)) => "system".to_owned(),
    | Ok(None) => {
      return Finding::new(
        Status::Warn,
        TOPIC,
        "this shell's link is missing (deleted while the shell ran); `rnm use` recreates it",
      );
    },
    | Err(e) => return Finding::new(Status::Fail, TOPIC, e.to_string()),
  };
  Finding::new(Status::Ok, TOPIC, format!("set up, using {using}"))
}

/// Paths from `PATH` are compared as text, ignoring case on Windows.
fn same_dir(a: &Path, b: &Path) -> bool {
  let trimmed = |p: &Path| {
    let text = p.to_string_lossy();
    let text = text.trim_end_matches(['/', '\\']);
    if cfg!(windows) {
      text.replace('/', "\\").to_lowercase()
    } else {
      text.to_owned()
    }
  };
  trimmed(a) == trimmed(b)
}

fn default_alias(ctx: &Context) -> Finding {
  const TOPIC: &str = "default alias";
  let target = match ctx.nvm_dir.aliases().get(DEFAULT_ALIAS) {
    | Ok(Some(target)) => target,
    | Ok(None) => {
      return Finding::new(
        Status::Warn,
        TOPIC,
        "none, so new shells start with the system Node.js; set one with \
         `rnm alias default <version>`",
      );
    },
    | Err(e) => return Finding::new(Status::Fail, TOPIC, e.to_string()),
  };
  let resolved = target
    .parse::<VersionSpec>()
    .map_err(|e| e.to_string())
    .and_then(|spec| ctx.nvm_dir.resolve(&spec).map_err(|e| e.to_string()));
  match resolved {
    | Ok(Resolved::Installed(installed)) => {
      Finding::new(Status::Ok, TOPIC, format!("{target} ({installed})"))
    },
    | Ok(Resolved::System | Resolved::Current) => Finding::new(Status::Ok, TOPIC, target),
    | Err(e) => Finding::new(
      Status::Warn,
      TOPIC,
      format!("{target}: {e}; new shells start with the system Node.js"),
    ),
  }
}

/// nvm refuses to run when npm's `prefix` is set, because global packages
/// then land outside the version directories.
fn npm_prefix(ctx: &Context) -> Finding {
  const TOPIC: &str = "npm prefix";
  const CONSEQUENCE: &str = "global packages and their commands do not follow `rnm use`; \
                             remove it (nvm refuses to run with it)";
  for var in ["NPM_CONFIG_PREFIX", "npm_config_prefix"] {
    if let Some(value) = ctx.var_string(var) {
      return Finding::new(
        Status::Warn,
        TOPIC,
        format!("{var}={value} is set: {CONSEQUENCE}"),
      );
    }
  }
  let npmrc = ctx
    .var("NPM_CONFIG_USERCONFIG")
    .or_else(|| ctx.var("npm_config_userconfig"))
    .map(PathBuf::from)
    .or_else(|| home_dir(ctx).map(|home| home.join(".npmrc")));
  if let Some(npmrc) = npmrc
    && let Ok(text) = std::fs::read_to_string(&npmrc)
    && let Some(prefix) = npmrc_prefix(&text)
  {
    return Finding::new(
      Status::Warn,
      TOPIC,
      format!("{} sets prefix={prefix}: {CONSEQUENCE}", npmrc.display()),
    );
  }
  Finding::new(Status::Ok, TOPIC, "not overridden")
}

fn home_dir(ctx: &Context) -> Option<PathBuf> {
  let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
  ctx.var(var).map(PathBuf::from)
}

/// The value of a `prefix` line in an `.npmrc` (`;` and `#` start
/// comments).
fn npmrc_prefix(text: &str) -> Option<&str> {
  text.lines().find_map(|line| {
    let line = line.trim();
    let is_comment = line.starts_with(';') || line.starts_with('#');
    let (key, value) = line.split_once('=')?;
    (!is_comment && key.trim() == "prefix").then(|| value.trim())
  })
}

/// Other Node.js version managers active in this shell, which also change
/// `PATH`.
fn other_managers(ctx: &Context) -> Finding {
  const TOPIC: &str = "other version managers";
  let mut found = Vec::new();
  // rnm sets NVM_BIN to its session link; nvm.sh to a version directory.
  let nvm_sh_is_active = ctx
    .var("NVM_BIN")
    .is_some_and(|bin| !Path::new(&bin).starts_with(ctx.multishells.dir()));
  if nvm_sh_is_active {
    found.push("nvm (NVM_BIN points into a version, as nvm.sh sets it)");
  }
  if ctx.var("FNM_MULTISHELL_PATH").is_some() {
    found.push("fnm (FNM_MULTISHELL_PATH)");
  }
  if ctx.var("VOLTA_HOME").is_some() {
    found.push("Volta (VOLTA_HOME)");
  }
  if cfg!(windows) && ctx.var("NVM_SYMLINK").is_some() {
    found.push("nvm-windows (NVM_SYMLINK)");
  }
  if found.is_empty() {
    return Finding::new(Status::Ok, TOPIC, "none active");
  }
  Finding::new(
    Status::Warn,
    TOPIC,
    format!(
      "{} also active in this shell; whichever changed PATH last wins",
      found.join(", ")
    ),
  )
}

/// The mirror settings are valid and the index downloads with them.
fn mirror(ctx: &Context) -> Finding {
  const TOPIC: &str = "mirror";
  let network = match network(ctx, &mut std::io::sink()) {
    | Ok(network) => network,
    | Err(e) => return Finding::new(Status::Fail, TOPIC, e.message),
  };
  let auth = if network.credential_sources.is_empty() {
    "no credentials".to_owned()
  } else {
    format!("credentials from {}", network.credential_sources.join(", "))
  };
  let fallbacks = network.mirrors.len() - 1;
  let fallback_note = match fallbacks {
    | 0 => String::new(),
    | 1 => ", 1 fallback mirror".to_owned(),
    | n => format!(", {n} fallback mirrors"),
  };
  let mut problems: Vec<String> = network.warnings.clone();
  match load_index(&network.client, &network.mirrors, &ctx.nvm_dir.cache_dir()) {
    | Ok(loaded) => {
      problems.extend(loaded.warnings.iter().map(ToString::to_string));
      let served = &network.mirrors[loaded.mirror].url;
      let summary = format!(
        "{served} answers ({auth}{fallback_note}, {} releases)",
        loaded.index.releases().len()
      );
      if problems.is_empty() {
        Finding::new(Status::Ok, TOPIC, summary)
      } else {
        Finding::new(
          Status::Fail,
          TOPIC,
          format!("{summary}: {}", problems.join("; ")),
        )
      }
    },
    | Err(e) => Finding::new(Status::Fail, TOPIC, format!("{e} ({auth})")),
  }
}

/// Windows PowerShell 5.1 skips `$PROFILE`, and with it `rnm env`, under
/// the `Restricted` and `AllSigned` policies. The first policy found wins,
/// in PowerShell's order: group policy (machine, then user), then the
/// user's and the machine's own setting.
#[cfg(windows)]
fn execution_policy() -> Finding {
  use crate::sys::{Hive, registry_string};
  const TOPIC: &str = "PowerShell execution policy";
  const POLICY_KEY: &str = r"SOFTWARE\Policies\Microsoft\Windows\PowerShell";
  const SHELL_KEY: &str = r"SOFTWARE\Microsoft\PowerShell\1\ShellIds\Microsoft.PowerShell";
  let sources = [
    (Hive::LocalMachine, POLICY_KEY, "group policy (machine)"),
    (Hive::CurrentUser, POLICY_KEY, "group policy (user)"),
    (Hive::CurrentUser, SHELL_KEY, "CurrentUser"),
    (Hive::LocalMachine, SHELL_KEY, "LocalMachine"),
  ];
  let found = sources.iter().find_map(|(hive, key, scope)| {
    registry_string(*hive, key, "ExecutionPolicy")
      .filter(|policy| !policy.is_empty() && policy != "Undefined")
      .map(|policy| (policy, *scope))
  });
  let fix = "Windows PowerShell 5.1 does not run $PROFILE, so rnm is not set up there; run \
             `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned` (pwsh 7 is not affected)";
  match found {
    | Some((policy, scope)) if matches!(policy.as_str(), "Restricted" | "AllSigned") => {
      Finding::new(Status::Warn, TOPIC, format!("{policy} ({scope}): {fix}"))
    },
    | Some((policy, scope)) => Finding::new(Status::Ok, TOPIC, format!("{policy} ({scope})")),
    | None => Finding::new(
      Status::Warn,
      TOPIC,
      format!("not set, which means Restricted on Windows 10 and 11: {fix}"),
    ),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn reads_npmrc_prefix() {
    assert_eq!(npmrc_prefix("prefix=/usr/local\n"), Some("/usr/local"));
    assert_eq!(
      npmrc_prefix("registry=x\n  prefix = ~/.npm-global  \n"),
      Some("~/.npm-global")
    );
    assert_eq!(npmrc_prefix("; prefix=/a\n# prefix=/b\n"), None);
    assert_eq!(npmrc_prefix("prefixes=/a\n"), None);
  }

  #[test]
  fn summarizes() {
    assert_eq!(summary(0, 0), "No problems found.");
    assert_eq!(summary(0, 1), "1 warning, no problems.");
    assert_eq!(summary(2, 3), "2 problems, 3 warnings.");
  }

  #[test]
  fn compares_path_entries() {
    assert!(same_dir(Path::new("/a/b/"), Path::new("/a/b")));
    assert!(!same_dir(Path::new("/a/b"), Path::new("/a/c")));
    #[cfg(windows)]
    assert!(same_dir(Path::new(r"C:\A\b"), Path::new("c:/a/B")));
  }
}
