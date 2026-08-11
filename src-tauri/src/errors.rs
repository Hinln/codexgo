use serde::Serialize;
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone)]
pub struct AppError {
    pub code: &'static str,
    pub message: String,
    pub config_changed: bool,
    pub rolled_back: bool,
    pub http_status: Option<u16>,
    pub request_elapsed_ms: Option<u64>,
    pub codex_restored: Option<bool>,
}

impl AppError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: crate::security::redact::redact_text(&message.into()),
            config_changed: false,
            rolled_back: false,
            http_status: None,
            request_elapsed_ms: None,
            codex_restored: None,
        }
    }

    pub fn changed(mut self, value: bool) -> Self {
        self.config_changed = value;
        self
    }

    pub fn rolled_back(mut self, value: bool) -> Self {
        self.rolled_back = value;
        self
    }

    pub fn http_status(mut self, value: u16) -> Self {
        self.http_status = Some(value);
        self
    }

    pub fn request_elapsed_ms(mut self, value: u64) -> Self {
        self.request_elapsed_ms = Some(value);
        self
    }

    pub fn codex_restored(mut self, value: bool) -> Self {
        self.codex_restored = Some(value);
        self
    }

    pub fn io(code: &'static str, context: &str, error: &std::io::Error) -> Self {
        Self::new(code, format!("{context}：{error}"))
    }
}

impl Display for AppError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AppError {}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorPayload {
    pub code: String,
    pub message: String,
    pub config_changed: bool,
    pub rolled_back: bool,
    pub http_status: Option<u16>,
    pub request_elapsed_ms: Option<u64>,
    pub codex_restored: Option<bool>,
}

impl From<AppError> for ErrorPayload {
    fn from(error: AppError) -> Self {
        Self {
            code: error.code.to_string(),
            message: error.message,
            config_changed: error.config_changed,
            rolled_back: error.rolled_back,
            http_status: error.http_status,
            request_elapsed_ms: error.request_elapsed_ms,
            codex_restored: error.codex_restored,
        }
    }
}

pub type AppResult<T> = Result<T, AppError>;
