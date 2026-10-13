//! nvm's version-list colors (`NVM_COLORS`, `nvm set-colors`).
//!
//! `NVM_COLORS` holds five letters, one per role, defaulting to `bygre`.
//! Each letter maps to an ANSI SGR code; `0` means "no color".

/// nvm's default `NVM_COLORS`.
const DEFAULT_COLORS: &str = "bygre";

const RESET: &str = "\x1b[0m";

/// The colors for the roles in a version list. `None` means uncolored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
  pub installed: Option<&'static str>,
  pub system: Option<&'static str>,
  pub current: Option<&'static str>,
  pub not_installed: Option<&'static str>,
  pub default: Option<&'static str>,
}

impl Palette {
  /// Reads `NVM_COLORS` as nvm does: unset or empty means the default, and
  /// a missing or unknown letter leaves its role uncolored.
  #[must_use]
  pub fn from_nvm_colors(value: Option<&str>) -> Self {
    let letters = value.filter(|v| !v.is_empty()).unwrap_or(DEFAULT_COLORS);
    let mut roles = letters.chars().map(sgr_code);
    let mut next = || roles.next().flatten();
    Self {
      installed: next(),
      system: next(),
      current: next(),
      not_installed: next(),
      default: next(),
    }
  }
}

/// Wraps `text` in the SGR `code` (for example `0;34m`), or returns it
/// unchanged when there is no code.
#[must_use]
pub fn paint(code: Option<&str>, text: &str) -> String {
  match code {
    | Some(code) => format!("\x1b[{code}{text}{RESET}"),
    | None => text.to_owned(),
  }
}

/// The bold variant of a `0;NNm` code, as nvm derives the "Latest LTS"
/// color from the "current" color.
#[must_use]
pub fn bold(code: Option<&'static str>) -> Option<String> {
  code.map(|code| code.replacen("0;", "1;", 1))
}

/// nvm's color letters (`nvm_print_color_code`). `None` for `0` ("no
/// color") and for unknown letters.
fn sgr_code(letter: char) -> Option<&'static str> {
  Some(match letter {
    | 'r' => "0;31m",
    | 'R' => "1;31m",
    | 'g' => "0;32m",
    | 'G' => "1;32m",
    | 'b' => "0;34m",
    | 'B' => "1;34m",
    | 'c' => "0;36m",
    | 'C' => "1;36m",
    | 'm' => "0;35m",
    | 'M' => "1;35m",
    | 'y' => "0;33m",
    | 'Y' => "1;33m",
    | 'k' => "0;30m",
    | 'K' => "1;30m",
    | 'e' => "0;37m",
    | 'W' => "1;37m",
    | _ => return None,
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn defaults_like_nvm() {
    let palette = Palette::from_nvm_colors(None);
    assert_eq!(palette.installed, Some("0;34m"));
    assert_eq!(palette.system, Some("0;33m"));
    assert_eq!(palette.current, Some("0;32m"));
    assert_eq!(palette.not_installed, Some("0;31m"));
    assert_eq!(palette.default, Some("0;37m"));
    assert_eq!(Palette::from_nvm_colors(Some("")), palette);
  }

  #[test]
  fn custom_colors_and_no_color() {
    let palette = Palette::from_nvm_colors(Some("RG0kW"));
    assert_eq!(palette.installed, Some("1;31m"));
    assert_eq!(palette.current, None);
    assert_eq!(palette.default, Some("1;37m"));
    let short = Palette::from_nvm_colors(Some("bx"));
    assert_eq!(short.installed, Some("0;34m"));
    assert_eq!(short.system, None);
    assert_eq!(short.default, None);
  }

  #[test]
  fn paints() {
    assert_eq!(paint(Some("0;32m"), "x"), "\x1b[0;32mx\x1b[0m");
    assert_eq!(paint(None, "x"), "x");
    assert_eq!(bold(Some("0;32m")).as_deref(), Some("1;32m"));
    assert_eq!(bold(Some("1;32m")).as_deref(), Some("1;32m"));
  }
}
