use crate::errors::{AppError, AppResult};
use crate::provider::MANAGED_ENV_KEYS;
use std::io::ErrorKind;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
};
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
use winreg::RegKey;
use zeroize::Zeroizing;

pub struct EnvironmentSnapshot {
    values: Vec<(&'static str, Option<Zeroizing<String>>)>,
}

fn environment_key(writable: bool) -> AppResult<RegKey> {
    let flags = if writable {
        KEY_READ | KEY_WRITE
    } else {
        KEY_READ
    };
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags("Environment", flags)
        .map_err(|error| AppError::io("ENV-001", "无法打开当前用户环境变量", &error))
}

pub fn read_user(name: &str) -> AppResult<Option<String>> {
    let key = environment_key(false)?;
    match key.get_value::<String, _>(name) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(AppError::io("ENV-002", "无法读取当前用户环境变量", &error)),
    }
}

fn broadcast_change() -> AppResult<()> {
    let mut setting: Vec<u16> = "Environment".encode_utf16().chain(Some(0)).collect();
    let mut result = 0_usize;
    let sent = unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            setting.as_mut_ptr() as isize,
            SMTO_ABORTIFHUNG,
            2_000,
            &mut result,
        )
    };
    if sent == 0 {
        return Err(AppError::io(
            "ENV-003",
            "环境变量已写入，但 Windows 变更通知发送失败",
            &std::io::Error::last_os_error(),
        ));
    }
    Ok(())
}

pub fn set_user(name: &str, value: &str) -> AppResult<()> {
    environment_key(true)?
        .set_value(name, &value)
        .map_err(|error| AppError::io("ENV-004", "无法写入当前用户环境变量", &error))?;
    std::env::set_var(name, value);
    broadcast_change()
}

pub fn delete_user(name: &str) -> AppResult<()> {
    let key = environment_key(true)?;
    match key.delete_value(name) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(AppError::io("ENV-005", "无法删除当前用户环境变量", &error));
        }
    }
    std::env::remove_var(name);
    broadcast_change()
}

pub fn snapshot_managed() -> AppResult<EnvironmentSnapshot> {
    let mut values = Vec::with_capacity(MANAGED_ENV_KEYS.len());
    for name in MANAGED_ENV_KEYS {
        values.push((name, read_user(name)?.map(Zeroizing::new)));
    }
    Ok(EnvironmentSnapshot { values })
}

pub fn restore(snapshot: &EnvironmentSnapshot) -> AppResult<()> {
    for (name, value) in &snapshot.values {
        match value {
            Some(value) => set_user(name, value.as_str())?,
            None => delete_user(name)?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_name_list_is_fixed() {
        assert_eq!(MANAGED_ENV_KEYS, ["VEXLUNE_HUB_API_KEY"]);
    }
}
