use crate::errors::{AppError, AppResult};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

const CODEX_APP_ID: &str = r"shell:AppsFolder\OpenAI.Codex_2p2nqsd0c76g0!App";

fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

fn execute(file: &std::ffi::OsStr, parameters: Option<&std::ffi::OsStr>) -> Result<(), isize> {
    let operation: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
    let file = wide(file);
    let parameters = parameters.map(wide);
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            parameters
                .as_ref()
                .map_or(std::ptr::null(), |value| value.as_ptr()),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if result as isize <= 32 {
        return Err(result as isize);
    }
    Ok(())
}

pub fn open_directory(path: &Path) -> AppResult<()> {
    execute(path.as_os_str(), None)
        .map_err(|_| AppError::new("SHELL-001", "Windows 无法打开所选目录。"))
}

pub fn open_url(url: &str) -> AppResult<()> {
    if url != "https://hub.vexlune.com" {
        return Err(AppError::new("SHELL-004", "不允许打开未授权的网址。"));
    }
    execute(std::ffi::OsStr::new(url), None)
        .map_err(|_| AppError::new("SHELL-005", "Windows 无法打开 Vexlune Hub。"))
}

pub fn launch_codex_package() -> AppResult<()> {
    execute(
        std::ffi::OsStr::new("explorer.exe"),
        Some(std::ffi::OsStr::new(CODEX_APP_ID)),
    )
    .map_err(|_| AppError::new("SHELL-002", "Windows 无法启动 Codex Desktop。"))
}

pub fn launch_executable(path: &Path) -> AppResult<()> {
    execute(path.as_os_str(), None)
        .map_err(|_| AppError::new("SHELL-003", "Codex Desktop 可执行文件启动失败。"))
}
