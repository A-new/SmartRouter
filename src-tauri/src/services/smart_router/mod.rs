mod server;

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::models::model::{ModelConfig, ModelScope, ModelType};
use crate::services::model_manager;
use crate::utils::platform::echobird_dir;

pub const SMART_ROUTER_PORT: u16 = 53683;
pub const SMART_ROUTER_INTERNAL_ID: &str = "smart-router";
pub const SMART_ROUTER_MODEL_ID: &str = "auto";

static CONFIG_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredConfig {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    candidate_ids: Vec<String>,
    #[serde(default)]
    api_key: String,
}

impl Default for StoredConfig {
    fn default() -> Self {
        Self {
            version: default_version(),
            candidate_ids: Vec::new(),
            api_key: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicConfig {
    pub candidate_ids: Vec<String>,
    /// Callable group names, so the UI can show what to put in a client's
    /// `model` field next to the base URL and API key.
    pub groups: Vec<String>,
    pub usable_candidate_count: usize,
    pub base_url: String,
    pub model_id: String,
    pub port: u16,
    pub running: bool,
    pub api_key: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicActivity {
    pub candidate_id: Option<String>,
    pub active: bool,
    pub sequence: u64,
    pub updated_at_ms: u64,
    /// Models the ledger is holding back right now, so the candidate list can
    /// explain why a pool is temporarily running on fewer models.
    pub cooling: Vec<PublicCandidateState>,
}

/// One model's temporary unavailability, as the routing ledger sees it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicCandidateState {
    pub candidate_id: String,
    /// Instant the cooldown expires, in ms since the epoch.
    pub cooldown_until_ms: u64,
}

fn default_version() -> u32 {
    1
}

fn config_path() -> PathBuf {
    echobird_dir().join("config").join("smart-router.json")
}

fn config_lock() -> &'static Mutex<()> {
    CONFIG_LOCK.get_or_init(|| Mutex::new(()))
}

fn generate_api_key() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    format!("eb-local-{}", hex::encode(bytes))
}

fn save_config_unlocked(config: &StoredConfig) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("create smart router config directory failed: {e}"))?;
    }
    let content = serde_json::to_string_pretty(config)
        .map_err(|e| format!("serialize smart router config failed: {e}"))?;
    fs::write(&path, content).map_err(|e| format!("write {} failed: {e}", path.display()))
}

fn load_config_unlocked() -> Result<StoredConfig, String> {
    let path = config_path();
    let mut config = if path.exists() {
        let content = fs::read_to_string(&path)
            .map_err(|e| format!("read {} failed: {e}", path.display()))?;
        serde_json::from_str(&content)
            .map_err(|e| format!("parse {} failed: {e}", path.display()))?
    } else {
        StoredConfig::default()
    };

    if config.api_key.is_empty() {
        config.api_key = model_manager::encrypt_key_for_storage(&generate_api_key());
        save_config_unlocked(&config)?;
    } else if !config.api_key.starts_with("enc:v1:") {
        config.api_key = model_manager::encrypt_key_for_storage(&config.api_key);
        save_config_unlocked(&config)?;
    }

    Ok(config)
}

fn load_config() -> Result<StoredConfig, String> {
    let _guard = config_lock()
        .lock()
        .map_err(|_| "smart router config lock poisoned".to_string())?;
    load_config_unlocked()
}

fn public_config(config: StoredConfig) -> PublicConfig {
    let usable_candidate_count = usable_candidate_count(&config.candidate_ids);
    let groups = groups_for(&config.candidate_ids);
    // Expose the decrypted local router key so the UI can show it next to the
    // OpenAI/Anthropic base URLs — clients pointing at the loopback router need
    // to send this key to authenticate.
    let api_key = model_manager::decrypt_key_for_use(&config.api_key);
    PublicConfig {
        candidate_ids: config.candidate_ids,
        groups,
        usable_candidate_count,
        base_url: format!("http://127.0.0.1:{SMART_ROUTER_PORT}/v1"),
        model_id: SMART_ROUTER_MODEL_ID.to_string(),
        port: SMART_ROUTER_PORT,
        running: RUNNING.load(Ordering::Relaxed),
        api_key,
    }
}

pub fn get_public_config() -> Result<PublicConfig, String> {
    load_config().map(public_config)
}

pub fn get_public_activity() -> PublicActivity {
    server::public_activity()
}

pub fn set_candidate_ids(candidate_ids: Vec<String>) -> Result<PublicConfig, String> {
    let valid_user_ids: HashSet<String> = model_manager::get_user_models()
        .into_iter()
        .filter(model_is_routable)
        .map(|model| model.internal_id)
        .collect();
    let mut seen = HashSet::new();
    let candidate_ids: Vec<String> = candidate_ids
        .into_iter()
        .filter(|id| id != SMART_ROUTER_INTERNAL_ID)
        .filter(|id| valid_user_ids.contains(id) || id == "local-server")
        .filter(|id| seen.insert(id.clone()))
        .collect();
    if candidate_ids.len() > server::MAX_ROUTE_CANDIDATES {
        return Err(format!(
            "Smart Router supports up to {} candidate models",
            server::MAX_ROUTE_CANDIDATES
        ));
    }

    let _guard = config_lock()
        .lock()
        .map_err(|_| "smart router config lock poisoned".to_string())?;
    let mut config = load_config_unlocked()?;
    config.candidate_ids = candidate_ids;
    save_config_unlocked(&config)?;
    server::retain_candidate_memory(&config.candidate_ids);
    Ok(public_config(config))
}

pub fn get_candidate_models() -> Vec<ModelConfig> {
    candidate_models_for(&candidate_ids())
}

/// Callable model names the router exposes beyond `auto`: every distinct router
/// group that currently has at least one usable candidate.
pub(crate) fn candidate_groups() -> Vec<String> {
    groups_for(&candidate_ids())
}

/// Group names for an already-loaded candidate id list. Kept separate from
/// [`candidate_groups`] so callers that already hold the config lock (see
/// [`set_candidate_ids`]) can reuse it without re-locking and deadlocking —
/// `config_lock` is a plain `std::sync::Mutex` and is not reentrant.
fn groups_for(ids: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut groups = Vec::new();
    for model in candidate_models_for(ids) {
        if let Some(group) = model.router_group {
            if seen.insert(group.to_lowercase()) {
                groups.push(group);
            }
        }
    }
    groups.sort_by_key(|group| group.to_lowercase());
    groups
}

fn candidate_models_for(ids: &[String]) -> Vec<ModelConfig> {
    let candidate_ids: HashSet<String> = ids.iter().cloned().collect();
    let mut models: Vec<ModelConfig> = model_manager::get_user_models()
        .into_iter()
        .filter(|model| candidate_ids.contains(&model.internal_id) && model_is_routable(model))
        .collect();
    if candidate_ids.contains("local-server") {
        let local_server = crate::services::local_llm::get_server_info_sync();
        if local_server_is_routable(&local_server) {
            models.push(ModelConfig {
                internal_id: "local-server".to_string(),
                name: local_server.model_name.clone(),
                model_id: Some(local_server.model_name),
                base_url: format!("http://127.0.0.1:{}/v1", local_server.port),
                api_key: local_server.api_key,
                anthropic_url: Some(format!("http://127.0.0.1:{}/anthropic", local_server.port)),
                model_type: Some(ModelType::Local),
                openai_tested: None,
                anthropic_tested: None,
                openai_latency: None,
                anthropic_latency: None,
                scope: ModelScope::ModelCenter,
                router_group: None,
                // A local llama-server has no quota to respect.
                router_rpm: None,
            });
        }
    }
    models
}

fn model_is_routable(model: &ModelConfig) -> bool {
    model.internal_id != SMART_ROUTER_INTERNAL_ID
        && model
            .model_id
            .as_deref()
            .is_some_and(|model_id| !model_id.trim().is_empty())
        // One URL is enough, and it has to be the one the router would actually
        // dial: guarding a URL the router never calls would let a candidate whose
        // real endpoint points back at this port recurse into its own listener.
        && server::upstream_endpoint(model).is_some_and(|(_, url)| !url.contains(":53683"))
        && !model_manager::decrypt_key_for_use(&model.api_key).is_empty()
}

fn local_server_is_routable(server: &crate::services::local_llm::LocalServerInfo) -> bool {
    server.running
        && server.port != 0
        && !server.model_name.trim().is_empty()
        && !server.api_key.is_empty()
}

fn router_owns_model(model: &ModelConfig) -> bool {
    model.scope == ModelScope::SmartRouter
}

fn usable_candidate_count(candidate_ids: &[String]) -> usize {
    let user_models = model_manager::get_user_models();
    let local_server = crate::services::local_llm::get_server_info_sync();
    candidate_ids
        .iter()
        .filter(|candidate_id| {
            (candidate_id.as_str() == "local-server" && local_server_is_routable(&local_server))
                || user_models.iter().any(|model| {
                    model.internal_id == candidate_id.as_str() && model_is_routable(model)
                })
        })
        .count()
}

pub fn remove_candidate(internal_id: &str) -> Result<PublicConfig, String> {
    let configured_ids = candidate_ids();
    let is_configured = configured_ids.iter().any(|id| id == internal_id);
    let stored_model = model_manager::get_user_models()
        .into_iter()
        .find(|model| model.internal_id == internal_id);
    let delete_owned_model = is_configured
        && internal_id != "local-server"
        && stored_model.as_ref().is_some_and(router_owns_model);

    let remaining = configured_ids
        .into_iter()
        .filter(|id| id != internal_id)
        .collect();
    let config = set_candidate_ids(remaining)?;
    if delete_owned_model && !model_manager::delete_model(internal_id) {
        return Err(format!("Smart Router candidate not found: {internal_id}"));
    }
    Ok(config)
}

pub(crate) fn forget_candidate_memory(internal_id: &str) {
    server::forget_candidate_memory(internal_id);
}

pub(crate) fn candidate_ids() -> Vec<String> {
    load_config()
        .map(|config| config.candidate_ids)
        .unwrap_or_else(|e| {
            log::error!("[SmartRouter] Failed to load candidates: {e}");
            Vec::new()
        })
}

pub(crate) fn api_key_for_use() -> Result<String, String> {
    let config = load_config()?;
    let api_key = model_manager::decrypt_key_for_use(&config.api_key);
    if api_key.is_empty() {
        Err("smart router API key could not be decrypted".to_string())
    } else {
        Ok(api_key)
    }
}

pub fn model_config() -> Option<ModelConfig> {
    let config = load_config().ok()?;
    if usable_candidate_count(&config.candidate_ids) == 0 {
        return None;
    }

    Some(ModelConfig {
        internal_id: SMART_ROUTER_INTERNAL_ID.to_string(),
        name: "Auto Router".to_string(),
        model_id: Some(SMART_ROUTER_MODEL_ID.to_string()),
        base_url: format!("http://127.0.0.1:{SMART_ROUTER_PORT}/v1"),
        api_key: config.api_key,
        anthropic_url: Some(format!("http://127.0.0.1:{SMART_ROUTER_PORT}")),
        model_type: Some(ModelType::Local),
        openai_tested: None,
        anthropic_tested: None,
        openai_latency: None,
        anthropic_latency: None,
        scope: ModelScope::ModelCenter,
        router_group: None,
        router_rpm: None,
    })
}

pub fn spawn_proxy_task() {
    tauri::async_runtime::spawn(async {
        match server::run(SMART_ROUTER_PORT).await {
            Ok(()) => RUNNING.store(false, Ordering::Relaxed),
            Err(e) => {
                RUNNING.store(false, Ordering::Relaxed);
                log::error!("[SmartRouter] {e}");
            }
        }
    });
}

pub(crate) fn mark_running() {
    RUNNING.store(true, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_api_key_is_prefixed_and_random_sized() {
        let key = generate_api_key();
        assert!(key.starts_with("eb-local-"));
        assert_eq!(key.len(), "eb-local-".len() + 64);
    }

    #[test]
    fn candidate_requires_model_url_and_key() {
        let mut model = ModelConfig {
            internal_id: "candidate".to_string(),
            name: "Candidate".to_string(),
            model_id: Some("model-id".to_string()),
            base_url: "https://example.com/v1".to_string(),
            api_key: "key".to_string(),
            anthropic_url: None,
            model_type: Some(ModelType::Cloud),
            openai_tested: None,
            anthropic_tested: None,
            openai_latency: None,
            anthropic_latency: None,
            scope: ModelScope::SmartRouter,
            router_group: None,
            router_rpm: None,
        };
        assert!(model_is_routable(&model));

        model.model_id = Some(String::new());
        assert!(!model_is_routable(&model));
        model.model_id = Some("model-id".to_string());
        model.base_url.clear();
        assert!(!model_is_routable(&model));

        // An Anthropic URL on its own is enough to route.
        model.anthropic_url = Some("https://api.anthropic.com".to_string());
        assert!(model_is_routable(&model));
        // ...unless it points back at this router, which would recurse.
        model.anthropic_url = Some("http://127.0.0.1:53683".to_string());
        assert!(!model_is_routable(&model));

        model.anthropic_url = None;
        model.base_url = "https://example.com/v1".to_string();
        model.api_key.clear();
        assert!(!model_is_routable(&model));
    }

    #[test]
    fn upstream_endpoint_prefers_openai_and_falls_back_to_anthropic() {
        let mut model = ModelConfig {
            internal_id: "candidate".to_string(),
            name: "Candidate".to_string(),
            model_id: Some("model-id".to_string()),
            base_url: "https://example.com/v1".to_string(),
            api_key: "key".to_string(),
            anthropic_url: Some("https://api.anthropic.com".to_string()),
            model_type: Some(ModelType::Cloud),
            openai_tested: None,
            anthropic_tested: None,
            openai_latency: None,
            anthropic_latency: None,
            scope: ModelScope::SmartRouter,
            router_group: None,
            router_rpm: None,
        };
        let endpoint = |model: &ModelConfig| {
            server::upstream_endpoint(model).map(|(protocol, url)| (protocol, url.to_string()))
        };

        // Both set: OpenAI wins, so existing models keep dialling the same host.
        assert_eq!(
            endpoint(&model),
            Some((
                server::UpstreamProtocol::OpenAi,
                "https://example.com/v1".to_string()
            ))
        );

        model.base_url.clear();
        assert_eq!(
            endpoint(&model),
            Some((
                server::UpstreamProtocol::Anthropic,
                "https://api.anthropic.com".to_string()
            ))
        );

        model.anthropic_url = None;
        assert!(endpoint(&model).is_none());
    }

    #[test]
    fn router_owns_only_router_scoped_models() {
        let mut model = ModelConfig {
            internal_id: "candidate".to_string(),
            name: "Candidate".to_string(),
            model_id: Some("model-id".to_string()),
            base_url: "https://example.com/v1".to_string(),
            api_key: "key".to_string(),
            anthropic_url: None,
            model_type: Some(ModelType::Cloud),
            openai_tested: None,
            anthropic_tested: None,
            openai_latency: None,
            anthropic_latency: None,
            scope: ModelScope::ModelCenter,
            router_group: None,
            router_rpm: None,
        };

        assert!(!router_owns_model(&model));
        model.scope = ModelScope::SmartRouter;
        assert!(router_owns_model(&model));
    }

    #[test]
    fn stopped_local_server_is_not_usable() {
        let server = crate::services::local_llm::LocalServerInfo {
            running: false,
            port: 53682,
            model_name: "local-model".to_string(),
            pid: None,
            api_key: "local-key".to_string(),
            runtime: "llama-server".to_string(),
        };
        assert!(!local_server_is_routable(&server));

        let running = crate::services::local_llm::LocalServerInfo {
            running: true,
            ..server
        };
        assert!(local_server_is_routable(&running));
    }
}
