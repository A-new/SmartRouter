// local_llm — remnants of the bundled local-LLM engine.
//
// The engine itself (llama-server lifecycle, GGUF/HF scanning, GPU probing,
// model downloads, the unified HTTP proxy) was removed together with its UI.
// What remains is the Anthropic↔OpenAI transform the Smart Router reuses,
// plus the LocalServerInfo shape it inspects before routing.

pub mod proxy;
pub mod types;

pub use types::LocalServerInfo;

/// The bundled engine is gone, so no local server is ever running.
///
/// Callers (Smart Router, ModelManager) keep their "is a local upstream
/// available?" branches; they now always take the false path.
pub fn get_server_info_sync() -> LocalServerInfo {
    LocalServerInfo::default()
}
