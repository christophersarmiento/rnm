//! Windows implementation: one Toolhelp snapshot per call, plus registry
//! access.

#![allow(unsafe_code)]

use std::collections::HashMap;

use windows_sys::Win32::Foundation::{
  CloseHandle, ERROR_ACCESS_DENIED, ERROR_SUCCESS, INVALID_HANDLE_VALUE, STILL_ACTIVE,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
  CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Registry::{
  HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_EXPAND_SZ, REG_ROUTINE_FLAGS, REG_SZ,
  REG_VALUE_TYPE, RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_MULTI_SZ, RRF_RT_REG_SZ,
  RegGetValueW, RegSetKeyValueW,
};
use windows_sys::Win32::System::SystemInformation::{IMAGE_FILE_MACHINE, IMAGE_FILE_MACHINE_ARM64};
use windows_sys::Win32::System::Threading::{
  GetCurrentProcess, GetExitCodeProcess, IsWow64Process2, OpenProcess,
  PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
  HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
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

/// An x64 (or x86) process on an Arm64 machine runs under emulation.
pub fn is_translated() -> bool {
  let mut process_machine: IMAGE_FILE_MACHINE = 0;
  let mut native_machine: IMAGE_FILE_MACHINE = 0;
  // SAFETY: the pseudo-handle from GetCurrentProcess needs no closing; both
  // out-pointers are valid for the call.
  let ok = unsafe {
    IsWow64Process2(
      GetCurrentProcess(),
      &raw mut process_machine,
      &raw mut native_machine,
    )
  };
  ok != 0 && native_machine == IMAGE_FILE_MACHINE_ARM64 && !cfg!(target_arch = "aarch64")
}

/// A registry hive for [`registry_string`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hive {
  CurrentUser,
  LocalMachine,
}

/// The `REG_SZ` value `name` under `subkey`, or `None` when the key or value
/// is missing or not a string.
#[must_use]
pub fn registry_string(hive: Hive, subkey: &str, name: &str) -> Option<String> {
  let (_, units) = registry_value(hive, subkey, name, RRF_RT_REG_SZ)?;
  Some(String::from_utf16_lossy(until_nul(&units)))
}

/// The `REG_MULTI_SZ` value `name` under `subkey`, one string per entry.
#[must_use]
pub fn registry_strings(hive: Hive, subkey: &str, name: &str) -> Option<Vec<String>> {
  let (_, units) = registry_value(hive, subkey, name, RRF_RT_REG_MULTI_SZ)?;
  Some(
    units
      .split(|&unit| unit == 0)
      .filter(|entry| !entry.is_empty())
      .map(String::from_utf16_lossy)
      .collect(),
  )
}

/// A registry string as stored, `%VARIABLES%` unexpanded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawRegistryString {
  pub value: String,
  /// `REG_EXPAND_SZ` (as opposed to `REG_SZ`).
  pub expandable: bool,
}

/// The `REG_SZ` or `REG_EXPAND_SZ` value `name` under `subkey`, without
/// expanding `%VARIABLES%`, so it can be written back unchanged.
#[must_use]
pub fn registry_raw_string(hive: Hive, subkey: &str, name: &str) -> Option<RawRegistryString> {
  let (kind, units) = registry_value(
    hive,
    subkey,
    name,
    RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND,
  )?;
  Some(RawRegistryString {
    value: String::from_utf16_lossy(until_nul(&units)),
    expandable: kind == REG_EXPAND_SZ,
  })
}

/// Writes a `REG_SZ` or `REG_EXPAND_SZ` value, creating the key if needed.
///
/// # Errors
///
/// The Win32 error, for example when access is denied.
pub fn set_registry_string(
  hive: Hive,
  subkey: &str,
  name: &str,
  value: &RawRegistryString,
) -> std::io::Result<()> {
  let subkey = wide(subkey);
  let name = wide(name);
  let data = wide(&value.value);
  let kind = if value.expandable {
    REG_EXPAND_SZ
  } else {
    REG_SZ
  };
  let bytes = u32::try_from(data.len() * 2)
    .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
  // SAFETY: the strings are NUL-terminated and `data` holds `bytes` bytes.
  let status = unsafe {
    RegSetKeyValueW(
      hive.key(),
      subkey.as_ptr(),
      name.as_ptr(),
      kind,
      data.as_ptr().cast(),
      bytes,
    )
  };
  if status == ERROR_SUCCESS {
    Ok(())
  } else {
    Err(std::io::Error::from_raw_os_error(
      i32::try_from(status).unwrap_or(i32::MAX),
    ))
  }
}

/// How long to wait for each window to handle the broadcast.
const BROADCAST_TIMEOUT_MS: u32 = 5000;

/// Tells running programs (Explorer, and through it new GUI apps) that the
/// user's environment variables changed, as the System Properties dialog
/// does.
pub fn broadcast_environment_change() {
  let area = wide("Environment");
  let mut result = 0usize;
  // SAFETY: `area` is NUL-terminated and outlives the call; the result
  // pointer is valid.
  unsafe {
    SendMessageTimeoutW(
      HWND_BROADCAST,
      WM_SETTINGCHANGE,
      0,
      area.as_ptr() as isize,
      SMTO_ABORTIFHUNG,
      BROADCAST_TIMEOUT_MS,
      &raw mut result,
    );
  }
}

impl Hive {
  fn key(self) -> HKEY {
    match self {
      | Self::CurrentUser => HKEY_CURRENT_USER,
      | Self::LocalMachine => HKEY_LOCAL_MACHINE,
    }
  }
}

/// The value's type and its data as UTF-16 units.
fn registry_value(
  hive: Hive,
  subkey: &str,
  name: &str,
  flags: REG_ROUTINE_FLAGS,
) -> Option<(REG_VALUE_TYPE, Vec<u16>)> {
  let subkey = wide(subkey);
  let name = wide(name);
  let mut size = 0u32;
  let mut kind: REG_VALUE_TYPE = 0;
  // SAFETY: both strings are NUL-terminated; a null buffer asks for the
  // size in bytes, including the terminating NUL.
  let status = unsafe {
    RegGetValueW(
      hive.key(),
      subkey.as_ptr(),
      name.as_ptr(),
      flags,
      &raw mut kind,
      std::ptr::null_mut(),
      &raw mut size,
    )
  };
  if status != ERROR_SUCCESS {
    return None;
  }
  let mut buffer = vec![0u16; usize::try_from(size).ok()?.div_ceil(2)];
  // SAFETY: `buffer` holds `size` bytes, as `size` tells the call.
  let status = unsafe {
    RegGetValueW(
      hive.key(),
      subkey.as_ptr(),
      name.as_ptr(),
      flags,
      &raw mut kind,
      buffer.as_mut_ptr().cast(),
      &raw mut size,
    )
  };
  if status != ERROR_SUCCESS {
    return None;
  }
  buffer.truncate(usize::try_from(size).ok()?.div_ceil(2));
  Some((kind, buffer))
}

fn until_nul(units: &[u16]) -> &[u16] {
  let len = units.iter().position(|&c| c == 0).unwrap_or(units.len());
  &units[..len]
}

fn wide(text: &str) -> Vec<u16> {
  text.encode_utf16().chain(std::iter::once(0)).collect()
}
