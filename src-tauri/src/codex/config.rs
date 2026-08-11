use crate::errors::{AppError, AppResult};
use crate::provider::{
    ProviderDefinition, GENERIC_ENV_KEY, GENERIC_PROVIDER_ID, MANAGED_PROVIDER_IDS,
};
use crate::security::hashes::sha256_bytes;
use crate::windows::atomic;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use toml_edit::{value, DocumentMut, Item, Table};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ConfigSnapshot {
    pub path: PathBuf,
    pub source: String,
    pub document: DocumentMut,
    pub current_provider: Option<String>,
    pub model: Option<String>,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderConfigStatus {
    pub configured: bool,
    pub requires_openai_auth: Option<bool>,
}

pub fn load(path: &Path) -> AppResult<ConfigSnapshot> {
    if !path.is_file() {
        return Err(AppError::new(
            "CONFIG-001",
            format!("Codex 配置文件不存在：{}", path.display()),
        ));
    }
    let source = fs::read_to_string(path)
        .map_err(|error| AppError::io("CONFIG-002", "无法读取 config.toml", &error))?;
    let document = source.parse::<DocumentMut>().map_err(|error| {
        AppError::new(
            "CONFIG-003",
            format!("config.toml 无法解析，原文件未修改：{error}"),
        )
    })?;
    let current_provider = document
        .get("model_provider")
        .and_then(Item::as_str)
        .map(ToOwned::to_owned);
    let model = document
        .get("model")
        .and_then(Item::as_str)
        .map(ToOwned::to_owned);
    let hash = sha256_bytes(source.as_bytes());
    Ok(ConfigSnapshot {
        path: path.to_path_buf(),
        source,
        document,
        current_provider,
        model,
        hash,
    })
}

pub fn preflight_writable(snapshot: &ConfigSnapshot) -> AppResult<()> {
    let metadata = fs::metadata(&snapshot.path)
        .map_err(|error| AppError::io("CONFIG-004", "无法检查 config.toml 权限", &error))?;
    if metadata.permissions().readonly() {
        return Err(AppError::new(
            "CONFIG-005",
            "config.toml 为只读文件，未执行切换。",
        ));
    }
    let parent = snapshot
        .path
        .parent()
        .ok_or_else(|| AppError::new("CONFIG-006", "config.toml 没有可写父目录。"))?;
    let probe = parent.join(format!(".switcher-write-test-{}", Uuid::new_v4()));
    let result = OpenOptions::new().create_new(true).write(true).open(&probe);
    match result {
        Ok(file) => {
            drop(file);
            let _ = fs::remove_file(probe);
            Ok(())
        }
        Err(error) => Err(AppError::io(
            "CONFIG-007",
            "Codex 配置目录不可写，未执行切换",
            &error,
        )),
    }
}

fn managed_table_mut<'a>(
    document: &'a mut DocumentMut,
    provider_id: &str,
) -> AppResult<&'a mut Table> {
    if !document.contains_key("model_providers") {
        document["model_providers"] = Item::Table(Table::new());
    }
    let providers = document["model_providers"].as_table_mut().ok_or_else(|| {
        AppError::new(
            "CONFIG-008",
            "model_providers 不是有效的 TOML 表，未执行切换。",
        )
    })?;
    if !providers.contains_key(provider_id) {
        providers.insert(provider_id, Item::Table(Table::new()));
    }
    providers
        .get_mut(provider_id)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| {
            AppError::new(
                "CONFIG-009",
                format!("model_providers.{provider_id} 不是有效的 TOML 表。"),
            )
        })
}

pub fn render_for_provider(
    snapshot: &ConfigSnapshot,
    provider: &ProviderDefinition,
    base_url: &str,
) -> AppResult<String> {
    let mut document = snapshot.document.clone();
    document["model_provider"] = value(provider.config_id.as_str());
    document["model"] = value(provider.model.as_str());
    document["model_reasoning_effort"] = value(provider.reasoning_effort.as_str());
    let table = managed_table_mut(&mut document, provider.config_id.as_str())?;
    table["name"] = value(provider.display_name.as_str());
    table["base_url"] = value(base_url);
    table["env_key"] = value(provider.env_key.as_str());
    table["wire_api"] = value("responses");
    table["requires_openai_auth"] = value(true);
    let rendered = document.to_string();
    rendered.parse::<DocumentMut>().map_err(|error| {
        AppError::new(
            "CONFIG-010",
            format!("生成的 Provider 配置未通过 TOML 校验：{error}"),
        )
    })?;
    Ok(rendered)
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn inspect_provider(
    snapshot: &ConfigSnapshot,
    provider: &ProviderDefinition,
    base_url: &str,
) -> ProviderConfigStatus {
    let table = snapshot
        .document
        .get("model_providers")
        .and_then(Item::as_table)
        .and_then(|providers| providers.get(provider.config_id.as_str()))
        .and_then(Item::as_table);
    let requires_openai_auth = table
        .and_then(|value| value.get("requires_openai_auth"))
        .and_then(Item::as_bool);
    let configured = table.is_some_and(|value| {
        value.get("name").and_then(Item::as_str) == Some(provider.display_name.as_str())
            && value.get("base_url").and_then(Item::as_str) == Some(base_url)
            && value.get("env_key").and_then(Item::as_str) == Some(provider.env_key.as_str())
            && value.get("wire_api").and_then(Item::as_str) == Some("responses")
            && requires_openai_auth == Some(true)
    });
    ProviderConfigStatus {
        configured,
        requires_openai_auth,
    }
}

pub fn inspect_managed_provider(snapshot: &ConfigSnapshot) -> ProviderConfigStatus {
    let table = snapshot
        .document
        .get("model_providers")
        .and_then(Item::as_table)
        .and_then(|providers| providers.get(GENERIC_PROVIDER_ID))
        .and_then(Item::as_table);
    let requires_openai_auth = table
        .and_then(|value| value.get("requires_openai_auth"))
        .and_then(Item::as_bool);
    let configured = table.is_some_and(|value| {
        value
            .get("base_url")
            .and_then(Item::as_str)
            .is_some_and(|base_url| base_url.trim_end_matches('/').ends_with("/v1"))
            && value.get("env_key").and_then(Item::as_str) == Some(GENERIC_ENV_KEY)
            && value.get("wire_api").and_then(Item::as_str) == Some("responses")
            && requires_openai_auth == Some(true)
    });
    ProviderConfigStatus {
        configured,
        requires_openai_auth,
    }
}

pub fn render_official_restore(
    current: &ConfigSnapshot,
    original: &ConfigSnapshot,
) -> AppResult<String> {
    let mut document = current.document.clone();
    for key in ["model_provider", "model", "model_reasoning_effort"] {
        match original.document.get(key) {
            Some(item) => {
                document[key] = item.clone();
            }
            None => {
                document.remove(key);
            }
        }
    }

    if !document.contains_key("model_providers") {
        document["model_providers"] = Item::Table(Table::new());
    }
    let current_providers = document["model_providers"]
        .as_table_mut()
        .ok_or_else(|| AppError::new("CONFIG-011", "当前 model_providers 结构无效，无法恢复。"))?;
    let original_providers = original
        .document
        .get("model_providers")
        .and_then(Item::as_table);

    for provider_id in MANAGED_PROVIDER_IDS {
        if let Some(original_item) = original_providers.and_then(|table| table.get(provider_id)) {
            current_providers.insert(provider_id, original_item.clone());
        } else {
            current_providers.remove(provider_id);
        }
    }

    let rendered = document.to_string();
    rendered.parse::<DocumentMut>().map_err(|error| {
        AppError::new(
            "CONFIG-012",
            format!("恢复后的配置未通过 TOML 校验：{error}"),
        )
    })?;
    Ok(rendered)
}

pub fn commit(path: &Path, rendered: &str) -> AppResult<()> {
    atomic::write_atomic(path, rendered.as_bytes())?;
    let verified = load(path)?;
    if verified.source != rendered {
        return Err(AppError::new(
            "CONFIG-013",
            "config.toml 写入后校验不一致，已停止后续操作。",
        )
        .changed(true));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{generic_provider, unrelated_provider, GENERIC_ENV_KEY};
    use std::fs;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, ConfigSnapshot) {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");
        fs::write(
            &path,
            "# keep me\nmodel = \"gpt-test\"\napproval_policy = \"on-request\"\n\n[mcp_servers.demo]\ncommand = \"demo\"\n",
        )
        .unwrap();
        let snapshot = load(&path).unwrap();
        (temp, snapshot)
    }

    #[test]
    fn provider_insert_preserves_unrelated_config_and_comments() {
        let (_temp, snapshot) = fixture();
        let provider = generic_provider("https://api.example.com/v1").unwrap();
        let rendered =
            render_for_provider(&snapshot, &provider, "https://api.example.com/v1").unwrap();
        assert!(rendered.contains("# keep me"));
        assert!(rendered.contains("approval_policy = \"on-request\""));
        assert!(rendered.contains("[mcp_servers.demo]"));
        assert!(rendered.contains("model_provider = \"vexlune_hub\""));
        assert!(rendered.contains("[model_providers.vexlune_hub]"));
        assert!(rendered.contains(format!("env_key = \"{GENERIC_ENV_KEY}\"").as_str()));
        assert!(rendered.contains("wire_api = \"responses\""));
        assert!(rendered.contains("requires_openai_auth = true"));
        assert!(rendered.contains("model = \"gpt-5.6-sol\""));
        assert!(rendered.contains("model_reasoning_effort = \"xhigh\""));
        assert!(!rendered.contains("API_KEY ="));
        assert_eq!(
            inspect_provider(
                &ConfigSnapshot {
                    document: rendered.parse::<DocumentMut>().unwrap(),
                    source: rendered.clone(),
                    current_provider: Some("vexlune_hub".to_string()),
                    model: Some("gpt-5.6-sol".to_string()),
                    hash: String::new(),
                    path: snapshot.path.clone(),
                },
                &provider,
                "https://api.example.com/v1",
            ),
            ProviderConfigStatus {
                configured: true,
                requires_openai_auth: Some(true),
            }
        );
    }

    #[test]
    fn provider_update_preserves_unrelated_provider() {
        let (_temp, snapshot) = fixture();
        let unrelated = unrelated_provider();
        let generic = generic_provider("https://api.example.com").unwrap();
        let first =
            render_for_provider(&snapshot, &unrelated, "https://api.example.invalid/v1").unwrap();
        fs::write(&snapshot.path, first).unwrap();
        let updated = load(&snapshot.path).unwrap();
        let second = render_for_provider(&updated, &generic, "https://api.example.com/v1").unwrap();
        assert!(second.contains("[model_providers.unrelated]"));
        assert!(second.contains("[model_providers.vexlune_hub]"));
        assert!(second.contains("model_provider = \"vexlune_hub\""));
    }

    #[test]
    fn selected_generic_model_is_written_without_changing_the_endpoint() {
        let (_temp, snapshot) = fixture();
        let mut provider = generic_provider("https://api.example.com").unwrap();
        provider.model = "gpt-5.6-terra".to_string();
        let rendered =
            render_for_provider(&snapshot, &provider, "https://api.example.com/v1").unwrap();
        assert!(rendered.contains("model = \"gpt-5.6-terra\""));
        assert!(rendered.contains("base_url = \"https://api.example.com/v1\""));
        assert!(rendered.contains("requires_openai_auth = true"));
    }

    #[test]
    fn official_restore_uses_real_original_provider() {
        let (_temp, original) = fixture();
        let generic = generic_provider("https://api.example.com").unwrap();
        let switched_text =
            render_for_provider(&original, &generic, "https://api.example.com/v1").unwrap();
        fs::write(&original.path, switched_text).unwrap();
        let switched = load(&original.path).unwrap();
        let restored = render_official_restore(&switched, &original).unwrap();
        assert!(!restored.contains("model_provider = \"vexlune_hub\""));
        assert!(!restored.contains("[model_providers.vexlune_hub]"));
        assert!(restored.contains("model = \"gpt-test\""));
        assert!(!restored.contains("model_reasoning_effort"));
        assert!(restored.contains("# keep me"));
    }
}
