use crate::errors::{AppError, AppResult};
use crate::security::redact::redact_text;
use chrono::{Duration, Local, Utc};
use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry<'a> {
    pub timestamp: String,
    pub operation: &'a str,
    pub provider: Option<&'a str>,
    pub api_host: Option<&'a str>,
    pub http_status: Option<u16>,
    pub config_path: Option<String>,
    pub backup_path: Option<String>,
    pub migration_count: Option<usize>,
    pub error_code: Option<&'a str>,
    pub request_elapsed_ms: Option<u64>,
    pub validation_method: Option<&'a str>,
    pub codex_was_running: Option<bool>,
    pub codex_restored: Option<bool>,
}

impl<'a> AuditEntry<'a> {
    pub fn new(operation: &'a str) -> Self {
        Self {
            timestamp: Utc::now().to_rfc3339(),
            operation,
            provider: None,
            api_host: None,
            http_status: None,
            config_path: None,
            backup_path: None,
            migration_count: None,
            error_code: None,
            request_elapsed_ms: None,
            validation_method: None,
            codex_was_running: None,
            codex_restored: None,
        }
    }
}

pub struct HttpDebugEntry<'a> {
    pub method: &'a str,
    pub request_url: &'a str,
    pub header_names: &'a str,
    pub request_body: Option<&'a str>,
    pub status: Option<u16>,
    pub elapsed_ms: u64,
    pub attempt: usize,
    pub proxy_environment: bool,
    pub response_content_type: Option<&'a str>,
    pub response_server: Option<&'a str>,
    pub response_preview: Option<&'a str>,
    pub error: Option<&'a str>,
}

pub fn log_directory() -> AppResult<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| AppError::new("LOG-001", "无法确定当前用户的本地应用数据目录。"))?;
    Ok(PathBuf::from(base).join("CodexGo").join("logs"))
}

pub fn append(entry: &AuditEntry<'_>) -> AppResult<()> {
    let directory = log_directory()?;
    fs::create_dir_all(&directory)
        .map_err(|error| AppError::io("LOG-002", "无法创建日志目录", &error))?;
    cleanup_old_logs()?;
    let file_name = format!("audit-{}.jsonl", Local::now().format("%Y%m%d"));
    let path = directory.join(file_name);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| AppError::io("LOG-003", "无法写入脱敏日志", &error))?;
    serde_json::to_writer(&mut file, entry)
        .map_err(|error| AppError::new("LOG-004", format!("日志序列化失败：{error}")))?;
    file.write_all(b"\n")
        .map_err(|error| AppError::io("LOG-005", "无法完成日志写入", &error))?;
    Ok(())
}

pub fn append_debug(
    operation: &str,
    request_url: Option<&str>,
    status: Option<u16>,
    error: Option<&str>,
) -> AppResult<()> {
    let directory = log_directory()?;
    fs::create_dir_all(&directory)
        .map_err(|value| AppError::io("LOG-012", "无法创建调试日志目录", &value))?;
    cleanup_old_logs()?;
    let path = directory.join("debug.log");
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|value| AppError::io("LOG-013", "无法写入调试日志", &value))?;
    let safe_url = request_url
        .map(redact_text)
        .unwrap_or_else(|| "-".to_string());
    let safe_error = error.map(redact_text).unwrap_or_else(|| "-".to_string());
    writeln!(
        file,
        "{} operation={} request={} status={} error={}",
        Utc::now().to_rfc3339(),
        operation,
        safe_url,
        status
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        safe_error.replace(['\r', '\n'], " ")
    )
    .map_err(|value| AppError::io("LOG-014", "无法完成调试日志写入", &value))
}

pub fn append_http_debug(entry: &HttpDebugEntry<'_>) -> AppResult<()> {
    let directory = log_directory()?;
    fs::create_dir_all(&directory)
        .map_err(|value| AppError::io("LOG-015", "无法创建 HTTP 调试日志目录", &value))?;
    cleanup_old_logs()?;
    let path = directory.join("debug.log");
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|value| AppError::io("LOG-016", "无法写入 HTTP 调试日志", &value))?;
    let safe = |value: Option<&str>| {
        value
            .map(redact_text)
            .unwrap_or_else(|| "-".to_string())
            .replace(['\r', '\n'], " ")
    };
    writeln!(
        file,
        "{} operation=api_request method={} request={} headers={} body={} status={} elapsed_ms={} attempt={} proxy_env={} content_type={} server={} response_preview={} error={}",
        Utc::now().to_rfc3339(),
        entry.method,
        redact_text(entry.request_url),
        entry.header_names,
        safe(entry.request_body),
        entry
            .status
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        entry.elapsed_ms,
        entry.attempt,
        entry.proxy_environment,
        safe(entry.response_content_type),
        safe(entry.response_server),
        safe(entry.response_preview),
        safe(entry.error),
    )
    .map_err(|value| AppError::io("LOG-017", "无法完成 HTTP 调试日志写入", &value))
}

pub fn cleanup_old_logs() -> AppResult<()> {
    let directory = log_directory()?;
    if !directory.exists() {
        return Ok(());
    }
    let cutoff = Utc::now() - Duration::days(14);
    for entry in fs::read_dir(&directory)
        .map_err(|error| AppError::io("LOG-006", "无法读取日志目录", &error))?
    {
        let entry = entry.map_err(|error| AppError::io("LOG-007", "无法检查日志文件", &error))?;
        let metadata = entry
            .metadata()
            .map_err(|error| AppError::io("LOG-008", "无法读取日志元数据", &error))?;
        if !metadata.is_file() {
            continue;
        }
        if let Ok(modified) = metadata.modified() {
            let modified: chrono::DateTime<Utc> = modified.into();
            if modified < cutoff {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    Ok(())
}

pub fn clear_all() -> AppResult<()> {
    let directory = log_directory()?;
    if !directory.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(&directory)
        .map_err(|error| AppError::io("LOG-009", "无法读取日志目录", &error))?
    {
        let entry = entry.map_err(|error| AppError::io("LOG-010", "无法检查日志文件", &error))?;
        if entry.path().is_file() {
            fs::remove_file(entry.path())
                .map_err(|error| AppError::io("LOG-011", "无法清理日志文件", &error))?;
        }
    }
    Ok(())
}
