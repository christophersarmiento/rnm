//! Which Node.js binary fits this machine, in nvm's naming
//! (`docs/research/nvm.md` §6.2, `docs/research/ecosystem.md` §4.2).
//!
//! The operating system and CPU come from rnm's own build target: a 32-bit
//! rnm runs in a 32-bit userland, so it installs 32-bit Node.js, as nvm's
//! `getconf LONG_BIT` check does. Two facts are only known at run time: an
//! x64 rnm under Rosetta 2 installs arm64 Node.js, and Linux with musl libc
//! (Alpine) needs the `-musl` builds.

use std::fmt;
use std::path::Path;

use crate::version::Version;

/// The first Node.js release with darwin-arm64 binaries.
const FIRST_DARWIN_ARM64_MAJOR: u64 = 16;

/// Before Node.js 4, 32-bit ARM Linux binaries were named `arm-pi`.
const FIRST_GENERIC_ARM_MAJOR: u64 = 4;

/// The operating system in Node.js file names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
  Linux,
  Darwin,
  Windows,
  Aix,
  SunOs,
}

impl Os {
  /// The name in tarball names: `linux`, `darwin`, `win`, `aix`, `sunos`.
  #[must_use]
  pub fn dist_name(self) -> &'static str {
    match self {
      | Self::Linux => "linux",
      | Self::Darwin => "darwin",
      | Self::Windows => "win",
      | Self::Aix => "aix",
      | Self::SunOs => "sunos",
    }
  }
}

/// The machine to install Node.js for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Platform {
  pub os: Os,
  /// The CPU in Node.js file names: `x64`, `arm64`, `armv7l`, `ppc64le`, ….
  pub arch: &'static str,
  /// Linux with musl libc, which needs the `-musl` builds.
  pub musl: bool,
}

/// What to download for one version on one platform.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artifact {
  /// nvm's download slug, also the archive's base name and its directory in
  /// `.cache/bin/`: `node-v20.11.1-darwin-arm64`.
  pub slug: String,
  /// The ID in `index.json`'s `files` list that announces this binary
  /// (`osx-arm64-tar`), when one exists for this naming scheme.
  pub file_id: Option<String>,
}

/// Node.js publishes no binaries for this system.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsupportedPlatform {
  pub os: &'static str,
  pub arch: &'static str,
}

impl fmt::Display for UnsupportedPlatform {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(
      f,
      "Node.js publishes no prebuilt binaries for {}-{}",
      self.os, self.arch
    )
  }
}

impl std::error::Error for UnsupportedPlatform {}

impl Platform {
  /// Detects the platform this process runs on.
  ///
  /// # Errors
  ///
  /// Fails on systems without official Node.js binaries, such as FreeBSD.
  pub fn detect() -> Result<Self, UnsupportedPlatform> {
    let unsupported = || UnsupportedPlatform {
      os: std::env::consts::OS,
      arch: std::env::consts::ARCH,
    };
    let os = build_os().ok_or_else(unsupported)?;
    let mut arch = build_arch().ok_or_else(unsupported)?;
    if os == Os::Darwin && arch == "x64" && crate::sys::is_translated() {
      arch = "arm64";
    }
    let musl = os == Os::Linux && uses_musl(Path::new("/"));
    Ok(Self { os, arch, musl })
  }

  /// The binary to download for `version`, after nvm's fallbacks: darwin
  /// arm64 before Node.js 16 uses x64 (it runs under Rosetta 2), and 32-bit
  /// ARM Linux before Node.js 4 uses `arm-pi`.
  #[must_use]
  pub fn artifact(&self, version: &Version) -> Artifact {
    let predates_darwin_arm64 = version.major < FIRST_DARWIN_ARM64_MAJOR;
    let predates_generic_arm = version.major < FIRST_GENERIC_ARM_MAJOR;
    let is_32_bit_arm = matches!(self.arch, "armv6l" | "armv7l");
    let (arch, has_file_id) = match (self.os, self.arch) {
      | (Os::Darwin, "arm64") if predates_darwin_arm64 => ("x64", true),
      | (Os::Linux, _) if is_32_bit_arm && predates_generic_arm => ("arm-pi", false),
      | (_, arch) => (arch, true),
    };
    let musl_suffix = if self.musl { "-musl" } else { "" };
    let os = self.os.dist_name();
    let file_id = has_file_id.then(|| match self.os {
      | Os::Darwin => format!("osx-{arch}-tar"),
      | Os::Windows => format!("win-{arch}-zip"),
      | _ => format!("{os}-{arch}{musl_suffix}"),
    });
    Artifact {
      slug: format!("node-{version}-{os}-{arch}{musl_suffix}"),
      file_id,
    }
  }
}

fn build_os() -> Option<Os> {
  Some(match std::env::consts::OS {
    | "linux" | "android" => Os::Linux,
    | "macos" => Os::Darwin,
    | "windows" => Os::Windows,
    | "aix" => Os::Aix,
    | "solaris" | "illumos" => Os::SunOs,
    | _ => return None,
  })
}

fn build_arch() -> Option<&'static str> {
  Some(match std::env::consts::ARCH {
    | "x86_64" => "x64",
    | "x86" => "x86",
    | "aarch64" => "arm64",
    | "arm" if cfg!(target_feature = "v7") => "armv7l",
    | "arm" => "armv6l",
    | "powerpc64" if cfg!(target_endian = "little") => "ppc64le",
    | "powerpc64" => "ppc64",
    | "s390x" => "s390x",
    | "loongarch64" => "loong64",
    | "riscv64" => "riscv64",
    | _ => return None,
  })
}

/// Alpine marks itself with `/etc/alpine-release` (nvm's check); other musl
/// systems are recognised by musl's dynamic loader, `/lib/ld-musl-*`.
fn uses_musl(root: &Path) -> bool {
  let is_alpine = root.join("etc/alpine-release").exists();
  let has_musl_loader = std::fs::read_dir(root.join("lib")).is_ok_and(|entries| {
    entries
      .flatten()
      .any(|entry| entry.file_name().to_string_lossy().starts_with("ld-musl-"))
  });
  is_alpine || has_musl_loader
}

#[cfg(test)]
mod tests {
  use super::*;

  fn v(s: &str) -> Version {
    s.parse().unwrap()
  }

  fn platform(os: Os, arch: &'static str, musl: bool) -> Platform {
    Platform { os, arch, musl }
  }

  fn artifact(p: &Platform, version: &str) -> (String, Option<String>) {
    let a = p.artifact(&v(version));
    (a.slug, a.file_id)
  }

  #[test]
  fn names_like_nvm() {
    let linux = platform(Os::Linux, "x64", false);
    assert_eq!(
      artifact(&linux, "20.11.1"),
      (
        "node-v20.11.1-linux-x64".to_owned(),
        Some("linux-x64".to_owned())
      )
    );
    let alpine = platform(Os::Linux, "arm64", true);
    assert_eq!(
      artifact(&alpine, "22.0.0"),
      (
        "node-v22.0.0-linux-arm64-musl".to_owned(),
        Some("linux-arm64-musl".to_owned())
      )
    );
    let windows = platform(Os::Windows, "x64", false);
    assert_eq!(
      artifact(&windows, "20.11.1"),
      (
        "node-v20.11.1-win-x64".to_owned(),
        Some("win-x64-zip".to_owned())
      )
    );
  }

  #[test]
  fn darwin_arm64_falls_back_to_x64_before_16() {
    let mac = platform(Os::Darwin, "arm64", false);
    assert_eq!(
      artifact(&mac, "16.0.0"),
      (
        "node-v16.0.0-darwin-arm64".to_owned(),
        Some("osx-arm64-tar".to_owned())
      )
    );
    assert_eq!(
      artifact(&mac, "14.21.3"),
      (
        "node-v14.21.3-darwin-x64".to_owned(),
        Some("osx-x64-tar".to_owned())
      )
    );
  }

  #[test]
  fn old_32_bit_arm_uses_arm_pi() {
    let pi = platform(Os::Linux, "armv7l", false);
    assert_eq!(
      artifact(&pi, "0.12.18"),
      ("node-v0.12.18-linux-arm-pi".to_owned(), None)
    );
    assert_eq!(artifact(&pi, "4.0.0").0, "node-v4.0.0-linux-armv7l");
  }

  #[test]
  fn detects_musl() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(!uses_musl(tmp.path()));
    std::fs::create_dir_all(tmp.path().join("lib")).unwrap();
    std::fs::write(tmp.path().join("lib/ld-linux-x86-64.so.2"), "").unwrap();
    assert!(!uses_musl(tmp.path()));
    std::fs::write(tmp.path().join("lib/ld-musl-x86_64.so.1"), "").unwrap();
    assert!(uses_musl(tmp.path()));

    let alpine = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(alpine.path().join("etc")).unwrap();
    std::fs::write(alpine.path().join("etc/alpine-release"), "3.20.0\n").unwrap();
    assert!(uses_musl(alpine.path()));
  }

  #[test]
  fn detects_this_machine() {
    let platform = Platform::detect().unwrap();
    assert_ne!(platform.arch, "");
  }
}
