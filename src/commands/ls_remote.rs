//! `rnm ls-remote`: list the versions available on the mirror, like
//! `nvm ls-remote` (nvm v0.40 output format).

use std::collections::HashSet;
use std::io::Write;

use super::colors::{Palette, bold, paint};
use super::current::active;
use super::{Context, Failure, exit, remote_index, say};
use crate::nvm_dir::Flavor;
use crate::remote::{Release, RemoteIndex};
use crate::version::{LTS_PREFIX, LtsSpec, SpecError, VersionReq, VersionSpec};

/// What nvm prints when nothing matches.
const NOT_AVAILABLE_LABEL: &str = "N/A";

/// The width nvm pads version numbers to.
const VERSION_WIDTH: usize = 15;

/// The width after the `->` current-version marker.
const CURRENT_VERSION_WIDTH: usize = VERSION_WIDTH - "->".len();

/// The `TERM` value of terminals without color support.
const DUMB_TERMINAL: &str = "dumb";

/// Options for [`ls_remote`].
#[derive(Clone, Debug, Default)]
pub struct LsRemoteOptions {
  /// A version prefix (`18`, `v20.1`), `node`, or `lts/...`.
  pub pattern: Option<String>,
  /// `--lts[=name]`: only LTS releases, of one line when named.
  pub lts: Option<String>,
  /// `--no-colors`.
  pub no_colors: bool,
  /// Whether `out` is a terminal; colors are only used there.
  pub terminal: bool,
}

/// Which releases to list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Filter {
  version: Option<VersionReq>,
  lts: Option<LtsSpec>,
  /// The pattern can never match a release, such as `system` or an alias.
  matches_nothing: bool,
}

/// How a version relates to this machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
  Current,
  Installed,
  NotInstalled,
}

/// One output line.
struct Row<'a> {
  release: &'a Release,
  state: State,
  /// The newest release of its LTS line.
  is_latest_lts: bool,
}

/// Fetches the index, refreshes nvm's LTS aliases, and prints matching
/// versions oldest first, marking installed ones and the active one.
///
/// # Errors
///
/// Exit code 3 when nothing matches (after printing `N/A`) or the LTS name
/// is invalid; 1 when the mirror settings are invalid or the index cannot
/// be loaded.
pub fn ls_remote(
  ctx: &Context,
  opts: &LsRemoteOptions,
  out: &mut dyn Write,
  err: &mut dyn Write,
) -> Result<(), Failure> {
  let not_available = |out: &mut dyn Write, message: String| {
    say(out, format_args!("{NOT_AVAILABLE_LABEL:>VERSION_WIDTH$}"));
    Failure::new(exit::NOT_AVAILABLE, message)
  };
  let filter = Filter::new(opts).map_err(|e| not_available(out, e.to_string()))?;
  let index = remote_index(ctx, err)?;
  let rows = filter
    .select(&index)
    .map_err(|message| not_available(out, message))?;
  if rows.is_empty() {
    return Err(not_available(out, String::new()));
  }

  let installed: HashSet<String> = ctx
    .nvm_dir
    .installed_versions()?
    .into_iter()
    .filter(|i| i.flavor == Flavor::Node)
    .map(|i| i.version.to_string())
    .collect();
  let current = active(ctx)?;
  let palette = use_colors(ctx, opts)
    .then(|| Palette::from_nvm_colors(ctx.var_string("NVM_COLORS").as_deref()));
  let latest_lts = index.lts_lines();

  for release in rows {
    let version = release.version.to_string();
    let state = if version == current {
      State::Current
    } else if installed.contains(&version) {
      State::Installed
    } else {
      State::NotInstalled
    };
    let is_latest_lts = latest_lts
      .iter()
      .any(|(_, newest)| newest.version == release.version);
    let row = Row {
      release,
      state,
      is_latest_lts,
    };
    say(out, format_row(&row, palette.as_ref()));
  }
  Ok(())
}

impl Filter {
  /// Combines the pattern and `--lts`. Like nvm, `--lts` wins over an
  /// `lts/...` pattern.
  fn new(opts: &LsRemoteOptions) -> Result<Self, SpecError> {
    let mut filter = Self::default();
    if let Some(pattern) = &opts.pattern {
      match pattern.parse::<VersionSpec>() {
        | Ok(VersionSpec::Version(req)) => filter.version = Some(req),
        | Ok(VersionSpec::Node) => {},
        | Ok(VersionSpec::Lts(lts)) => filter.lts = Some(lts),
        | Err(e @ (SpecError::LtsUppercase(_) | SpecError::InvalidLts(_))) => return Err(e),
        | Ok(_) | Err(_) => filter.matches_nothing = true,
      }
    }
    if let Some(name) = &opts.lts {
      match format!("{LTS_PREFIX}{name}").parse::<VersionSpec>() {
        | Ok(VersionSpec::Lts(lts)) => filter.lts = Some(lts),
        | Ok(_) => return Err(SpecError::InvalidLts(name.clone())),
        | Err(e) => return Err(e),
      }
    }
    Ok(filter)
  }

  /// The matching releases, oldest first. Fails with a message when
  /// `lts/-N` goes past the oldest LTS line.
  fn select<'a>(&self, index: &'a RemoteIndex) -> Result<Vec<&'a Release>, String> {
    if self.matches_nothing {
      return Ok(Vec::new());
    }
    let line = match &self.lts {
      | Some(LtsSpec::Offset(n)) => {
        let lines = index.lts_lines();
        let nth = usize::try_from(*n).ok().and_then(|n| lines.get(n));
        match nth {
          | Some((name, _)) => Some(LtsSpec::Named(name.as_str().into())),
          | None => return Err("That many LTS releases do not exist yet.".to_owned()),
        }
      },
      | other => other.clone(),
    };
    let matches_lts = |release: &Release| match &line {
      | None => true,
      | Some(LtsSpec::Latest) => release.lts.is_some(),
      | Some(LtsSpec::Named(name)) => release.lts_name().as_deref() == Some(&**name),
      | Some(LtsSpec::Offset(_)) => false,
    };
    let matches_version = |release: &Release| {
      self
        .version
        .as_ref()
        .is_none_or(|req| req.matches(&release.version))
    };
    Ok(
      index
        .releases()
        .iter()
        .rev()
        .filter(|release| matches_lts(release) && matches_version(release))
        .collect(),
    )
  }
}

/// nvm colors a list only on a terminal, without `--no-colors`, and when the
/// terminal supports colors.
fn use_colors(ctx: &Context, opts: &LsRemoteOptions) -> bool {
  let is_dumb_terminal = ctx.var_string("TERM").as_deref() == Some(DUMB_TERMINAL);
  opts.terminal && !opts.no_colors && !is_dumb_terminal
}

/// Formats a row like nvm's `nvm_print_versions`. Without colors, installed
/// versions get a ` *` suffix and the active one a `->` prefix.
fn format_row(row: &Row<'_>, palette: Option<&Palette>) -> String {
  let version = row.release.version.to_string();
  let plain = format!("{version:>VERSION_WIDTH$}");
  let current = format!("->{version:>CURRENT_VERSION_WIDTH$}");
  let version_cell = match (row.state, palette) {
    | (State::Current, None) => format!("{current} *"),
    | (State::Installed, None) => format!("{plain} *"),
    | (
      State::Current,
      Some(Palette {
        current: Some(code),
        ..
      }),
    ) => paint(Some(code), &current),
    | (State::Installed, Some(palette)) => paint(palette.installed, &plain),
    | (State::Current | State::NotInstalled, _) => plain,
  };
  let Some(codename) = &row.release.lts else {
    return version_cell;
  };
  let is_installed = row.state != State::NotInstalled;
  let padding = if palette.is_none() && is_installed {
    ""
  } else {
    "  "
  };
  let lts_cell = if row.is_latest_lts {
    let color = palette.and_then(|p| bold(p.current));
    paint(color.as_deref(), &format!(" (Latest LTS: {codename})"))
  } else {
    let color = palette.and_then(|p| p.default);
    paint(color, &format!(" (LTS: {codename})"))
  };
  format!("{version_cell}{padding}{lts_cell}")
}

#[cfg(test)]
mod tests {
  use super::*;

  fn release(version: &str, lts: Option<&str>) -> Release {
    Release {
      version: version.parse().unwrap(),
      date: String::new(),
      files: Vec::new(),
      npm: None,
      lts: lts.map(str::to_owned),
      security: false,
    }
  }

  fn row(release: &Release, state: State, is_latest_lts: bool) -> Row<'_> {
    Row {
      release,
      state,
      is_latest_lts,
    }
  }

  #[test]
  fn formats_without_colors_like_nvm() {
    let plain = release("24.9.0", None);
    let old_lts = release("20.19.4", Some("Iron"));
    let latest_lts = release("22.20.0", Some("Jod"));
    let format = |release, state, latest| format_row(&row(release, state, latest), None);

    assert_eq!(
      format(&plain, State::NotInstalled, false),
      "        v24.9.0"
    );
    assert_eq!(format(&plain, State::Installed, false), "        v24.9.0 *");
    assert_eq!(format(&plain, State::Current, false), "->      v24.9.0 *");
    assert_eq!(
      format(&old_lts, State::NotInstalled, false),
      "       v20.19.4   (LTS: Iron)"
    );
    assert_eq!(
      format(&old_lts, State::Installed, false),
      "       v20.19.4 * (LTS: Iron)"
    );
    assert_eq!(
      format(&latest_lts, State::Current, true),
      "->     v22.20.0 * (Latest LTS: Jod)"
    );
  }

  #[test]
  fn formats_with_colors_like_nvm() {
    let palette = Palette::from_nvm_colors(None);
    let lts = release("22.20.0", Some("Jod"));
    let format = |state, latest| format_row(&row(&lts, state, latest), Some(&palette));
    assert_eq!(
      format(State::Installed, false),
      "\x1b[0;34m       v22.20.0\x1b[0m  \x1b[0;37m (LTS: Jod)\x1b[0m"
    );
    assert_eq!(
      format(State::Current, true),
      "\x1b[0;32m->     v22.20.0\x1b[0m  \x1b[1;32m (Latest LTS: Jod)\x1b[0m"
    );
    assert_eq!(
      format(State::NotInstalled, true),
      "       v22.20.0  \x1b[1;32m (Latest LTS: Jod)\x1b[0m"
    );
  }

  fn select(pattern: Option<&str>, lts: Option<&str>) -> Result<Vec<String>, String> {
    let index = RemoteIndex::from_releases(vec![
      release("18.20.8", Some("Hydrogen")),
      release("20.19.4", Some("Iron")),
      release("20.19.5", Some("Iron")),
      release("22.20.0", Some("Jod")),
      release("24.9.0", None),
    ]);
    let opts = LsRemoteOptions {
      pattern: pattern.map(str::to_owned),
      lts: lts.map(str::to_owned),
      ..LsRemoteOptions::default()
    };
    let filter = Filter::new(&opts).map_err(|e| e.to_string())?;
    Ok(
      filter
        .select(&index)?
        .into_iter()
        .map(|r| r.version.to_string())
        .collect(),
    )
  }

  #[test]
  fn filters() {
    assert_eq!(select(None, None).unwrap().len(), 5);
    assert_eq!(select(Some("node"), None).unwrap().len(), 5);
    assert_eq!(select(Some("20"), None).unwrap(), ["v20.19.4", "v20.19.5"]);
    assert_eq!(
      select(None, Some("*")).unwrap(),
      ["v18.20.8", "v20.19.4", "v20.19.5", "v22.20.0"]
    );
    assert_eq!(
      select(Some("lts/iron"), None).unwrap(),
      ["v20.19.4", "v20.19.5"]
    );
    assert_eq!(select(None, Some("-2")).unwrap(), ["v18.20.8"]);
    assert_eq!(select(Some("lts/*"), Some("jod")).unwrap(), ["v22.20.0"]);
    let none: [String; 0] = [];
    for (pattern, lts) in [
      (Some("22"), Some("iron")),
      (Some("system"), None),
      (Some("my-alias"), None),
      (Some("18..1"), None),
    ] {
      assert_eq!(select(pattern, lts).unwrap(), none, "{pattern:?} {lts:?}");
    }
  }

  #[test]
  fn rejects_bad_lts_names() {
    assert_eq!(
      select(None, Some("Iron")).unwrap_err(),
      "'lts/Iron': LTS names must be lowercase"
    );
    assert!(select(Some("lts/"), None).is_err());
    assert_eq!(
      select(None, Some("-4")).unwrap_err(),
      "That many LTS releases do not exist yet."
    );
  }
}
