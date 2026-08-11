use crate::codex::config;
use crate::errors::{AppError, AppResult};
use crate::provider::{from_config_id, GENERIC_ENV_KEY, GENERIC_PROVIDER_ID};
use crate::security::hashes::sha256_bytes;
use crate::storage::state;
use crate::windows::{environment, process};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectionStatus {
    pub codex_home: String,
    pub codex_detected: bool,
    pub auth_present: bool,
    pub config_present: bool,
    pub sessions_present: bool,
    pub account_connected: bool,
    pub current_provider: Option<String>,
    pub current_model: Option<String>,
    pub current_route: String,
    pub config_status: String,
    pub provider_configured: bool,
    pub provider_requires_openai_auth: Option<bool>,
    pub api_key_stored: bool,
    pub key_validation_state: String,
    pub codex_running: bool,
    pub backup_count: usize,
    pub recovery_pending: bool,
}

pub fn resolve_codex_home() -> AppResult<PathBuf> {
    if let Some(path) = std::env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = environment::read_user("CODEX_HOME")?.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let profile = std::env::var_os("USERPROFILE")
        .ok_or_else(|| AppError::new("CODEX-001", "无法确定当前 Windows 用户目录。"))?;
    Ok(PathBuf::from(profile).join(".codex"))
}

pub fn require_codex_home() -> AppResult<PathBuf> {
    let home = resolve_codex_home()?;
    if !home.is_dir() {
        return Err(AppError::new(
            "CODEX-002",
            format!("未找到 Codex 配置目录：{}", home.display()),
        ));
    }
    Ok(home)
}

fn backup_count(home: &Path) -> usize {
    let root = home.join("switcher-backups");
    fs::read_dir(root)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().is_dir())
                .count()
        })
        .unwrap_or(0)
}

pub fn detect() -> AppResult<DetectionStatus> {
    let home = resolve_codex_home()?;
    let codex_detected = home.is_dir();
    let config_path = home.join("config.toml");
    let auth_present = home.join("auth.json").is_file();
    let config_present = config_path.is_file();
    let sessions_present =
        home.join("sessions").is_dir() || home.join("archived_sessions").is_dir();

    let (config_status, current_provider, current_model, provider_status) = if !config_present {
        (
            "missing".to_string(),
            None,
            None,
            config::ProviderConfigStatus {
                configured: false,
                requires_openai_auth: None,
            },
        )
    } else if fs::metadata(&config_path)
        .map(|metadata| metadata.permissions().readonly())
        .unwrap_or(false)
    {
        match config::load(&config_path) {
            Ok(snapshot) => {
                let provider_status = config::inspect_managed_provider(&snapshot);
                (
                    "readonly".to_string(),
                    snapshot.current_provider,
                    snapshot.model,
                    provider_status,
                )
            }
            Err(_) => (
                "readonly".to_string(),
                None,
                None,
                config::ProviderConfigStatus {
                    configured: false,
                    requires_openai_auth: None,
                },
            ),
        }
    } else {
        match config::load(&config_path) {
            Ok(snapshot) => {
                let provider_status = config::inspect_managed_provider(&snapshot);
                (
                    "normal".to_string(),
                    snapshot.current_provider,
                    snapshot.model,
                    provider_status,
                )
            }
            Err(_) => (
                "invalid".to_string(),
                None,
                None,
                config::ProviderConfigStatus {
                    configured: false,
                    requires_openai_auth: None,
                },
            ),
        }
    };

    let current_route = match current_provider.as_deref() {
        Some(value) if value == GENERIC_PROVIDER_ID => "generic".to_string(),
        Some(value) if from_config_id(value).is_some() => "custom".to_string(),
        Some("openai") | None => "official".to_string(),
        Some(_) => "custom".to_string(),
    };
    let stored_key = environment::read_user(GENERIC_ENV_KEY)?.map(Zeroizing::new);
    let api_key_stored = stored_key
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty());
    let switcher_state = state::load_switcher_state(&home)?;
    let key_validation_state = match (stored_key.as_ref(), switcher_state.as_ref()) {
        (Some(key), Some(saved))
            if saved.key_verified_at.is_some()
                && saved.key_fingerprint.as_deref()
                    == Some(sha256_bytes(key.as_bytes()).as_str()) =>
        {
            "verified"
        }
        (Some(_), _) => "stored",
        (None, _) => "missing",
    }
    .to_string();

    Ok(DetectionStatus {
        codex_home: home.to_string_lossy().into_owned(),
        codex_detected,
        auth_present,
        config_present,
        sessions_present,
        account_connected: auth_present,
        current_provider,
        current_model,
        current_route,
        config_status,
        provider_configured: provider_status.configured,
        provider_requires_openai_auth: provider_status.requires_openai_auth,
        api_key_stored,
        key_validation_state,
        codex_running: process::is_codex_running().unwrap_or(false),
        backup_count: backup_count(&home),
        recovery_pending: state::has_incomplete_transaction(&home),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn process_codex_home_has_priority() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let old = std::env::var_os("CODEX_HOME");
        std::env::set_var("CODEX_HOME", temp.path());
        assert_eq!(resolve_codex_home().unwrap(), temp.path());
        match old {
            Some(value) => std::env::set_var("CODEX_HOME", value),
            None => std::env::remove_var("CODEX_HOME"),
        }
    }
}
