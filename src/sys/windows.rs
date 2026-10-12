//! Windows implementation: one Toolhelp snapshot per call.

#![allow(unsafe_code)]

use std::collections::HashMap;

use windows_sys::Win32::Foundation::{
  CloseHandle, ERROR_ACCESS_DENIED, INVALID_HANDLE_VALUE, STILL_ACTIVE,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
  CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
  GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::{Process, ProcessInfo};

const EXE_SUFFIX: &str = ".exe";

fn snapshot() -> HashMap<u32, ProcessInfo> {
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
        ProcessInfo {
          parent: entry.th32ParentProcessID,
          name: strip_exe(&exe),
        },
      );
      ok = Process32NextW(handle, &raw mut entry);
    }
    CloseHandle(handle);
  }
  map
}

fn strip_exe(name: &str) -> String {
  let stem_len = name.len().saturating_sub(EXE_SUFFIX.len());
  let has_exe_suffix = name
    .get(stem_len..)
    .is_some_and(|suffix| suffix.eq_ignore_ascii_case(EXE_SUFFIX));
  if stem_len > 0 && has_exe_suffix {
    name[..stem_len].to_owned()
  } else {
    name.to_owned()
  }
}

pub fn parent_pid() -> Option<u32> {
  snapshot().get(&std::process::id()).map(|info| info.parent)
}

pub fn ancestors(limit: usize) -> Vec<Process> {
  let map = snapshot();
  let mut chain = Vec::new();
  let Some(mut pid) = map.get(&std::process::id()).map(|info| info.parent) else {
    return chain;
  };
  while chain.len() < limit && pid > 0 {
    let Some(info) = map.get(&pid) else {
      break;
    };
    chain.push(Process {
      pid,
      name: info.name.clone(),
    });
    let is_own_parent = info.parent == pid;
    if is_own_parent {
      break;
    }
    pid = info.parent;
  }
  chain
}

pub fn is_alive(pid: u32) -> bool {
  // SAFETY: the handle is checked and closed.
  unsafe {
    let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
    if handle.is_null() {
      let exists_but_protected =
        std::io::Error::last_os_error().raw_os_error() == i32::try_from(ERROR_ACCESS_DENIED).ok();
      return exists_but_protected;
    }
    let mut code = 0u32;
    let ok = GetExitCodeProcess(handle, &raw mut code);
    CloseHandle(handle);
    ok != 0 && i32::try_from(code).is_ok_and(|code| code == STILL_ACTIVE)
  }
}
