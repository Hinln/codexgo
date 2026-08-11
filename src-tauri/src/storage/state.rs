use crate::codex::sessions::{SessionBackupRecord, SessionFileRecord};
use crate::errors::{AppError, AppResult};
use crate::windows::atomic;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const STATE_FILE: &str = "switcher-state.json";
pub const TRANSACTION_FILE: &str = "transaction.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitcherState {
    pub version: u32,
    pub baseline_folder: String,
    pub original_provider: Option<String>,
    pub active_provider: String,
    #[serde(default)]
    pub key_fingerprint: Option<String>,
    #[serde(default)]
    pub key_verified_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransactionStage {
    Preflight,
    BackupComplete,
    KeyCommitted,
    AuthenticationCommitted,
    ConfigCommitted,
    SessionsCommitted,
    Verified,
    Completed,
    RolledBack,
    RollbackFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionManifest {
    pub version: u32,
    pub transaction_id: String,
    pub created_at: String,
    pub updated_at: String,
    pub stage: TransactionStage,
    pub previous_provider: Option<String>,
    pub target_provider: Option<String>,
    pub config_relative_path: String,
    pub config_backup_relative_path: String,
    pub config_before_sha256: String,
    pub config_after_sha256: Option<String>,
    pub session_backups: Vec<SessionBackupRecord>,
    pub session_migrations: Vec<SessionFileRecord>,
    pub completed_at: Option<String>,
}

impl TransactionManifest {
    pub fn set_stage(&mut self, stage: TransactionStage) {
        self.stage = stage;
        self.updated_at = Utc::now().to_rfc3339();
        if self.stage == TransactionStage::Completed {
            self.completed_at = Some(self.updated_at.clone());
        }
    }
}

pub fn backups_root(home: &Path) -> PathBuf {
    home.join("switcher-backups")
}

pub fn state_path(home: &Path) -> PathBuf {
    backups_root(home).join(STATE_FILE)
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> AppResult<()> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| AppError::new("STATE-001", format!("状态序列化失败：{error}")))?;
    atomic::write_atomic(path, &bytes)
}

pub fn save_switcher_state(home: &Path, value: &SwitcherState) -> AppResult<()> {
    fs::create_dir_all(backups_root(home))
        .map_err(|error| AppError::io("STATE-002", "无法创建状态目录", &error))?;
    write_json(&state_path(home), value)
}

pub fn load_switcher_state(home: &Path) -> AppResult<Option<SwitcherState>> {
    let path = state_path(home);
    if !path.is_file() {
        return Ok(None);
    }
    let bytes =
        fs::read(&path).map_err(|error| AppError::io("STATE-003", "无法读取切换状态", &error))?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| AppError::new("STATE-004", format!("切换状态已损坏：{error}")))
}

pub fn remove_switcher_state(home: &Path) -> AppResult<()> {
    let path = state_path(home);
    if path.exists() {
        fs::remove_file(path)
            .map_err(|error| AppError::io("STATE-005", "无法清除切换状态", &error))?;
    }
    Ok(())
}

pub fn save_manifest(directory: &Path, manifest: &TransactionManifest) -> AppResult<()> {
    write_json(&directory.join(TRANSACTION_FILE), manifest)
}

pub fn has_incomplete_transaction(home: &Path) -> bool {
    let root = backups_root(home);
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path().join(TRANSACTION_FILE);
        if !path.is_file() {
            continue;
        }
        let Ok(bytes) = fs::read(path) else {
            return true;
        };
        let Ok(manifest) = serde_json::from_slice::<TransactionManifest>(&bytes) else {
            return true;
        };
        if !matches!(
            manifest.stage,
            TransactionStage::Completed | TransactionStage::RolledBack
        ) {
            return true;
        }
    }
    false
}
