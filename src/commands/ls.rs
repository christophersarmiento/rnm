//! `rnm ls`: list installed versions and aliases, like `nvm ls`.

use std::io::Write;

use super::alias::alias_lines;
use super::current::active;
use super::listing::{NOT_AVAILABLE_LABEL, State, VERSION_WIDTH, palette, version_cell};
use super::{Context, Failure, SYSTEM_LABEL, exit, say};
use crate::nvm_dir::{Flavor, Resolved};
use crate::version::{VersionReq, VersionSpec};

/// Options for [`ls`].
#[derive(Clone, Debug, Default)]
pub struct LsOptions {
  /// Only versions matching this: a version prefix, `iojs`, `system`, or
  /// any spec (an alias or `lts/*` shows the version it resolves to).
  pub pattern: Option<String>,
  pub no_colors: bool,
  /// `--no-alias`: skip the alias list.
  pub no_alias: bool,
  /// Whether `out` is a terminal; colors are only used there.
  pub terminal: bool,
}

/// Prints installed versions oldest first (plus `system` when a Node.js
/// outside rnm exists), marking the active one, then the aliases unless a
/// pattern is given or `--no-alias` is set.
///
/// # Errors
///
/// Exit code 3 (after printing `N/A`) when nothing matches; 55 for
/// `--no-alias` with a pattern.
pub fn ls(ctx: &Context, opts: &LsOptions, out: &mut dyn Write) -> Result<(), Failure> {
  if opts.pattern.is_some() && opts.no_alias {
    return Err(Failure::new(
      exit::INVALID_OPTION,
      "`--no-alias` is not supported when a pattern is provided.",
    ));
  }
  let versions = matching_versions(ctx, opts.pattern.as_deref())?;
  let palette = palette(ctx, opts.no_colors, opts.terminal);
  if versions.is_empty() {
    say(out, format_args!("{NOT_AVAILABLE_LABEL:>VERSION_WIDTH$}"));
    return Err(Failure::new(exit::NOT_AVAILABLE, ""));
  }
  let current = active(ctx)?;
  for version in &versions {
    let state = if *version == current {
      State::Current
    } else if version == SYSTEM_LABEL {
      State::System
    } else {
      State::Installed
    };
    say(out, version_cell(version, state, palette.as_ref()));
  }
  if opts.pattern.is_none() && !opts.no_alias {
    for line in alias_lines(ctx, palette)? {
      say(out, line);
    }
  }
  Ok(())
}

/// nvm's `nvm_ls`: version prefixes list every installed match, `iojs`
/// every io.js install, other specs the one version they resolve to.
fn matching_versions(ctx: &Context, pattern: Option<&str>) -> Result<Vec<String>, Failure> {
  let installed = ctx.nvm_dir.installed_versions()?;
  let has_system = ctx.system_node().is_some();
  let Some(pattern) = pattern else {
    let mut all: Vec<String> = installed.iter().map(ToString::to_string).collect();
    if has_system {
      all.push(SYSTEM_LABEL.to_owned());
    }
    return Ok(all);
  };
  let Ok(spec) = pattern.parse::<VersionSpec>() else {
    return Ok(Vec::new());
  };
  let list = |flavor: Flavor, req: Option<&VersionReq>| -> Vec<String> {
    installed
      .iter()
      .filter(|i| i.flavor == flavor && req.is_none_or(|req| req.matches(&i.version)))
      .map(ToString::to_string)
      .collect()
  };
  Ok(match &spec {
    | VersionSpec::Version(req @ (VersionReq::Major(_) | VersionReq::MajorMinor(..))) => {
      list(Flavor::Node, Some(req))
    },
    | VersionSpec::Iojs(req) => list(Flavor::Iojs, req.as_ref()),
    | VersionSpec::System if has_system => vec![SYSTEM_LABEL.to_owned()],
    | _ => match ctx.nvm_dir.resolve(&spec) {
      | Ok(Resolved::Installed(installed)) => vec![installed.to_string()],
      | Ok(Resolved::System) if has_system => vec![SYSTEM_LABEL.to_owned()],
      | _ => Vec::new(),
    },
  })
}
