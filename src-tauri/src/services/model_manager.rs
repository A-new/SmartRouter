// Model Manager �?mirrors old modelManager.ts
// Handles model CRUD, API testing, ping, and API key encryption

use std::fs;

use crate::models::model::{ModelConfig, ModelType, ROUTER_RPM_UNLIMITED};
use crate::utils::platform::echobird_dir;

// ─── Config paths ───

/// Models config file: ~/.smartrouter/config/models.json
fn models_config_path() -> std::path::PathBuf {
    echobird_dir().join("config").join("models.json")
}

/// Ensure ~/.smartrouter/config/ exists
fn ensure_config_dir() {
    let config_dir = echobird_dir().join("config");
    let _ = fs::create_dir_all(&config_dir);
}

// ─── User-defined display order ───
//
// The visible model list is a merge (local server → user → built-in). An
// older build persisted a drag-reorder here as a JSON array of internal_ids,
// which get_models() still honours on read. Ids absent from the stored
// order (newly added models, built-ins shipped in a later update) keep their
// default merge position and sit below the ordered ones.

/// Model order file: ~/.smartrouter/config/model-order.json
fn model_order_path() -> std::path::PathBuf {
    echobird_dir().join("config").join("model-order.json")
}

fn load_model_order() -> Vec<String> {
    let path = model_order_path();
    if !path.exists() {
        return Vec::new();
    }
    match fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
            log::error!("[ModelManager] Failed to parse model order: {}", e);
            Vec::new()
        }),
        Err(e) => {
            log::error!("[ModelManager] Failed to read model order: {}", e);
            Vec::new()
        }
    }
}

/// Sort models by the persisted user order. Known ids follow the stored rank;
/// unknown ids keep their default merge order and sink below ordered ones.
fn apply_user_order(mut models: Vec<ModelConfig>, order: &[String]) -> Vec<ModelConfig> {
    if order.is_empty() || models.len() < 2 {
        return models;
    }
    let rank: std::collections::HashMap<&str, usize> = order
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect();
    let default_rank: std::collections::HashMap<String, usize> = models
        .iter()
        .enumerate()
        .map(|(i, m)| (m.internal_id.clone(), i))
        .collect();
    models.sort_by_key(|m| match rank.get(m.internal_id.as_str()) {
        Some(&r) => (0usize, r),
        None => (
            1usize,
            default_rank
                .get(&m.internal_id)
                .copied()
                .unwrap_or(usize::MAX),
        ),
    });
    models
}

// ─── API Key encryption (AES-256-GCM) ───

const ENCRYPTED_PREFIX: &str = "enc:v1:";

/// Check if a key is already encrypted
fn is_encrypted(key: &str) -> bool {
    key.starts_with(ENCRYPTED_PREFIX)
}

/// Get stable machine fingerprint (does NOT change on app upgrade/reinstall)
/// Windows: HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid
/// macOS: IOPlatformUUID via ioreg
/// Linux: /etc/machine-id
fn get_machine_fingerprint() -> String {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use std::process::Command;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        if let Ok(output) = Command::new("reg")
            .args([
                "query",
                r"HKLM\SOFTWARE\Microsoft\Cryptography",
                "/v",
                "MachineGuid",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            // Output format: "    MachineGuid    REG_SZ    <guid>"
            if let Some(line) = text.lines().find(|l| l.contains("MachineGuid")) {
                if let Some(guid) = line.split_whitespace().last() {
                    return guid.to_string();
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        if let Ok(output) = Command::new("ioreg")
            .args(["-rd1", "-c", "IOPlatformExpertDevice"])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                if line.contains("IOPlatformUUID") {
                    if let Some(uuid) = line.split('"').nth(3) {
                        return uuid.to_string();
                    }
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(id) = std::fs::read_to_string("/etc/machine-id") {
            let trimmed = id.trim().to_string();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
    }

    // Fallback: hostname via env var
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "echobird-fallback".to_string())
}

/// Get or create the persistent encryption key, derived with machine fingerprint.
/// Actual AES key = HMAC-SHA256(random_file_key, machine_fingerprint)
/// This ensures: same machine + same file = works; different machine = self-destruct.
fn get_encryption_key() -> Result<[u8; 32], String> {
    use hmac::Mac;
    type HmacSha256 = hmac::Hmac<sha2::Sha256>;

    let key_path = echobird_dir().join("config").join(".encryption_key");

    let file_key = if key_path.exists() {
        let data =
            fs::read(&key_path).map_err(|e| format!("Failed to read encryption key: {}", e))?;
        if data.len() == 32 {
            data
        } else {
            log::warn!("[ModelManager] Invalid encryption key file, regenerating");
            generate_key_file(&key_path)?
        }
    } else {
        generate_key_file(&key_path)?
    };

    // Derive actual key: HMAC-SHA256(file_key, machine_fingerprint + username)
    let fingerprint = get_machine_fingerprint();
    let username = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "default".to_string());
    let identity = format!("{}|{}", fingerprint, username);

    let mut mac = <HmacSha256 as Mac>::new_from_slice(&file_key)
        .map_err(|e| format!("HMAC init error: {}", e))?;
    hmac::Mac::update(&mut mac, identity.as_bytes());
    let result = mac.finalize().into_bytes();

    let mut derived_key = [0u8; 32];
    derived_key.copy_from_slice(&result);
    Ok(derived_key)
}

fn generate_key_file(key_path: &std::path::Path) -> Result<Vec<u8>, String> {
    use rand::RngCore;
    let mut key = vec![0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);

    ensure_config_dir();
    fs::write(key_path, &key).map_err(|e| format!("Failed to write encryption key: {}", e))?;
    log::info!("[ModelManager] Generated new encryption key");
    Ok(key)
}

/// Encrypt API key using AES-256-GCM.
/// Stores "enc:v1:<hex(nonce + ciphertext)>" in models.json.
fn encrypt_api_key(plain_key: &str) -> String {
    use aes_gcm::{aead::Aead, Aes256Gcm, KeyInit};
    use rand::RngCore;

    if plain_key.is_empty() || is_encrypted(plain_key) || plain_key == "local" {
        return plain_key.to_string();
    }

    let key_bytes = match get_encryption_key() {
        Ok(k) => k,
        Err(e) => {
            log::error!("[ModelManager] Encryption key error: {}", e);
            return plain_key.to_string();
        }
    };

    let cipher = Aes256Gcm::new((&key_bytes).into());

    // Generate random 12-byte nonce
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = aes_gcm::Nonce::from_slice(&nonce_bytes);

    match cipher.encrypt(nonce, plain_key.as_bytes()) {
        Ok(ciphertext) => {
            // Concatenate nonce + ciphertext, encode as hex
            let mut combined = Vec::with_capacity(12 + ciphertext.len());
            combined.extend_from_slice(&nonce_bytes);
            combined.extend_from_slice(&ciphertext);
            let hex = combined
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect::<String>();
            log::info!("[ModelManager] API key encrypted successfully");
            format!("{}{}", ENCRYPTED_PREFIX, hex)
        }
        Err(e) => {
            log::error!("[ModelManager] AES encryption failed: {}", e);
            plain_key.to_string()
        }
    }
}

/// Decrypt API key using AES-256-GCM.
fn decrypt_api_key(stored_key: &str) -> String {
    use aes_gcm::{aead::Aead, Aes256Gcm, KeyInit};

    if stored_key.is_empty() || !is_encrypted(stored_key) {
        return stored_key.to_string();
    }

    let hex_data = &stored_key[ENCRYPTED_PREFIX.len()..];

    // Decode hex to bytes
    let combined: Vec<u8> = match (0..hex_data.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex_data[i..i + 2], 16))
        .collect::<Result<Vec<u8>, _>>()
    {
        Ok(bytes) => bytes,
        Err(e) => {
            log::error!("[ModelManager] Hex decode failed: {}", e);
            return String::new();
        }
    };

    if combined.len() < 13 {
        log::error!("[ModelManager] Encrypted data too short");
        return String::new();
    }

    let key_bytes = match get_encryption_key() {
        Ok(k) => k,
        Err(e) => {
            log::error!("[ModelManager] Encryption key error: {}", e);
            return String::new();
        }
    };

    let cipher = Aes256Gcm::new((&key_bytes).into());
    let nonce = aes_gcm::Nonce::from_slice(&combined[..12]);
    let ciphertext = &combined[12..];

    match cipher.decrypt(nonce, ciphertext) {
        Ok(plaintext) => String::from_utf8(plaintext).unwrap_or_else(|_| {
            log::error!("[ModelManager] Decrypted key is not valid UTF-8");
            String::new()
        }),
        Err(e) => {
            log::error!("[ModelManager] AES decryption failed: {}", e);
            String::new()
        }
    }
}

/// Get usable plaintext API key (auto-decrypts if needed)
pub fn decrypt_key_for_use(api_key: &str) -> String {
    decrypt_api_key(api_key)
}

/// Encrypt a key/password for storage (public, reused by SSH)
pub fn encrypt_key_for_storage(plain_key: &str) -> String {
    encrypt_api_key(plain_key)
}

// ─── Model CRUD ───

/// Read raw user models from disk
fn get_raw_user_models() -> Vec<ModelConfig> {
    let path = models_config_path();
    if !path.exists() {
        return Vec::new();
    }
    match fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(e) => {
            log::error!("[ModelManager] Failed to load user models: {}", e);
            Vec::new()
        }
    }
}

/// Get user models
pub fn get_user_models() -> Vec<ModelConfig> {
    get_raw_user_models()
}

/// Get built-in demo models from bundled config
fn get_built_in_models() -> Vec<ModelConfig> {
    // Try multiple possible paths for bundled models.json
    let possible_paths = vec![
        // Development: project_root/config/models.json
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("config").join("models.json")))
            .unwrap_or_default(),
        // Alternative location
        echobird_dir().join("config").join("built-in-models.json"),
    ];

    for path in possible_paths {
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(models) = serde_json::from_str::<Vec<ModelConfig>>(&content) {
                    log::info!(
                        "[ModelManager] Loaded {} built-in models from {:?}",
                        models.len(),
                        path
                    );
                    return models;
                }
            }
        }
    }

    log::debug!("[ModelManager] No built-in models found");
    Vec::new()
}

/// Get all models (local server + user + built-in)
pub fn get_models() -> Vec<ModelConfig> {
    let user_models = get_user_models()
        .into_iter()
        .filter(|model| model.scope == crate::models::model::ModelScope::ModelCenter)
        .collect::<Vec<_>>();
    let built_in_models = get_built_in_models();

    // Local server model (dynamic injection when running)
    let server_info = crate::services::local_llm::get_server_info_sync();
    let local_models: Vec<ModelConfig> = if server_info.running {
        vec![ModelConfig {
            internal_id: "local-server".to_string(),
            name: server_info.model_name.clone(),
            model_id: Some(server_info.model_name.clone()),
            base_url: format!("http://127.0.0.1:{}/v1", server_info.port),
            api_key: server_info.api_key.clone(),
            anthropic_url: Some(format!("http://127.0.0.1:{}/anthropic", server_info.port)),
            model_type: Some(crate::models::model::ModelType::Local),
            openai_tested: None,
            anthropic_tested: None,
            openai_latency: None,
            anthropic_latency: None,
            scope: crate::models::model::ModelScope::ModelCenter,
            router_group: None,
            router_rpm: None,
        }]
    } else {
        Vec::new()
    };

    let smart_router_models: Vec<ModelConfig> = crate::services::smart_router::model_config()
        .into_iter()
        .collect();

    // Order: local → smart router → user → built-in, then apply the user's drag-reorder
    let mut all = Vec::new();
    all.extend(local_models);
    all.extend(smart_router_models);
    all.extend(user_models);
    all.extend(built_in_models);
    apply_user_order(all, &load_model_order())
}

/// Generate unique internal ID (m-abc123 format)
fn generate_internal_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("m-{:x}", seed % 0xFFFFFF)
}

/// Auto-detect model type from URL
fn detect_model_type(url: &str) -> Option<ModelType> {
    if url.is_empty() {
        return None;
    }
    if url.contains("localhost") || url.contains("127.0.0.1") || url.contains("192.168.") {
        Some(ModelType::Local)
    } else if url::Url::parse(url).is_ok() {
        Some(ModelType::Cloud)
    } else {
        None
    }
}

/// Save user models to disk
fn save_user_models(models: &[ModelConfig]) {
    ensure_config_dir();
    let path = models_config_path();
    let content = serde_json::to_string_pretty(models).unwrap_or_default();
    if let Err(e) = fs::write(&path, content) {
        log::error!("[ModelManager] Failed to save models: {}", e);
    }
}

/// Trim a user-supplied Smart Router group name. Blank input clears the group,
/// which leaves the candidate reachable through the `auto` alias only.
pub fn normalize_router_group(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Upper bound for a declared RPM budget. Anything above this is nonsense input
/// (the router tracks at most a few hundred sends per minute), so it is treated
/// as "no declared limit" rather than honoured.
pub const MAX_ROUTER_RPM: i32 = 100_000;

/// Sanitize a declared Smart Router RPM budget. `None` and `0` both mean "no
/// declaration", matching how a blank group name clears the group.
///
/// [`ROUTER_RPM_UNLIMITED`] survives the filter: it is a declaration, just not a
/// numeric one ("this key is paid, don't throttle it").
pub fn normalize_router_rpm(raw: Option<i32>) -> Option<i32> {
    raw.filter(|rpm| *rpm == ROUTER_RPM_UNLIMITED || (1..=MAX_ROUTER_RPM).contains(rpm))
}

/// Add a new model
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddModelInput {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub anthropic_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub router_group: Option<String>,
    #[serde(default)]
    pub router_rpm: Option<i32>,
    #[serde(default)]
    pub scope: crate::models::model::ModelScope,
}

pub fn add_model(input: AddModelInput) -> ModelConfig {
    let mut models = get_user_models();

    let base_url = input.base_url.unwrap_or_default();
    // A model may carry only an Anthropic URL now, so the type follows whichever
    // of the two URLs the user actually gave — otherwise a perfectly usable
    // model would be saved with no type and show as untested in the UI.
    let auto_type = detect_model_type(&base_url)
        .or_else(|| input.anthropic_url.as_deref().and_then(detect_model_type));
    let internal_id = generate_internal_id();

    let new_model = ModelConfig {
        internal_id: internal_id.clone(),
        name: input.name.unwrap_or_default(),
        model_id: input.model_id,
        base_url,
        api_key: input.api_key.unwrap_or_default(),
        anthropic_url: input.anthropic_url,
        model_type: auto_type,
        openai_tested: None,
        anthropic_tested: None,
        openai_latency: None,
        anthropic_latency: None,
        scope: input.scope,
        router_group: input
            .router_group
            .as_deref()
            .and_then(normalize_router_group),
        router_rpm: normalize_router_rpm(input.router_rpm),
    };

    log::info!(
        "[ModelManager] Added model: {} ({})",
        new_model.name,
        internal_id
    );
    models.push(new_model.clone());
    save_user_models(&models);
    new_model
}

/// Delete a user model by internal ID
pub fn delete_model(internal_id: &str) -> bool {
    let mut models = get_user_models();
    let original_len = models.len();
    models.retain(|m| m.internal_id != internal_id);

    if models.len() == original_len {
        return false; // Not found
    }

    save_user_models(&models);
    log::info!("[ModelManager] Deleted model: {}", internal_id);
    true
}

/// Update model fields
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateModelInput {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub anthropic_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub router_group: Option<String>,
    #[serde(default)]
    pub router_rpm: Option<i32>,
    #[serde(default)]
    pub openai_tested: Option<bool>,
    #[serde(default)]
    pub anthropic_tested: Option<bool>,
    #[serde(default)]
    pub openai_latency: Option<f64>,
    #[serde(default)]
    pub anthropic_latency: Option<f64>,
}

pub fn update_model(internal_id: &str, updates: UpdateModelInput) -> Option<ModelConfig> {
    let mut models = get_user_models();
    let index = models.iter().position(|m| m.internal_id == internal_id)?;

    // Apply updates
    if let Some(name) = updates.name {
        models[index].name = name;
    }
    if let Some(url) = &updates.base_url {
        models[index].base_url = url.clone();
        // Falling back to the Anthropic URL keeps the type (and its UI badge)
        // when the OpenAI field is the one left blank.
        let anthropic_url = models[index].anthropic_url.clone();
        models[index].model_type =
            detect_model_type(url).or_else(|| anthropic_url.as_deref().and_then(detect_model_type));
    }
    if let Some(url) = updates.anthropic_url {
        models[index].anthropic_url = if url.is_empty() { None } else { Some(url) };
    }
    if let Some(key) = updates.api_key {
        models[index].api_key = key;
    }
    if let Some(id) = updates.model_id {
        models[index].model_id = Some(id);
    }
    if let Some(group) = updates.router_group.as_deref() {
        models[index].router_group = normalize_router_group(group);
    }
    if let Some(rpm) = updates.router_rpm {
        // `0` clears the declared budget, same as a blank group name.
        models[index].router_rpm = normalize_router_rpm(Some(rpm));
    }
    if let Some(v) = updates.openai_tested {
        models[index].openai_tested = Some(v);
    }
    if let Some(v) = updates.anthropic_tested {
        models[index].anthropic_tested = Some(v);
    }
    if let Some(v) = updates.openai_latency {
        models[index].openai_latency = Some(v);
    }
    if let Some(v) = updates.anthropic_latency {
        models[index].anthropic_latency = Some(v);
    }

    let updated = models[index].clone();
    save_user_models(&models);
    log::info!("[ModelManager] Updated model: {}", internal_id);
    Some(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::model::ModelConfig;

    fn mk_model(id: &str) -> ModelConfig {
        ModelConfig {
            internal_id: id.to_string(),
            name: id.to_string(),
            model_id: None,
            base_url: String::new(),
            api_key: String::new(),
            anthropic_url: None,
            model_type: None,
            openai_tested: None,
            anthropic_tested: None,
            openai_latency: None,
            anthropic_latency: None,
            scope: crate::models::model::ModelScope::ModelCenter,
            router_group: None,
            router_rpm: None,
        }
    }

    fn ids(models: &[ModelConfig]) -> Vec<String> {
        models.iter().map(|m| m.internal_id.clone()).collect()
    }

    #[test]
    fn apply_user_order_puts_known_ids_first_in_stored_order() {
        let models = vec![mk_model("a"), mk_model("b"), mk_model("c")];
        let order = vec!["c".to_string(), "a".to_string()];
        assert_eq!(ids(&apply_user_order(models, &order)), vec!["c", "a", "b"]);
    }

    #[test]
    fn apply_user_order_empty_order_keeps_default_merge() {
        let models = vec![mk_model("a"), mk_model("b")];
        assert_eq!(ids(&apply_user_order(models, &[])), vec!["a", "b"]);
    }

    #[test]
    fn apply_user_order_new_ids_sink_below_in_default_order() {
        // "b" is absent from the stored order (newly added) → below known "a"
        let models = vec![mk_model("b"), mk_model("a")];
        let order = vec!["a".to_string()];
        assert_eq!(ids(&apply_user_order(models, &order)), vec!["a", "b"]);
    }

    #[test]
    fn apply_user_order_ignores_stale_ids_from_deleted_models() {
        let models = vec![mk_model("a"), mk_model("b")];
        let order = vec!["b".to_string(), "gone".to_string(), "a".to_string()];
        assert_eq!(ids(&apply_user_order(models, &order)), vec!["b", "a"]);
    }

    #[test]
    fn normalize_router_rpm_treats_blank_and_zero_as_no_declaration() {
        assert_eq!(normalize_router_rpm(None), None);
        assert_eq!(normalize_router_rpm(Some(0)), None);
        assert_eq!(normalize_router_rpm(Some(20)), Some(20));
        assert_eq!(
            normalize_router_rpm(Some(MAX_ROUTER_RPM)),
            Some(MAX_ROUTER_RPM)
        );
        assert_eq!(normalize_router_rpm(Some(MAX_ROUTER_RPM + 1)), None);
        // `-1` is a declaration of a different kind and is kept; anything else
        // negative is nonsense and reads as "no declaration".
        assert_eq!(
            normalize_router_rpm(Some(ROUTER_RPM_UNLIMITED)),
            Some(ROUTER_RPM_UNLIMITED)
        );
        assert_eq!(normalize_router_rpm(Some(-2)), None);
    }

    #[test]
    fn apply_user_order_handles_single_model_and_duplicate_ids() {
        let single = vec![mk_model("a")];
        assert_eq!(
            ids(&apply_user_order(single, &["a".to_string()])),
            vec!["a"]
        );

        let models = vec![mk_model("a"), mk_model("b")];
        let order = vec!["b".to_string(), "b".to_string(), "a".to_string()];
        assert_eq!(ids(&apply_user_order(models, &order)), vec!["b", "a"]);
    }
}
