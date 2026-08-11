use crate::errors::{AppError, AppResult};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use uuid::Uuid;
use windows_sys::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

pub fn temporary_sibling(path: &Path) -> AppResult<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::new("FILE-001", "目标文件没有可用的父目录。"))?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("file");
    Ok(parent.join(format!(".{name}.switcher-{}.tmp", Uuid::new_v4())))
}

pub fn replace_file(source: &Path, destination: &Path) -> AppResult<()> {
    let source_wide = wide(source);
    let destination_wide = wide(destination);
    let result = unsafe {
        MoveFileExW(
            source_wide.as_ptr(),
            destination_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        return Err(AppError::io(
            "FILE-002",
            "Windows 原子替换文件失败",
            &std::io::Error::last_os_error(),
        ));
    }
    Ok(())
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> AppResult<()> {
    let temporary = temporary_sibling(path)?;
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| AppError::io("FILE-003", "无法创建临时文件", &error))?;
        file.write_all(bytes)
            .map_err(|error| AppError::io("FILE-004", "无法写入临时文件", &error))?;
        file.flush()
            .map_err(|error| AppError::io("FILE-005", "无法刷新临时文件", &error))?;
        file.sync_all()
            .map_err(|error| AppError::io("FILE-006", "无法同步临时文件", &error))?;
        drop(file);
        replace_file(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn copy_atomic(source: &Path, destination: &Path) -> AppResult<()> {
    let temporary = temporary_sibling(destination)?;
    let result = (|| {
        let mut input = File::open(source)
            .map_err(|error| AppError::io("FILE-007", "无法读取备份文件", &error))?;
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| AppError::io("FILE-008", "无法创建恢复临时文件", &error))?;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = input
                .read(&mut buffer)
                .map_err(|error| AppError::io("FILE-009", "无法读取备份数据", &error))?;
            if read == 0 {
                break;
            }
            output
                .write_all(&buffer[..read])
                .map_err(|error| AppError::io("FILE-010", "无法写入恢复数据", &error))?;
        }
        output
            .sync_all()
            .map_err(|error| AppError::io("FILE-011", "无法同步恢复文件", &error))?;
        drop(output);
        replace_file(&temporary, destination)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
