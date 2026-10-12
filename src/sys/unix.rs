//! Unix implementation.

#![allow(unsafe_code)]

use super::Process;

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
  // SAFETY: signal 0 performs only the existence and permission checks.
  let result = unsafe { libc::kill(pid, 0) };
  result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

pub fn ancestors(limit: usize) -> Vec<Process> {
  let mut chain = Vec::new();
  let mut pid = std::os::unix::process::parent_id();
  while chain.len() < limit && pid > 1 {
    let Some((parent, name)) = info(pid) else {
      break;
    };
    chain.push(Process { pid, name });
    pid = parent;
  }
  chain
}

/// `(ppid, name)` for a process.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn info(pid: u32) -> Option<(u32, String)> {
  // /proc/<pid>/stat is "pid (comm) state ppid ...". comm may contain spaces
  // and parentheses, so split at the *last* ')'.
  let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
  let open = stat.find('(')?;
  let close = stat.rfind(')')?;
  let name = stat.get(open + 1..close)?.to_owned();
  let mut fields = stat.get(close + 1..)?.split_ascii_whitespace();
  let _state = fields.next()?;
  let parent = fields.next()?.parse().ok()?;
  Some((parent, name))
}

#[cfg(target_os = "macos")]
fn info(pid: u32) -> Option<(u32, String)> {
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
  // pbi_name holds the full name (32 bytes); pbi_comm is truncated to 16.
  let name = c_chars_to_string(&bsd.pbi_name);
  let name = if name.is_empty() {
    c_chars_to_string(&bsd.pbi_comm)
  } else {
    name
  };
  // A login shell's argv[0] (`-zsh`) is not visible here; the executable
  // name is, which is what classification needs.
  Some((bsd.pbi_ppid, name))
}

#[cfg(target_os = "macos")]
fn c_chars_to_string(chars: &[libc::c_char]) -> String {
  #[allow(clippy::cast_sign_loss)] // c_char is i8 on some targets; reinterpret the byte.
  let bytes: Vec<u8> = chars
    .iter()
    .take_while(|&&c| c != 0)
    .map(|&c| c as u8)
    .collect();
  String::from_utf8_lossy(&bytes).into_owned()
}

/// Other unix systems: the POSIX `ps -o ppid= -o comm= -p <pid>`.
#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
fn info(pid: u32) -> Option<(u32, String)> {
  let output = std::process::Command::new("ps")
    .args(["-o", "ppid=", "-o", "comm=", "-p", &pid.to_string()])
    .output()
    .ok()?;
  let text = String::from_utf8_lossy(&output.stdout);
  let line = text.lines().next()?.trim_start();
  let (ppid, comm) = line.split_once(char::is_whitespace)?;
  let name = comm.trim().rsplit('/').next()?.to_owned();
  Some((ppid.parse().ok()?, name))
}
