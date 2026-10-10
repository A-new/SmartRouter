// Types shared across the remaining local_llm pieces.

/// Local server status.
///
/// The bundled engine was removed, so in production this is always the
/// default (not running) value; the shape is kept because the Smart Router
/// inspects it before deciding whether a local upstream is available.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalServerInfo {
    pub running: bool,
    pub port: u16,
    pub model_name: String,
    pub pid: Option<u32>,
    pub api_key: String,
    pub runtime: String,
}

impl Default for LocalServerInfo {
    fn default() -> Self {
        Self {
            running: false,
            port: 0,
            model_name: String::new(),
            pid: None,
            api_key: String::new(),
            runtime: "llama-server".to_string(),
        }
    }
}
