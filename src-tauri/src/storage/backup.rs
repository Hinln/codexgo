use crate::codex::config::ConfigSnapshot;
use crate::codex::sessions;
use crate::errors::{AppError, AppResult};
use crate::security::hashes::sha256_file;
use crate::storage::state::{self, TransactionManifest, TransactionStage};
use crate::windows::atomic;
use chrono::{Local, Utc};
use fs2::available_space;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub struct OperationBackup {
    pub directory: PathBuf,
    pub manifest: TransactionManifest,
}

fn required_space(session_size: u64, config: &ConfigSnapshot) -> u64 {
    session_size
        .saturating_add(config.source.len() as u64)
        .saturating_mul(2)
        .saturating_add(16 * 1024 * 1024)
}

pub fn create(
    home: &Path,
    config: &ConfigSnapshot,
    target_provider: Option<&str>,
) -> AppResult<OperationBackup> {
    let root = state::backups_root(home);
    fs::create_dir_all(&root)
        .map_err(|error| AppError::io("BACKUP-001", "无法创建备份根目录", &error))?;
    let session_files = sessions::candidate_files(home)?;
    let session_size = sessions::estimated_size_for(&session_files)?;
    let available = available_space(&root)
        .map_err(|error| AppError::io("BACKUP-002", "无法检查磁盘可用空间", &error))?;
    let required = required_space(session_size, config);
    if available < required {
        return Err(AppError::new(
            "BACKUP-003",
            format!(
                "磁盘空间不足，需要至少 {} MB 可用空间。",
                (required / 1024 / 1024).max(1)
            ),
        ));
    }

    let transaction_id = Uuid::new_v4().to_string();
    let folder = format!(
        "{}-{}",
        Local::now().format("%Y%m%d-%H%M%S"),
        &transaction_id[..8]
    );
    let directory = root.join(folder);
    let data_directory = directory.join("data");
    fs::create_dir_all(&data_directory)
        .map_err(|error| AppError::io("BACKUP-004", "无法创建事务备份目录", &error))?;

    let config_backup = data_directory.join("config.toml");
    fs::copy(&config.path, &config_backup)
        .map_err(|error| AppError::io("BACKUP-005", "无法备份 config.toml", &error))?;
    let copied_hash = sha256_file(&config_backup)?;
    if copied_hash != config.hash {
        return Err(AppError::new(
            "BACKUP-006",
            "config.toml 备份哈希校验失败，未执行切换。",
        ));
    }
    let session_backups = sessions::backup_files(home, &directory, &session_files)?;
    let now = Utc::now().to_rfc3339();
    let manifest = TransactionManifest {
        version: 1,
        transaction_id,
        created_at: now.clone(),
        updated_at: now,
        stage: TransactionStage::BackupComplete,
        previous_provider: config.current_provider.clone(),
        target_provider: target_provider.map(ToOwned::to_owned),
        config_relative_path: "config.toml".to_string(),
        config_backup_relative_path: "data/config.toml".to_string(),
        config_before_sha256: config.hash.clone(),
        config_after_sha256: None,
        session_backups,
        session_migrations: Vec::new(),
        completed_at: None,
    };
    state::save_manifest(&directory, &manifest)?;
    Ok(OperationBackup {
        directory,
        manifest,
    })
}

impl OperationBackup {
    pub fn folder_name(&self) -> AppResult<String> {
        self.directory
            .file_name()
            .and_then(|value| value.to_str())
            .map(ToOwned::to_owned)
            .ok_or_else(|| AppError::new("BACKUP-007", "备份目录名称无效。"))
    }

    pub fn save(&self) -> AppResult<()> {
        state::save_manifest(&self.directory, &self.manifest)
    }

    pub fn set_stage(&mut self, stage: TransactionStage) -> AppResult<()> {
        self.manifest.set_stage(stage);
        self.save()
    }

    pub fn restore_full(&self, home: &Path) -> AppResult<()> {
        let config_backup = self
            .directory
            .join(&self.manifest.config_backup_relative_path);
        if sha256_file(&config_backup)? != self.manifest.config_before_sha256 {
            return Err(AppError::new(
                "BACKUP-008",
                "config.toml 备份哈希不一致，停止自动回滚。",
            ));
        }
        atomic::copy_atomic(&config_backup, &home.join("config.toml"))?;
        sessions::restore_full_backup(home, &self.directory, &self.manifest.session_backups)?;
        Ok(())
    }
}
