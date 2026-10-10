// Model configuration structures — mirrors old modelManager.ts types

use serde::{Deserialize, Serialize};

/// Model type indicator
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "UPPERCASE")]
pub enum ModelType {
    Cloud,
    Local,
    Tunnel,
    Demo,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ModelScope {
    #[default]
    ModelCenter,
    SmartRouter,
}

impl ModelScope {
    fn is_model_center(&self) -> bool {
        *self == Self::ModelCenter
    }
}

/// Model configuration (stored in ~/.smartrouter/models.json)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelConfig {
    pub internal_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anthropic_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "type")]
    pub model_type: Option<ModelType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openai_tested: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anthropic_tested: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openai_latency: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anthropic_latency: Option<f64>,
    #[serde(default, skip_serializing_if = "ModelScope::is_model_center")]
    pub scope: ModelScope,
    /// Optional logical-model group for the Smart Router. Candidates sharing a
    /// group are failover targets for each other, so a client calling the router
    /// with `model: <group>` stays inside one logical model even though each
    /// platform spells the underlying model id differently. `None` leaves the
    /// candidate reachable through the `auto` alias alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub router_group: Option<String>,
    /// Requests-per-minute budget the user declares for the account this model
    /// belongs to. Free tiers count requests per API key, so several models
    /// added from one key share this number.
    ///
    /// `None` (or `0` on input) leaves the limit to whatever the provider
    /// advertises and to its 429 responses; [`ROUTER_RPM_UNLIMITED`] says the
    /// account is paid and must not be throttled locally at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub router_rpm: Option<i32>,
}

/// [`ModelConfig::router_rpm`] value meaning "paid account, do not rate-limit it
/// locally": the router applies no cap of its own and ignores the one the
/// provider advertises, so only a 429 can slow the account down.
///
/// The UI sends `-1`. A blank field (`0` on the wire) is weaker — it means "I
/// don't know my limit", which still honours an advertised one.
pub const ROUTER_RPM_UNLIMITED: i32 = -1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_scope_defaults_to_model_center() {
        let model: ModelConfig = serde_json::from_value(serde_json::json!({
            "internalId": "existing",
            "name": "Existing",
            "baseUrl": "https://example.com/v1",
            "apiKey": "key"
        }))
        .unwrap();

        assert_eq!(model.scope, ModelScope::ModelCenter);
    }

    #[test]
    fn smart_router_scope_is_serialized() {
        let value = serde_json::to_value(ModelConfig {
            internal_id: "router-model".to_string(),
            name: "Router Model".to_string(),
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
        })
        .unwrap();

        assert_eq!(value["scope"], "smartRouter");
    }

    #[test]
    fn router_rpm_is_optional_and_camel_cased() {
        let model: ModelConfig = serde_json::from_value(serde_json::json!({
            "internalId": "existing",
            "name": "Existing",
            "baseUrl": "https://example.com/v1",
            "apiKey": "key"
        }))
        .unwrap();
        assert_eq!(model.router_rpm, None);

        let value = serde_json::to_value(ModelConfig {
            router_rpm: Some(20),
            ..model
        })
        .unwrap();
        assert_eq!(value["routerRpm"], 20);
    }

    #[test]
    fn router_rpm_round_trips_the_unlimited_sentinel() {
        let model: ModelConfig = serde_json::from_value(serde_json::json!({
            "internalId": "paid",
            "name": "Paid",
            "baseUrl": "https://example.com/v1",
            "apiKey": "key",
            "routerRpm": ROUTER_RPM_UNLIMITED
        }))
        .unwrap();
        assert_eq!(model.router_rpm, Some(ROUTER_RPM_UNLIMITED));
        assert_eq!(
            serde_json::to_value(model).unwrap()["routerRpm"],
            ROUTER_RPM_UNLIMITED
        );
    }
}
