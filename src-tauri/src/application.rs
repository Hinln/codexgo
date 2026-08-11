use crate::api::validator::ValidationResult;
use crate::codex::{config, sessions};
use crate::errors::{AppError, AppResult};
use crate::provider::{from_config_id, ProviderDefinition, MANAGED_ENV_KEYS};
use crate::security::hashes::{sha256_bytes, sha256_file};
use crate::storage::backup::{self, OperationBackup};
use crate::storage::state::{self, SwitcherState, TransactionStage};
use crate::windows::environment::{self, EnvironmentSnapshot};
use chrono::{Local, Utc};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationResult {
    pub success: bool,
    pub route: String,
    pub base_url: Option<String>,
    pub completed_at: String,
    pub message: String,
    pub detail: String,
    pub backup_path: Option<String>,
    pub migration_count: Option<usize>,
    pub error_code: Option<String>,
    pub rolled_back: bool,
    pub config_changed: bool,
    pub http_status: Option<u16>,
    pub request_elapsed_ms: Option<u64>,
    pub codex_restored: Option<bool>,
}

fn completed_at() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn rollback_error(original: AppError, rollback: AppResult<()>, changed: bool) -> AppError {
    match rollback {
        Ok(()) => original.changed(changed).rolled_back(true),
        Err(rollback_error) => AppError::new(
            "TXN-ROLLBACK",
            format!(
                "{}；自动回滚未能完整完成：{}。请使用本次备份手动恢复。",
                original.message, rollback_error.message
            ),
        )
        .changed(changed)
        .rolled_back(false),
    }
}

pub struct PreparedSwitch {
    prior_state: Option<SwitcherState>,
    operation_backup: OperationBackup,
    environment_before: EnvironmentSnapshot,
}

impl PreparedSwitch {
    pub fn cancel(mut self) -> AppResult<()> {
        self.operation_backup
            .set_stage(TransactionStage::RolledBack)
    }
}

pub fn prepare_switch(
    home: &Path,
    initial_config: &config::ConfigSnapshot,
    provider: &ProviderDefinition,
) -> AppResult<PreparedSwitch> {
    let prior_state = state::load_switcher_state(home)?;
    if from_config_id(
        initial_config
            .current_provider
            .as_deref()
            .unwrap_or_default(),
    )
    .is_some()
        && prior_state.is_none()
    {
        return Err(AppError::new(
            "STATE-006",
            "当前配置已使用本工具管理的 Provider，但缺少原始恢复状态。请先从备份确认原 Provider。",
        ));
    }

    let mut operation_backup =
        backup::create(home, initial_config, Some(provider.config_id.as_str()))?;
    let environment_before = match environment::snapshot_managed() {
        Ok(value) => value,
        Err(error) => {
            let _ = operation_backup.set_stage(TransactionStage::RolledBack);
            return Err(error);
        }
    };
    Ok(PreparedSwitch {
        prior_state,
        operation_backup,
        environment_before,
    })
}

pub fn apply_prepared_switch(
    home: &Path,
    initial_config: &config::ConfigSnapshot,
    provider: &ProviderDefinition,
    validation: &ValidationResult,
    api_key: &str,
    prepared: PreparedSwitch,
    mut progress: impl FnMut(usize),
) -> AppResult<OperationResult> {
    let PreparedSwitch {
        prior_state,
        mut operation_backup,
        environment_before,
    } = prepared;
    let mut changed = false;

    let operation = (|| {
        progress(4);
        let rendered = config::render_for_provider(initial_config, provider, &validation.base_url)?;
        config::commit(&initial_config.path, &rendered)?;
        changed = true;
        let committed_config = config::load(&initial_config.path)?;
        operation_backup.manifest.config_after_sha256 = Some(committed_config.hash.clone());
        operation_backup.set_stage(TransactionStage::ConfigCommitted)?;

        let migration = sessions::migrate_all(home, provider.config_id.as_str())?;
        operation_backup.manifest.session_migrations = migration.records.clone();
        operation_backup.set_stage(TransactionStage::SessionsCommitted)?;

        progress(5);
        environment::set_user(provider.env_key.as_str(), api_key)?;
        operation_backup.set_stage(TransactionStage::AuthenticationCommitted)?;

        progress(6);
        let verified = config::load(&initial_config.path)?;
        if verified.current_provider.as_deref() != Some(provider.config_id.as_str()) {
            return Err(AppError::new("TXN-001", "写入后的当前 Provider 校验失败。").changed(true));
        }
        if verified.model.as_deref() != Some(provider.model.as_str()) {
            return Err(AppError::new("TXN-006", "写入后的目标模型校验失败。").changed(true));
        }
        for record in &operation_backup.manifest.session_migrations {
            let path = home.join(record.relative_path.replace('/', "\\"));
            if sha256_file(&path)? != record.after_sha256 {
                return Err(AppError::new(
                    "TXN-002",
                    format!("会话迁移后哈希校验失败：{}", record.relative_path),
                )
                .changed(true));
            }
        }

        for other in MANAGED_ENV_KEYS {
            if other != provider.env_key.as_str() {
                environment::delete_user(other)?;
            }
        }

        let now = Utc::now().to_rfc3339();
        let mut switcher_state = match prior_state.clone() {
            Some(value) => value,
            None => SwitcherState {
                version: 2,
                baseline_folder: operation_backup.folder_name()?,
                original_provider: initial_config.current_provider.clone(),
                active_provider: provider.config_id.to_string(),
                key_fingerprint: None,
                key_verified_at: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        };
        switcher_state.version = 2;
        switcher_state.active_provider = provider.config_id.to_string();
        switcher_state.key_fingerprint = Some(sha256_bytes(api_key.as_bytes()));
        switcher_state.key_verified_at = Some(now.clone());
        switcher_state.updated_at = now;
        state::save_switcher_state(home, &switcher_state)?;
        operation_backup.set_stage(TransactionStage::Verified)?;
        operation_backup.set_stage(TransactionStage::Completed)?;

        Ok::<OperationResult, AppError>(OperationResult {
            success: true,
            route: provider.display_name.clone(),
            base_url: Some(validation.base_url.clone()),
            completed_at: completed_at(),
            message: format!("已成功切换至 {}。", provider.display_name),
            detail: "配置已完成，正在重新启动 Codex。".to_string(),
            backup_path: Some(operation_backup.directory.to_string_lossy().into_owned()),
            migration_count: Some(migration.total_changes),
            error_code: None,
            rolled_back: false,
            config_changed: true,
            http_status: Some(validation.http_status),
            request_elapsed_ms: Some(validation.request_elapsed_ms),
            codex_restored: None,
        })
    })();

    match operation {
        Ok(result) => Ok(result),
        Err(error) => {
            let rollback_result = (|| {
                operation_backup.restore_full(home)?;
                environment::restore(&environment_before)?;
                match &prior_state {
                    Some(value) => state::save_switcher_state(home, value)?,
                    None => state::remove_switcher_state(home)?,
                }
                operation_backup.set_stage(TransactionStage::RolledBack)?;
                Ok(())
            })();
            if rollback_result.is_err() {
                let _ = operation_backup.set_stage(TransactionStage::RollbackFailed);
            }
            Err(rollback_error(error, rollback_result, changed))
        }
    }
}

#[cfg(test)]
pub fn apply_switch(
    home: &Path,
    initial_config: &config::ConfigSnapshot,
    provider: &ProviderDefinition,
    validation: &ValidationResult,
    api_key: &str,
    mut progress: impl FnMut(usize),
) -> AppResult<OperationResult> {
    progress(2);
    let prepared = prepare_switch(home, initial_config, provider)?;
    apply_prepared_switch(
        home,
        initial_config,
        provider,
        validation,
        api_key,
        prepared,
        progress,
    )
}

pub fn restore_official(
    home: &Path,
    mut progress: impl FnMut(usize),
) -> AppResult<OperationResult> {
    let current_config = config::load(&home.join("config.toml"))?;
    config::preflight_writable(&current_config)?;
    let switcher_state = match state::load_switcher_state(home)? {
        Some(value) => value,
        None => {
            if current_config
                .current_provider
                .as_deref()
                .and_then(from_config_id)
                .is_some()
            {
                return Err(AppError::new(
                    "STATE-007",
                    "缺少首次切换前的恢复状态，无法安全推断原 Provider。",
                ));
            }
            let current_route = current_config
                .current_provider
                .clone()
                .unwrap_or_else(|| "官方 Codex".to_string());
            return Ok(OperationResult {
                success: true,
                route: current_route,
                base_url: None,
                completed_at: completed_at(),
                message: "当前已经是非托管 Provider。".to_string(),
                detail: "已保存的自定义 API Key 保持不变。".to_string(),
                backup_path: None,
                migration_count: Some(0),
                error_code: None,
                rolled_back: false,
                config_changed: false,
                http_status: None,
                request_elapsed_ms: None,
                codex_restored: None,
            });
        }
    };

    progress(2);
    let mut rollback_backup = backup::create(home, &current_config, None)?;
    let environment_before = match environment::snapshot_managed() {
        Ok(value) => value,
        Err(error) => {
            let _ = rollback_backup.set_stage(TransactionStage::RolledBack);
            return Err(error);
        }
    };

    progress(3);
    let baseline_root = state::backups_root(home).join(&switcher_state.baseline_folder);
    let baseline_config_path = baseline_root.join("data").join("config.toml");
    let original_config = match config::load(&baseline_config_path) {
        Ok(value) => value,
        Err(error) => {
            let _ = rollback_backup.set_stage(TransactionStage::RolledBack);
            return Err(AppError::new(
                "STATE-008",
                format!("首次切换前的配置备份不可用：{}", error.message),
            ));
        }
    };
    let mut changed = false;

    let operation = (|| {
        progress(4);
        let restored = config::render_official_restore(&current_config, &original_config)?;
        config::commit(&current_config.path, &restored)?;
        changed = true;
        rollback_backup.set_stage(TransactionStage::ConfigCommitted)?;

        let fallback = switcher_state
            .original_provider
            .as_deref()
            .unwrap_or("openai");
        let migration = sessions::restore_from_baseline(home, &baseline_root, fallback)?;
        rollback_backup.manifest.session_migrations = migration.records.clone();
        rollback_backup.set_stage(TransactionStage::SessionsCommitted)?;

        progress(5);
        progress(6);
        let verified = config::load(&current_config.path)?;
        if verified.current_provider != switcher_state.original_provider {
            return Err(
                AppError::new("TXN-003", "恢复后的 Provider 与首次切换前状态不一致。")
                    .changed(true),
            );
        }
        state::remove_switcher_state(home)?;
        rollback_backup.manifest.config_after_sha256 = Some(verified.hash);
        rollback_backup.set_stage(TransactionStage::Verified)?;
        rollback_backup.set_stage(TransactionStage::Completed)?;
        let restored_route = switcher_state
            .original_provider
            .clone()
            .unwrap_or_else(|| "官方 Codex".to_string());

        Ok::<OperationResult, AppError>(OperationResult {
            success: true,
            route: restored_route,
            base_url: None,
            completed_at: completed_at(),
            message: "已恢复切换前的 Codex Provider。".to_string(),
            detail: "自定义 API Key 已保留，可在下次切换时继续使用。".to_string(),
            backup_path: Some(rollback_backup.directory.to_string_lossy().into_owned()),
            migration_count: Some(migration.total_changes),
            error_code: None,
            rolled_back: false,
            config_changed: true,
            http_status: None,
            request_elapsed_ms: None,
            codex_restored: None,
        })
    })();

    match operation {
        Ok(result) => Ok(result),
        Err(error) => {
            let rollback_result = (|| {
                rollback_backup.restore_full(home)?;
                environment::restore(&environment_before)?;
                state::save_switcher_state(home, &switcher_state)?;
                rollback_backup.set_stage(TransactionStage::RolledBack)?;
                Ok(())
            })();
            if rollback_result.is_err() {
                let _ = rollback_backup.set_stage(TransactionStage::RollbackFailed);
            }
            Err(rollback_error(error, rollback_result, changed))
        }
    }
}
