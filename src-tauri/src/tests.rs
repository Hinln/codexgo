use crate::codex::{config, sessions};
use crate::provider::{generic_provider, ProviderDefinition, GENERIC_ENV_KEY, GENERIC_PROVIDER_ID};
use crate::security::hashes::sha256_file;
use crate::storage::state;
use crate::windows::environment;
use std::fs;
use std::path::Path;
use tempfile::TempDir;
use zeroize::Zeroizing;

const MESSAGE_LINE: &str =
    r#"{"type":"response_item","payload":{"role":"user","content":"keep this message exact"}}"#;
const TEST_API_ROOT: &str = "https://api.example.com";
const TEST_API_BASE: &str = "https://api.example.com/v1";

fn create_home(original_provider: Option<&str>) -> TempDir {
    let home = TempDir::new().unwrap();
    let provider_line = original_provider
        .map(|value| format!("model_provider = \"{value}\"\n"))
        .unwrap_or_default();
    fs::write(
        home.path().join("config.toml"),
        format!(
            "# preserved\nmodel = \"test-model\"\n{provider_line}approval_policy = \"on-request\"\n\n[mcp_servers.demo]\ncommand = \"demo\"\n"
        ),
    )
    .unwrap();
    fs::write(
        home.path().join("auth.json"),
        r#"{"synthetic":"credential"}"#,
    )
    .unwrap();
    let sessions_directory = home.path().join("sessions");
    fs::create_dir_all(&sessions_directory).unwrap();
    let session_provider = original_provider.unwrap_or("openai");
    fs::write(
        sessions_directory.join("rollout.jsonl"),
        format!(
            "{{\"timestamp\":\"2026-07-25T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"session-1\",\"cwd\":\"C:\\\\project\",\"model_provider\":\"{session_provider}\"}}}}\n{MESSAGE_LINE}\n"
        ),
    )
    .unwrap();
    home
}

fn switch(home: &Path, provider: &ProviderDefinition) {
    let snapshot = config::load(&home.join("config.toml")).unwrap();
    let rendered = config::render_for_provider(&snapshot, provider, TEST_API_BASE).unwrap();
    config::commit(&snapshot.path, &rendered).unwrap();
    sessions::migrate_all(home, provider.config_id.as_str()).unwrap();
}

fn assert_identity_and_messages_unchanged(home: &Path, auth_hash: &str) {
    assert_eq!(sha256_file(&home.join("auth.json")).unwrap(), auth_hash);
    let session = fs::read_to_string(home.join("sessions").join("rollout.jsonl")).unwrap();
    assert!(session.contains(MESSAGE_LINE));
    assert!(session.contains(r#""id":"session-1""#));
    assert!(session.contains(r#""cwd":"C:\\project""#));
    assert!(!home.join("vexlune-api-key.txt").exists());
}

struct EnvironmentGuard(Option<environment::EnvironmentSnapshot>);

impl EnvironmentGuard {
    fn capture() -> Self {
        Self(Some(environment::snapshot_managed().unwrap()))
    }
}

impl Drop for EnvironmentGuard {
    fn drop(&mut self) {
        if let Some(snapshot) = self.0.take() {
            let _ = environment::restore(&snapshot);
        }
    }
}

#[test]
fn generic_switch_preserves_login_identity_and_session_content() {
    let home = create_home(None);
    let auth_hash = sha256_file(&home.path().join("auth.json")).unwrap();
    let provider = generic_provider(TEST_API_ROOT).unwrap();

    switch(home.path(), &provider);

    let switched = config::load(&home.path().join("config.toml")).unwrap();
    assert_eq!(
        switched.current_provider.as_deref(),
        Some(GENERIC_PROVIDER_ID)
    );
    assert!(switched.source.contains("[model_providers.vexlune_hub]"));
    assert!(switched
        .source
        .contains("base_url = \"https://api.example.com/v1\""));
    assert!(switched
        .source
        .contains("env_key = \"VEXLUNE_HUB_API_KEY\""));
    assert!(switched.source.contains("wire_api = \"responses\""));
    assert!(switched.source.contains("requires_openai_auth = true"));

    let session = fs::read_to_string(home.path().join("sessions").join("rollout.jsonl")).unwrap();
    assert!(session.contains(r#""model_provider":"vexlune_hub""#));
    assert_identity_and_messages_unchanged(home.path(), &auth_hash);
}

#[test]
fn generic_restore_recovers_the_real_original_provider() {
    let home = create_home(Some("custom-original"));
    let auth_hash = sha256_file(&home.path().join("auth.json")).unwrap();
    let original = config::load(&home.path().join("config.toml")).unwrap();
    let baseline = home.path().join("switcher-backups").join("baseline");
    fs::create_dir_all(&baseline).unwrap();
    sessions::backup_all(home.path(), &baseline).unwrap();

    switch(home.path(), &generic_provider(TEST_API_ROOT).unwrap());
    let current = config::load(&home.path().join("config.toml")).unwrap();
    let restored = config::render_official_restore(&current, &original).unwrap();
    config::commit(&current.path, &restored).unwrap();
    sessions::restore_from_baseline(home.path(), &baseline, "custom-original").unwrap();

    let final_config = config::load(&home.path().join("config.toml")).unwrap();
    assert_eq!(
        final_config.current_provider.as_deref(),
        Some("custom-original")
    );
    let session = fs::read_to_string(home.path().join("sessions").join("rollout.jsonl")).unwrap();
    assert!(session.contains(r#""model_provider":"custom-original""#));
    assert_identity_and_messages_unchanged(home.path(), &auth_hash);
}

#[test]
#[ignore = "writes and restores the current user's VEXLUNE_HUB_API_KEY"]
fn offline_transaction_switches_and_restores_with_temporary_home() {
    let _environment_guard = EnvironmentGuard::capture();
    let home = create_home(None);
    let auth_hash = sha256_file(&home.path().join("auth.json")).unwrap();
    let initial_config = config::load(&home.path().join("config.toml")).unwrap();
    let provider = generic_provider(TEST_API_ROOT).unwrap();
    let validation = crate::api::validator::ValidationResult {
        base_url: TEST_API_BASE.to_string(),
        http_status: 200,
        request_elapsed_ms: 1,
        validation_method: "test",
    };

    let switched = crate::application::apply_switch(
        home.path(),
        &initial_config,
        &provider,
        &validation,
        "synthetic-integration-key",
        |_| {},
    )
    .unwrap();
    assert!(switched.success);
    assert_eq!(
        config::load(&home.path().join("config.toml"))
            .unwrap()
            .current_provider
            .as_deref(),
        Some(GENERIC_PROVIDER_ID)
    );
    assert_identity_and_messages_unchanged(home.path(), &auth_hash);

    let restored = crate::application::restore_official(home.path(), |_| {}).unwrap();
    assert!(restored.success);
    assert_eq!(
        config::load(&home.path().join("config.toml"))
            .unwrap()
            .current_provider,
        None
    );
    assert!(!state::state_path(home.path()).exists());
    assert_eq!(
        environment::read_user(GENERIC_ENV_KEY).unwrap().as_deref(),
        Some("synthetic-integration-key")
    );
    assert_identity_and_messages_unchanged(home.path(), &auth_hash);
}

#[test]
#[ignore = "writes and restores the current user's VEXLUNE_HUB_API_KEY"]
fn offline_failed_transaction_rolls_back_temporary_home() {
    let _environment_guard = EnvironmentGuard::capture();
    let home = create_home(None);
    let config_path = home.path().join("config.toml");
    let session_path = home.path().join("sessions").join("rollout.jsonl");
    fs::write(&session_path, "{malformed\n").unwrap();
    let auth_hash = sha256_file(&home.path().join("auth.json")).unwrap();
    let config_hash = sha256_file(&config_path).unwrap();
    let session_hash = sha256_file(&session_path).unwrap();
    let initial_config = config::load(&config_path).unwrap();
    let provider = generic_provider(TEST_API_ROOT).unwrap();
    let validation = crate::api::validator::ValidationResult {
        base_url: TEST_API_BASE.to_string(),
        http_status: 200,
        request_elapsed_ms: 1,
        validation_method: "test",
    };

    let error = crate::application::apply_switch(
        home.path(),
        &initial_config,
        &provider,
        &validation,
        "synthetic-integration-key",
        |_| {},
    )
    .unwrap_err();

    assert!(error.config_changed);
    assert!(error.rolled_back);
    assert_eq!(sha256_file(&config_path).unwrap(), config_hash);
    assert_eq!(sha256_file(&session_path).unwrap(), session_hash);
    assert_eq!(
        sha256_file(&home.path().join("auth.json")).unwrap(),
        auth_hash
    );
    assert!(!state::state_path(home.path()).exists());
}

#[test]
#[ignore = "requires CODEX_SWITCHER_LIVE_KEY"]
fn live_generic_transaction_uses_temporary_codex_home() {
    let _environment_guard = EnvironmentGuard::capture();
    let key = Zeroizing::new(
        std::env::var("CODEX_SWITCHER_LIVE_KEY")
            .unwrap_or_default()
            .trim()
            .to_string(),
    );
    assert!(key.len() >= 8, "live test key was not provided");

    let home = create_home(None);
    let auth_hash = sha256_file(&home.path().join("auth.json")).unwrap();
    let initial_config = config::load(&home.path().join("config.toml")).unwrap();
    let provider = generic_provider("https://api.vexlune.com").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let validation = runtime
        .block_on(crate::api::validator::validate(
            &provider,
            key.as_str(),
            None,
        ))
        .unwrap();
    assert_eq!(validation.base_url, "https://api.vexlune.com/v1");

    let switched = crate::application::apply_switch(
        home.path(),
        &initial_config,
        &provider,
        &validation,
        key.as_str(),
        |_| {},
    )
    .unwrap();
    assert!(switched.success);
    assert_eq!(
        config::load(&home.path().join("config.toml"))
            .unwrap()
            .current_provider
            .as_deref(),
        Some(GENERIC_PROVIDER_ID)
    );
    assert!(environment::read_user(GENERIC_ENV_KEY).unwrap().is_some());
    assert_identity_and_messages_unchanged(home.path(), &auth_hash);

    let restored = crate::application::restore_official(home.path(), |_| {}).unwrap();
    assert!(restored.success);
    assert_eq!(
        config::load(&home.path().join("config.toml"))
            .unwrap()
            .current_provider,
        None
    );
    assert!(environment::read_user(GENERIC_ENV_KEY).unwrap().is_some());
    assert_identity_and_messages_unchanged(home.path(), &auth_hash);
}
