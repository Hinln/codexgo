use crate::errors::{AppError, AppResult};
use url::Url;

pub const GENERIC_PROVIDER_ID: &str = "vexlune_hub";
pub const GENERIC_ENV_KEY: &str = "VEXLUNE_HUB_API_KEY";
pub const GENERIC_DEFAULT_MODEL: &str = "gpt-5.6-sol";
pub const GENERIC_REASONING_EFFORT: &str = "xhigh";

#[derive(Debug, Clone)]
pub struct ProviderDefinition {
    pub config_id: String,
    pub display_name: String,
    pub root_url: String,
    pub expected_host: String,
    pub env_key: String,
    pub model: String,
    pub reasoning_effort: String,
}

fn provider(
    config_id: &str,
    display_name: &str,
    root_url: &str,
    expected_host: &str,
    env_key: &str,
    model: &str,
    reasoning_effort: &str,
) -> ProviderDefinition {
    ProviderDefinition {
        config_id: config_id.to_string(),
        display_name: display_name.to_string(),
        root_url: root_url.to_string(),
        expected_host: expected_host.to_string(),
        env_key: env_key.to_string(),
        model: model.to_string(),
        reasoning_effort: reasoning_effort.to_string(),
    }
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// Returns the user-facing API root without any trailing `/v1` segment.
pub fn normalize_user_api_root(input: &str) -> AppResult<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(AppError::new("API_URL_MISSING", "请输入 API 地址。"));
    }

    let mut url = Url::parse(trimmed)
        .map_err(|error| AppError::new("API_URL_FORMAT", format!("API 地址无效：{error}")))?;
    let host = url
        .host_str()
        .ok_or_else(|| AppError::new("API_URL_HOST", "API 地址缺少有效域名或主机。"))?;
    match url.scheme() {
        "https" => {}
        "http" if is_loopback_host(host) => {}
        "http" => {
            return Err(AppError::new(
                "API_URL_INSECURE",
                "远程 API 地址必须使用 HTTPS；只有本机地址可使用 HTTP。",
            ));
        }
        _ => {
            return Err(AppError::new(
                "API_URL_SCHEME",
                "API 地址仅支持 HTTPS，或本机 HTTP 地址。",
            ));
        }
    }
    if url.username() != "" || url.password().is_some() {
        return Err(AppError::new(
            "API_URL_USERINFO",
            "API 地址不得包含用户信息。",
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(AppError::new(
            "API_URL_COMPONENTS",
            "API 地址不得包含查询参数或片段。",
        ));
    }

    let mut segments = url
        .path_segments()
        .map(|values| {
            values
                .filter(|segment| !segment.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    while segments
        .last()
        .is_some_and(|segment| segment.eq_ignore_ascii_case("v1"))
    {
        segments.pop();
    }
    if segments.is_empty() {
        url.set_path("");
    } else {
        url.set_path(format!("/{}", segments.join("/")).as_str());
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

pub fn generic_provider(api_root: &str) -> AppResult<ProviderDefinition> {
    let normalized_root = normalize_user_api_root(api_root)?;
    let parsed = Url::parse(&normalized_root)
        .map_err(|error| AppError::new("API_URL_FORMAT", format!("API 地址无效：{error}")))?;
    let expected_host = parsed
        .host_str()
        .ok_or_else(|| AppError::new("API_URL_HOST", "API 地址缺少有效域名或主机。"))?;
    Ok(provider(
        GENERIC_PROVIDER_ID,
        "Vexlune Hub Custom API",
        normalized_root.as_str(),
        expected_host,
        GENERIC_ENV_KEY,
        GENERIC_DEFAULT_MODEL,
        GENERIC_REASONING_EFFORT,
    ))
}

#[cfg(test)]
pub fn unrelated_provider() -> ProviderDefinition {
    provider(
        "unrelated",
        "Unrelated Provider",
        "https://api.example.invalid",
        "api.example.invalid",
        "UNRELATED_API_KEY",
        "gpt-test",
        "high",
    )
}

pub const MANAGED_PROVIDER_IDS: [&str; 1] = [GENERIC_PROVIDER_ID];
pub const MANAGED_ENV_KEYS: [&str; 1] = [GENERIC_ENV_KEY];

pub fn from_config_id(value: &str) -> Option<&'static str> {
    MANAGED_PROVIDER_IDS
        .iter()
        .copied()
        .find(|candidate| *candidate == value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_one_or_more_trailing_v1_segments() {
        assert_eq!(
            normalize_user_api_root(" https://api.example.com/v1/v1/ ").unwrap(),
            "https://api.example.com"
        );
        assert_eq!(
            normalize_user_api_root("https://api.example.com/openai/v1").unwrap(),
            "https://api.example.com/openai"
        );
    }

    #[test]
    fn dynamic_provider_uses_normalized_root_and_managed_identity() {
        let provider = generic_provider("https://api.example.com/v1").unwrap();
        assert_eq!(provider.config_id, GENERIC_PROVIDER_ID);
        assert_eq!(provider.root_url, "https://api.example.com");
        assert_eq!(provider.expected_host, "api.example.com");
        assert_eq!(provider.env_key, GENERIC_ENV_KEY);
    }

    #[test]
    fn allows_local_http_but_rejects_remote_http_and_url_extras() {
        assert_eq!(
            normalize_user_api_root("http://127.0.0.1:1234/v1").unwrap(),
            "http://127.0.0.1:1234"
        );
        assert!(normalize_user_api_root("http://api.example.com").is_err());
        assert!(normalize_user_api_root("https://api.example.com?token=nope").is_err());
        assert!(normalize_user_api_root("https://user@api.example.com").is_err());
    }

    #[test]
    fn generic_edition_manages_only_its_own_provider_and_key() {
        assert_eq!(from_config_id("unrelated"), None);
        assert_eq!(from_config_id("vexlune"), None);
        assert_eq!(MANAGED_PROVIDER_IDS, [GENERIC_PROVIDER_ID]);
        assert_eq!(MANAGED_ENV_KEYS, [GENERIC_ENV_KEY]);
    }
}
