//! `rnm self-update`: replace this rnm with a newer release from GitHub.
//!
//! Only a copy installed by `install.sh` or `install.ps1` updates itself.
//! Those scripts write [`MARKER_FILE`] next to the binary, recording the
//! release channel. A copy installed any other way (cargo, a package
//! manager) belongs to that tool, so rnm names the right command instead of
//! overwriting files another program keeps track of.
//!
//! The update downloads the release archive and `SHA256SUMS` for this
//! platform, checks the digest, unpacks the new binary next to the old one,
//! runs it once, and renames it into place.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use super::{Context, Failure, exit, network, say};
use crate::http::{Client, HttpError, Url};
use crate::install::{ArchiveKind, find_checksum, sha256_of, unpack};
use crate::style::{self, Stream};
use crate::version::Version;

/// The GitHub repository that publishes releases.
const REPOSITORY: &str = "christophersarmiento/rnm";

/// Written by the install scripts next to the binary.
pub const MARKER_FILE: &str = "install.json";

/// The prerelease the nightly workflow replaces every day.
const NIGHTLY_TAG: &str = "nightly";

/// The nightly release's title is `Nightly <version>`.
const NIGHTLY_TITLE_PREFIX: &str = "Nightly ";

/// The checksum file in every release.
const CHECKSUMS_FILE: &str = "SHA256SUMS";

/// GitHub's API and web addresses, overridable for GitHub Enterprise
/// mirrors and for tests.
const API_URL_VAR: &str = "RNM_GITHUB_API_URL";
const WEB_URL_VAR: &str = "RNM_GITHUB_URL";
const DEFAULT_API_URL: &str = "https://api.github.com";
const DEFAULT_WEB_URL: &str = "https://github.com";

/// Release metadata is a few kilobytes; this is generous.
const MAX_METADATA_BYTES: u64 = 1024 * 1024;

#[cfg(windows)]
const BINARY_NAME: &str = "rnm.exe";
#[cfg(not(windows))]
const BINARY_NAME: &str = "rnm";

/// Which releases an installation follows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
  /// Tagged releases, `vX.Y.Z`.
  #[default]
  Stable,
  /// The daily build of the `dev` branch.
  Nightly,
}

impl Channel {
  fn name(self) -> &'static str {
    match self {
      | Self::Stable => "stable",
      | Self::Nightly => "nightly",
    }
  }
}

/// `install.json`: how this copy of rnm was installed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallMarker {
  /// The script that installed it: `install.sh` or `install.ps1`.
  pub source: String,
  #[serde(default)]
  pub channel: Channel,
}

/// Options for [`self_update`].
#[derive(Clone, Debug, Default)]
pub struct SelfUpdateOptions {
  /// `--stable` or `--nightly`: switch to that channel.
  pub channel: Option<Channel>,
  /// `--version <tag>`: install exactly this release.
  pub version: Option<String>,
  /// `--check`: only report whether an update exists.
  pub check: bool,
}

/// A release to update to.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Release {
  /// The git tag: `v0.2.0` or `nightly`.
  tag: String,
  /// What the release's `rnm --version` prints, without `rnm `.
  version: String,
}

/// Updates this rnm, or with `--check` only reports whether it can.
///
/// # Errors
///
/// Exit code 1 when rnm was not installed by an install script, when the
/// release cannot be found or downloaded, when the checksum does not match,
/// or when the binary cannot be replaced.
pub fn self_update(
  ctx: &Context,
  opts: &SelfUpdateOptions,
  out: &mut dyn Write,
  err: &mut dyn Write,
) -> Result<(), Failure> {
  let exe = current_exe()?;
  let install_dir = exe.parent().unwrap_or(Path::new(".")).to_path_buf();
  let marker = read_marker(&install_dir);
  let channel = opts
    .channel
    .or(marker.as_ref().map(|m| m.channel))
    .unwrap_or_default();

  let releases = Releases::new(ctx, network(ctx, err)?.client);
  let release = match &opts.version {
    | Some(tag) => releases.tagged(&normalize_tag(tag))?,
    | None => releases.newest(channel)?,
  };
  let switches_channel = marker.as_ref().is_some_and(|m| m.channel != channel);
  let wants_specific_release = opts.version.is_some() || switches_channel;
  let status = compare(crate::VERSION, &release.version);

  if opts.check {
    let how = match &marker {
      | Some(_) => "rnm self-update".to_owned(),
      | None => update_hint(&exe),
    };
    match status {
      | Status::Same => say(
        out,
        format_args!(
          "rnm {} is up to date ({} channel).",
          crate::VERSION,
          channel.name()
        ),
      ),
      | Status::Newer => say(
        out,
        format_args!(
          "rnm {} is newer than the newest {} release ({}).",
          crate::VERSION,
          channel.name(),
          release.version
        ),
      ),
      | Status::Older => say(
        out,
        format_args!(
          "rnm {} is available (installed: {}). Update with: {how}",
          release.version,
          crate::VERSION
        ),
      ),
    }
    return Ok(());
  }

  let Some(marker) = marker else {
    return Err(Failure::new(
      exit::ERROR,
      format!(
        "This rnm ({}) was not installed by rnm's install script, so it does not update \
         itself. Update it the way you installed it: {}",
        exe.display(),
        update_hint(&exe)
      ),
    ));
  };
  let up_to_date = match status {
    | Status::Same => true,
    | Status::Newer => !wants_specific_release,
    | Status::Older => false,
  };
  if up_to_date {
    if switches_channel {
      write_marker(&install_dir, &InstallMarker { channel, ..marker })?;
    }
    say(
      out,
      format_args!(
        "rnm {} is up to date ({} channel).",
        crate::VERSION,
        channel.name()
      ),
    );
    return Ok(());
  }

  say(
    out,
    format_args!(
      "Updating rnm {} to {} ({})...",
      crate::VERSION,
      release.version,
      release.tag
    ),
  );
  let installed = replace_binary(&releases, &release, &exe)?;
  let new_marker = InstallMarker { channel, ..marker };
  write_marker(&install_dir, &new_marker)?;
  say(
    out,
    style::success(Stream::Out, format_args!("Updated to {installed}.")),
  );
  Ok(())
}

/// How a release compares with the running version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
  Same,
  /// The running rnm is newer (a development or nightly build).
  Newer,
  Older,
}

fn compare(running: &str, release: &str) -> Status {
  if running == release {
    return Status::Same;
  }
  let parse = |text: &str| -> Option<Version> {
    text
      .split('+')
      .next()
      .unwrap_or(text)
      .trim_start_matches('v')
      .parse()
      .ok()
  };
  match (parse(running), parse(release)) {
    | (Some(running), Some(release)) if running > release => Status::Newer,
    | _ => Status::Older,
  }
}

/// `0.2.0` and `v0.2.0` both name the tag `v0.2.0`; `nightly` stays as it
/// is.
fn normalize_tag(tag: &str) -> String {
  let starts_with_digit = tag.bytes().next().is_some_and(|b| b.is_ascii_digit());
  if starts_with_digit {
    format!("v{tag}")
  } else {
    tag.to_owned()
  }
}

fn current_exe() -> Result<PathBuf, Failure> {
  std::env::current_exe()
    .and_then(std::fs::canonicalize)
    .map_err(|e| Failure::new(exit::ERROR, format!("cannot locate the rnm binary: {e}")))
}

/// Reads `install.json` next to the binary, if the install script wrote
/// one.
#[must_use]
pub fn read_marker(install_dir: &Path) -> Option<InstallMarker> {
  let text = std::fs::read_to_string(install_dir.join(MARKER_FILE)).ok()?;
  serde_json::from_str(&text).ok()
}

fn write_marker(install_dir: &Path, marker: &InstallMarker) -> Result<(), Failure> {
  let path = install_dir.join(MARKER_FILE);
  let text = serde_json::to_string(marker).unwrap_or_default();
  crate::fs::write_atomic(&path, &format!("{text}\n"))
    .map_err(|e| Failure::new(exit::ERROR, format!("{}: {e}", path.display())))
}

/// The command that updates a copy of rnm installed without the install
/// script, guessed from where the binary is.
#[must_use]
pub fn update_hint(exe: &Path) -> String {
  let path = exe.to_string_lossy().replace('\\', "/");
  if path.contains("/.cargo/bin/") {
    format!("cargo install --locked --git https://github.com/{REPOSITORY}")
  } else if path.contains("/Cellar/") || path.contains("/homebrew/") || path.contains("/linuxbrew/")
  {
    "brew upgrade rnm".to_owned()
  } else if path.contains("/scoop/") {
    "scoop update rnm".to_owned()
  } else if path.contains("/WinGet/") {
    "winget upgrade rnm".to_owned()
  } else {
    format!(
      "run the install script again (https://github.com/{REPOSITORY}/blob/main/docs/installation.md)"
    )
  }
}

/// The release archive name for the machine this runs on. An x64 rnm under
/// Rosetta 2 or Windows on Arm emulation updates to the native arm64 build.
fn archive_name() -> Result<(String, ArchiveKind), Failure> {
  let unsupported = || {
    Failure::new(
      exit::ERROR,
      format!(
        "rnm publishes no binary for {}-{}; build it from source",
        std::env::consts::OS,
        std::env::consts::ARCH
      ),
    )
  };
  let (os, kind) = match std::env::consts::OS {
    | "macos" => ("apple-darwin", ArchiveKind::TarGz),
    | "linux" => ("unknown-linux-musl", ArchiveKind::TarGz),
    | "windows" => ("pc-windows-msvc", ArchiveKind::Zip),
    | _ => return Err(unsupported()),
  };
  let arch = match std::env::consts::ARCH {
    | "x86_64" if os != "unknown-linux-musl" && crate::sys::is_translated() => "aarch64",
    | "x86_64" => "x86_64",
    | "aarch64" => "aarch64",
    | _ => return Err(unsupported()),
  };
  Ok((format!("rnm-{arch}-{os}.{}", kind.extension()), kind))
}

/// GitHub releases of rnm.
struct Releases {
  client: Client,
  api: String,
  web: String,
}

impl Releases {
  fn new(ctx: &Context, client: Client) -> Self {
    let base = |var: &str, default: &str| {
      ctx
        .var_string(var)
        .unwrap_or_else(|| default.to_owned())
        .trim_end_matches('/')
        .to_owned()
    };
    Self {
      client,
      api: base(API_URL_VAR, DEFAULT_API_URL),
      web: base(WEB_URL_VAR, DEFAULT_WEB_URL),
    }
  }

  fn newest(&self, channel: Channel) -> Result<Release, Failure> {
    match channel {
      | Channel::Stable => {
        let json = self.metadata("releases/latest")?;
        let tag = json_text(&json, "tag_name")?;
        Ok(Release {
          version: tag.trim_start_matches('v').to_owned(),
          tag,
        })
      },
      | Channel::Nightly => self.tagged(NIGHTLY_TAG),
    }
  }

  fn tagged(&self, tag: &str) -> Result<Release, Failure> {
    let json = self.metadata(&format!("releases/tags/{tag}"))?;
    let version = if tag == NIGHTLY_TAG {
      let title = json_text(&json, "name")?;
      title
        .strip_prefix(NIGHTLY_TITLE_PREFIX)
        .unwrap_or(&title)
        .to_owned()
    } else {
      tag.trim_start_matches('v').to_owned()
    };
    Ok(Release {
      tag: tag.to_owned(),
      version,
    })
  }

  fn metadata(&self, path: &str) -> Result<serde_json::Value, Failure> {
    let url = parse_url(&format!("{}/repos/{REPOSITORY}/{path}", self.api))?;
    let response = self.client.get(&url).map_err(|e| match e {
      | HttpError::NotFound { .. } => {
        let missing = match path {
          | "releases/latest" => "no rnm release has been published yet".to_owned(),
          | _ if path.ends_with(NIGHTLY_TAG) => {
            "no nightly build has been published yet".to_owned()
          },
          | _ => format!(
            "there is no rnm release {}",
            path.rsplit('/').next().unwrap_or(path)
          ),
        };
        Failure::new(
          exit::ERROR,
          format!("{missing}; see https://github.com/{REPOSITORY}/releases"),
        )
      },
      | e => Failure::new(exit::ERROR, e.to_string()),
    })?;
    let mut text = String::new();
    std::io::Read::read_to_string(
      &mut std::io::Read::take(response.body, MAX_METADATA_BYTES),
      &mut text,
    )
    .map_err(|e| Failure::new(exit::ERROR, format!("{url}: {e}")))?;
    serde_json::from_str(&text)
      .map_err(|e| Failure::new(exit::ERROR, format!("{url}: not a GitHub release: {e}")))
  }

  fn download(&self, tag: &str, file: &str, to: &Path) -> Result<(), Failure> {
    let url = parse_url(&format!(
      "{}/{REPOSITORY}/releases/download/{tag}/{file}",
      self.web
    ))?;
    let failure = |e: &dyn std::fmt::Display| Failure::new(exit::ERROR, e.to_string());
    let response = self.client.get(&url).map_err(|e| failure(&e))?;
    let mut file = std::fs::File::create(to).map_err(|e| failure(&e))?;
    let mut body = response.body;
    std::io::copy(&mut body, &mut file).map_err(|e| failure(&format!("{url}: {e}")))?;
    file.sync_all().map_err(|e| failure(&e))
  }
}

fn parse_url(text: &str) -> Result<Url, Failure> {
  Url::parse(text).map_err(|e| Failure::new(exit::ERROR, e.to_string()))
}

fn json_text(json: &serde_json::Value, field: &str) -> Result<String, Failure> {
  json[field]
    .as_str()
    .map(str::to_owned)
    .ok_or_else(|| Failure::new(exit::ERROR, format!("the release has no {field}")))
}

/// Downloads, verifies, and installs `release` over `exe`. Returns what the
/// new binary's `--version` printed.
fn replace_binary(releases: &Releases, release: &Release, exe: &Path) -> Result<String, Failure> {
  let failure = |e: &dyn std::fmt::Display| Failure::new(exit::ERROR, e.to_string());
  let (archive, kind) = archive_name()?;
  let install_dir = exe.parent().unwrap_or(Path::new("."));
  remove_previous_binary(exe);
  // Next to the binary, so the final rename stays on one file system.
  let work = install_dir.join(format!(".rnm-update-{}", std::process::id()));
  let _ = std::fs::remove_dir_all(&work);
  std::fs::create_dir(&work).map_err(|e| failure(&format!("{}: {e}", work.display())))?;
  let result = (|| {
    let archive_path = work.join(&archive);
    let sums_path = work.join(CHECKSUMS_FILE);
    releases.download(&release.tag, &archive, &archive_path)?;
    releases.download(&release.tag, CHECKSUMS_FILE, &sums_path)?;
    let sums = std::fs::read_to_string(&sums_path).map_err(|e| failure(&e))?;
    let expected = find_checksum(&sums, &archive).ok_or_else(|| {
      failure(&format!(
        "{CHECKSUMS_FILE} of {} lists no {archive}",
        release.tag
      ))
    })?;
    let actual = std::fs::File::open(&archive_path)
      .and_then(sha256_of)
      .map_err(|e| failure(&e))?;
    if actual != expected {
      return Err(failure(&format!(
        "checksum mismatch for {archive}: expected {expected}, got {actual}; the download is \
         corrupt or was tampered with"
      )));
    }
    let unpacked = work.join("unpacked");
    std::fs::create_dir(&unpacked).map_err(|e| failure(&e))?;
    unpack(kind, &archive_path, &unpacked).map_err(|e| failure(&format!("{archive}: {e}")))?;
    let new_binary = unpacked.join(BINARY_NAME);
    let printed = version_of(&new_binary)?;
    swap_in(&new_binary, exe).map_err(|e| failure(&format!("{}: {e}", exe.display())))?;
    Ok(printed)
  })();
  let _ = std::fs::remove_dir_all(&work);
  result
}

/// Runs the new binary once, so a broken download never replaces a working
/// rnm.
fn version_of(binary: &Path) -> Result<String, Failure> {
  let broken = |detail: String| {
    Failure::new(
      exit::ERROR,
      format!("the downloaded rnm does not run ({detail}); nothing was changed"),
    )
  };
  let output = Command::new(binary)
    .arg("--version")
    .output()
    .map_err(|e| broken(e.to_string()))?;
  let printed = String::from_utf8_lossy(&output.stdout).trim().to_owned();
  if output.status.success() && printed.starts_with("rnm ") {
    Ok(printed)
  } else {
    Err(broken(format!("--version printed '{printed}'")))
  }
}

/// The previous binary on Windows, renamed aside because a running `.exe`
/// cannot be deleted.
fn previous_binary(exe: &Path) -> PathBuf {
  exe.with_file_name(format!("{BINARY_NAME}.old"))
}

fn remove_previous_binary(exe: &Path) {
  let _ = std::fs::remove_file(previous_binary(exe));
}

/// Unix: a rename over the running binary is atomic, and the running process
/// keeps its old file. Windows: a running `.exe` can be renamed but not
/// replaced, so it moves aside first and is deleted by the next update.
fn swap_in(new_binary: &Path, exe: &Path) -> std::io::Result<()> {
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(new_binary, std::fs::Permissions::from_mode(0o755))?;
    std::fs::rename(new_binary, exe)
  }
  #[cfg(windows)]
  {
    let previous = previous_binary(exe);
    std::fs::rename(exe, &previous)?;
    std::fs::rename(new_binary, exe).inspect_err(|_| {
      let _ = std::fs::rename(&previous, exe);
    })
  }
}

/// For `rnm doctor`: the newest release on this installation's channel, or
/// why it could not be found.
pub(crate) fn update_finding(ctx: &Context) -> (bool, String) {
  let exe = current_exe().ok();
  let marker = exe.as_deref().and_then(Path::parent).and_then(read_marker);
  let channel = marker.as_ref().map(|m| m.channel).unwrap_or_default();
  let Ok(network) = network(ctx, &mut std::io::sink()) else {
    return (
      true,
      "could not check for updates: invalid network settings".to_owned(),
    );
  };
  let releases = Releases::new(ctx, network.client);
  match releases.newest(channel) {
    | Ok(release) if compare(crate::VERSION, &release.version) == Status::Older => {
      let how = match (&marker, &exe) {
        | (Some(_), _) => "rnm self-update".to_owned(),
        | (None, Some(exe)) => update_hint(exe),
        | (None, None) => "the install script".to_owned(),
      };
      (
        false,
        format!(
          "rnm {} is available (installed: {}); update with: {how}",
          release.version,
          crate::VERSION
        ),
      )
    },
    | Ok(_) => (
      true,
      format!(
        "rnm {} is up to date ({} channel)",
        crate::VERSION,
        channel.name()
      ),
    ),
    | Err(failure) => (
      true,
      format!("could not check for updates: {}", failure.message),
    ),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn compares_versions() {
    assert_eq!(compare("0.1.0", "0.1.0"), Status::Same);
    assert_eq!(compare("0.1.0", "0.2.0"), Status::Older);
    assert_eq!(compare("0.3.0", "0.2.0"), Status::Newer);
    assert_eq!(
      compare(
        "0.2.0-nightly.20261013+ec57833",
        "0.2.0-nightly.20261014+aa11bb2"
      ),
      Status::Older
    );
    assert_eq!(
      compare("0.2.0-nightly.20261013+ec57833", "0.1.0"),
      Status::Newer
    );
  }

  #[test]
  fn normalizes_tags() {
    assert_eq!(normalize_tag("0.2.0"), "v0.2.0");
    assert_eq!(normalize_tag("v0.2.0"), "v0.2.0");
    assert_eq!(normalize_tag("nightly"), "nightly");
  }

  #[test]
  fn reads_markers() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(read_marker(tmp.path()), None);
    std::fs::write(
      tmp.path().join(MARKER_FILE),
      r#"{"source":"install.sh","channel":"nightly"}"#,
    )
    .unwrap();
    assert_eq!(
      read_marker(tmp.path()),
      Some(InstallMarker {
        source: "install.sh".to_owned(),
        channel: Channel::Nightly
      })
    );
    std::fs::write(tmp.path().join(MARKER_FILE), r#"{"source":"install.ps1"}"#).unwrap();
    assert_eq!(read_marker(tmp.path()).unwrap().channel, Channel::Stable);
  }

  #[test]
  fn names_the_right_update_command() {
    assert!(update_hint(Path::new("/home/u/.cargo/bin/rnm")).starts_with("cargo install"));
    assert_eq!(
      update_hint(Path::new("/opt/homebrew/Cellar/rnm/0.1.0/bin/rnm")),
      "brew upgrade rnm"
    );
    assert!(update_hint(Path::new("/usr/local/bin/rnm")).contains("install script"));
  }
}
