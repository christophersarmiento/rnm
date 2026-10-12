//! Small, dependency-light platform helpers: process ancestry and liveness.
//!
//! No subprocesses and no `sysinfo`: Linux reads `/proc`, macOS calls
//! `proc_pidinfo`, Windows takes one Toolhelp snapshot. Other unix systems
//! fall back to `ps`.

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
use windows as imp;

/// One process in the ancestry chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Process {
  pub pid: u32,
  /// Executable name: a file name without directories and without `.exe`.
  /// Login shells keep their leading `-` (for example `-zsh`).
  pub name: String,
}

/// The ancestors of the current process, starting with its parent, at most
/// `limit` of them. Stops early at PID 0 or 1, or when information is not
/// available.
#[must_use]
pub fn ancestors(limit: usize) -> Vec<Process> {
  imp::ancestors(limit)
}

/// The parent process ID of the current process.
#[must_use]
pub fn parent_pid() -> Option<u32> {
  imp::parent_pid()
}

/// Returns `true` if a process with this ID exists. A process that exists but
/// belongs to another user counts as alive.
#[must_use]
pub fn is_alive(pid: u32) -> bool {
  imp::is_alive(pid)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn current_parent_is_alive() {
    let parent = parent_pid().expect("parent pid");
    assert!(parent > 0);
    assert!(is_alive(parent));
    assert!(is_alive(std::process::id()));
  }

  #[test]
  fn ancestors_start_with_parent() {
    let chain = ancestors(3);
    assert!(!chain.is_empty(), "no ancestors found");
    assert_eq!(chain[0].pid, parent_pid().unwrap());
    assert_ne!(chain[0].name, "");
    assert!(!chain[0].name.contains(['/', '\\']), "{:?}", chain[0].name);
  }

  #[test]
  fn finds_dead_process() {
    // Spawn and reap a child; its PID is then free (reuse within this test is
    // vanishingly unlikely).
    let mut child = if cfg!(windows) {
      std::process::Command::new("cmd")
        .args(["/C", "exit"])
        .spawn()
        .unwrap()
    } else {
      std::process::Command::new("true").spawn().unwrap()
    };
    let pid = child.id();
    child.wait().unwrap();
    assert!(!is_alive(pid));
  }
}
