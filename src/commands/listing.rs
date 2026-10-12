//! nvm's version-list layout (`nvm_print_versions`), shared by `ls` and
//! `ls-remote`.

use super::Context;
use super::colors::{Palette, paint};

/// What nvm prints when nothing matches.
pub const NOT_AVAILABLE_LABEL: &str = "N/A";

/// The width nvm pads version numbers to.
pub const VERSION_WIDTH: usize = 15;

/// The width after the `->` current-version marker.
const CURRENT_VERSION_WIDTH: usize = VERSION_WIDTH - "->".len();

/// The `TERM` value of terminals without color support.
const DUMB_TERMINAL: &str = "dumb";

/// How a listed version relates to this machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
  /// The active version.
  Current,
  /// A Node.js outside the tree (`system`), not active.
  System,
  Installed,
  NotInstalled,
}

impl State {
  /// The version is on this machine (nvm's `is_installed`).
  #[must_use]
  pub fn is_installed(self) -> bool {
    self != Self::NotInstalled
  }
}

/// The colors for a list, or `None` when it is printed plain. nvm colors a
/// list only on a terminal, without `--no-colors`, and when the terminal
/// supports colors.
#[must_use]
pub fn palette(ctx: &Context, no_colors: bool, terminal: bool) -> Option<Palette> {
  let is_dumb_terminal = ctx.var_string("TERM").as_deref() == Some(DUMB_TERMINAL);
  let colored = terminal && !no_colors && !is_dumb_terminal;
  colored.then(|| Palette::from_nvm_colors(ctx.var_string("NVM_COLORS").as_deref()))
}

/// The version column: right-aligned to 15 characters. Without colors,
/// versions on this machine get a ` *` suffix and the active one a `->`
/// prefix.
#[must_use]
pub fn version_cell(version: &str, state: State, palette: Option<&Palette>) -> String {
  let plain = format!("{version:>VERSION_WIDTH$}");
  let current = format!("->{version:>CURRENT_VERSION_WIDTH$}");
  match (state, palette) {
    | (State::Current, None) => format!("{current} *"),
    | (State::System | State::Installed, None) => format!("{plain} *"),
    | (State::Current, Some(palette)) if palette.current.is_some() => {
      paint(palette.current, &current)
    },
    | (State::System, Some(palette)) => paint(palette.system, &plain),
    | (State::Installed, Some(palette)) => paint(palette.installed, &plain),
    | (State::Current | State::NotInstalled, _) => plain,
  }
}

/// The padding between the version column and an annotation: none after
/// the plain ` *` marker, two spaces otherwise.
#[must_use]
pub fn annotation_padding(state: State, palette: Option<&Palette>) -> &'static str {
  if palette.is_none() && state.is_installed() {
    ""
  } else {
    "  "
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn formats_versions_like_nvm() {
    assert_eq!(
      version_cell("v20.0.0", State::NotInstalled, None),
      "        v20.0.0"
    );
    assert_eq!(
      version_cell("v20.0.0", State::Installed, None),
      "        v20.0.0 *"
    );
    assert_eq!(
      version_cell("system", State::System, None),
      "         system *"
    );
    assert_eq!(
      version_cell("v20.0.0", State::Current, None),
      "->      v20.0.0 *"
    );
    let palette = Palette::from_nvm_colors(None);
    assert_eq!(
      version_cell("system", State::System, Some(&palette)),
      "\x1b[0;33m         system\x1b[0m"
    );
    let no_current_color = Palette::from_nvm_colors(Some("by0re"));
    assert_eq!(
      version_cell("v20.0.0", State::Current, Some(&no_current_color)),
      "        v20.0.0"
    );
  }
}
