//! `rnm alias` and `rnm unalias`, with nvm's listing format
//! (`nvm_list_aliases`, `nvm_print_formatted_alias`).

use std::io::Write;

use super::colors::{Palette, paint};
use super::current::active;
use super::listing::{NOT_AVAILABLE_LABEL, palette};
use super::{Context, Failure, SYSTEM_LABEL, exit, say};
use crate::nvm_dir::{Flavor, ResolveError, Resolved};
use crate::version::{IOJS_PREFIX, LTS_PREFIX, Version, VersionSpec};

/// What nvm prints for an alias chain that loops.
const LOOP_LABEL: &str = "∞";

/// nvm's arrow color: dark gray.
const ARROW_COLOR: &str = "0;90m";

/// Built-in aliases that nvm computes from the installed versions when no
/// alias file of that name exists, in nvm's order.
const IMPLICIT_ALIASES: [&str; 4] = ["node", "stable", "unstable", "iojs"];

/// Names that cannot be removed with `unalias` unless a file shadows them.
const BUILT_IN_NAMES: [&str; 5] = ["node", "stable", "unstable", "iojs", "system"];

/// One alias as `nvm alias` prints it.
struct AliasLine {
  name: String,
  /// The alias target as written (`lts/*`, `20`, `v20.11.1`).
  target: String,
  /// The target resolved among installed versions: `v20.11.1`, `system`,
  /// `N/A`, or `∞`.
  resolved: String,
  /// A built-in alias, marked ` (default)`.
  is_implicit: bool,
  is_lts: bool,
  /// Color an installed resolved version. nvm does this when it creates an
  /// alias, but its listing never does (it has the color, but the check
  /// does not fire there), so listed lines leave it out.
  colors_installed: bool,
}

/// What a list of aliases needs to know about the machine.
struct Listing<'a> {
  ctx: &'a Context,
  current: String,
  palette: Option<Palette>,
}

/// `rnm alias`: with no name, lists every alias; with a name, lists aliases
/// starting with it; with a name and a target, creates or replaces it.
///
/// # Errors
///
/// Exit code 1 for invalid names or I/O failures.
pub fn alias(
  ctx: &Context,
  name: Option<&str>,
  target: Option<&str>,
  no_colors: bool,
  terminal: bool,
  out: &mut dyn Write,
  err: &mut dyn Write,
) -> Result<(), Failure> {
  let listing = Listing {
    ctx,
    current: active(ctx)?,
    palette: palette(ctx, no_colors, terminal),
  };
  match (name, target) {
    | (Some(name), Some("")) => unalias(ctx, name, out, err),
    | (Some(name), Some(target)) => {
      if name.contains('#') {
        return Err(Failure::new(
          exit::ERROR,
          "Aliases with a comment delimiter (#) are not supported.",
        ));
      }
      reject_subdirectory(name)?;
      let resolved = listing.resolve(target);
      if resolved == NOT_AVAILABLE_LABEL {
        say(
          err,
          format_args!("! WARNING: Version '{target}' does not exist."),
        );
      }
      ctx.nvm_dir.aliases().set(name, target)?;
      let line = AliasLine {
        name: name.to_owned(),
        target: target.to_owned(),
        resolved,
        is_implicit: false,
        is_lts: false,
        colors_installed: true,
      };
      say(out, listing.format(&line));
      Ok(())
    },
    | (Some(lts), None) if lts.starts_with(LTS_PREFIX) => match ctx.nvm_dir.aliases().get(lts)? {
      | Some(target) => {
        say(out, target);
        Ok(())
      },
      | None => Err(Failure::new(exit::ERROR, "Alias does not exist.")),
    },
    | (prefix, _) => {
      for line in listing.lines(prefix.unwrap_or_default())? {
        say(out, line);
      }
      Ok(())
    },
  }
}

/// `rnm unalias <name>`: removes a user alias.
///
/// # Errors
///
/// Exit code 1 for built-in names, names with `/`, and I/O failures. Like
/// nvm, a missing alias is reported on stderr with exit code 0.
pub fn unalias(
  ctx: &Context,
  name: &str,
  out: &mut dyn Write,
  err: &mut dyn Write,
) -> Result<(), Failure> {
  reject_subdirectory(name)?;
  let aliases = ctx.nvm_dir.aliases();
  let Some(target) = aliases.get(name)? else {
    if BUILT_IN_NAMES.contains(&name) {
      return Err(Failure::new(
        exit::ERROR,
        format!("{name} is a default (built-in) alias and cannot be deleted."),
      ));
    }
    say(err, format_args!("Alias {name} doesn't exist!"));
    return Ok(());
  };
  aliases.remove(name)?;
  say(
    out,
    format_args!("Deleted alias {name} - restore it with `rnm alias \"{name}\" \"{target}\"`"),
  );
  Ok(())
}

/// The alias lines `rnm ls` appends after the versions.
pub(crate) fn alias_lines(ctx: &Context, palette: Option<Palette>) -> Result<Vec<String>, Failure> {
  Listing {
    ctx,
    current: active(ctx)?,
    palette,
  }
  .lines("")
}

fn reject_subdirectory(name: &str) -> Result<(), Failure> {
  if name.contains('/') {
    return Err(Failure::new(
      exit::ERROR,
      "Aliases in subdirectories are not supported.",
    ));
  }
  Ok(())
}

impl Listing<'_> {
  /// User aliases starting with `prefix`, then the built-in aliases (only
  /// those not shadowed by a file, and only the one named `prefix` when it
  /// is set), then LTS aliases whose codename starts with `prefix`. Each
  /// group is sorted, as nvm sorts each group's output.
  fn lines(&self, prefix: &str) -> Result<Vec<String>, Failure> {
    let aliases = self.ctx.nvm_dir.aliases();
    let all = aliases.list(true)?;
    let mut user = Vec::new();
    let mut lts = Vec::new();
    for (name, target) in all {
      let (group, is_lts, short_name) = match name.strip_prefix(LTS_PREFIX) {
        | Some(codename) => (&mut lts, true, codename),
        | None => (&mut user, false, name.as_str()),
      };
      if short_name.starts_with(prefix) {
        let resolved = self.resolve(&target);
        group.push(AliasLine {
          name,
          target,
          resolved,
          is_implicit: false,
          is_lts,
          colors_installed: false,
        });
      }
    }

    let mut implicit = Vec::new();
    for name in IMPLICIT_ALIASES {
      let is_shadowed = aliases.get(name)?.is_some();
      let is_requested = prefix.is_empty() || prefix == name;
      if is_shadowed || !is_requested {
        continue;
      }
      if let Some(target) = self.implicit_target(name)? {
        implicit.push(AliasLine {
          name: name.to_owned(),
          resolved: self.resolve(&target),
          target,
          is_implicit: true,
          is_lts: false,
          colors_installed: false,
        });
      }
    }

    // nvm pipes each group through `sort`, so colored lines (which start
    // with an escape character) sort before plain ones.
    Ok(
      [user, implicit, lts]
        .into_iter()
        .flat_map(|group| {
          let mut lines: Vec<String> = group.iter().map(|line| self.format(line)).collect();
          lines.sort();
          lines
        })
        .collect(),
    )
  }

  /// nvm's `nvm_print_implicit_alias local`: `node` → `stable`; `stable` →
  /// the newest stable `major.minor` (nothing when none is installed);
  /// `unstable` → the newest odd `0.minor`; `iojs` → the newest io.js
  /// `iojs-vX.Y`. The last two print `N/A` when nothing matches.
  fn implicit_target(&self, name: &str) -> Result<Option<String>, Failure> {
    let installed = self.ctx.nvm_dir.installed_versions()?;
    let newest = |flavor: Flavor, accept: fn(&Version) -> bool| {
      installed
        .iter()
        .filter(|i| i.flavor == flavor && accept(&i.version))
        .map(|i| &i.version)
        .max()
        .map(|v| format!("{}.{}", v.major, v.minor))
    };
    let not_available = || NOT_AVAILABLE_LABEL.to_owned();
    Ok(match name {
      | "node" => Some("stable".to_owned()),
      | "stable" => newest(Flavor::Node, Version::is_stable),
      | "unstable" => {
        Some(newest(Flavor::Node, Version::is_unstable).unwrap_or_else(not_available))
      },
      | _ => Some(
        newest(Flavor::Iojs, |_| true)
          .map_or_else(not_available, |minor| format!("{IOJS_PREFIX}v{minor}")),
      ),
    })
  }

  /// nvm's `nvm_version` of an alias target, as a label.
  fn resolve(&self, target: &str) -> String {
    let Ok(spec) = target.parse::<VersionSpec>() else {
      return NOT_AVAILABLE_LABEL.to_owned();
    };
    match self.ctx.nvm_dir.resolve(&spec) {
      | Ok(Resolved::Installed(installed)) => installed.to_string(),
      | Ok(Resolved::System) if self.ctx.system_node().is_some() => SYSTEM_LABEL.to_owned(),
      | Ok(Resolved::Current) => self.current.clone(),
      | Err(ResolveError::AliasCycle(_)) => LOOP_LABEL.to_owned(),
      | Ok(Resolved::System) | Err(_) => NOT_AVAILABLE_LABEL.to_owned(),
    }
  }

  /// `name -> target (-> resolved)`, or `name -> target` when the target is
  /// the resolved version itself. Without colors, a resolved version gets a
  /// ` *` suffix; built-in aliases end with ` (default)`.
  fn format(&self, line: &AliasLine) -> String {
    let is_unresolved = line.resolved == NOT_AVAILABLE_LABEL || line.resolved == LOOP_LABEL;
    let Some(palette) = &self.palette else {
      let marker = if is_unresolved { "" } else { " *" };
      let suffix = if line.is_implicit { " (default)" } else { "" };
      return if line.target == line.resolved {
        format!("{} -> {}{marker}{suffix}", line.name, line.target)
      } else {
        format!(
          "{} -> {} (-> {}{marker}){suffix}",
          line.name, line.target, line.resolved
        )
      };
    };

    let state_color = if line.resolved == self.current {
      palette.current
    } else if line.colors_installed && self.is_installed(&line.resolved) {
      palette.installed
    } else if is_unresolved {
      palette.not_installed
    } else {
      None
    };
    let lts_color = lts_color(palette.system);
    let name_color = if line.is_lts {
      lts_color.as_deref()
    } else {
      state_color
    };
    let target_color = if line.target.starts_with(LTS_PREFIX) {
      lts_color.as_deref()
    } else {
      state_color
    };
    let arrow = paint(Some(ARROW_COLOR), "->");
    let suffix = if line.is_implicit {
      format!(" {}", paint(palette.default, "(default)"))
    } else {
      String::new()
    };
    let name = paint(name_color, &line.name);
    if line.target == line.resolved {
      format!(
        "{name} {arrow} {}{suffix}",
        paint(state_color, &line.target)
      )
    } else {
      format!(
        "{name} {arrow} {} ({arrow} {}){suffix}",
        paint(target_color, &line.target),
        paint(state_color, &line.resolved)
      )
    }
  }
}

impl Listing<'_> {
  /// `label` names an installed version (nvm's `nvm_is_version_installed`).
  fn is_installed(&self, label: &str) -> bool {
    self
      .ctx
      .nvm_dir
      .installed_versions()
      .is_ok_and(|installed| installed.iter().any(|i| i.to_string() == label))
  }
}

/// nvm's LTS color: the system color with every `0` turned into `1` (its
/// `tr '0;' '1;'`), so `0;33m` becomes bold `1;33m`.
fn lts_color(system: Option<&str>) -> Option<String> {
  system.map(|code| code.replace('0', "1"))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn lts_color_matches_nvm() {
    assert_eq!(lts_color(Some("0;33m")).as_deref(), Some("1;33m"));
    assert_eq!(lts_color(Some("0;30m")).as_deref(), Some("1;31m"));
    assert_eq!(lts_color(None), None);
  }
}
