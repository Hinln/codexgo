use crate::api::endpoint::{candidates, validate_provider_url, EndpointCandidate};
use crate::errors::{AppError, AppResult};
use crate::provider::ProviderDefinition;
use crate::storage::audit::{self, HttpDebugEntry};
use reqwest::header::ACCEPT;
use reqwest::redirect::Policy;
use reqwest::{Client, RequestBuilder, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::error::Error;
use std::time::{Duration, Instant};

const RESPONSE_PREVIEW_LIMIT: usize = 2_048;

#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub base_url: String,
    pub http_status: u16,
    pub request_elapsed_ms: u64,
    pub validation_method: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalog {
    pub models: Vec<String>,
    pub http_status: u16,
    pub request_elapsed_ms: u64,
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ModelEntry>,
}

#[derive(Debug, Deserialize)]
struct ModelEntry {
    id: String,
}

struct HttpResponse {
    status: StatusCode,
    body: String,
    elapsed_ms: u64,
}

pub fn normalize_api_key(input: &str) -> AppResult<String> {
    let mut trimmed = input.trim();
    if trimmed
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("bearer "))
    {
        trimmed = trimmed[7..].trim();
    }

    let has_quote = matches!(trimmed.chars().next(), Some('"' | '\''))
        || matches!(trimmed.chars().last(), Some('"' | '\''));
    if trimmed.len() < 8 || trimmed.chars().any(char::is_whitespace) || has_quote {
        return Err(AppError::new(
            "API_KEY_FORMAT",
            "API Key 格式无效，请粘贴不带引号的完整 API Key。",
        ));
    }
    Ok(trimmed.to_string())
}

fn proxy_environment_present() -> bool {
    [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
    ]
    .iter()
    .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

fn client(provider: &ProviderDefinition) -> AppResult<Client> {
    let expected_root = url::Url::parse(&provider.root_url)
        .map_err(|error| AppError::new("API_CLIENT_INIT", format!("API 地址无效：{error}")))?;
    let expected_scheme = expected_root.scheme().to_string();
    let expected_host = provider.expected_host.clone();
    let expected_port = expected_root.port_or_known_default();
    Client::builder()
        .https_only(expected_scheme == "https")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .user_agent("CodexGo/1.0.0")
        .redirect(Policy::custom(move |attempt| {
            let url = attempt.url();
            if url.scheme() != expected_scheme
                || url.host_str() != Some(expected_host.as_str())
                || url.port_or_known_default() != expected_port
            {
                attempt.error("cross-domain redirect blocked")
            } else if attempt.previous().len() >= 5 {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|error| {
            AppError::new(
                "API_CLIENT_INIT",
                format!("无法创建安全 HTTP 客户端：{error}"),
            )
        })
}

fn error_chain(error: &reqwest::Error) -> String {
    let mut messages = vec![error.to_string()];
    let mut source = error.source();
    while let Some(cause) = source {
        messages.push(cause.to_string());
        source = cause.source();
    }
    messages.join(": ").to_ascii_lowercase()
}

fn classify_transport(error: reqwest::Error, elapsed_ms: u64) -> AppError {
    if error.is_timeout() {
        return AppError::new("API_TIMEOUT", "连接目标 API 超时，请检查地址和网络后重试。")
            .request_elapsed_ms(elapsed_ms);
    }

    let message = error_chain(&error);
    if message.contains("cross-domain redirect") {
        return AppError::new(
            "API_REDIRECT_BLOCKED",
            "API 请求被重定向到非预期域名，已为安全起见停止。",
        )
        .request_elapsed_ms(elapsed_ms);
    }
    if message.contains("proxy") || message.contains("tunnel") {
        return AppError::new(
            "API_PROXY",
            "无法通过当前网络代理连接目标 API，请检查代理设置。",
        )
        .request_elapsed_ms(elapsed_ms);
    }
    if message.contains("certificate")
        || message.contains("tls")
        || message.contains("ssl")
        || message.contains("unknown issuer")
    {
        return AppError::new(
            "API_TLS",
            "与目标 API 建立安全连接失败，请检查系统时间、证书或网络拦截软件。",
        )
        .request_elapsed_ms(elapsed_ms);
    }
    if message.contains("dns")
        || message.contains("name resolution")
        || message.contains("failed to lookup address")
        || message.contains("no such host")
    {
        return AppError::new(
            "API_DNS",
            "无法解析目标 API 域名，请检查地址、DNS 或网络连接。",
        )
        .request_elapsed_ms(elapsed_ms);
    }
    AppError::new(
        "API_CONNECT",
        "无法连接目标 API，请检查地址、网络、防火墙或代理设置。",
    )
    .request_elapsed_ms(elapsed_ms)
}

fn status_error(status: StatusCode, stage: &str, elapsed_ms: u64) -> AppError {
    let error = match status {
        StatusCode::UNAUTHORIZED => {
            AppError::new("API_AUTH_401", "API Key 无效或已过期，请检查后重试。")
        }
        StatusCode::FORBIDDEN => AppError::new("API_AUTH_403", "当前 API Key 没有访问权限。"),
        StatusCode::TOO_MANY_REQUESTS => {
            AppError::new("API_RATE_LIMIT_429", "目标 API 请求过于频繁，请稍后重试。")
        }
        StatusCode::NOT_FOUND => AppError::new(
            "API_HTTP_404",
            format!("目标 API 的{stage}不存在（HTTP 404）。"),
        ),
        StatusCode::METHOD_NOT_ALLOWED => AppError::new(
            "API_HTTP_405",
            format!("目标 API 的{stage}不支持当前请求方式（HTTP 405）。"),
        ),
        StatusCode::INTERNAL_SERVER_ERROR => AppError::new(
            "API_HTTP_500",
            format!("目标 API 的{stage}暂时返回服务器错误（HTTP 500）。"),
        ),
        StatusCode::BAD_GATEWAY => AppError::new(
            "API_HTTP_502",
            format!("目标 API 的{stage}暂时返回网关错误（HTTP 502），这不代表 API Key 无效。"),
        ),
        StatusCode::SERVICE_UNAVAILABLE => AppError::new(
            "API_HTTP_503",
            format!("目标 API 的{stage}暂时不可用（HTTP 503）。"),
        ),
        StatusCode::GATEWAY_TIMEOUT => AppError::new(
            "API_HTTP_504",
            format!("目标 API 的{stage}网关响应超时（HTTP 504）。"),
        ),
        _ => AppError::new(
            "API_HTTP_STATUS",
            format!(
                "目标 API 的{stage}返回 HTTP {}，未执行切换。",
                status.as_u16()
            ),
        ),
    };
    error
        .http_status(status.as_u16())
        .request_elapsed_ms(elapsed_ms)
}

fn response_preview(body: &str) -> String {
    body.chars().take(RESPONSE_PREVIEW_LIMIT).collect()
}

async fn send_request(
    request: RequestBuilder,
    method: &str,
    header_names: &str,
    request_body: Option<&str>,
) -> AppResult<HttpResponse> {
    let request_url = request
        .try_clone()
        .and_then(|builder| builder.build().ok())
        .map(|value| value.url().to_string())
        .ok_or_else(|| AppError::new("API_REQUEST_BUILD", "无法构造 API 请求。"))?;
    let started = Instant::now();
    let response = match request.send().await {
        Ok(value) => value,
        Err(error) => {
            let elapsed_ms = started.elapsed().as_millis() as u64;
            let error_text = error_chain(&error);
            let _ = audit::append_http_debug(&HttpDebugEntry {
                method,
                request_url: request_url.as_str(),
                header_names,
                request_body,
                status: None,
                elapsed_ms,
                attempt: 1,
                proxy_environment: proxy_environment_present(),
                response_content_type: None,
                response_server: None,
                response_preview: None,
                error: Some(error_text.as_str()),
            });
            return Err(classify_transport(error, elapsed_ms));
        }
    };

    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let server = response
        .headers()
        .get(reqwest::header::SERVER)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let body = match response.text().await {
        Ok(value) => value,
        Err(error) => {
            let elapsed_ms = started.elapsed().as_millis() as u64;
            let error_text = error_chain(&error);
            let _ = audit::append_http_debug(&HttpDebugEntry {
                method,
                request_url: request_url.as_str(),
                header_names,
                request_body,
                status: Some(status.as_u16()),
                elapsed_ms,
                attempt: 1,
                proxy_environment: proxy_environment_present(),
                response_content_type: content_type.as_deref(),
                response_server: server.as_deref(),
                response_preview: None,
                error: Some(error_text.as_str()),
            });
            return Err(
                AppError::new("API_RESPONSE_READ", "目标 API 响应读取失败，请重试。")
                    .http_status(status.as_u16())
                    .request_elapsed_ms(elapsed_ms),
            );
        }
    };
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let preview = response_preview(body.as_str());
    let _ = audit::append_http_debug(&HttpDebugEntry {
        method,
        request_url: request_url.as_str(),
        header_names,
        request_body,
        status: Some(status.as_u16()),
        elapsed_ms,
        attempt: 1,
        proxy_environment: proxy_environment_present(),
        response_content_type: content_type.as_deref(),
        response_server: server.as_deref(),
        response_preview: Some(preview.as_str()),
        error: None,
    });
    Ok(HttpResponse {
        status,
        body,
        elapsed_ms,
    })
}

fn choose_model(models: &[ModelEntry], configured_model: Option<&str>) -> AppResult<String> {
    if let Some(model) = configured_model {
        if models.iter().any(|entry| entry.id == model) {
            return Ok(model.to_string());
        }
        return Err(AppError::new(
            "API_MODEL_UNAVAILABLE",
            format!("当前模型 {model} 不在目标 API 的模型列表中，未执行切换。"),
        ));
    }
    models
        .first()
        .map(|entry| entry.id.clone())
        .ok_or_else(|| AppError::new("API_MODEL_EMPTY", "模型接口未返回可用模型。"))
}

fn model_ids(response: &HttpResponse) -> AppResult<Vec<String>> {
    let parsed = serde_json::from_str::<ModelsResponse>(response.body.as_str()).map_err(|_| {
        AppError::new("API_INVALID_RESPONSE", "模型接口返回了无法识别的 JSON。")
            .http_status(response.status.as_u16())
            .request_elapsed_ms(response.elapsed_ms)
    })?;
    let mut models = Vec::new();
    for entry in parsed.data {
        let id = entry.id.trim();
        if !id.is_empty() && !models.iter().any(|value| value == id) {
            models.push(id.to_string());
        }
    }
    if models.is_empty() {
        return Err(AppError::new("API_MODEL_EMPTY", "模型接口未返回可用模型。")
            .http_status(response.status.as_u16())
            .request_elapsed_ms(response.elapsed_ms));
    }
    Ok(models)
}

fn response_error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .and_then(Value::as_str)
                .or_else(|| value.get("message").and_then(Value::as_str))
                .map(ToOwned::to_owned)
        })
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn should_fallback_models(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
    ) || status.is_server_error()
}

fn validate_responses_body(response: &HttpResponse) -> AppResult<()> {
    let value = serde_json::from_str::<Value>(response.body.as_str()).map_err(|_| {
        AppError::new(
            "API_INVALID_RESPONSE",
            "Responses 备用接口返回了无法识别的 JSON。",
        )
        .http_status(response.status.as_u16())
        .request_elapsed_ms(response.elapsed_ms)
    })?;
    if !value.is_object()
        || !["id", "output", "status", "object"]
            .iter()
            .any(|key| value.get(*key).is_some())
    {
        return Err(
            AppError::new("API_INVALID_RESPONSE", "Responses 备用接口返回结构异常。")
                .http_status(response.status.as_u16())
                .request_elapsed_ms(response.elapsed_ms),
        );
    }
    Ok(())
}

async fn probe_responses(
    client: &Client,
    candidate: &EndpointCandidate,
    api_key: &str,
    model: &str,
) -> AppResult<ValidationResult> {
    let payload = json!({
        "model": model,
        "input": "ping",
        "max_output_tokens": 1,
        "store": false
    });
    let request_body = payload.to_string();
    let response = send_request(
        client
            .post(candidate.responses_url.clone())
            .header(ACCEPT, "application/json")
            .bearer_auth(api_key)
            .json(&payload),
        "POST",
        "Authorization,Accept,Content-Type",
        Some(request_body.as_str()),
    )
    .await?;

    if !response.status.is_success() {
        if matches!(
            response.status,
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS
        ) {
            return Err(status_error(
                response.status,
                "Responses 备用接口",
                response.elapsed_ms,
            ));
        }
        let message = response_error_message(response.body.as_str());
        if matches!(
            response.status,
            StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND
        ) && message.contains("model")
        {
            return Err(AppError::new(
                "API_MODEL_UNAVAILABLE",
                "目标模型在 Responses API 中不可用。",
            )
            .http_status(response.status.as_u16())
            .request_elapsed_ms(response.elapsed_ms));
        }
        return Err(status_error(
            response.status,
            "Responses 备用接口",
            response.elapsed_ms,
        ));
    }

    validate_responses_body(&response)?;
    Ok(ValidationResult {
        base_url: candidate
            .base_url
            .as_str()
            .trim_end_matches('/')
            .to_string(),
        http_status: response.status.as_u16(),
        request_elapsed_ms: response.elapsed_ms,
        validation_method: "responses_fallback",
    })
}

async fn probe_candidate(
    client: &Client,
    provider: &ProviderDefinition,
    candidate: &EndpointCandidate,
    api_key: &str,
    configured_model: Option<&str>,
) -> AppResult<ValidationResult> {
    validate_provider_url(&candidate.models_url, &provider.expected_host)?;
    validate_provider_url(&candidate.responses_url, &provider.expected_host)?;

    let models_response = send_request(
        client
            .get(candidate.models_url.clone())
            .header(ACCEPT, "application/json")
            .bearer_auth(api_key),
        "GET",
        "Authorization,Accept",
        None,
    )
    .await?;

    if models_response.status.is_success() {
        let models = serde_json::from_str::<ModelsResponse>(models_response.body.as_str())
            .map_err(|_| {
                AppError::new(
                    "API_INVALID_RESPONSE",
                    "模型接口返回了无法识别的 JSON，未执行切换。",
                )
                .http_status(models_response.status.as_u16())
                .request_elapsed_ms(models_response.elapsed_ms)
            })?;
        choose_model(&models.data, configured_model).map_err(|error| {
            error
                .http_status(models_response.status.as_u16())
                .request_elapsed_ms(models_response.elapsed_ms)
        })?;
        return Ok(ValidationResult {
            base_url: candidate
                .base_url
                .as_str()
                .trim_end_matches('/')
                .to_string(),
            http_status: models_response.status.as_u16(),
            request_elapsed_ms: models_response.elapsed_ms,
            validation_method: "models",
        });
    }

    if should_fallback_models(models_response.status) {
        let model = configured_model.unwrap_or(provider.model.as_str());
        return probe_responses(client, candidate, api_key, model).await;
    }

    Err(status_error(
        models_response.status,
        "模型接口",
        models_response.elapsed_ms,
    ))
}

pub async fn validate(
    provider: &ProviderDefinition,
    api_key: &str,
    configured_model: Option<&str>,
) -> AppResult<ValidationResult> {
    let normalized = normalize_api_key(api_key)?;
    let client = client(provider)?;
    let candidate = candidates(provider)?
        .into_iter()
        .next()
        .ok_or_else(|| AppError::new("API_ENDPOINT", "未生成有效的 API 地址。"))?;
    probe_candidate(
        &client,
        provider,
        &candidate,
        normalized.as_str(),
        configured_model,
    )
    .await
}

pub async fn list_models(provider: &ProviderDefinition, api_key: &str) -> AppResult<ModelCatalog> {
    let normalized = normalize_api_key(api_key)?;
    let client = client(provider)?;
    let candidate = candidates(provider)?
        .into_iter()
        .next()
        .ok_or_else(|| AppError::new("API_ENDPOINT", "未生成有效的 API 地址。"))?;
    validate_provider_url(&candidate.models_url, &provider.expected_host)?;

    let response = send_request(
        client
            .get(candidate.models_url)
            .header(ACCEPT, "application/json")
            .bearer_auth(normalized),
        "GET",
        "Authorization,Accept",
        None,
    )
    .await?;
    if !response.status.is_success() {
        return Err(status_error(
            response.status,
            "模型接口",
            response.elapsed_ms,
        ));
    }
    let models = model_ids(&response)?;
    Ok(ModelCatalog {
        models,
        http_status: response.status.as_u16(),
        request_elapsed_ms: response.elapsed_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_api_key_without_duplicating_bearer() {
        assert_eq!(
            normalize_api_key("  sk-test-12345678\r\n").unwrap(),
            "sk-test-12345678"
        );
        assert_eq!(
            normalize_api_key("Bearer sk-test-12345678").unwrap(),
            "sk-test-12345678"
        );
        assert_eq!(
            normalize_api_key("bearer sk-test-12345678").unwrap(),
            "sk-test-12345678"
        );
        assert!(normalize_api_key("   ").is_err());
        assert!(normalize_api_key("\"sk-test-12345678\"").is_err());
        assert!(normalize_api_key("'sk-test-12345678'").is_err());
        assert!(normalize_api_key("sk-test 12345678").is_err());
    }

    #[test]
    fn maps_auth_rate_limit_and_gateway_status_codes() {
        assert_eq!(
            status_error(StatusCode::UNAUTHORIZED, "测试接口", 1).code,
            "API_AUTH_401"
        );
        assert_eq!(
            status_error(StatusCode::FORBIDDEN, "测试接口", 1).code,
            "API_AUTH_403"
        );
        assert_eq!(
            status_error(StatusCode::TOO_MANY_REQUESTS, "测试接口", 1).code,
            "API_RATE_LIMIT_429"
        );
        let gateway = status_error(StatusCode::BAD_GATEWAY, "测试接口", 25);
        assert_eq!(gateway.code, "API_HTTP_502");
        assert_eq!(gateway.http_status, Some(502));
        assert_eq!(gateway.request_elapsed_ms, Some(25));
    }

    #[test]
    fn falls_back_only_for_missing_or_server_error_models_endpoint() {
        for status in [
            StatusCode::NOT_FOUND,
            StatusCode::METHOD_NOT_ALLOWED,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::GATEWAY_TIMEOUT,
        ] {
            assert!(should_fallback_models(status));
        }
        for status in [
            StatusCode::OK,
            StatusCode::BAD_REQUEST,
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::TOO_MANY_REQUESTS,
        ] {
            assert!(!should_fallback_models(status));
        }
    }

    #[test]
    fn configured_model_must_exist_when_catalog_is_available() {
        let models = vec![ModelEntry {
            id: "model-a".to_string(),
        }];
        assert!(choose_model(&models, Some("model-b")).is_err());
        assert_eq!(choose_model(&models, None).unwrap(), "model-a");
    }

    #[test]
    fn model_catalog_deduplicates_and_rejects_empty_ids() {
        let response = HttpResponse {
            status: StatusCode::OK,
            body: r#"{"data":[{"id":"gpt-5.6-sol"},{"id":"gpt-5.6-sol"},{"id":"  "},{"id":"gpt-5.6-terra"}]}"#.to_string(),
            elapsed_ms: 12,
        };
        assert_eq!(
            model_ids(&response).unwrap(),
            vec!["gpt-5.6-sol".to_string(), "gpt-5.6-terra".to_string()]
        );
    }

    #[test]
    fn validates_minimal_responses_shape() {
        let valid = HttpResponse {
            status: StatusCode::OK,
            body: r#"{"id":"resp_test","status":"completed"}"#.to_string(),
            elapsed_ms: 1,
        };
        assert!(validate_responses_body(&valid).is_ok());

        let invalid_json = HttpResponse {
            status: StatusCode::OK,
            body: "not-json".to_string(),
            elapsed_ms: 1,
        };
        assert_eq!(
            validate_responses_body(&invalid_json).unwrap_err().code,
            "API_INVALID_RESPONSE"
        );

        let invalid_shape = HttpResponse {
            status: StatusCode::OK,
            body: r#"{"ok":true}"#.to_string(),
            elapsed_ms: 1,
        };
        assert_eq!(
            validate_responses_body(&invalid_shape).unwrap_err().code,
            "API_INVALID_RESPONSE"
        );
    }
}
