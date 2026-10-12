//! Windows implementation: one Toolhelp snapshot per call.

#![allow(unsafe_code)]

use std::collections::HashMap;

use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE, STILL_ACTIVE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
  CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
  GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::Process;

/// pid → (ppid, exe name).
fn snapshot() -> HashMap<u32, (u32, String)> {
  let mut map = HashMap::new();
  // SAFETY: plain Win32 calls; the handle is closed below, and `entry.dwSize`
  // is set before the first use as the API requires.
  unsafe {
    let handle = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
    if handle == INVALID_HANDLE_VALUE {
      return map;
    }
    let mut entry: PROCESSENTRY32W = std::mem::zeroed();
    entry.dwSize = u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).unwrap_or(0);
    let mut ok = Process32FirstW(handle, &raw mut entry);
    while ok != 0 {
      let len = entry
        .szExeFile
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(entry.szExeFile.len());
      let exe = String::from_utf16_lossy(&entry.szExeFile[..len]);
      map.insert(
        entry.th32ProcessID,
        (entry.th32ParentProcessID, strip_exe(&exe)),
      );
      ok = Process32NextW(handle, &raw mut entry);
    }
    CloseHandle(handle);
  }
  map
}

fn strip_exe(name: &str) -> String {
  let len = name.len();
  if len > 4 && name[len - 4..].eq_ignore_ascii_case(".exe") {
    name[..len - 4].to_owned()
  } else {
    name.to_owned()
  }
}

pub fn parent_pid() -> Option<u32> {
  snapshot().get(&std::process::id()).map(|&(ppid, _)| ppid)
}

pub fn ancestors(limit: usize) -> Vec<Process> {
  let map = snapshot();
  let mut chain = Vec::new();
  let Some(&(mut pid, _)) = map.get(&std::process::id()) else {
    return chain;
  };
  while chain.len() < limit && pid > 0 {
    let Some((parent, name)) = map.get(&pid) else {
      break;
    };
    chain.push(Process {
      pid,
      name: name.clone(),
    });
    if *parent == pid {
      break;
    }
    pid = *parent;
  }
  chain
}

pub fn is_alive(pid: u32) -> bool {
  // SAFETY: the handle is checked and closed.
  unsafe {
    let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
    if handle.is_null() {
      // Access denied means the process exists but is protected.
      return std::io::Error::last_os_error().raw_os_error() == Some(5);
    }
    let mut code = 0u32;
    let ok = GetExitCodeProcess(handle, &raw mut code);
    CloseHandle(handle);
    ok != 0 && i32::try_from(code).is_ok_and(|code| code == STILL_ACTIVE)
  }
}
