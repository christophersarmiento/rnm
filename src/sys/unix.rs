//! Unix implementation.

#![allow(unsafe_code)]

use super::{Process, ProcessInfo};

/// `init` (or `launchd`), the root of the process tree. Its ancestors are not
/// interesting.
const INIT_PID: u32 = 1;

/// `kill` with signal 0 sends nothing; it only checks that the process exists
/// and that we may signal it.
const EXISTENCE_CHECK: libc::c_int = 0;

#[allow(
  clippy::unnecessary_wraps,
  reason = "same signature as the Windows version"
)]
pub fn parent_pid() -> Option<u32> {
  Some(std::os::unix::process::parent_id())
}

pub fn is_alive(pid: u32) -> bool {
  let Ok(pid) = libc::pid_t::try_from(pid) else {
    return false;
  };
  if pid <= 0 {
    return false;
  }
  // SAFETY: EXISTENCE_CHECK sends no signal.
  let exists = unsafe { libc::kill(pid, EXISTENCE_CHECK) } == 0;
  let exists_but_not_ours = std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
  exists || exists_but_not_ours
}

/// Asks the kernel whether this process runs under Rosetta 2. The sysctl
/// does not exist on Intel Macs, which counts as "not translated".
#[cfg(target_os = "macos")]
pub fn is_translated() -> bool {
  let mut translated: libc::c_int = 0;
  let mut size = std::mem::size_of::<libc::c_int>();
  // SAFETY: the name is NUL-terminated, and `translated`/`size` describe a
  // valid, writable c_int buffer.
  let status = unsafe {
    libc::sysctlbyname(
      c"sysctl.proc_translated".as_ptr(),
      (&raw mut translated).cast(),
      &raw mut size,
      std::ptr::null_mut(),
      0,
    )
  };
  status == 0 && translated == 1
}

#[cfg(not(target_os = "macos"))]
pub fn is_translated() -> bool {
  false
}

pub fn ancestors(limit: usize) -> Vec<Process> {
  let mut chain = Vec::new();
  let mut pid = std::os::unix::process::parent_id();
  while chain.len() < limit && pid > INIT_PID {
    let Some(info) = info(pid) else {
      break;
    };
    chain.push(Process {
      pid,
      name: info.name,
    });
    pid = info.parent;
  }
  chain
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn info(pid: u32) -> Option<ProcessInfo> {
  // /proc/<pid>/stat is "pid (comm) state ppid ...". comm may contain spaces
  // and parentheses, so split at the *last* ')'.
  let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
  let open = stat.find('(')?;
  let close = stat.rfind(')')?;
  let name = stat.get(open + 1..close)?.to_owned();
  let mut fields = stat.get(close + 1..)?.split_ascii_whitespace();
  let _state = fields.next()?;
  let parent = fields.next()?.parse().ok()?;
  Some(ProcessInfo { parent, name })
}

#[cfg(target_os = "macos")]
fn info(pid: u32) -> Option<ProcessInfo> {
  let mut bsd = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
  let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).ok()?;
  // SAFETY: `bsd` is a writable buffer of exactly `size` bytes.
  let written = unsafe {
    libc::proc_pidinfo(
      libc::c_int::try_from(pid).ok()?,
      libc::PROC_PIDTBSDINFO,
      0,
      bsd.as_mut_ptr().cast(),
      size,
    )
  };
  if written != size {
    return None;
  }
  // SAFETY: proc_pidinfo filled the whole struct.
  let bsd = unsafe { bsd.assume_init() };
  // pbi_comm is truncated to 16 bytes; pbi_name holds up to 32 but can be
  // empty.
  let full_name = c_chars_to_string(&bsd.pbi_name);
  let name = if full_name.is_empty() {
    c_chars_to_string(&bsd.pbi_comm)
  } else {
    full_name
  };
  Some(ProcessInfo {
    parent: bsd.pbi_ppid,
    name,
  })
}

#[cfg(target_os = "macos")]
fn c_chars_to_string(chars: &[libc::c_char]) -> String {
  let bytes: Vec<u8> = chars
    .iter()
    .take_while(|&&c| c != 0)
    .map(|&c| c.cast_unsigned())
    .collect();
  String::from_utf8_lossy(&bytes).into_owned()
}

/// Other unix systems: the POSIX `ps -o ppid= -o comm= -p <pid>`.
#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
fn info(pid: u32) -> Option<ProcessInfo> {
  let output = std::process::Command::new("ps")
    .args(["-o", "ppid=", "-o", "comm=", "-p", &pid.to_string()])
    .output()
    .ok()?;
  let text = String::from_utf8_lossy(&output.stdout);
  let line = text.lines().next()?.trim_start();
  let (parent, comm) = line.split_once(char::is_whitespace)?;
  Some(ProcessInfo {
    parent: parent.parse().ok()?,
    name: comm.trim().rsplit('/').next()?.to_owned(),
  })
}
