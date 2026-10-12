//! nvm's per-version install lock: the directory
//! `<root>/.cache/locks/<version>`, created with `mkdir`, which is atomic.
//! nvm and rnm use the same directory, so they never install the same
//! version at the same time.
//!
//! rnm also writes its PID into the lock. A lock whose owner died (for
//! example after Ctrl-C) is taken over at once instead of after nvm's
//! timeout. Locks without a PID file (nvm's) are only taken over when older
//! than `NVM_INSTALL_LOCK_STALE` minutes, as in nvm.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The file inside the lock directory that names the owning rnm process.
const PID_FILE: &str = "rnm.pid";

/// nvm's default `NVM_INSTALL_LOCK_TIMEOUT`, in seconds.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

/// How often to retry a held lock (nvm polls every second).
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(1);

const SECONDS_PER_MINUTE: u64 = 60;

/// How long to wait for a held lock and when to take over an abandoned one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockSettings {
  /// Give up after waiting this long.
  pub timeout: Duration,
  /// Take over a lock older than this. `None` never does (nvm's default).
  pub stale_after: Option<Duration>,
  pub poll_interval: Duration,
}

impl Default for LockSettings {
  fn default() -> Self {
    Self {
      timeout: DEFAULT_TIMEOUT,
      stale_after: None,
      poll_interval: DEFAULT_POLL_INTERVAL,
    }
  }
}

impl LockSettings {
  /// Reads `NVM_INSTALL_LOCK_TIMEOUT` (seconds) and `NVM_INSTALL_LOCK_STALE`
  /// (minutes; `0` means never). Invalid values keep the defaults.
  #[must_use]
  pub fn from_env(env: impl Fn(&str) -> Option<String>) -> Self {
    let number = |key: &str| env(key)?.trim().parse::<u64>().ok();
    let mut settings = Self::default();
    if let Some(seconds) = number("NVM_INSTALL_LOCK_TIMEOUT") {
      settings.timeout = Duration::from_secs(seconds);
    }
    settings.stale_after = number("NVM_INSTALL_LOCK_STALE")
      .filter(|minutes| *minutes > 0)
      .map(|minutes| Duration::from_secs(minutes.saturating_mul(SECONDS_PER_MINUTE)));
    settings
  }
}

/// A held install lock, released on drop.
#[derive(Debug)]
pub struct InstallLock {
  dir: PathBuf,
}

/// Why the lock could not be taken.
#[derive(Debug)]
pub enum LockError {
  /// Another install held the lock for the whole timeout.
  Timeout {
    lock: PathBuf,
    waited: Duration,
  },
  Io {
    lock: PathBuf,
    source: io::Error,
  },
}

impl fmt::Display for LockError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      | Self::Timeout { lock, waited } => write!(
        f,
        "timed out after {}s waiting for another install to finish; if none is running, \
         remove {} and try again",
        waited.as_secs(),
        lock.display()
      ),
      | Self::Io { lock, source } => write!(f, "cannot create {}: {source}", lock.display()),
    }
  }
}

impl std::error::Error for LockError {}

impl InstallLock {
  /// Takes the lock for `version` under `locks_dir`, waiting while another
  /// live install holds it. `on_wait` runs once, when waiting starts.
  ///
  /// # Errors
  ///
  /// See [`LockError`].
  pub fn acquire(
    locks_dir: &Path,
    version: &str,
    settings: &LockSettings,
    mut on_wait: impl FnMut(),
  ) -> Result<Self, LockError> {
    let dir = locks_dir.join(lock_name(version));
    let io_error = |source| LockError::Io {
      lock: dir.clone(),
      source,
    };
    std::fs::create_dir_all(locks_dir).map_err(io_error)?;
    let mut waited = Duration::ZERO;
    let mut announced = false;
    loop {
      match std::fs::create_dir(&dir) {
        | Ok(()) => {
          // Without the PID file the lock still works; it just cannot be
          // taken over early after a crash.
          let _ = std::fs::write(dir.join(PID_FILE), std::process::id().to_string());
          return Ok(Self { dir });
        },
        | Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {},
        | Err(e) => return Err(io_error(e)),
      }
      if is_abandoned(&dir, settings) {
        let _ = std::fs::remove_dir_all(&dir);
        continue;
      }
      if waited >= settings.timeout {
        return Err(LockError::Timeout { lock: dir, waited });
      }
      if !announced {
        on_wait();
        announced = true;
      }
      std::thread::sleep(settings.poll_interval);
      waited += settings.poll_interval;
    }
  }
}

impl Drop for InstallLock {
  fn drop(&mut self) {
    let _ = std::fs::remove_dir_all(&self.dir);
  }
}

/// The owner is a dead rnm process, or the lock is older than the stale
/// limit.
fn is_abandoned(dir: &Path, settings: &LockSettings) -> bool {
  let owner = std::fs::read_to_string(dir.join(PID_FILE))
    .ok()
    .and_then(|pid| pid.trim().parse::<u32>().ok());
  let owner_died = owner.is_some_and(|pid| !crate::sys::is_alive(pid));
  let age = std::fs::metadata(dir)
    .and_then(|metadata| metadata.modified())
    .ok()
    .and_then(|modified| SystemTime::now().duration_since(modified).ok());
  let is_stale = settings
    .stale_after
    .zip(age)
    .is_some_and(|(limit, age)| age > limit);
  owner_died || is_stale
}

/// nvm's `nvm_install_lock_name`: characters outside `[0-9A-Za-z._+-]`
/// become `_`.
fn lock_name(version: &str) -> String {
  let is_allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-');
  version
    .chars()
    .map(|c| if is_allowed(c) { c } else { '_' })
    .collect()
}

#[cfg(test)]
mod tests {
  use super::*;

  fn quick(timeout: Duration) -> LockSettings {
    LockSettings {
      timeout,
      stale_after: None,
      poll_interval: Duration::from_millis(10),
    }
  }

  #[test]
  fn excludes_a_second_holder_until_released() {
    let tmp = tempfile::tempdir().unwrap();
    let lock = InstallLock::acquire(tmp.path(), "v20.0.0", &quick(Duration::ZERO), || {}).unwrap();
    assert!(tmp.path().join("v20.0.0").is_dir());

    let mut waits = 0;
    let error = InstallLock::acquire(
      tmp.path(),
      "v20.0.0",
      &quick(Duration::from_millis(30)),
      || waits += 1,
    )
    .unwrap_err();
    assert!(matches!(error, LockError::Timeout { .. }), "{error}");
    assert_eq!(waits, 1);

    InstallLock::acquire(tmp.path(), "v22.0.0", &quick(Duration::ZERO), || {}).unwrap();

    drop(lock);
    assert!(!tmp.path().join("v20.0.0").exists());
    InstallLock::acquire(tmp.path(), "v20.0.0", &quick(Duration::ZERO), || {}).unwrap();
  }

  #[test]
  fn takes_over_a_lock_whose_owner_died() {
    let tmp = tempfile::tempdir().unwrap();
    let mut child = if cfg!(windows) {
      std::process::Command::new("cmd")
        .args(["/C", "exit"])
        .spawn()
        .unwrap()
    } else {
      std::process::Command::new("true").spawn().unwrap()
    };
    let dead_pid = child.id();
    child.wait().unwrap();
    let dir = tmp.path().join("v20.0.0");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join(PID_FILE), dead_pid.to_string()).unwrap();

    InstallLock::acquire(tmp.path(), "v20.0.0", &quick(Duration::ZERO), || {}).unwrap();
  }

  #[test]
  fn waits_for_nvm_locks_unless_stale() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("v20.0.0")).unwrap();
    assert!(InstallLock::acquire(tmp.path(), "v20.0.0", &quick(Duration::ZERO), || {}).is_err());

    let stale = LockSettings {
      stale_after: Some(Duration::ZERO),
      ..quick(Duration::ZERO)
    };
    std::thread::sleep(Duration::from_millis(20));
    InstallLock::acquire(tmp.path(), "v20.0.0", &stale, || {}).unwrap();
  }

  #[test]
  fn reads_nvm_settings() {
    let env = |vars: &'static [(&'static str, &'static str)]| {
      move |key: &str| {
        vars
          .iter()
          .find(|(k, _)| *k == key)
          .map(|(_, v)| (*v).to_owned())
      }
    };
    assert_eq!(LockSettings::from_env(env(&[])), LockSettings::default());
    let settings = LockSettings::from_env(env(&[
      ("NVM_INSTALL_LOCK_TIMEOUT", "5"),
      ("NVM_INSTALL_LOCK_STALE", "2"),
    ]));
    assert_eq!(settings.timeout, Duration::from_secs(5));
    assert_eq!(settings.stale_after, Some(Duration::from_secs(120)));
    let never = LockSettings::from_env(env(&[("NVM_INSTALL_LOCK_STALE", "0")]));
    assert_eq!(never.stale_after, None);
  }

  #[test]
  fn sanitises_lock_names() {
    assert_eq!(lock_name("v20.0.0-rc.1+b"), "v20.0.0-rc.1+b");
    assert_eq!(lock_name("../x y"), ".._x_y");
  }
}
