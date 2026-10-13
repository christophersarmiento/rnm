//! Global npm packages: nvm's `default-packages` file,
//! `--reinstall-packages-from`, and `--latest-npm`
//! (`docs/research/nvm.md` §4.4, §10 SHOULD-4 and SHOULD-5).
//!
//! npm runs as `node <npm-cli.js>` from the target version, so no shell
//! script (`npm`, `npm.cmd`) is involved and the right `node` always runs
//! it.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{Context, Failure, SYSTEM_LABEL, exit, say};
use crate::nvm_dir::{
  InstalledVersion, Resolved, bin_dir, global_modules_dir, node_binary, version_dir_of,
};
use crate::version::Version;

/// The file listing packages to install into every new version.
pub const DEFAULT_PACKAGES_FILE: &str = "default-packages";

/// Packages that ship with Node.js; reinstalling them would replace the
/// bundled copies (nvm skips the same two).
const BUNDLED_PACKAGES: [&str; 2] = ["npm", "corepack"];

/// Runs the npm bundled with one installed version.
pub(crate) struct Npm<'a> {
  ctx: &'a Context,
  version_dir: &'a Path,
}

impl<'a> Npm<'a> {
  pub(crate) fn of(ctx: &'a Context, version_dir: &'a Path) -> Self {
    Self { ctx, version_dir }
  }

  fn cli(&self) -> PathBuf {
    global_modules_dir(self.version_dir)
      .join("npm")
      .join("bin")
      .join("npm-cli.js")
  }

  /// Runs `npm <args>` in `cwd` (rnm's working directory when `None`),
  /// with the version's `bin` directory first on `PATH` so that install
  /// scripts find the same `node`. npm's output goes straight to the
  /// terminal.
  fn run(&self, args: &[&str], cwd: Option<&Path>) -> Result<(), Failure> {
    let cli = self.cli();
    if !cli.is_file() {
      return Err(Failure::new(
        exit::ERROR,
        format!("npm is not installed in {}", self.version_dir.display()),
      ));
    }
    let path = std::env::join_paths(
      std::iter::once(bin_dir(self.version_dir)).chain(self.ctx.path.iter().cloned()),
    )
    .unwrap_or_else(|_| OsString::from(bin_dir(self.version_dir)));
    let mut command = Command::new(node_binary(self.version_dir));
    command.arg(&cli).args(args).env("PATH", path);
    if let Some(cwd) = cwd {
      command.current_dir(cwd);
    }
    let status = command
      .status()
      .map_err(|e| Failure::new(exit::ERROR, format!("cannot run npm: {e}")))?;
    if status.success() {
      Ok(())
    } else {
      Err(Failure::new(
        exit::ERROR,
        format!("npm {} failed ({status})", args.join(" ")),
      ))
    }
  }

  fn install_global(&self, packages: &[String]) -> Result<(), Failure> {
    let mut args = vec!["install", "-g", "--quiet"];
    args.extend(packages.iter().map(String::as_str));
    self.run(&args, None)
  }
}

/// The packages in `<root>/default-packages`, with nvm's rules: one package
/// spec per line; lines that start with `#` (after blanks) and empty lines
/// are skipped; any other line with a blank in it is an error. A missing
/// file means no packages.
///
/// # Errors
///
/// Exit code 1 when a line holds more than one word or the file cannot be
/// read.
pub(crate) fn default_packages(file: &Path) -> Result<Vec<String>, Failure> {
  let text = match std::fs::read_to_string(file) {
    | Ok(text) => text,
    | Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
    | Err(e) => {
      return Err(Failure::new(
        exit::ERROR,
        format!("{}: {e}", file.display()),
      ));
    },
  };
  let mut packages = Vec::new();
  for line in text.lines().map(|line| line.trim_end_matches('\r')) {
    let content = line.trim_start_matches([' ', '\t']);
    if content.is_empty() || content.starts_with('#') {
      continue;
    }
    if line.contains([' ', '\t']) {
      return Err(Failure::new(
        exit::ERROR,
        format!(
          "Only one package per line is allowed in `{}`. Please remove any lines with multiple \
           space-separated values.",
          file.display()
        ),
      ));
    }
    packages.push(line.to_owned());
  }
  Ok(packages)
}

/// Installs the packages from `default-packages` into `installed`.
///
/// # Errors
///
/// Exit code 1 when the file is invalid or npm fails.
pub(crate) fn install_default_packages(
  ctx: &Context,
  installed: &InstalledVersion,
  out: &mut dyn Write,
) -> Result<(), Failure> {
  let file = ctx.nvm_dir.root().join(DEFAULT_PACKAGES_FILE);
  let packages = default_packages(&file)?;
  if packages.is_empty() {
    return Ok(());
  }
  say(
    out,
    format_args!(
      "Installing default global packages from {}...",
      file.display()
    ),
  );
  say(
    out,
    format_args!("npm install -g --quiet {}", packages.join(" ")),
  );
  let _ = out.flush();
  Npm::of(ctx, &installed.path)
    .install_global(&packages)
    .map_err(|_| {
      Failure::new(
        exit::ERROR,
        "Failed installing default packages. Please check if your default-packages file or a \
         package in it has problems!",
      )
    })
}

/// The global packages of one version, read from its `node_modules`
/// without running npm.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct GlobalPackages {
  /// `name@version` specs for packages installed from the registry.
  pub installs: Vec<String>,
  /// The directories of packages installed with `npm link`.
  pub links: Vec<PathBuf>,
}

/// Lists the global packages in `modules_dir`, scoped ones included,
/// except the ones bundled with Node.js.
#[must_use]
pub(crate) fn global_packages(modules_dir: &Path) -> GlobalPackages {
  let mut packages = GlobalPackages::default();
  for (name, path) in package_dirs(modules_dir) {
    if BUNDLED_PACKAGES.contains(&name.as_str()) {
      continue;
    }
    let is_link = std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink());
    if is_link {
      if let Ok(target) = std::fs::canonicalize(&path) {
        packages.links.push(target);
      }
      continue;
    }
    let version = std::fs::read_to_string(path.join("package.json"))
      .ok()
      .and_then(|text| super::json_string_field(&text, "version"));
    packages.installs.push(match version {
      | Some(version) => format!("{name}@{version}"),
      | None => name,
    });
  }
  packages.installs.sort();
  packages.links.sort();
  packages
}

/// `(package name, directory)` for every package in `modules_dir`; a
/// `@scope` directory contributes its children as `@scope/name`.
fn package_dirs(modules_dir: &Path) -> Vec<(String, PathBuf)> {
  let entries = |dir: &Path| -> Vec<(String, PathBuf)> {
    std::fs::read_dir(dir)
      .into_iter()
      .flatten()
      .flatten()
      .map(|entry| {
        (
          entry.file_name().to_string_lossy().into_owned(),
          entry.path(),
        )
      })
      .filter(|(name, _)| !name.starts_with('.'))
      .collect()
  };
  let mut found = Vec::new();
  for (name, path) in entries(modules_dir) {
    if name.starts_with('@') {
      found.extend(
        entries(&path)
          .into_iter()
          .map(|(child, child_path)| (format!("{name}/{child}"), child_path)),
      );
    } else {
      found.push((name, path));
    }
  }
  found
}

/// Reinstalls the global packages of `from` (a version directory) into
/// `into`: registry packages with `npm install -g`, linked ones with
/// `npm link` in their directories, as nvm does.
///
/// # Errors
///
/// Exit code 1 when npm fails.
pub(crate) fn reinstall_packages(
  ctx: &Context,
  from: &Path,
  from_label: &str,
  into: &InstalledVersion,
  out: &mut dyn Write,
) -> Result<(), Failure> {
  let packages = global_packages(&global_modules_dir(from));
  let npm = Npm::of(ctx, &into.path);
  say(
    out,
    format_args!("Reinstalling global packages from {from_label}..."),
  );
  let _ = out.flush();
  if packages.installs.is_empty() {
    say(out, "No installed global packages found...");
  } else {
    npm.install_global(&packages.installs)?;
  }
  say(
    out,
    format_args!("Linking global packages from {from_label}..."),
  );
  let _ = out.flush();
  for link in &packages.links {
    npm.run(&["link"], Some(link))?;
  }
  Ok(())
}

/// The newest npm that works on `node`, with nvm's reason, following
/// nvm's `nvm_install_latest_npm` ladder (v0.40). `None` for Node.js 0.6
/// and 0.9, which cannot upgrade npm at all.
#[must_use]
pub(crate) fn latest_npm_for(node: &Version) -> Option<(&'static str, &'static str)> {
  let at_least = |major: u64, minor: u64| (node.major, node.minor) >= (major, minor);
  let below = |major: u64, minor: u64| !at_least(major, minor);
  let in_major = |major: u64| node.major == major;
  let cannot_upgrade = in_major(0) && matches!(node.minor, 6 | 9);
  if cannot_upgrade {
    return None;
  }
  Some(if below(1, 1) {
    (
      "4.5",
      "npm v4.5.x is the last version that works on node versions < v1.1.0",
    )
  } else if below(4, 0) {
    (
      "4",
      "npm v5 and higher do not work on node versions below v4.0.0",
    )
  } else if below(4, 5) || (in_major(5) && below(5, 10)) {
    (
      "5.3",
      "npm v5.3.x is the last version that works on node 4.x versions below v4.4, or 5.x \
       versions below v5.10, due to Buffer.alloc",
    )
  } else if below(4, 7) {
    (
      "5.4.1",
      "npm v5.4.1 is the last version that works on node v4.5 and v4.6",
    )
  } else if below(6, 0) {
    (
      "5",
      "npm v5.x is the last version that works on node below v6.0.0",
    )
  } else if (in_major(6) && below(6, 2)) || (in_major(9) && below(9, 3)) {
    (
      "6.9",
      "npm v6.9 is the last version that works on node v6.0.x, v6.1.x, v9.0.x, v9.1.x, or \
       v9.2.x",
    )
  } else if below(10, 0) {
    (
      "6",
      "npm v6.x is the last version that works on node below v10.0.0",
    )
  } else if below(12, 13) || in_major(13) || (in_major(14) && below(14, 15)) || in_major(15) {
    (
      "7",
      "npm v7.x is the last version that works on node v13, v15, below v12.13, or v14.0 - v14.15",
    )
  } else if in_major(12)
    || (in_major(14) && below(14, 17))
    || (in_major(16) && below(16, 13))
    || in_major(17)
  {
    (
      "8.6",
      "npm v8.6 is the last version that works on node v12, v14.13 - v14.16, or v16.0 - v16.12",
    )
  } else if below(18, 17) || in_major(19) || (in_major(20) && below(20, 5)) {
    (
      "9",
      "npm v9.x is the last version that works on node < v18.17, v19, or v20.0 - v20.4",
    )
  } else if below(20, 17) || in_major(21) || (in_major(22) && below(22, 9)) {
    (
      "10",
      "npm v10.x is the last version that works on node < v20.17, v21, or v22.0 - v22.8",
    )
  } else {
    ("latest", "Installing latest npm")
  })
}

/// Upgrades the npm of `installed` to the newest one that works with it.
///
/// # Errors
///
/// Exit code 1 when npm fails.
pub(crate) fn install_latest_npm(
  ctx: &Context,
  installed: &InstalledVersion,
  out: &mut dyn Write,
) -> Result<(), Failure> {
  say(
    out,
    "Attempting to upgrade to the latest working version of npm...",
  );
  let Some((spec, reason)) = latest_npm_for(&installed.version) else {
    say(out, "* node v0.6 and v0.9 are unable to upgrade further");
    return Ok(());
  };
  say(out, format_args!("* {reason}"));
  let _ = out.flush();
  Npm::of(ctx, &installed.path).install_global(&[format!("npm@{spec}")])?;
  if let Some(version) = super::npm_version(&installed.path) {
    say(out, format_args!("* npm upgraded to: v{version}"));
  }
  Ok(())
}

/// Where `--reinstall-packages-from` (or `reinstall-packages`) copies
/// from: an installed version's directory, or the system Node.js prefix.
pub(crate) struct PackageSource {
  pub dir: PathBuf,
  pub label: String,
}

/// Resolves the source version among installed versions; `system` means the
/// Node.js outside rnm.
///
/// # Errors
///
/// Exit code [`exit::SOURCE_NOT_INSTALLED`] when it is neither installed nor
/// `system`.
pub(crate) fn package_source(ctx: &Context, provided: &str) -> Result<PackageSource, Failure> {
  let not_installed = || {
    Failure::new(
      exit::SOURCE_NOT_INSTALLED,
      "If --reinstall-packages-from is provided, it must point to an installed version of node.",
    )
  };
  let spec: crate::version::VersionSpec = provided.parse().map_err(|_| not_installed())?;
  match ctx.nvm_dir.resolve(&spec) {
    | Ok(Resolved::Installed(installed)) => Ok(PackageSource {
      label: installed.to_string(),
      dir: installed.path,
    }),
    | Ok(Resolved::System) => {
      let node = ctx.system_node().ok_or_else(not_installed)?;
      let dir = version_dir_of(&node).ok_or_else(not_installed)?;
      Ok(PackageSource {
        dir: dir.to_path_buf(),
        label: SYSTEM_LABEL.to_owned(),
      })
    },
    | _ => Err(not_installed()),
  }
}

/// The version active in this shell, for commands that change it.
///
/// # Errors
///
/// Exit code 1 when no rnm-managed version is active.
fn active_version(ctx: &Context) -> Result<InstalledVersion, Failure> {
  let label = super::current::active(ctx)?;
  let resolved = label
    .parse()
    .ok()
    .and_then(|spec| ctx.nvm_dir.resolve(&spec).ok());
  match resolved {
    | Some(Resolved::Installed(installed)) => Ok(installed),
    | _ => Err(Failure::new(
      exit::ERROR,
      format!("No rnm-managed version of node is active ({label}); run `rnm use` first."),
    )),
  }
}

/// `rnm reinstall-packages <version>`: copies the global packages of
/// `<version>` into the active version.
///
/// # Errors
///
/// Exit code 2 when `<version>` is the active version, 5 when it is not
/// installed, and 1 when npm fails.
pub fn reinstall_packages_command(
  ctx: &Context,
  provided: &str,
  out: &mut dyn Write,
) -> Result<(), Failure> {
  let into = active_version(ctx)?;
  let source = package_source(ctx, provided)?;
  if source.dir == into.path {
    return Err(Failure::new(
      exit::SAME_AS_CURRENT,
      "Can not reinstall packages from the current version of node.",
    ));
  }
  reinstall_packages(ctx, &source.dir, &source.label, &into, out)
}

/// `rnm install-latest-npm`: upgrades the active version's npm.
///
/// # Errors
///
/// Exit code 1 when no version is active or npm fails.
pub fn install_latest_npm_command(ctx: &Context, out: &mut dyn Write) -> Result<(), Failure> {
  let installed = active_version(ctx)?;
  install_latest_npm(ctx, &installed, out)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn latest(node: &str) -> &'static str {
    latest_npm_for(&node.parse().unwrap()).map_or("none", |(spec, _)| spec)
  }

  #[test]
  fn follows_nvms_npm_ladder() {
    for (node, npm) in [
      ("0.6.21", "none"),
      ("0.9.12", "none"),
      ("0.10.48", "4.5"),
      ("1.0.4", "4.5"),
      ("3.3.1", "4"),
      ("4.4.7", "5.3"),
      ("4.6.2", "5.4.1"),
      ("4.9.1", "5"),
      ("5.9.1", "5.3"),
      ("5.12.0", "5"),
      ("6.1.0", "6.9"),
      ("6.17.1", "6"),
      ("9.2.1", "6.9"),
      ("9.11.2", "6"),
      ("10.24.1", "7"),
      ("12.12.0", "7"),
      ("12.22.12", "8.6"),
      ("13.14.0", "7"),
      ("14.14.0", "7"),
      ("14.16.1", "8.6"),
      ("14.21.3", "9"),
      ("15.14.0", "7"),
      ("16.12.0", "8.6"),
      ("16.20.2", "9"),
      ("17.9.1", "8.6"),
      ("18.16.1", "9"),
      ("18.20.4", "10"),
      ("19.9.0", "9"),
      ("20.4.0", "9"),
      ("20.16.0", "10"),
      ("20.17.0", "latest"),
      ("21.7.3", "10"),
      ("22.8.0", "10"),
      ("22.9.0", "latest"),
      ("24.0.0", "latest"),
    ] {
      assert_eq!(latest(node), npm, "node {node}");
    }
  }

  #[test]
  fn reads_default_packages_like_nvm() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join(DEFAULT_PACKAGES_FILE);
    assert_eq!(default_packages(&file).unwrap(), Vec::<String>::new());
    std::fs::write(
      &file,
      "# tools\ntypescript\r\n\n  # indented comment\n@scope/pkg@1\n",
    )
    .unwrap();
    assert_eq!(
      default_packages(&file).unwrap(),
      ["typescript", "@scope/pkg@1"]
    );
    std::fs::write(&file, "typescript # the compiler\n").unwrap();
    assert!(default_packages(&file).is_err());
    std::fs::write(&file, "  eslint\n").unwrap();
    assert!(default_packages(&file).is_err());
  }

  #[test]
  fn lists_global_packages() {
    let tmp = tempfile::tempdir().unwrap();
    let modules = tmp.path().join("node_modules");
    let package = |name: &str, version: &str| {
      let dir = modules.join(name);
      std::fs::create_dir_all(&dir).unwrap();
      std::fs::write(
        dir.join("package.json"),
        format!(r#"{{"name":"{name}","version":"{version}"}}"#),
      )
      .unwrap();
    };
    package("npm", "10.0.0");
    package("corepack", "0.29.0");
    package("typescript", "5.4.2");
    package("@scope/tool", "1.0.0");
    std::fs::create_dir_all(modules.join(".bin")).unwrap();
    let linked = tmp.path().join("my-cli");
    std::fs::create_dir_all(&linked).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&linked, modules.join("my-cli")).unwrap();

    let found = global_packages(&modules);
    assert_eq!(found.installs, ["@scope/tool@1.0.0", "typescript@5.4.2"]);
    #[cfg(unix)]
    assert_eq!(found.links, [std::fs::canonicalize(&linked).unwrap()]);
  }
}
