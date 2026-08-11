use crate::errors::{AppError, AppResult};
use std::collections::HashSet;
use std::ffi::OsString;
use std::mem::size_of;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};
use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{CloseHandle, HWND, INVALID_HANDLE_VALUE, LPARAM};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_TERMINATE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, PostMessageW, WM_CLOSE,
};

#[derive(Debug, Clone, Default)]
pub struct CodexProcessSnapshot {
    pub was_running: bool,
    pub executable: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct DesktopProcess {
    pid: u32,
    executable: Option<PathBuf>,
}

unsafe extern "system" fn collect_window(window: HWND, parameter: LPARAM) -> BOOL {
    let windows = &mut *(parameter as *mut Vec<(HWND, u32)>);
    let mut pid = 0_u32;
    GetWindowThreadProcessId(window, &mut pid);
    if pid != 0 {
        windows.push((window, pid));
    }
    1
}

fn top_level_windows() -> Vec<(HWND, u32)> {
    let mut windows = Vec::new();
    unsafe {
        EnumWindows(
            Some(collect_window),
            &mut windows as *mut Vec<(HWND, u32)> as LPARAM,
        );
    }
    windows
}

fn executable_path(pid: u32) -> Option<PathBuf> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let mut buffer = vec![0_u16; 32_768];
    let mut length = buffer.len() as u32;
    let succeeded =
        unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length) } != 0;
    unsafe {
        CloseHandle(handle);
    }
    succeeded.then(|| PathBuf::from(OsString::from_wide(&buffer[..length as usize])))
}

fn is_packaged_codex_path(path: &Path) -> bool {
    let value = path.to_string_lossy().to_ascii_lowercase();
    (value.contains("\\openai.codex_") || value.contains("\\openai.chatgpt_"))
        && !value.ends_with("\\resources\\codex.exe")
}

fn is_desktop_process(name: &str, path: Option<&Path>, has_top_level_window: bool) -> bool {
    match name {
        "chatgpt.exe" => path.map(is_packaged_codex_path).unwrap_or(true) || has_top_level_window,
        "codex.exe" => path.map(is_packaged_codex_path).unwrap_or(false) || has_top_level_window,
        _ => false,
    }
}

fn desktop_processes() -> AppResult<Vec<DesktopProcess>> {
    let windows = top_level_windows();
    let window_pids = windows.iter().map(|(_, pid)| *pid).collect::<HashSet<_>>();
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(AppError::io(
            "PROC-001",
            "无法检查 Codex 进程状态",
            &std::io::Error::last_os_error(),
        ));
    }

    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..unsafe { std::mem::zeroed() }
    };
    let mut processes = Vec::new();
    let mut has_entry = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while has_entry {
        let length = entry
            .szExeFile
            .iter()
            .position(|character| *character == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..length]).to_ascii_lowercase();
        if matches!(name.as_str(), "codex.exe" | "chatgpt.exe") {
            let executable = executable_path(entry.th32ProcessID);
            if is_desktop_process(
                name.as_str(),
                executable.as_deref(),
                window_pids.contains(&entry.th32ProcessID),
            ) {
                processes.push(DesktopProcess {
                    pid: entry.th32ProcessID,
                    executable,
                });
            }
        }
        has_entry = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    unsafe {
        CloseHandle(snapshot);
    }
    Ok(processes)
}

pub fn snapshot() -> AppResult<CodexProcessSnapshot> {
    let processes = desktop_processes()?;
    Ok(CodexProcessSnapshot {
        was_running: !processes.is_empty(),
        executable: processes
            .iter()
            .filter_map(|process| process.executable.clone())
            .find(|path| {
                path.file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|name| name.eq_ignore_ascii_case("ChatGPT.exe"))
            })
            .or_else(|| {
                processes
                    .iter()
                    .filter_map(|process| process.executable.clone())
                    .next()
            }),
    })
}

pub fn is_codex_running() -> AppResult<bool> {
    desktop_processes().map(|processes| !processes.is_empty())
}

fn request_graceful_close(processes: &[DesktopProcess]) {
    let target_pids = processes
        .iter()
        .map(|process| process.pid)
        .collect::<HashSet<_>>();
    for (window, pid) in top_level_windows() {
        if target_pids.contains(&pid) {
            unsafe {
                PostMessageW(window, WM_CLOSE, 0, 0);
            }
        }
    }
}

fn force_terminate(processes: &[DesktopProcess]) {
    for process in processes {
        let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, process.pid) };
        if handle.is_null() {
            continue;
        }
        unsafe {
            TerminateProcess(handle, 1);
            CloseHandle(handle);
        }
    }
}

pub fn close_codex(timeout: Duration) -> AppResult<()> {
    let processes = desktop_processes()?;
    if processes.is_empty() {
        return Ok(());
    }
    request_graceful_close(&processes);
    let deadline = Instant::now() + timeout;
    let force_deadline = deadline
        .checked_sub(Duration::from_millis(500))
        .unwrap_or(deadline);
    loop {
        if desktop_processes()?.is_empty() {
            return Ok(());
        }
        if Instant::now() >= force_deadline {
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }

    let remaining = desktop_processes()?;
    force_terminate(&remaining);
    loop {
        if desktop_processes()?.is_empty() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(AppError::new(
                "PROC-002",
                "Codex 未能在 5 秒内关闭，已停止本次操作。",
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

pub fn wait_for_codex(timeout: Duration) -> AppResult<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        if is_codex_running()? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(Duration::from_millis(250));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excludes_cli_and_resource_codex_processes() {
        assert!(!is_desktop_process(
            "codex.exe",
            Some(Path::new(
                r"C:\Users\User\AppData\Local\OpenAI\Codex\bin\hash\codex.exe"
            )),
            false,
        ));
        assert!(!is_desktop_process(
            "codex.exe",
            Some(Path::new(
                r"C:\Program Files\WindowsApps\OpenAI.Codex_1.0_x64__id\app\resources\codex.exe"
            )),
            false,
        ));
    }

    #[test]
    fn detects_packaged_desktop_hosts() {
        assert!(is_desktop_process(
            "chatgpt.exe",
            Some(Path::new(
                r"C:\Program Files\WindowsApps\OpenAI.Codex_1.0_x64__id\app\ChatGPT.exe"
            )),
            false,
        ));
        assert!(is_desktop_process(
            "codex.exe",
            Some(Path::new(
                r"C:\Program Files\WindowsApps\OpenAI.Codex_1.0_x64__id\app\Codex.exe"
            )),
            false,
        ));
    }
}
