// Free-model catalog fetched only after the user requests it.

use serde_json::Value;

const REMOTE_URL: &str = "https://echobird.ai/api/free-models/index.json";

pub async fn fetch_free_model_directory() -> Value {
    let cache_dir = crate::utils::platform::echobird_dir().join("cache");
    let cache_path = cache_dir.join("free-models.json");

    if let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
    {
        if let Ok(response) = client
            .get(REMOTE_URL)
            .header("User-Agent", "SmartRouter/1.1")
            .send()
            .await
        {
            if response.status().is_success() {
                if let Ok(text) = response.text().await {
                    if let Ok(parsed) = serde_json::from_str::<Value>(&text) {
                        if has_catalog_shape(&parsed) {
                            let _ = std::fs::create_dir_all(&cache_dir);
                            let _ = std::fs::write(&cache_path, &text);
                            log::info!("[FreeModels] Loaded from remote");
                            return parsed;
                        }
                    }
                }
            }
        }
    }

    if let Ok(text) = std::fs::read_to_string(&cache_path) {
        if let Ok(parsed) = serde_json::from_str::<Value>(&text) {
            if has_catalog_shape(&parsed) {
                log::info!("[FreeModels] Loaded from disk cache");
                return parsed;
            }
        }
    }

    log::warn!("[FreeModels] Remote catalog and disk cache unavailable");
    Value::Null
}

fn has_catalog_shape(value: &Value) -> bool {
    value.get("version").and_then(Value::as_u64).is_some()
        && value.get("models").is_some_and(Value::is_array)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_versioned_catalog() {
        assert!(has_catalog_shape(&serde_json::json!({
            "version": 1,
            "models": []
        })));
    }

    #[test]
    fn rejects_missing_or_wrong_fields() {
        assert!(!has_catalog_shape(&serde_json::json!({ "models": [] })));
        assert!(!has_catalog_shape(&serde_json::json!({
            "version": 1,
            "models": {}
        })));
    }
}
