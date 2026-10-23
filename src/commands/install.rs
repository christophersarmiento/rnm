//! `rnm install`: download and install a version, like `nvm install`.

use std::io::Write;
use std::time::{Duration, Instant};

use super::packages::{
  install_default_packages, install_latest_npm, package_source, reinstall_packages,
};
use super::use_version::version_file;
use super::{
  Context, DEFAULT_ALIAS, Failure, Remote, UseOptions, exit, remote_index, say, use_version,
};
use crate::install::{Event, Installer, LockSettings, cached_versions, install_from_cache};
use crate::nvm_dir::{Flavor, InstalledVersion, Resolved};
use crate::platform::Platform;
use crate::remote::{Release, RemoteResolveError};
use crate::style::{self, Spinner, Stream};
use crate::version::{LTS_PREFIX, VersionFileKind, VersionReq, VersionSpec, looks_like_range};

/// The width of the download progress bar, in cells.
const PROGRESS_BAR_WIDTH: u64 = 30;

/// Bytes in a MiB, for sizes and rates.
const MIB: u64 = 1024 * 1024;

/// Progress is redrawn in steps of 0.1 %.
const PER_MILLE: u64 = 1000;

/// Options for [`install`].
#[derive(Clone, Debug, Default)]
#[allow(
  clippy::struct_excessive_bools,
  reason = "each bool is one command-line flag"
)]
pub struct InstallOptions {
  /// The version spec. When `None` (and no `--lts`), the nearest version
  /// file is used.
  pub version: Option<String>,
  /// `--lts[=name]`.
  pub lts: Option<String>,
  /// `--default`: point the `default` alias at the requested spec.
  pub default: bool,
  /// `--alias=<name>`: point this alias at the requested spec.
  pub alias: Option<String>,
  /// `--save`: write the installed version to `./.nvmrc`.
  pub save: bool,
  /// `--no-use`: do not switch this shell to the installed version.
  pub no_use: bool,
  /// Whether to draw a download progress bar. `NVM_NO_PROGRESS` turns it
  /// off as well.
  pub progress: Progress,
  /// `--offline`: choose among installed and cached versions, and install
  /// from the download cache without network access.
  pub offline: bool,
  /// `--latest-npm`: upgrade npm to the newest one that works with this
  /// version.
  pub latest_npm: bool,
  /// `--skip-default-packages`: do not install `<root>/default-packages`.
  pub skip_default_packages: bool,
  /// `--reinstall-packages-from=<version>`: copy that installed version's
  /// global packages.
  pub reinstall_packages_from: Option<String>,
}

/// Download progress output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Progress {
  /// A bar that redraws in place; only for terminals.
  Bar,
  /// Status lines only (`--no-progress`, or not a terminal).
  #[default]
  Hidden,
}

/// Installs a version from the mirror, then (like nvm) switches this shell
/// to it, creates the `default` alias if there is none, and applies
/// `--default`, `--alias`, and `--save`.
///
/// An exact version that is already installed is used without contacting
/// the mirror.
///
/// # Errors
///
/// Exit code 3 when the version does not exist on the mirror, 127 when no
/// version is given or found, and 1 for download, verification, or file
/// system failures.
pub fn install(
  ctx: &Context,
  opts: &InstallOptions,
  out: &mut dyn Write,
  err: &mut dyn Write,
) -> Result<(), Failure> {
  let requested = requested_spec(ctx, opts, out)?;
  let spec: VersionSpec = requested.parse().map_err(|_| not_found(&requested))?;
  let package_source = opts
    .reinstall_packages_from
    .as_deref()
    .map(|provided| package_source(ctx, provided))
    .transpose()?;

  let installed = match installed_exact_version(ctx, &spec) {
    | Some(installed) => {
      say(err, already_installed(&installed));
      installed
    },
    | None if opts.offline => install_offline(ctx, opts, &requested, &spec, out, err)?,
    | None => {
      let remote = remote_index(ctx, err)?;
      let release = remote
        .index
        .resolve(&spec)
        .map_err(|e| remote_failure(&requested, &e))?;
      let version_dir = ctx.nvm_dir.version_dir(Flavor::Node, &release.version);
      if let Some(installed) = ctx.nvm_dir.installed_at(&version_dir) {
        say(err, already_installed(&installed));
        installed
      } else {
        say(
          out,
          format_args!(
            "{} node {}...",
            style::paint(Stream::Out, style::BOLD, "Downloading and installing"),
            style::paint(Stream::Out, style::BOLD_CYAN, &release.version)
          ),
        );
        download(ctx, opts, &remote, release, err)?
      }
    },
  };
  if package_source
    .as_ref()
    .is_some_and(|source| source.dir == installed.path)
  {
    return Err(Failure::new(
      exit::SAME_AS_INSTALLING,
      "You can't reinstall global packages from the same version of node you're installing.",
    ));
  }
  finish(ctx, opts, &requested, &installed, out, err)?;
  if opts.latest_npm {
    install_latest_npm(ctx, &installed, out)?;
  }
  if !opts.skip_default_packages {
    install_default_packages(ctx, &installed, out)?;
  }
  if let Some(source) = package_source {
    reinstall_packages(ctx, &source.dir, &source.label, &installed, out)?;
  }
  Ok(())
}

/// The spec as the user gave it: `--lts`, the argument, or the version file.
/// nvm stores this text, not the resolved version, in new aliases.
fn requested_spec(
  ctx: &Context,
  opts: &InstallOptions,
  out: &mut dyn Write,
) -> Result<String, Failure> {
  if let Some(lts) = &opts.lts {
    return Ok(format!("{LTS_PREFIX}{lts}"));
  }
  if let Some(version) = &opts.version {
    return Ok(version.clone());
  }
  let file = version_file(ctx)?;
  say(
    out,
    format_args!(
      "Found '{}' with version <{}>",
      file.path.display(),
      file.version
    ),
  );
  Ok(file.version)
}

/// nvm's `--offline`: the newest installed or cached version that matches,
/// installed from the cache if needed. Aliases such as `lts/*` resolve
/// through their alias files, as they do for `use`.
fn install_offline(
  ctx: &Context,
  opts: &InstallOptions,
  requested: &str,
  spec: &VersionSpec,
  out: &mut dyn Write,
  err: &mut dyn Write,
) -> Result<InstalledVersion, Failure> {
  let failure = |e: &dyn std::fmt::Display| Failure::new(exit::ERROR, e.to_string());
  let platform = Platform::detect().map_err(|e| failure(&e))?;
  let mut candidates = ctx.nvm_dir.installed_versions()?;
  for version in cached_versions(&ctx.nvm_dir, &platform) {
    let already_listed = candidates
      .iter()
      .any(|c| c.flavor == Flavor::Node && c.version == version);
    if !already_listed {
      candidates.push(InstalledVersion {
        flavor: Flavor::Node,
        path: ctx.nvm_dir.version_dir(Flavor::Node, &version),
        version,
      });
    }
  }
  candidates.sort_by(|a, b| a.version.cmp(&b.version));
  let not_available = || {
    Failure::new(
      exit::NOT_AVAILABLE,
      format!(
        "Version '{requested}' is neither installed nor in the download cache; \
         run without --offline to download it."
      ),
    )
  };
  let Ok(Resolved::Installed(chosen)) = ctx.nvm_dir.resolve_among(spec, &candidates) else {
    return Err(not_available());
  };
  if let Some(installed) = ctx.nvm_dir.installed_at(&chosen.path) {
    say(err, already_installed(&installed));
    return Ok(installed);
  }
  say(
    out,
    format_args!(
      "Installing node {} from the download cache...",
      chosen.version
    ),
  );
  let mut reporter = Reporter::new(err, chosen.version.to_string(), animates(ctx, opts));
  install_from_cache(
    &ctx.nvm_dir,
    &platform,
    &LockSettings::from_env(|key| ctx.var_string(key)),
    &chosen.version,
    &mut |event| reporter.report(&event),
  )
  .map_err(|e| failure(&e))
}

fn installed_exact_version(ctx: &Context, spec: &VersionSpec) -> Option<InstalledVersion> {
  let VersionSpec::Version(VersionReq::Exact(version)) = spec else {
    return None;
  };
  ctx
    .nvm_dir
    .installed_at(&ctx.nvm_dir.version_dir(Flavor::Node, version))
}

fn download(
  ctx: &Context,
  opts: &InstallOptions,
  remote: &Remote,
  release: &Release,
  err: &mut dyn Write,
) -> Result<InstalledVersion, Failure> {
  let failure = |e: &dyn std::fmt::Display| Failure::new(exit::ERROR, e.to_string());
  let platform = Platform::detect().map_err(|e| failure(&e))?;
  let installer = Installer {
    nvm_dir: &ctx.nvm_dir,
    mirrors: remote.download_mirrors(),
    client: &remote.network.client,
    platform: &platform,
    lock: LockSettings::from_env(|key| ctx.var_string(key)),
  };
  let mut reporter = Reporter::new(err, release.version.to_string(), animates(ctx, opts));
  installer
    .install(release, &mut |event| reporter.report(&event))
    .map_err(|e| failure(&e))
}

fn already_installed(installed: &InstalledVersion) -> String {
  style::success(
    Stream::Err,
    format_args!(
      "{} is already installed.",
      style::paint(Stream::Err, style::BOLD_CYAN, installed)
    ),
  )
}

/// Whether to draw the progress bar and spinners. `NVM_NO_PROGRESS` turns
/// them off as well.
fn animates(ctx: &Context, opts: &InstallOptions) -> bool {
  opts.progress == Progress::Bar && ctx.var("NVM_NO_PROGRESS").is_none()
}

/// Prints install events to stderr, as nvm does.
struct Reporter<'a> {
  err: &'a mut dyn Write,
  version: String,
  draws_progress: bool,
  /// The progress last drawn, so the bar is redrawn only when it changes
  /// (or, for the shimmer, when a frame has passed).
  drawn_per_mille: Option<u64>,
  drawn_at: Option<Instant>,
  /// When the current download started, for its rate.
  started: Option<Instant>,
  /// Runs while the archive is unpacked; stops when the reporter is dropped.
  spinner: Option<Spinner>,
}

impl<'a> Reporter<'a> {
  fn new(err: &'a mut dyn Write, version: String, draws_progress: bool) -> Self {
    Self {
      err,
      version,
      draws_progress,
      drawn_per_mille: None,
      drawn_at: None,
      started: None,
      spinner: None,
    }
  }

  fn report(&mut self, event: &Event<'_>) {
    let dim = |text: &dyn std::fmt::Display| style::paint(Stream::Err, style::DIM, text);
    match event {
      | Event::WaitingForLock => say(
        self.err,
        format_args!(
          "Waiting for another install of {} to finish...",
          self.version
        ),
      ),
      | Event::Downloading { url, .. } => {
        say(self.err, format_args!("Downloading {}...", dim(url)));
        self.drawn_per_mille = None;
        self.started = Some(Instant::now());
      },
      | Event::Progress {
        received,
        size: Some(size),
      } if self.draws_progress && *size > 0 => self.draw(*received, *size),
      | Event::UsingCached { path } => say(
        self.err,
        format_args!(
          "Using the verified archive in the cache: {}",
          dim(&path.display())
        ),
      ),
      | Event::ChecksumMatched => say(self.err, style::success(Stream::Err, "Checksums matched!")),
      | Event::TryingNextMirror { error } => {
        say(
          self.err,
          style::warning(format_args!("warning: {error}; trying the next mirror")),
        );
      },
      | Event::UsingCachedOffline { path } => say(
        self.err,
        format_args!(
          "Using the archive in the cache (verified when it was downloaded): {}",
          dim(&path.display())
        ),
      ),
      | Event::Unpacking if self.draws_progress => {
        self.spinner = Some(Spinner::start(format!(
          "Unpacking node {}...",
          self.version
        )));
      },
      | Event::Progress { .. } | Event::Unpacking => {},
    }
  }

  /// Redraws the bar in place, ending the line at 100 %.
  fn draw(&mut self, received: u64, size: u64) {
    let per_mille = received.min(size) * PER_MILLE / size;
    let shimmer_due = style::truecolor(Stream::Err)
      && self
        .drawn_at
        .is_some_and(|drawn| drawn.elapsed() >= SHIMMER_FRAME);
    if self.drawn_per_mille == Some(per_mille) && !shimmer_due {
      return;
    }
    self.drawn_per_mille = Some(per_mille);
    self.drawn_at = Some(Instant::now());
    let elapsed = self.started.map(|started| started.elapsed());
    let line = bar_line(received.min(size), size, elapsed);
    let _ = write!(self.err, "\r{line}\x1b[K");
    if per_mille == PER_MILLE {
      let _ = writeln!(self.err);
    }
    let _ = self.err.flush();
  }
}

/// Node.js green, the fill of the 24-bit color bar.
const NODE_GREEN: (u8, u8, u8) = (95, 160, 78);

/// The center of the shimmer that sweeps across the fill.
const SHIMMER: (u8, u8, u8) = (190, 235, 150);

/// The shimmer's half-width, in cells.
const SHIMMER_CELLS: i64 = 5;

/// How fast the shimmer moves, in cells per second.
const SHIMMER_SPEED: u128 = 30;

/// The shimmer redraws at least this often while bytes arrive.
// shortcut: the shimmer pauses while the download stalls, since only
// incoming bytes trigger redraws; add a ticker thread if that looks broken.
const SHIMMER_FRAME: Duration = Duration::from_millis(40);

/// The background of the unfilled part of the 24-bit color bar.
const TRACK: (u8, u8, u8) = (50, 52, 60);

/// Partial cells, in eighths: `EIGHTHS[n]` fills `n`/8 of a cell.
const EIGHTHS: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];

/// The bar, `42.0%  12.3/29.1 MiB  5.2 MiB/s  4s left`; `received` is at
/// most `size`, which is not zero.
fn bar_line(received: u64, size: u64, elapsed: Option<Duration>) -> String {
  let per_mille = received * PER_MILLE / size;
  let eighths = received * PROGRESS_BAR_WIDTH * 8 / size;
  let bar = if style::truecolor(Stream::Err) {
    let millis = elapsed
      .filter(|_| per_mille < PER_MILLE)
      .map(|e| e.as_millis());
    shimmer_bar(eighths, millis)
  } else {
    smooth_bar(eighths, per_mille == PER_MILLE)
  };
  let percent = format!("{:>3}.{}%", per_mille / 10, per_mille % 10);
  let mut line = format!(
    "  {bar} {}  {}/{} MiB",
    style::paint(Stream::Err, style::BOLD, percent),
    mib(received),
    mib(size),
  );
  let millis = elapsed.map_or(0, |e| e.as_millis());
  if let Some(per_second) = (u128::from(received) * 1000).checked_div(millis) {
    let per_second = u64::try_from(per_second).unwrap_or(u64::MAX);
    line.push_str(&style::paint(
      Stream::Err,
      style::DIM,
      format_args!("  {} MiB/s", mib(per_second)),
    ));
    if received < size && per_second > 0 {
      let left = (size - received).div_ceil(per_second);
      line.push_str(&style::paint(
        Stream::Err,
        style::DIM,
        format_args!("  {} left", duration(left)),
      ));
    }
  }
  line
}

/// `▕████▋     ▏`: block characters, filled to the eighth of a cell.
fn smooth_bar(eighths: u64, done: bool) -> String {
  let (full, part) = split_eighths(eighths);
  let width = usize::try_from(PROGRESS_BAR_WIDTH).unwrap_or(0);
  let mut body = "█".repeat(full);
  if part > 0 {
    body.push(EIGHTHS[part]);
  }
  let pad = width.saturating_sub(full + usize::from(part > 0));
  let color = if done { style::GREEN } else { style::CYAN };
  format!(
    "{}{}{}{}",
    style::paint(Stream::Err, style::DIM, "▕"),
    style::paint(Stream::Err, color, body),
    " ".repeat(pad),
    style::paint(Stream::Err, style::DIM, "▏"),
  )
}

/// Node.js green cells on a dark track, with a highlight sweeping across
/// them `millis` into the download (none when `None`); the partial cell is
/// an eighth block. Needs 24-bit color.
fn shimmer_bar(eighths: u64, millis: Option<u128>) -> String {
  use std::fmt::Write as _;
  let (full, part) = split_eighths(eighths);
  let width = usize::try_from(PROGRESS_BAR_WIDTH).unwrap_or(0);
  let (tr, tg, tb) = TRACK;
  let mut bar = String::new();
  for cell in 0..width {
    let (r, g, b) = millis.map_or(NODE_GREEN, |millis| shimmer_color(cell, millis));
    if cell < full {
      let _ = write!(bar, "\x1b[48;2;{r};{g};{b}m ");
    } else if cell == full && part > 0 {
      let _ = write!(
        bar,
        "\x1b[38;2;{r};{g};{b};48;2;{tr};{tg};{tb}m{}",
        EIGHTHS[part]
      );
    } else {
      let _ = write!(bar, "\x1b[48;2;{tr};{tg};{tb}m ");
    }
  }
  bar.push_str("\x1b[0m");
  bar
}

/// Whole cells and the eighths of the next one.
fn split_eighths(eighths: u64) -> (usize, usize) {
  let full = usize::try_from(eighths / 8).unwrap_or(0);
  let part = usize::try_from(eighths % 8).unwrap_or(0);
  (full, part)
}

/// The color of `cell` `millis` into the sweep: Node.js green, brightening
/// toward [`SHIMMER`] near the highlight's center. The highlight enters at
/// the left, leaves at the right, and starts over.
fn shimmer_color(cell: usize, millis: u128) -> (u8, u8, u8) {
  // Positions are in thousandths of a cell.
  let width = i64::try_from(PROGRESS_BAR_WIDTH).unwrap_or(0);
  let period = u128::try_from((width + 2 * SHIMMER_CELLS) * 1000).unwrap_or(1);
  let travelled = i64::try_from(millis * SHIMMER_SPEED % period).unwrap_or(0);
  let center = travelled - SHIMMER_CELLS * 1000;
  let at = i64::try_from(cell).unwrap_or(0) * 1000 + 500;
  let distance = (at - center).abs();
  if distance >= SHIMMER_CELLS * 1000 {
    return NODE_GREEN;
  }
  let k = 1000 - distance / SHIMMER_CELLS;
  let k = k * k / 1000; // eases the edges in
  let mix = |x: u8, y: u8| {
    let value = i64::from(x) + (i64::from(y) - i64::from(x)) * k / 1000;
    u8::try_from(value).unwrap_or(u8::MAX)
  };
  let (a, b) = (NODE_GREEN, SHIMMER);
  (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

/// `42s` or `1m 05s`.
fn duration(seconds: u64) -> String {
  if seconds < 60 {
    format!("{seconds}s")
  } else {
    format!("{}m {:02}s", seconds / 60, seconds % 60)
  }
}

/// Bytes as MiB with one decimal: `12.3`.
fn mib(bytes: u64) -> String {
  let tenths = bytes * 10 / MIB;
  format!("{}.{}", tenths / 10, tenths % 10)
}

/// The steps after the version is on disk.
fn finish(
  ctx: &Context,
  opts: &InstallOptions,
  requested: &str,
  installed: &InstalledVersion,
  out: &mut dyn Write,
  err: &mut dyn Write,
) -> Result<(), Failure> {
  if ctx.session.is_some() && !opts.no_use {
    let use_opts = UseOptions {
      version: Some(installed.to_string()),
      ..UseOptions::default()
    };
    use_version(ctx, &use_opts, out, err)?;
  }

  // nvm stores the spec as given, but nvm cannot read an npm range, so a
  // range from package.json is stored as the version it resolved to.
  let installed_text = installed.to_string();
  let requested = if looks_like_range(requested) {
    installed_text.as_str()
  } else {
    requested
  };
  let aliases = ctx.nvm_dir.aliases();
  let has_default = aliases.get(DEFAULT_ALIAS)?.is_some();
  if !has_default {
    aliases.set(DEFAULT_ALIAS, requested)?;
    say(
      out,
      format_args!(
        "Creating default alias: {}",
        alias_line(DEFAULT_ALIAS, requested, installed)
      ),
    );
  }
  let requested_alias = if opts.default {
    Some(DEFAULT_ALIAS)
  } else {
    opts.alias.as_deref()
  };
  if let Some(name) = requested_alias
    && (has_default || name != DEFAULT_ALIAS)
  {
    aliases.set(name, requested)?;
    say(out, alias_line(name, requested, installed));
  }

  if opts.save {
    std::fs::write(
      ctx.cwd.join(VersionFileKind::Nvmrc.file_name()),
      format!("{installed}\n"),
    )?;
  }
  super::default_version::refresh_persistent_default(ctx, err);
  Ok(())
}

/// nvm's alias display: `default -> 20 (-> v20.11.1)`, or
/// `default -> v20.11.1` when the target is the version itself.
fn alias_line(name: &str, target: &str, installed: &InstalledVersion) -> String {
  let version = installed.to_string();
  if target == version {
    format!("{name} -> {target}")
  } else {
    format!("{name} -> {target} (-> {version})")
  }
}

fn not_found(requested: &str) -> Failure {
  Failure::new(
    exit::NOT_AVAILABLE,
    format!("Version '{requested}' not found - try `rnm ls-remote` to browse available versions."),
  )
}

fn remote_failure(requested: &str, e: &RemoteResolveError) -> Failure {
  match e {
    | RemoteResolveError::NotFound(_) => not_found(requested),
    | _ => Failure::new(exit::NOT_AVAILABLE, e.to_string()),
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::path::PathBuf;

  fn installed(version: &str) -> InstalledVersion {
    InstalledVersion {
      flavor: Flavor::Node,
      version: version.parse().unwrap(),
      path: PathBuf::from("/x"),
    }
  }

  #[test]
  fn formats_aliases_like_nvm() {
    let v = installed("20.11.1");
    assert_eq!(
      alias_line("default", "20", &v),
      "default -> 20 (-> v20.11.1)"
    );
    assert_eq!(alias_line("default", "v20.11.1", &v), "default -> v20.11.1");
  }

  #[test]
  fn draws_progress_bar() {
    let mut err = Vec::new();
    let mut reporter = Reporter::new(&mut err, "v20.0.0".to_owned(), true);
    reporter.draw(1, 2);
    reporter.draw(1, 2);
    reporter.draw(2, 2);
    let text = String::from_utf8(err).unwrap();
    let frames: Vec<&str> = text.split('\r').filter(|s| !s.is_empty()).collect();
    assert_eq!(frames.len(), 2);
    assert_eq!(
      frames[0],
      format!(
        "  ▕{}{}▏  50.0%  0.0/0.0 MiB\x1b[K",
        "█".repeat(15),
        " ".repeat(15)
      )
    );
    assert_eq!(
      frames[1],
      format!("  ▕{}▏ 100.0%  0.0/0.0 MiB\x1b[K\n", "█".repeat(30))
    );
  }

  #[test]
  fn fills_to_the_eighth() {
    // 3 cells and 5/8 of the next.
    let bar = smooth_bar(29, false);
    assert_eq!(bar, format!("▕███▋{}▏", " ".repeat(26)));
  }

  #[test]
  fn shows_size_rate_and_time_left() {
    let line = bar_line(3 * MIB, 6 * MIB, Some(Duration::from_secs(2)));
    assert!(
      line.ends_with(" 50.0%  3.0/6.0 MiB  1.5 MiB/s  2s left"),
      "{line}"
    );
    let line = bar_line(6 * MIB, 6 * MIB, Some(Duration::from_secs(2)));
    assert!(line.ends_with("  3.0 MiB/s"), "{line}");
    assert_eq!(duration(65), "1m 05s");
  }

  #[test]
  fn shimmer_sweeps_across_the_fill() {
    // At 0 ms the highlight sits just left of the bar.
    assert_eq!(shimmer_color(0, 0), NODE_GREEN);
    // 1/6 s later, at 30 cells/s, its center is on cell 0.
    assert!(shimmer_color(0, 1000 / 6 + 1) > NODE_GREEN);
    assert_eq!(shimmer_color(20, 1000 / 6 + 1), NODE_GREEN);
    let bar = shimmer_bar(29, None);
    let width = usize::try_from(PROGRESS_BAR_WIDTH).unwrap();
    assert_eq!(bar.matches(' ').count(), width - 1);
    assert!(bar.contains('▋'));
  }
}
