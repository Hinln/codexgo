use crate::errors::{AppError, AppResult};
use crate::provider::ProviderDefinition;
use url::Url;

#[derive(Debug, Clone)]
pub struct EndpointCandidate {
    pub base_url: Url,
    pub models_url: Url,
    pub responses_url: Url,
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

pub fn validate_provider_url(url: &Url, expected_host: &str) -> AppResult<()> {
    let scheme_allowed = url.scheme() == "https"
        || (url.scheme() == "http" && url.host_str().is_some_and(is_loopback_host));
    if !scheme_allowed {
        return Err(AppError::new(
            "API-005",
            "远程 API 地址必须使用 HTTPS；只有本机地址可使用 HTTP。",
        ));
    }
    if url.host_str() != Some(expected_host) {
        return Err(AppError::new(
            "API-006",
            "API 地址域名在处理过程中发生变化，已停止请求。",
        ));
    }
    if url.username() != "" || url.password().is_some() {
        return Err(AppError::new("API-007", "API 地址不得包含用户信息。"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(AppError::new("API-008", "API 地址不得包含查询参数或片段。"));
    }
    Ok(())
}

pub fn normalize_api_base_url(provider: &ProviderDefinition) -> AppResult<Url> {
    let mut root = Url::parse(&provider.root_url)
        .map_err(|error| AppError::new("API-001", format!("API 地址无效：{error}")))?;
    validate_provider_url(&root, &provider.expected_host)?;

    let mut segments = root
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
    segments.push("v1".to_string());
    root.set_path(format!("/{}/", segments.join("/")).as_str());
    root.set_query(None);
    root.set_fragment(None);
    validate_provider_url(&root, &provider.expected_host)?;
    Ok(root)
}

pub fn candidates(provider: &ProviderDefinition) -> AppResult<Vec<EndpointCandidate>> {
    let base_url = normalize_api_base_url(provider)?;
    let models_url = base_url
        .join("models")
        .map_err(|error| AppError::new("API-003", format!("模型地址拼接失败：{error}")))?;
    let responses_url = base_url
        .join("responses")
        .map_err(|error| AppError::new("API-004", format!("Responses 地址拼接失败：{error}")))?;
    Ok(vec![EndpointCandidate {
        base_url,
        models_url,
        responses_url,
    }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::generic_provider;

    #[test]
    fn appends_exactly_one_v1_for_all_supported_input_forms() {
        for root in [
            "https://api.example.com",
            "https://api.example.com/",
            "https://api.example.com/v1",
            "https://api.example.com/v1/",
            "https://api.example.com/v1/v1/",
            "https://api.example.com/openai/v1",
        ] {
            let provider = generic_provider(root).unwrap();
            let values = candidates(&provider).unwrap();
            let expected_prefix = if root.contains("/openai") {
                "https://api.example.com/openai/v1"
            } else {
                "https://api.example.com/v1"
            };
            assert_eq!(values.len(), 1);
            assert_eq!(
                values[0].models_url.as_str(),
                format!("{expected_prefix}/models")
            );
            assert_eq!(
                values[0].responses_url.as_str(),
                format!("{expected_prefix}/responses")
            );
        }
    }

    #[test]
    fn accepts_loopback_http_and_preserves_port() {
        let provider = generic_provider("http://localhost:1234/v1").unwrap();
        let values = candidates(&provider).unwrap();
        assert_eq!(
            values[0].models_url.as_str(),
            "http://localhost:1234/v1/models"
        );
    }

    #[test]
    fn rejects_remote_http_and_unexpected_host() {
        assert!(generic_provider("http://api.example.com").is_err());
        assert!(validate_provider_url(
            &Url::parse("https://other.example.com/v1").unwrap(),
            "api.example.com"
        )
        .is_err());
    }
}
