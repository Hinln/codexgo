use crate::api::validator;
use crate::application::{self, OperationResult};
use crate::codex::{config, locator};
use crate::errors::{AppError, ErrorPayload};
use crate::provider::{generic_provider, GENERIC_ENV_KEY, GENERIC_PROVIDER_ID};
use crate::storage::{audit, state};
use crate::windows::{environment, process, shell};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

#[derive(Default)]
pub struct ApplicationState {
    operation_lock: Mutex<()>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchRequest {
    pub api_url: String,
    pub api_key: String,
    pub model: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelListRequest {
    pub api_url: String,
    pub api_key: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressEvent {
    step: &'static str,
    index: usize,
    total: usize,
    state: &'static str,
}

const SWITCH_STEPS: [&str; 9] = [
    "检测 Codex 状态",
    "关闭 Codex",
    "备份配置",
    "验证 API Key",
    "更新 Provider",
    "更新认证信息",
    "验证配置",
    "启动 Codex",
    "完成",
];

const RESTORE_STEPS: [&str; 9] = [
    "检测 Codex 状态",
    "关闭 Codex",
    "备份当前配置",
    "读取恢复基线",
    "恢复 Provider",
    "保留自定义 API 密钥",
    "验证恢复结果",
    "启动 Codex",
    "完成",
];

fn emit_progress(
    app: &AppHandle,
    steps: &'static [&'static str],
    index: usize,
    state: &'static str,
) {
    let _ = app.emit(
        "switch-progress",
        ProgressEvent {
            step: steps[index],
            index,
            total: steps.len(),
            state,
        },
    );
}

fn audit_failure(
    operation: &str,
    provider: Option<&str>,
    error: &AppError,
    codex_was_running: Option<bool>,
) {
    let mut entry = audit::AuditEntry::new(operation);
    entry.provider = provider;
    entry.error_code = Some(error.code);
    entry.http_status = error.http_status;
    entry.request_elapsed_ms = error.request_elapsed_ms;
    entry.codex_was_running = codex_was_running;
    entry.codex_restored = error.codex_restored;
    let _ = audit::append(&entry);
    let _ = audit::append_debug(operation, None, None, Some(error.message.as_str()));
}

fn task_error(code: &'static str, action: &str, error: impl std::fmt::Display) -> AppError {
    AppError::new(code, format!("{action}意外终止：{error}"))
}

async fn capture_codex() -> Result<process::CodexProcessSnapshot, AppError> {
    tauri::async_runtime::spawn_blocking(process::snapshot)
        .await
        .map_err(|error| task_error("PROC-010", "进程检测任务", error))?
}

async fn close_codex() -> Result<(), AppError> {
    tauri::async_runtime::spawn_blocking(|| process::close_codex(Duration::from_secs(5)))
        .await
        .map_err(|error| task_error("PROC-011", "Codex 关闭任务", error))?
}

fn restart_codex_sync(target: &process::CodexProcessSnapshot) -> Result<(), AppError> {
    let package_result = shell::launch_codex_package();
    let first_wait = if target.executable.is_some() { 10 } else { 15 };
    if package_result.is_ok() && process::wait_for_codex(Duration::from_secs(first_wait))? {
        return Ok(());
    }

    if let Some(executable) = target.executable.as_deref().filter(|path| path.is_file()) {
        shell::launch_executable(executable)?;
        if process::wait_for_codex(Duration::from_secs(15 - first_wait))? {
            return Ok(());
        }
    } else if let Err(error) = package_result {
        return Err(error);
    }

    Err(AppError::new(
        "PROC-012",
        "配置已完成，但 Codex 启动失败，请手动启动 Codex Desktop。",
    ))
}

async fn restart_codex(target: &process::CodexProcessSnapshot) -> Result<(), AppError> {
    let target = target.clone();
    tauri::async_runtime::spawn_blocking(move || restart_codex_sync(&target))
        .await
        .map_err(|error| task_error("PROC-013", "Codex 启动任务", error))?
}

async fn restore_previous_runtime(target: &process::CodexProcessSnapshot, error: &mut AppError) {
    if target.was_running {
        if restart_codex(target).await.is_ok() {
            error.codex_restored = Some(true);
        } else {
            error.codex_restored = Some(false);
            error
                .message
                .push_str("；Codex 未能自动重新启动，请手动打开。");
        }
    }
}

#[tauri::command]
pub async fn detect_status() -> Result<locator::DetectionStatus, ErrorPayload> {
    let _ = audit::cleanup_old_logs();
    locator::detect().map_err(Into::into)
}

#[tauri::command]
pub async fn fetch_models(
    state: State<'_, ApplicationState>,
    request: ModelListRequest,
) -> Result<validator::ModelCatalog, ErrorPayload> {
    let _guard = state.operation_lock.lock().await;
    let provider = generic_provider(request.api_url.as_str()).map_err(ErrorPayload::from)?;
    let supplied_key = Zeroizing::new(if request.api_key.trim().is_empty() {
        environment::read_user(GENERIC_ENV_KEY)
            .map_err(ErrorPayload::from)?
            .ok_or_else(|| {
                ErrorPayload::from(AppError::new(
                    "API_KEY_MISSING",
                    "请输入 API Key，或先保存一个可复用的 Key。",
                ))
            })?
    } else {
        request.api_key
    });
    validator::list_models(&provider, supplied_key.as_str())
        .await
        .map_err(|error| {
            audit_failure(
                "fetch_models",
                Some(provider.config_id.as_str()),
                &error,
                None,
            );
            error.into()
        })
}

#[tauri::command]
pub async fn switch_provider(
    app: AppHandle,
    state: State<'_, ApplicationState>,
    request: SwitchRequest,
) -> Result<OperationResult, ErrorPayload> {
    let _guard = state.operation_lock.lock().await;
    let mut provider = generic_provider(request.api_url.as_str()).map_err(ErrorPayload::from)?;
    if let Some(model) = request
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if model.len() > 128 || model.chars().any(char::is_whitespace) {
            return Err(AppError::new(
                "API_MODEL_FORMAT",
                "模型 ID 格式无效，请重新获取模型列表。",
            )
            .into());
        }
        provider.model = model.to_string();
    }
    let supplied_key = Zeroizing::new(if request.api_key.trim().is_empty() {
        environment::read_user(GENERIC_ENV_KEY)
            .map_err(ErrorPayload::from)?
            .ok_or_else(|| {
                ErrorPayload::from(AppError::new(
                    "API_KEY_MISSING",
                    "请输入 API Key，或先保存一个可复用的 Key。",
                ))
            })?
    } else {
        request.api_key
    });
    let key = Zeroizing::new(
        validator::normalize_api_key(supplied_key.as_str()).map_err(ErrorPayload::from)?,
    );

    emit_progress(&app, &SWITCH_STEPS, 0, "active");
    let home = match locator::require_codex_home() {
        Ok(value) => value,
        Err(error) => {
            emit_progress(&app, &SWITCH_STEPS, 0, "error");
            audit_failure("switch", Some(provider.config_id.as_str()), &error, None);
            return Err(error.into());
        }
    };
    let target = match capture_codex().await {
        Ok(value) => value,
        Err(error) => {
            emit_progress(&app, &SWITCH_STEPS, 0, "error");
            audit_failure("switch", Some(provider.config_id.as_str()), &error, None);
            return Err(error.into());
        }
    };
    emit_progress(&app, &SWITCH_STEPS, 0, "done");

    emit_progress(&app, &SWITCH_STEPS, 1, "active");
    if target.was_running {
        if let Err(mut error) = close_codex().await {
            emit_progress(&app, &SWITCH_STEPS, 1, "error");
            restore_previous_runtime(&target, &mut error).await;
            audit_failure(
                "switch",
                Some(provider.config_id.as_str()),
                &error,
                Some(target.was_running),
            );
            return Err(error.into());
        }
    }
    emit_progress(&app, &SWITCH_STEPS, 1, "done");

    let snapshot = match config::load(&home.join("config.toml")).and_then(|value| {
        config::preflight_writable(&value)?;
        Ok(value)
    }) {
        Ok(value) => value,
        Err(mut error) => {
            emit_progress(&app, &SWITCH_STEPS, 2, "error");
            restore_previous_runtime(&target, &mut error).await;
            audit_failure(
                "switch",
                Some(provider.config_id.as_str()),
                &error,
                Some(target.was_running),
            );
            return Err(error.into());
        }
    };

    emit_progress(&app, &SWITCH_STEPS, 2, "active");
    let home_for_prepare = home.clone();
    let snapshot_for_prepare = snapshot.clone();
    let provider_for_prepare = provider.clone();
    let prepared = match tauri::async_runtime::spawn_blocking(move || {
        application::prepare_switch(
            &home_for_prepare,
            &snapshot_for_prepare,
            &provider_for_prepare,
        )
    })
    .await
    {
        Ok(Ok(value)) => value,
        Ok(Err(mut error)) => {
            emit_progress(&app, &SWITCH_STEPS, 2, "error");
            restore_previous_runtime(&target, &mut error).await;
            audit_failure(
                "switch",
                Some(provider.config_id.as_str()),
                &error,
                Some(target.was_running),
            );
            return Err(error.into());
        }
        Err(join_error) => {
            let mut error = task_error("TXN-004", "备份任务", join_error);
            emit_progress(&app, &SWITCH_STEPS, 2, "error");
            restore_previous_runtime(&target, &mut error).await;
            audit_failure(
                "switch",
                Some(provider.config_id.as_str()),
                &error,
                Some(target.was_running),
            );
            return Err(error.into());
        }
    };
    emit_progress(&app, &SWITCH_STEPS, 2, "done");

    emit_progress(&app, &SWITCH_STEPS, 3, "active");
    let validation =
        match validator::validate(&provider, key.as_str(), Some(provider.model.as_str())).await {
            Ok(value) => value,
            Err(mut error) => {
                let cancellation =
                    tauri::async_runtime::spawn_blocking(move || prepared.cancel()).await;
                if let Ok(Err(cancel_error)) = cancellation {
                    error
                        .message
                        .push_str(&format!("；备份状态标记失败：{}", cancel_error.message));
                } else if let Err(join_error) = cancellation {
                    error
                        .message
                        .push_str(&format!("；备份状态任务异常：{join_error}"));
                }
                emit_progress(&app, &SWITCH_STEPS, 3, "error");
                restore_previous_runtime(&target, &mut error).await;
                audit_failure(
                    "switch",
                    Some(provider.config_id.as_str()),
                    &error,
                    Some(target.was_running),
                );
                return Err(error.into());
            }
        };
    emit_progress(&app, &SWITCH_STEPS, 3, "done");

    let current_step = Arc::new(AtomicUsize::new(4));
    let current_step_for_work = current_step.clone();
    let validation_for_work = validation.clone();
    let app_for_work = app.clone();
    let home_for_work = home.clone();
    let snapshot_for_work = snapshot.clone();
    let provider_for_work = provider.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        application::apply_prepared_switch(
            &home_for_work,
            &snapshot_for_work,
            &provider_for_work,
            &validation_for_work,
            key.as_str(),
            prepared,
            |index| {
                current_step_for_work.store(index, Ordering::Relaxed);
                emit_progress(&app_for_work, &SWITCH_STEPS, index, "active");
            },
        )
    })
    .await;

    let mut value = match result {
        Ok(Ok(value)) => value,
        Ok(Err(mut error)) => {
            emit_progress(
                &app,
                &SWITCH_STEPS,
                current_step.load(Ordering::Relaxed),
                "error",
            );
            restore_previous_runtime(&target, &mut error).await;
            audit_failure(
                "switch",
                Some(provider.config_id.as_str()),
                &error,
                Some(target.was_running),
            );
            return Err(error.into());
        }
        Err(join_error) => {
            let mut error = task_error("TXN-005", "切换任务", join_error);
            emit_progress(
                &app,
                &SWITCH_STEPS,
                current_step.load(Ordering::Relaxed),
                "error",
            );
            restore_previous_runtime(&target, &mut error).await;
            audit_failure(
                "switch",
                Some(provider.config_id.as_str()),
                &error,
                Some(target.was_running),
            );
            return Err(error.into());
        }
    };
    for index in 4..=6 {
        emit_progress(&app, &SWITCH_STEPS, index, "done");
    }

    emit_progress(&app, &SWITCH_STEPS, 7, "active");
    if let Err(error) = restart_codex(&target).await {
        let error = error.changed(true).codex_restored(false);
        emit_progress(&app, &SWITCH_STEPS, 7, "error");
        audit_failure(
            "switch",
            Some(provider.config_id.as_str()),
            &error,
            Some(target.was_running),
        );
        return Err(error.into());
    }
    emit_progress(&app, &SWITCH_STEPS, 7, "done");
    emit_progress(&app, &SWITCH_STEPS, 8, "done");
    value.detail = "Codex 已自动重新启动，自定义 API 配置已生效。".to_string();
    value.codex_restored = target.was_running.then_some(true);

    let mut entry = audit::AuditEntry::new("switch");
    entry.provider = Some(provider.config_id.as_str());
    entry.api_host = Some(provider.expected_host.as_str());
    entry.http_status = Some(validation.http_status);
    entry.request_elapsed_ms = Some(validation.request_elapsed_ms);
    entry.validation_method = Some(validation.validation_method);
    entry.codex_was_running = Some(target.was_running);
    entry.codex_restored = target.was_running.then_some(true);
    entry.config_path = Some(home.join("config.toml").to_string_lossy().into_owned());
    entry.backup_path = value.backup_path.clone();
    entry.migration_count = value.migration_count;
    let _ = audit::append(&entry);
    Ok(value)
}

#[tauri::command]
pub async fn restore_official(
    app: AppHandle,
    state: State<'_, ApplicationState>,
) -> Result<OperationResult, ErrorPayload> {
    let _guard = state.operation_lock.lock().await;
    emit_progress(&app, &RESTORE_STEPS, 0, "active");
    let home = match locator::require_codex_home() {
        Ok(value) => value,
        Err(error) => {
            emit_progress(&app, &RESTORE_STEPS, 0, "error");
            audit_failure("restore_official", None, &error, None);
            return Err(error.into());
        }
    };
    let target = match capture_codex().await {
        Ok(value) => value,
        Err(error) => {
            emit_progress(&app, &RESTORE_STEPS, 0, "error");
            audit_failure("restore_official", None, &error, None);
            return Err(error.into());
        }
    };
    emit_progress(&app, &RESTORE_STEPS, 0, "done");

    emit_progress(&app, &RESTORE_STEPS, 1, "active");
    if target.was_running {
        if let Err(mut error) = close_codex().await {
            emit_progress(&app, &RESTORE_STEPS, 1, "error");
            restore_previous_runtime(&target, &mut error).await;
            audit_failure("restore_official", None, &error, Some(target.was_running));
            return Err(error.into());
        }
    }
    emit_progress(&app, &RESTORE_STEPS, 1, "done");

    let current_step = Arc::new(AtomicUsize::new(2));
    let current_step_for_work = current_step.clone();
    let app_for_work = app.clone();
    let home_for_work = home.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        application::restore_official(&home_for_work, |index| {
            current_step_for_work.store(index, Ordering::Relaxed);
            emit_progress(&app_for_work, &RESTORE_STEPS, index, "active")
        })
    })
    .await;

    let mut value = match result {
        Ok(Ok(value)) => value,
        Ok(Err(mut error)) => {
            emit_progress(
                &app,
                &RESTORE_STEPS,
                current_step.load(Ordering::Relaxed),
                "error",
            );
            restore_previous_runtime(&target, &mut error).await;
            audit_failure("restore_official", None, &error, Some(target.was_running));
            return Err(error.into());
        }
        Err(join_error) => {
            let mut error = task_error("TXN-006", "恢复任务", join_error);
            emit_progress(
                &app,
                &RESTORE_STEPS,
                current_step.load(Ordering::Relaxed),
                "error",
            );
            restore_previous_runtime(&target, &mut error).await;
            audit_failure("restore_official", None, &error, Some(target.was_running));
            return Err(error.into());
        }
    };
    for index in 2..=6 {
        emit_progress(&app, &RESTORE_STEPS, index, "done");
    }

    emit_progress(&app, &RESTORE_STEPS, 7, "active");
    if let Err(error) = restart_codex(&target).await {
        let error = error.changed(value.config_changed).codex_restored(false);
        emit_progress(&app, &RESTORE_STEPS, 7, "error");
        audit_failure("restore_official", None, &error, Some(target.was_running));
        return Err(error.into());
    }
    emit_progress(&app, &RESTORE_STEPS, 7, "done");
    emit_progress(&app, &RESTORE_STEPS, 8, "done");
    value.detail = "Codex 已自动重新启动，切换前线路已恢复。".to_string();
    value.codex_restored = target.was_running.then_some(true);

    let mut entry = audit::AuditEntry::new("restore_official");
    entry.config_path = Some(home.join("config.toml").to_string_lossy().into_owned());
    entry.backup_path = value.backup_path.clone();
    entry.migration_count = value.migration_count;
    entry.codex_was_running = Some(target.was_running);
    entry.codex_restored = target.was_running.then_some(true);
    let _ = audit::append(&entry);
    Ok(value)
}

#[tauri::command]
pub async fn open_codex_home() -> Result<(), ErrorPayload> {
    let home = locator::require_codex_home().map_err(ErrorPayload::from)?;
    shell::open_directory(&home).map_err(Into::into)
}

#[tauri::command]
pub async fn open_backup_directory() -> Result<(), ErrorPayload> {
    let home = locator::require_codex_home().map_err(ErrorPayload::from)?;
    let backups = state::backups_root(&home);
    let target = if backups.is_dir() { backups } else { home };
    shell::open_directory(&target).map_err(Into::into)
}

#[tauri::command]
pub async fn clear_logs() -> Result<(), ErrorPayload> {
    audit::clear_all().map_err(Into::into)
}

#[tauri::command]
pub async fn open_vexlune_hub() -> Result<(), ErrorPayload> {
    shell::open_url("https://hub.vexlune.com").map_err(Into::into)
}

#[tauri::command]
pub async fn delete_saved_key(state: State<'_, ApplicationState>) -> Result<(), ErrorPayload> {
    let _guard = state.operation_lock.lock().await;
    let status = locator::detect().map_err(ErrorPayload::from)?;
    if status.current_route == "generic" {
        return Err(AppError::new(
            "KEY-DELETE-ACTIVE",
            "请先切换回 OpenAI 官方线路，再删除已保存的 API Key。",
        )
        .into());
    }
    environment::delete_user(GENERIC_ENV_KEY).map_err(ErrorPayload::from)?;
    let mut entry = audit::AuditEntry::new("delete_saved_key");
    entry.provider = Some(GENERIC_PROVIDER_ID);
    let _ = audit::append(&entry);
    Ok(())
}
