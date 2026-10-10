use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use futures_util::{stream, StreamExt};
use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio_stream::wrappers::ReceiverStream;

use super::{
    api_key_for_use, candidate_ids, mark_running, PublicActivity, PublicCandidateState,
    SMART_ROUTER_INTERNAL_ID, SMART_ROUTER_MODEL_ID,
};
use crate::models::model::{ModelConfig, ROUTER_RPM_UNLIMITED};
use crate::services::model_manager;
use crate::utils::platform::echobird_dir;

const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_ROUTE_CANDIDATES: usize = 20;
/// Total time one client request may spend hopping between candidates.
///
/// Sized against the header timeout, not against a single call: a dead endpoint
/// burns the whole [`UPSTREAM_HEADER_TIMEOUT`] before it is written off, so a
/// budget only slightly above it gives up after one or two tries and reports
/// "all models failed" with a dozen healthy candidates never dialled.
const FALLBACK_TIME_BUDGET: Duration = Duration::from_secs(120);
const UPSTREAM_HEADER_TIMEOUT: Duration = Duration::from_secs(30);
const UPSTREAM_FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(120);
const TRANSIENT_COOLDOWN: Duration = Duration::from_secs(30);
const MAX_TRANSIENT_COOLDOWN: Duration = Duration::from_secs(5 * 60);
const RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(60);
const AUTH_OR_BILLING_COOLDOWN: Duration = Duration::from_secs(60 * 60);
const MODEL_UNAVAILABLE_COOLDOWN: Duration = Duration::from_secs(6 * 60 * 60);
const MAX_RETRY_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// Length of the per-account rate limit window. Free tiers are almost always
/// advertised as requests-per-minute, so the ledger is a sliding minute.
const RATE_WINDOW: Duration = Duration::from_secs(60);
/// Load added by each in-flight request when ranking accounts. Without it a
/// burst of concurrent requests would all pick the same (sticky) candidate and
/// burn its whole minute of quota in one go.
const INFLIGHT_LOAD_WEIGHT: f64 = 0.35;
/// The last-known-good candidate is only a tie-break (see [`plan_candidates`]);
/// this is the share of its quota below which it still qualifies as one. Past it
/// it ranks like every other candidate, so a nearly spent account is never
/// reused ahead of an idle one.
const STICKY_QUOTA_SHARE: f64 = 0.7;
/// Hard cap on tracked send timestamps per account, so a bogus `x-ratelimit-limit`
/// (or a paid key with a huge quota) cannot grow the ledger without bound.
const MAX_WINDOW_SLOTS: usize = 256;
/// A `Retry-After` at least this long is read as an account-wide instruction
/// (daily/hours quota) rather than a hiccup on a single model.
const ACCOUNT_RETRY_AFTER_FLOOR: Duration = RATE_WINDOW;

static ROUTE_MEMORY: OnceLock<Arc<Mutex<RouteMemory>>> = OnceLock::new();
static ROUTE_ACTIVITY: OnceLock<Mutex<RouteActivity>> = OnceLock::new();

#[derive(Default)]
struct RouteActivity {
    next_attempt_id: u64,
    active_attempts: BTreeMap<u64, String>,
    last_candidate_id: Option<String>,
    sequence: u64,
    updated_at_ms: u64,
}

impl RouteActivity {
    fn begin(&mut self, candidate_id: &str, updated_at_ms: u64) -> u64 {
        self.next_attempt_id = self.next_attempt_id.saturating_add(1);
        let attempt_id = self.next_attempt_id;
        self.active_attempts
            .insert(attempt_id, candidate_id.to_string());
        self.last_candidate_id = Some(candidate_id.to_string());
        self.sequence = self.sequence.saturating_add(1);
        self.updated_at_ms = updated_at_ms;
        attempt_id
    }

    fn end(&mut self, attempt_id: u64, updated_at_ms: u64) {
        if self.active_attempts.remove(&attempt_id).is_none() {
            return;
        }
        if let Some((_, candidate_id)) = self.active_attempts.last_key_value() {
            self.last_candidate_id = Some(candidate_id.clone());
        }
        self.sequence = self.sequence.saturating_add(1);
        self.updated_at_ms = updated_at_ms;
    }

    fn snapshot(&self) -> PublicActivity {
        PublicActivity {
            candidate_id: self.last_candidate_id.clone(),
            active: !self.active_attempts.is_empty(),
            sequence: self.sequence,
            updated_at_ms: self.updated_at_ms,
            cooling: Vec::new(),
        }
    }
}

struct RouteActivityGuard {
    attempt_id: Option<u64>,
}

impl RouteActivityGuard {
    fn begin(candidate_id: &str) -> Self {
        let attempt_id = ROUTE_ACTIVITY
            .get_or_init(|| Mutex::new(RouteActivity::default()))
            .lock()
            .ok()
            .map(|mut activity| activity.begin(candidate_id, now_ms()));
        Self { attempt_id }
    }
}

impl Drop for RouteActivityGuard {
    fn drop(&mut self) {
        let Some(attempt_id) = self.attempt_id else {
            return;
        };
        let Some(activity) = ROUTE_ACTIVITY.get() else {
            return;
        };
        if let Ok(mut activity) = activity.lock() {
            activity.end(attempt_id, now_ms());
        }
    }
}

pub(crate) fn public_activity() -> PublicActivity {
    // Read the ledger before taking the activity lock, so the two locks are
    // never held at once (a request holds the ledger while it routes).
    let cooling = cooling_candidates();
    let mut activity = ROUTE_ACTIVITY
        .get_or_init(|| Mutex::new(RouteActivity::default()))
        .lock()
        .map(|activity| activity.snapshot())
        .unwrap_or_default();
    activity.cooling = cooling;
    activity
}

/// Models the ledger is holding back at this instant, either because the attempt
/// failed (authentication, transport) or because the provider answered 429.
fn cooling_candidates() -> Vec<PublicCandidateState> {
    let memory = shared_route_memory();
    let Ok(memory) = memory.lock() else {
        return Vec::new();
    };
    let now = now_ms();
    memory
        .candidates
        .iter()
        .filter_map(|(candidate_id, health)| {
            health
                .cooldown_until_ms
                .filter(|until| *until > now)
                .map(|until| PublicCandidateState {
                    candidate_id: candidate_id.clone(),
                    cooldown_until_ms: until,
                })
        })
        .collect()
}

#[derive(Clone)]
struct AppState {
    http_client: reqwest::Client,
    route_memory: Arc<Mutex<RouteMemory>>,
    route_memory_path: Option<PathBuf>,
}

impl AppState {
    fn new() -> Result<Self, String> {
        let http_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .tcp_keepalive(Duration::from_secs(60))
            .build()
            .map_err(|e| format!("reqwest client build failed: {e}"))?;
        Ok(Self {
            http_client,
            route_memory: shared_route_memory(),
            route_memory_path: Some(route_memory_path()),
        })
    }

    #[cfg(test)]
    fn for_tests() -> Result<Self, String> {
        let http_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .tcp_keepalive(Duration::from_secs(60))
            .build()
            .map_err(|e| format!("reqwest client build failed: {e}"))?;
        Ok(Self {
            http_client,
            route_memory: Arc::new(Mutex::new(RouteMemory::default())),
            route_memory_path: None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RouteMemory {
    #[serde(default = "route_memory_version")]
    version: u32,
    #[serde(default)]
    last_success_id: Option<String>,
    #[serde(default)]
    candidates: HashMap<String, CandidateHealth>,
    /// Per-upstream-account rate limit ledger, keyed by [`account_key`]. Rate
    /// limits are enforced against the account (the API key), not the model, so
    /// several models configured from one free key share a single bucket.
    #[serde(default)]
    accounts: HashMap<String, AccountLimit>,
    /// When each candidate was last picked, keyed by internal id. Kept apart from
    /// [`Self::candidates`] on purpose: that map tracks failures and is dropped on
    /// success, while rotation needs these timestamps to outlive a good call.
    #[serde(default)]
    last_picked_ms: HashMap<String, u64>,
}

impl Default for RouteMemory {
    fn default() -> Self {
        Self {
            version: route_memory_version(),
            last_success_id: None,
            candidates: HashMap::new(),
            accounts: HashMap::new(),
            last_picked_ms: HashMap::new(),
        }
    }
}

/// What the user declared about one upstream account's rate limit, as read from
/// the configs of the models spending from it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum DeclaredRpm {
    /// The field was left blank on every model of this account: only what the
    /// provider advertises applies.
    #[default]
    Unknown,
    /// Some model on the account is marked `-1` — a paid key with no rate limit.
    /// The advertised limit is deliberately ignored too: the user said the plan
    /// has no ceiling, and a 429 still slows the account down on its own.
    Unlimited,
    /// The strictest positive budget declared by the models on this account.
    Capped(u32),
}

impl DeclaredRpm {
    /// Strictest of two declarations about the same account. A cap always beats
    /// [`Self::Unlimited`], so marking one model unlimited never loosens the
    /// quota its key-mates share.
    fn strictest(self, other: Self) -> Self {
        match (self, other) {
            (Self::Capped(left), Self::Capped(right)) => Self::Capped(left.min(right)),
            (Self::Capped(cap), _) | (_, Self::Capped(cap)) => Self::Capped(cap),
            (Self::Unlimited, _) | (_, Self::Unlimited) => Self::Unlimited,
            (Self::Unknown, Self::Unknown) => Self::Unknown,
        }
    }
}

/// Rolling minute-window ledger for one upstream account.
///
/// The limit itself can come from two places, and neither is a guess:
/// [`Self::declared_rpm`] is what the user typed for a model on this account,
/// [`Self::learned_rpm`] is what the provider advertised in a response header.
/// While both are absent the account is never blocked locally — a paid key that
/// advertises nothing must not be throttled on a guess — but it still takes part
/// in load-based ordering through its in-flight count.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountLimit {
    /// What the user declared for the models on this account. Derived from the
    /// model config on every request rather than persisted, so editing it in the
    /// UI takes effect immediately (and clearing it does too).
    #[serde(skip)]
    declared_rpm: DeclaredRpm,
    /// Requests per minute the provider advertised (`x-ratelimit-limit*`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    learned_rpm: Option<u32>,
    #[serde(default = "default_window_ms")]
    window_ms: u64,
    /// Send timestamps of the current window, oldest first after [`Self::prune`].
    /// Deliberately not persisted: it changes on every request, and a restart
    /// only forgets a few seconds of history — the response headers (or the next
    /// 429) re-teach the limit right away.
    #[serde(skip)]
    sent_ms: Vec<u64>,
    /// Hard block until this instant, set from an explicit `Retry-After` /
    /// `x-ratelimit-reset*` header that said the account is out of quota.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_allowed_ms: Option<u64>,
    /// Requests currently on the wire to this account. Not persisted.
    #[serde(skip)]
    inflight: u32,
}

impl Default for AccountLimit {
    fn default() -> Self {
        Self {
            declared_rpm: DeclaredRpm::Unknown,
            learned_rpm: None,
            window_ms: default_window_ms(),
            sent_ms: Vec::new(),
            next_allowed_ms: None,
            inflight: 0,
        }
    }
}

fn default_window_ms() -> u64 {
    RATE_WINDOW.as_millis().min(u128::from(u64::MAX)) as u64
}

impl AccountLimit {
    /// Effective requests-per-minute budget: the stricter of what the user
    /// declared and what the provider advertised, so neither side is overruled.
    /// `None` means "nothing limits this account locally".
    fn known_rpm(&self) -> Option<u32> {
        match self.declared_rpm {
            // A paid plan with no ceiling: the advertised number is not a limit
            // the user asked to honour.
            DeclaredRpm::Unlimited => None,
            DeclaredRpm::Capped(cap) => {
                Some(self.learned_rpm.map_or(cap, |learned| cap.min(learned)))
            }
            DeclaredRpm::Unknown => self.learned_rpm,
        }
    }

    /// Number of sends inside the window. Only meaningful right after [`Self::prune`].
    fn live_sends(&self) -> usize {
        self.sent_ms.len()
    }

    /// Drop aged-out sends and clear an expired block.
    fn prune(&mut self, now: u64) {
        let window_ms = self.window_ms.max(1);
        self.sent_ms
            .retain(|sent| sent.saturating_add(window_ms) > now);
        if self.sent_ms.len() > MAX_WINDOW_SLOTS {
            let excess = self.sent_ms.len() - MAX_WINDOW_SLOTS;
            self.sent_ms.drain(..excess);
        }
        self.sent_ms.sort_unstable();
        if self.next_allowed_ms.is_some_and(|until| until <= now) {
            self.next_allowed_ms = None;
        }
    }

    /// Earliest instant this account may be used again, or `None` when it is
    /// usable right now.
    fn retry_at(&self, now: u64) -> Option<u64> {
        let mut retry_at = self.next_allowed_ms.filter(|until| *until > now);
        if let Some(rpm) = self.known_rpm() {
            let live = self.live_sends();
            // The window cannot hold more than `MAX_WINDOW_SLOTS` sends, so a
            // larger declared budget throttles at that cap instead of silently
            // doing nothing.
            let quota = (rpm as usize).min(MAX_WINDOW_SLOTS);
            if live >= quota {
                // The account frees up once the (live - quota + 1)-th oldest
                // send ages out, i.e. once `live` drops below `quota` again.
                let index = live - quota;
                let frees = self.sent_ms[index].saturating_add(self.window_ms.max(1));
                retry_at = Some(retry_at.map_or(frees, |current| current.max(frees)));
            }
        }
        retry_at
    }

    /// Ordering score; lower is preferred. In-flight requests dominate so a
    /// burst spreads, used-quota breaks ties between equally busy accounts.
    fn load(&self) -> f64 {
        let inflight = f64::from(self.inflight) * INFLIGHT_LOAD_WEIGHT;
        match self.known_rpm() {
            Some(rpm) => self.live_sends() as f64 / f64::from(rpm) + inflight,
            None => inflight,
        }
    }

    /// Whether this account still has enough headroom to justify reusing the
    /// sticky candidate instead of rotating.
    fn sticky_headroom(&self) -> bool {
        match self.known_rpm() {
            Some(rpm) => {
                let share = (f64::from(rpm) * STICKY_QUOTA_SHARE).floor() as usize;
                self.live_sends() < share.max(1)
            }
            None => true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CandidateHealth {
    #[serde(default)]
    consecutive_failures: u32,
    #[serde(default)]
    cooldown_until_ms: Option<u64>,
    #[serde(default)]
    fingerprint: String,
    #[serde(default)]
    failure: Option<FailureClass>,
}

/// Which wire protocol an upstream endpoint speaks.
///
/// The router's canonical body is OpenAI-shaped: its Anthropic endpoint is
/// translated up front, and every hop after that stays OpenAI. A candidate that
/// only exposes an Anthropic URL therefore needs translating in both directions,
/// which is what makes one of the two URLs enough to route a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UpstreamProtocol {
    /// OpenAI Chat Completions, appended as `{base}/chat/completions`.
    OpenAi,
    /// Anthropic Messages, appended as `{base}/v1/messages`.
    Anthropic,
}

/// Pick a model's upstream endpoint and the protocol to speak to it.
///
/// An OpenAI base wins when both are set, so a model that predates Anthropic
/// support keeps its exact behaviour. Shared with the parent module's "is this
/// candidate usable" check, so a candidate can never be counted as usable on one
/// URL while the router dials a different one.
pub(crate) fn upstream_endpoint(model: &ModelConfig) -> Option<(UpstreamProtocol, &str)> {
    let openai = model.base_url.trim();
    if !openai.is_empty() {
        return Some((UpstreamProtocol::OpenAi, openai));
    }
    let anthropic = model.anthropic_url.as_deref()?.trim();
    if anthropic.is_empty() {
        return None;
    }
    Some((UpstreamProtocol::Anthropic, anthropic))
}

#[derive(Debug, Clone)]
struct Candidate {
    internal_id: String,
    model_id: String,
    /// Endpoint base for [`Self::protocol`] — whichever URL the user filled in.
    base_url: String,
    protocol: UpstreamProtocol,
    api_key: String,
    fingerprint: String,
    /// Bucket key of the upstream account this candidate spends quota from.
    /// See [`account_key`].
    account_key: String,
    /// Requests-per-minute budget the user declared for this model's account.
    /// `None` leaves the limit to the provider's headers and 429 responses;
    /// [`ROUTER_RPM_UNLIMITED`] removes the local limit altogether.
    router_rpm: Option<i32>,
    /// Smart Router group this candidate belongs to, if any. Drives
    /// `model: <group>` routing; `None` leaves it reachable through `auto` only.
    router_group: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FailureClass {
    Authentication,
    Billing,
    ModelUnavailable,
    RateLimit,
    Transient,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AttemptRecord {
    candidate_id: String,
    model_id: String,
    failure: FailureClass,
    status: Option<u16>,
}

#[derive(Clone, Copy)]
struct RoutePolicy {
    max_attempts: usize,
    time_budget: Duration,
    header_timeout: Duration,
    first_byte_timeout: Duration,
}

impl Default for RoutePolicy {
    fn default() -> Self {
        Self {
            max_attempts: MAX_ROUTE_CANDIDATES,
            time_budget: FALLBACK_TIME_BUDGET,
            header_timeout: UPSTREAM_HEADER_TIMEOUT,
            first_byte_timeout: UPSTREAM_FIRST_BYTE_TIMEOUT,
        }
    }
}

fn route_memory_version() -> u32 {
    2
}

fn route_memory_path() -> PathBuf {
    echobird_dir()
        .join("config")
        .join("smart-router-state.json")
}

fn shared_route_memory() -> Arc<Mutex<RouteMemory>> {
    ROUTE_MEMORY
        .get_or_init(|| Arc::new(Mutex::new(load_route_memory(&route_memory_path()))))
        .clone()
}

fn load_route_memory(path: &Path) -> RouteMemory {
    if !path.exists() {
        return RouteMemory::default();
    }
    fs::read_to_string(path)
        .map_err(|e| e.to_string())
        .and_then(|content| serde_json::from_str(&content).map_err(|e| e.to_string()))
        .unwrap_or_else(|error| {
            log::warn!(
                "[SmartRouter] Ignoring invalid route memory {}: {error}",
                path.display()
            );
            RouteMemory::default()
        })
}

fn save_route_memory(path: &Path, memory: &RouteMemory) {
    let Some(parent) = path.parent() else {
        return;
    };
    let result = fs::create_dir_all(parent)
        .map_err(|e| e.to_string())
        .and_then(|_| serde_json::to_string_pretty(memory).map_err(|e| e.to_string()))
        .and_then(|content| fs::write(path, content).map_err(|e| e.to_string()));
    if let Err(error) = result {
        log::warn!(
            "[SmartRouter] Failed to save route memory {}: {error}",
            path.display()
        );
    }
}

fn persist_route_memory(state: &AppState, memory: &RouteMemory) {
    if let Some(path) = &state.route_memory_path {
        save_route_memory(path, memory);
    }
}

pub(crate) fn retain_candidate_memory(candidate_ids: &[String]) {
    let memory = shared_route_memory();
    let Ok(mut memory) = memory.lock() else {
        return;
    };
    let valid: HashSet<&str> = candidate_ids.iter().map(String::as_str).collect();
    let previous_len = memory.candidates.len();
    memory
        .candidates
        .retain(|candidate_id, _| valid.contains(candidate_id.as_str()));
    memory
        .last_picked_ms
        .retain(|candidate_id, _| valid.contains(candidate_id.as_str()));
    let removed_preferred = memory
        .last_success_id
        .as_deref()
        .is_some_and(|candidate_id| !valid.contains(candidate_id));
    if removed_preferred {
        memory.last_success_id = None;
    }
    if memory.candidates.len() != previous_len || removed_preferred {
        save_route_memory(&route_memory_path(), &memory);
    }
}

pub(crate) fn forget_candidate_memory(candidate_id: &str) {
    let memory = shared_route_memory();
    let Ok(mut memory) = memory.lock() else {
        return;
    };
    let removed = memory.candidates.remove(candidate_id).is_some();
    memory.last_picked_ms.remove(candidate_id);
    let removed_preferred = memory.last_success_id.as_deref() == Some(candidate_id);
    if removed_preferred {
        memory.last_success_id = None;
    }
    if removed || removed_preferred {
        save_route_memory(&route_memory_path(), &memory);
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn candidate_fingerprint(model_id: &str, base_url: &str, api_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(model_id.as_bytes());
    hasher.update([0]);
    hasher.update(base_url.as_bytes());
    hasher.update([0]);
    hasher.update(api_key.as_bytes());
    hex::encode(hasher.finalize())
}

/// Bucket key for the per-account rate limit ledger.
///
/// Free tiers count requests per API key, so two models added from the same key
/// share one 20 RPM budget. Keying the ledger by [`Candidate::internal_id`]
/// instead would let each of them spend the full quota and guarantee a 429.
fn account_key(base_url: &str, api_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(upstream_origin(base_url).as_bytes());
    hasher.update([0]);
    hasher.update(api_key.as_bytes());
    hex::encode(hasher.finalize())
}

/// Scheme-less `host[:port]` of an upstream, so two base URLs that only differ
/// in path (`/v1` vs `/v1beta`) still map to one account. Ports are kept because
/// local servers on different ports are distinct backends.
fn upstream_origin(base_url: &str) -> String {
    let trimmed = base_url.trim();
    let without_scheme = trimmed.split_once("://").map_or(trimmed, |(_, rest)| rest);
    let origin = without_scheme.split('/').next().unwrap_or(without_scheme);
    if origin.is_empty() {
        trimmed.to_ascii_lowercase()
    } else {
        origin.to_ascii_lowercase()
    }
}

pub async fn run(port: u16) -> Result<(), String> {
    let state = AppState::new()?;
    let app = Router::new()
        .route("/health", get(handle_health))
        .route("/v1/models", get(handle_models))
        .route("/models", get(handle_models))
        .route("/v1/chat/completions", post(handle_chat))
        .route("/chat/completions", post(handle_chat))
        .route("/v1/messages", post(handle_messages))
        .route("/messages", post(handle_messages))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .with_state(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("bind 127.0.0.1:{port} failed: {e}"))?;

    mark_running();
    log::info!("[SmartRouter] listening on 127.0.0.1:{port}");
    axum::serve(listener, app)
        .await
        .map_err(|e| format!("serve failed: {e}"))
}

async fn handle_health() -> Json<Value> {
    let ids = candidate_ids();
    Json(json!({
        "status": "ok",
        "model": SMART_ROUTER_MODEL_ID,
        "candidates": ids.len(),
        "usableCandidates": super::usable_candidate_count(&ids),
    }))
}

async fn handle_models(headers: HeaderMap) -> Response {
    let expected_key = match api_key_for_use() {
        Ok(key) => key,
        Err(e) => return openai_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };
    if !is_authorized(&headers, &expected_key) {
        return openai_error(
            StatusCode::UNAUTHORIZED,
            "Invalid SmartRouter router API key",
        );
    }

    let mut data = vec![json!({
        "id": SMART_ROUTER_MODEL_ID,
        "object": "model",
        "created": 0,
        "owned_by": "smartrouter"
    })];
    // Group names are callable model names too, so clients that list models
    // before sending a request can discover them.
    for group in super::candidate_groups() {
        data.push(json!({
            "id": group,
            "object": "model",
            "created": 0,
            "owned_by": "smartrouter"
        }));
    }

    Json(json!({ "object": "list", "data": data })).into_response()
}

async fn handle_chat(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let expected_key = match api_key_for_use() {
        Ok(key) => key,
        Err(e) => return openai_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };
    if !is_authorized(&headers, &expected_key) {
        return openai_error(
            StatusCode::UNAUTHORIZED,
            "Invalid SmartRouter router API key",
        );
    }

    let requested_model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let candidates = match candidates_for_request(requested_model) {
        Ok(candidates) => candidates,
        Err(message) => return openai_error(StatusCode::NOT_FOUND, &message),
    };

    route_chat_with_candidates(&state, &headers, body, candidates).await
}

/// The reserved names that mean "route over every configured candidate".
fn is_auto_alias(model_id: &str) -> bool {
    matches!(
        model_id,
        SMART_ROUTER_MODEL_ID | "echobird/auto" | "echobird-auto" | SMART_ROUTER_INTERNAL_ID
    )
}

/// Resolve the client's `model` field into the candidates to route over.
///
/// The auto aliases use every candidate. Any other name selects the candidates
/// carrying that router group, which is what keeps failover inside one logical
/// model: platforms spell the same model differently, so without a group a
/// fallback would silently answer with a different model.
fn candidates_for_request(requested_model: &str) -> Result<Vec<Candidate>, String> {
    let candidates = resolve_candidates();
    if is_auto_alias(requested_model) {
        return Ok(candidates);
    }

    let scoped = candidates_in_group(candidates, &requested_model.trim().to_lowercase());
    if scoped.is_empty() {
        return Err(format!("Unknown router model: {requested_model}"));
    }
    Ok(scoped)
}

fn candidates_in_group(candidates: Vec<Candidate>, group: &str) -> Vec<Candidate> {
    candidates
        .into_iter()
        .filter(|candidate| {
            candidate
                .router_group
                .as_deref()
                .is_some_and(|candidate_group| candidate_group.to_lowercase() == group)
        })
        .collect()
}

async fn handle_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let expected_key = match api_key_for_use() {
        Ok(key) => key,
        Err(e) => return anthropic_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };
    if !is_authorized(&headers, &expected_key) {
        return anthropic_error(
            StatusCode::UNAUTHORIZED,
            "Invalid SmartRouter router API key",
        );
    }

    let requested_model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let candidates = match candidates_for_request(requested_model) {
        Ok(candidates) => candidates,
        Err(message) => return anthropic_error(StatusCode::NOT_FOUND, &message),
    };

    route_messages_with_candidates(&state, &headers, body, candidates).await
}

async fn route_messages_with_candidates(
    state: &AppState,
    headers: &HeaderMap,
    body: Value,
    candidates: Vec<Candidate>,
) -> Response {
    let stream_requested = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let mut openai_body = crate::services::local_llm::proxy::anthropic_to_openai(&body);
    openai_body["model"] = Value::String(SMART_ROUTER_MODEL_ID.to_string());
    let response = route_chat_with_candidates(state, headers, openai_body, candidates).await;

    if !response.status().is_success() {
        return openai_error_to_anthropic(response).await;
    }
    if stream_requested {
        openai_stream_to_anthropic(response)
    } else {
        openai_response_to_anthropic(response).await
    }
}

fn resolve_candidates() -> Vec<Candidate> {
    let ids = candidate_ids();
    let user_models = model_manager::get_user_models();
    let local_server = crate::services::local_llm::get_server_info_sync();

    ids.into_iter()
        .filter_map(|id| {
            if id == "local-server" && local_server.running {
                let model_id = local_server.model_name.clone();
                let base_url = format!("http://127.0.0.1:{}/v1", local_server.port);
                let api_key = local_server.api_key.clone();
                return Some(Candidate {
                    internal_id: id,
                    fingerprint: candidate_fingerprint(&model_id, &base_url, &api_key),
                    account_key: account_key(&base_url, &api_key),
                    model_id,
                    base_url,
                    protocol: UpstreamProtocol::OpenAi,
                    api_key,
                    router_group: None,
                    // A local llama-server has no quota to respect.
                    router_rpm: None,
                });
            }

            let model = user_models.iter().find(|model| model.internal_id == id)?;
            let model_id = model.model_id.as_deref()?.trim();
            if model_id.is_empty() {
                return None;
            }
            // Either URL is enough. An OpenAI base wins when both are set, so a
            // model that worked before this change keeps its exact behaviour.
            let (protocol, base_url) = upstream_endpoint(model)?;
            let base_url = base_url.to_string();
            let api_key = model_manager::decrypt_key_for_use(&model.api_key);
            if api_key.is_empty() {
                return None;
            }

            Some(Candidate {
                internal_id: model.internal_id.clone(),
                fingerprint: candidate_fingerprint(model_id, &base_url, &model.api_key),
                account_key: account_key(&base_url, &api_key),
                model_id: model_id.to_string(),
                base_url,
                protocol,
                api_key,
                router_group: model.router_group.clone(),
                router_rpm: model.router_rpm,
            })
        })
        .filter(|candidate| !candidate.base_url.contains(":53683"))
        .collect()
}

async fn route_chat_with_candidates(
    state: &AppState,
    inbound_headers: &HeaderMap,
    body: Value,
    candidates: Vec<Candidate>,
) -> Response {
    route_chat_with_policy(
        state,
        inbound_headers,
        body,
        candidates,
        RoutePolicy::default(),
    )
    .await
}

async fn route_chat_with_policy(
    state: &AppState,
    inbound_headers: &HeaderMap,
    body: Value,
    candidates: Vec<Candidate>,
    policy: RoutePolicy,
) -> Response {
    if candidates.is_empty() {
        return openai_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "No usable models are configured for Auto Router",
        );
    }

    let stream_requested = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let mut attempts = Vec::new();
    let mut attempted: HashSet<String> = HashSet::new();
    let mut blocked: Option<Duration> = None;
    let routing_started = Instant::now();

    while attempts.len() < policy.max_attempts {
        let Some(header_timeout) =
            remaining_timeout(routing_started, policy.time_budget, policy.header_timeout)
        else {
            break;
        };
        let (candidate, mut attempt_guard) = match select_candidate(state, &candidates, &attempted)
        {
            Selection::Ready(candidate, guard) => (*candidate, guard),
            Selection::RateLimited(wait) => {
                blocked = Some(wait);
                break;
            }
            Selection::Exhausted => break,
        };
        attempted.insert(candidate.internal_id.clone());
        let (url, upstream_body) = match candidate.protocol {
            UpstreamProtocol::OpenAi => {
                let mut upstream_body = body.clone();
                upstream_body["model"] = Value::String(candidate.model_id.clone());
                (chat_completions_url(&candidate.base_url), upstream_body)
            }
            UpstreamProtocol::Anthropic => (
                messages_url(&candidate.base_url),
                crate::services::local_llm::proxy::openai_to_anthropic_request(
                    &body,
                    &candidate.model_id,
                ),
            ),
        };
        let mut request = state
            .http_client
            .post(url)
            .header(header::CONTENT_TYPE, "application/json");
        request = match candidate.protocol {
            UpstreamProtocol::OpenAi => request
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", candidate.api_key),
                )
                .header("HTTP-Referer", "https://echobird.ai")
                .header("X-OpenRouter-Title", "SmartRouter")
                .header("X-OpenRouter-Categories", "programming-app,personal-agent"),
            // `x-api-key` is Anthropic's own spelling; the Bearer header rides
            // along because gateways fronting Anthropic frequently keep
            // expecting the OpenAI credential shape.
            UpstreamProtocol::Anthropic => request
                .header("x-api-key", candidate.api_key.as_str())
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", candidate.api_key),
                )
                .header("anthropic-version", "2023-06-01"),
        };
        let mut request = request.json(&upstream_body);
        if let Some(user_agent) = inbound_headers.get(header::USER_AGENT) {
            request = request.header(header::USER_AGENT, user_agent.clone());
        }

        let activity_guard = RouteActivityGuard::begin(&candidate.internal_id);
        let sent = tokio::time::timeout(header_timeout, request.send()).await;
        let response = match sent {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => {
                if error.is_connect() {
                    // Never reached the upstream, so the reserved slot was never
                    // spent; give it back instead of burning the minute's quota.
                    attempt_guard.rollback();
                }
                mark_failure(state, &candidate, FailureClass::Transient, None);
                attempts.push(AttemptRecord {
                    candidate_id: candidate.internal_id.clone(),
                    model_id: candidate.model_id.clone(),
                    failure: FailureClass::Transient,
                    status: None,
                });
                log::warn!(
                    "[SmartRouter] {} transport failure: {}",
                    candidate.internal_id,
                    error
                );
                continue;
            }
            Err(_) => {
                mark_failure(state, &candidate, FailureClass::Transient, None);
                attempts.push(AttemptRecord {
                    candidate_id: candidate.internal_id.clone(),
                    model_id: candidate.model_id.clone(),
                    failure: FailureClass::Transient,
                    status: None,
                });
                log::warn!(
                    "[SmartRouter] {} timed out before response headers",
                    candidate.internal_id
                );
                continue;
            }
        };

        let status = response.status();
        learn_account_limits(state, &candidate, response.headers());
        let content_type = response.headers().get(header::CONTENT_TYPE).cloned();
        let cache_control = response.headers().get(header::CACHE_CONTROL).cloned();
        if !status.is_success() {
            let retry_after = retry_after(response.headers());
            let Some(error_body_timeout) =
                remaining_timeout(routing_started, policy.time_budget, policy.header_timeout)
            else {
                mark_failure(state, &candidate, FailureClass::Transient, None);
                attempts.push(AttemptRecord {
                    candidate_id: candidate.internal_id.clone(),
                    model_id: candidate.model_id.clone(),
                    failure: FailureClass::Transient,
                    status: Some(status.as_u16()),
                });
                break;
            };
            let response_body =
                match tokio::time::timeout(error_body_timeout, response.bytes()).await {
                    Ok(Ok(body)) => body,
                    Ok(Err(_)) | Err(_) => {
                        mark_failure(state, &candidate, FailureClass::Transient, None);
                        attempts.push(AttemptRecord {
                            candidate_id: candidate.internal_id.clone(),
                            model_id: candidate.model_id.clone(),
                            failure: FailureClass::Transient,
                            status: Some(status.as_u16()),
                        });
                        break;
                    }
                };
            if let Some(failure) = classify_failure(status, &response_body) {
                mark_failure(state, &candidate, failure, retry_after);
                attempts.push(AttemptRecord {
                    candidate_id: candidate.internal_id.clone(),
                    model_id: candidate.model_id.clone(),
                    failure,
                    status: Some(status.as_u16()),
                });
                log::warn!(
                    "[SmartRouter] {} returned {}, trying next candidate",
                    candidate.internal_id,
                    status.as_u16()
                );
                continue;
            }

            return upstream_response(
                status,
                content_type,
                cache_control,
                response_body,
                &candidate,
            );
        }

        if stream_requested {
            let mut upstream_stream = response.bytes_stream();
            let Some(first_byte_timeout) = remaining_timeout(
                routing_started,
                policy.time_budget,
                policy.first_byte_timeout,
            ) else {
                mark_failure(state, &candidate, FailureClass::Transient, None);
                attempts.push(AttemptRecord {
                    candidate_id: candidate.internal_id.clone(),
                    model_id: candidate.model_id.clone(),
                    failure: FailureClass::Transient,
                    status: Some(status.as_u16()),
                });
                break;
            };
            let first = tokio::time::timeout(first_byte_timeout, upstream_stream.next()).await;
            match first {
                Ok(Some(Ok(first_chunk))) if !first_chunk.is_empty() => {
                    mark_success(state, &candidate);
                    let stream_state = state.clone();
                    let stream_candidate = candidate.clone();
                    let activity_guard = Arc::new(activity_guard);
                    let rest = upstream_stream.then(move |item| {
                        let stream_state = stream_state.clone();
                        let stream_candidate = stream_candidate.clone();
                        let activity_guard = activity_guard.clone();
                        async move {
                            let _activity_guard = activity_guard;
                            match item {
                                Ok(bytes) => Ok(bytes),
                                Err(error) => {
                                    mark_failure(
                                        &stream_state,
                                        &stream_candidate,
                                        FailureClass::Transient,
                                        None,
                                    );
                                    Err(std::io::Error::other(error))
                                }
                            }
                        }
                    });
                    let body_stream =
                        stream::once(async move { Ok::<Bytes, std::io::Error>(first_chunk) })
                            .chain(rest);
                    let body = match candidate.protocol {
                        UpstreamProtocol::OpenAi => Body::from_stream(body_stream),
                        // The first byte is already spent proving this attempt
                        // succeeded, so the stream stays anchored there and only
                        // the frames travelling inside it get translated.
                        UpstreamProtocol::Anthropic => Body::from_stream(anthropic_sse_to_openai(
                            body_stream,
                            &candidate.model_id,
                        )),
                    };
                    return streaming_response(content_type, cache_control, body, &candidate);
                }
                Ok(Some(Ok(_))) | Ok(None) | Ok(Some(Err(_))) | Err(_) => {
                    mark_failure(state, &candidate, FailureClass::Transient, None);
                    attempts.push(AttemptRecord {
                        candidate_id: candidate.internal_id.clone(),
                        model_id: candidate.model_id.clone(),
                        failure: FailureClass::Transient,
                        status: Some(status.as_u16()),
                    });
                    continue;
                }
            }
        }

        let Some(body_timeout) =
            remaining_timeout(routing_started, policy.time_budget, policy.time_budget)
        else {
            mark_failure(state, &candidate, FailureClass::Transient, None);
            attempts.push(AttemptRecord {
                candidate_id: candidate.internal_id.clone(),
                model_id: candidate.model_id.clone(),
                failure: FailureClass::Transient,
                status: Some(status.as_u16()),
            });
            break;
        };
        match tokio::time::timeout(body_timeout, response.bytes()).await {
            Ok(Ok(response_body)) => {
                mark_success(state, &candidate);
                let response_body = match candidate.protocol {
                    UpstreamProtocol::OpenAi => response_body,
                    UpstreamProtocol::Anthropic => {
                        match serde_json::from_slice::<Value>(&response_body) {
                            Ok(data) => Bytes::from(
                                crate::services::local_llm::proxy::anthropic_to_openai_response(
                                    &data,
                                    &candidate.model_id,
                                )
                                .to_string(),
                            ),
                            // A body that will not parse is passed through
                            // unchanged: the client's own error handling beats a
                            // second-hand "invalid response" from the router.
                            Err(_) => response_body,
                        }
                    }
                };
                return upstream_response(
                    status,
                    content_type,
                    cache_control,
                    response_body,
                    &candidate,
                );
            }
            Ok(Err(error)) => {
                mark_failure(state, &candidate, FailureClass::Transient, None);
                attempts.push(AttemptRecord {
                    candidate_id: candidate.internal_id.clone(),
                    model_id: candidate.model_id.clone(),
                    failure: FailureClass::Transient,
                    status: Some(status.as_u16()),
                });
                log::warn!(
                    "[SmartRouter] {} response body failure: {}",
                    candidate.internal_id,
                    error
                );
            }
            Err(_) => {
                mark_failure(state, &candidate, FailureClass::Transient, None);
                attempts.push(AttemptRecord {
                    candidate_id: candidate.internal_id.clone(),
                    model_id: candidate.model_id.clone(),
                    failure: FailureClass::Transient,
                    status: Some(status.as_u16()),
                });
                log::warn!(
                    "[SmartRouter] {} response body exceeded the routing budget",
                    candidate.internal_id
                );
                break;
            }
        }
    }

    let detail = serde_json::to_value(&attempts).unwrap_or_else(|_| json!([]));
    if let Some(wait) = blocked {
        // Every candidate is real, the accounts are just out of quota. A 429
        // with `Retry-After` lets the client back off and retry, where a 503
        // would read as "the router is broken".
        log::warn!(
            "[SmartRouter] all accounts rate limited, retry in {}s",
            wait.as_secs()
        );
        return json_error(
            StatusCode::TOO_MANY_REQUESTS,
            "All Auto Router accounts are rate limited, retry later",
            Some(detail),
            Some(wait),
        );
    }
    if attempts.is_empty() {
        return openai_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "All Auto Router models are cooling down",
        );
    }
    log::error!("[SmartRouter] all candidates failed: {detail}");
    json_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "All Auto Router models failed",
        Some(detail),
        None,
    )
}

#[derive(Debug, Default)]
struct ToolCallBuffer {
    id: String,
    name: String,
    arguments: String,
}

struct AnthropicSseAdapter {
    buffer: String,
    message_id: String,
    text_index: Option<usize>,
    next_block_index: usize,
    tool_calls: BTreeMap<u64, ToolCallBuffer>,
    finish_reason: Option<String>,
    output_tokens: u64,
    finished: bool,
}

impl AnthropicSseAdapter {
    fn new() -> Self {
        Self {
            buffer: String::new(),
            message_id: format!("msg_{}", chrono::Utc::now().timestamp_millis()),
            text_index: None,
            next_block_index: 0,
            tool_calls: BTreeMap::new(),
            finish_reason: None,
            output_tokens: 0,
            finished: false,
        }
    }

    fn start_event(&self) -> Bytes {
        anthropic_sse_event(
            "message_start",
            json!({
                "type": "message_start",
                "message": {
                    "id": self.message_id,
                    "type": "message",
                    "role": "assistant",
                    "content": [],
                    "model": SMART_ROUTER_MODEL_ID,
                    "stop_reason": null,
                    "stop_sequence": null,
                    "usage": {"input_tokens": 0, "output_tokens": 0}
                }
            }),
        )
    }

    fn push(&mut self, chunk: &[u8]) -> Vec<Bytes> {
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        let mut events = Vec::new();
        while let Some(newline) = self.buffer.find('\n') {
            let line = self.buffer[..newline].trim_end_matches('\r').to_string();
            self.buffer.drain(..=newline);
            events.extend(self.process_line(&line));
        }
        events
    }

    fn finish(mut self) -> Vec<Bytes> {
        let mut events = Vec::new();
        if !self.buffer.trim().is_empty() {
            let line = std::mem::take(&mut self.buffer);
            events.extend(self.process_line(line.trim_end_matches('\r')));
        }
        events.extend(self.finish_events());
        events
    }

    fn process_line(&mut self, line: &str) -> Vec<Bytes> {
        let Some(data) = line.strip_prefix("data:").map(str::trim) else {
            return Vec::new();
        };
        if data == "[DONE]" {
            return self.finish_events();
        }
        let Ok(chunk) = serde_json::from_str::<Value>(data) else {
            return Vec::new();
        };

        if let Some(tokens) = chunk
            .get("usage")
            .and_then(|usage| usage.get("completion_tokens"))
            .and_then(Value::as_u64)
        {
            self.output_tokens = tokens;
        }

        let Some(choice) = chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            return Vec::new();
        };
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = Some(reason.to_string());
        }

        let Some(delta) = choice.get("delta") else {
            return Vec::new();
        };
        let mut events = Vec::new();
        if let Some(text) = delta.get("content").and_then(Value::as_str) {
            if !text.is_empty() {
                let index = match self.text_index {
                    Some(index) => index,
                    None => {
                        let index = self.next_block_index;
                        self.next_block_index += 1;
                        self.text_index = Some(index);
                        events.push(anthropic_sse_event(
                            "content_block_start",
                            json!({
                                "type": "content_block_start",
                                "index": index,
                                "content_block": {"type": "text", "text": ""}
                            }),
                        ));
                        index
                    }
                };
                events.push(anthropic_sse_event(
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta",
                        "index": index,
                        "delta": {"type": "text_delta", "text": text}
                    }),
                ));
            }
        }

        if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for tool_call in tool_calls {
                let index = tool_call.get("index").and_then(Value::as_u64).unwrap_or(0);
                let buffered = self.tool_calls.entry(index).or_default();
                if let Some(id) = tool_call.get("id").and_then(Value::as_str) {
                    buffered.id.push_str(id);
                }
                if let Some(function) = tool_call.get("function") {
                    if let Some(name) = function.get("name").and_then(Value::as_str) {
                        buffered.name.push_str(name);
                    }
                    if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                        buffered.arguments.push_str(arguments);
                    }
                }
            }
        }
        events
    }

    fn finish_events(&mut self) -> Vec<Bytes> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        let mut events = Vec::new();

        if let Some(index) = self.text_index {
            events.push(anthropic_sse_event(
                "content_block_stop",
                json!({"type": "content_block_stop", "index": index}),
            ));
        }

        for tool_call in self.tool_calls.values() {
            let index = self.next_block_index;
            self.next_block_index += 1;
            let id = if tool_call.id.is_empty() {
                format!("call_{index}")
            } else {
                tool_call.id.clone()
            };
            events.push(anthropic_sse_event(
                "content_block_start",
                json!({
                    "type": "content_block_start",
                    "index": index,
                    "content_block": {
                        "type": "tool_use",
                        "id": id,
                        "name": tool_call.name,
                        "input": {}
                    }
                }),
            ));
            events.push(anthropic_sse_event(
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": index,
                    "delta": {
                        "type": "input_json_delta",
                        "partial_json": if tool_call.arguments.is_empty() {
                            "{}"
                        } else {
                            tool_call.arguments.as_str()
                        }
                    }
                }),
            ));
            events.push(anthropic_sse_event(
                "content_block_stop",
                json!({"type": "content_block_stop", "index": index}),
            ));
        }

        if self.text_index.is_none() && self.tool_calls.is_empty() {
            events.push(anthropic_sse_event(
                "content_block_start",
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {"type": "text", "text": ""}
                }),
            ));
            events.push(anthropic_sse_event(
                "content_block_stop",
                json!({"type": "content_block_stop", "index": 0}),
            ));
        }

        let stop_reason = if !self.tool_calls.is_empty() {
            "tool_use"
        } else {
            match self.finish_reason.as_deref() {
                Some("length") => "max_tokens",
                Some("tool_calls") => "tool_use",
                _ => "end_turn",
            }
        };
        events.push(anthropic_sse_event(
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": stop_reason, "stop_sequence": null},
                "usage": {"output_tokens": self.output_tokens}
            }),
        ));
        events.push(anthropic_sse_event(
            "message_stop",
            json!({"type": "message_stop"}),
        ));
        events
    }
}

fn anthropic_sse_event(event: &str, data: Value) -> Bytes {
    Bytes::from(format!("event: {event}\ndata: {data}\n\n"))
}

/// Wrap an Anthropic SSE byte stream so it leaves as OpenAI SSE chunks.
///
/// The routing loop has already consumed the upstream's first byte to prove the
/// attempt succeeded, so health bookkeeping, the routing budget and the activity
/// guard all stay attached to the original stream — this only rewrites the
/// frames travelling through it.
fn anthropic_sse_to_openai<S>(
    source: S,
    model: &str,
) -> ReceiverStream<Result<Bytes, std::io::Error>>
where
    S: futures_util::Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static,
{
    use crate::services::local_llm::proxy::AnthropicToOpenAiStream;

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(32);
    let model = model.to_string();
    tokio::spawn(async move {
        let mut source = Box::pin(source);
        let mut adapter = AnthropicToOpenAiStream::new(&model);
        while let Some(item) = source.next().await {
            match item {
                Ok(chunk) => {
                    for frame in adapter.push(&chunk) {
                        if tx.send(Ok(Bytes::from(frame))).await.is_err() {
                            return;
                        }
                    }
                }
                Err(error) => {
                    let _ = tx.send(Err(std::io::Error::other(error))).await;
                    return;
                }
            }
        }
        for frame in adapter.finish() {
            if tx.send(Ok(Bytes::from(frame))).await.is_err() {
                return;
            }
        }
    });
    ReceiverStream::new(rx)
}

fn openai_stream_to_anthropic(response: Response) -> Response {
    let routed_via = response.headers().get("X-EchoBird-Routed-Via").cloned();
    let mut source = response.into_body().into_data_stream();
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(32);

    tokio::spawn(async move {
        let mut adapter = AnthropicSseAdapter::new();
        if tx.send(Ok(adapter.start_event())).await.is_err() {
            return;
        }
        while let Some(item) = source.next().await {
            match item {
                Ok(chunk) => {
                    for event in adapter.push(&chunk) {
                        if tx.send(Ok(event)).await.is_err() {
                            return;
                        }
                    }
                }
                Err(error) => {
                    let _ = tx.send(Err(std::io::Error::other(error))).await;
                    return;
                }
            }
        }
        for event in adapter.finish() {
            if tx.send(Ok(event)).await.is_err() {
                return;
            }
        }
    });

    anthropic_response(
        StatusCode::OK,
        "text/event-stream",
        Body::from_stream(ReceiverStream::new(rx)),
        routed_via,
    )
}

async fn openai_response_to_anthropic(response: Response) -> Response {
    let routed_via = response.headers().get("X-EchoBird-Routed-Via").cloned();
    let body = match response.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(error) => {
            return anthropic_error(
                StatusCode::BAD_GATEWAY,
                &format!("Failed to read Smart Router response: {error}"),
            )
        }
    };
    let data: Value = match serde_json::from_slice(&body) {
        Ok(data) => data,
        Err(error) => {
            return anthropic_error(
                StatusCode::BAD_GATEWAY,
                &format!("Invalid Smart Router response: {error}"),
            )
        }
    };
    let converted = crate::services::local_llm::proxy::openai_to_anthropic(&data);
    anthropic_response(
        StatusCode::OK,
        "application/json",
        Body::from(converted.to_string()),
        routed_via,
    )
}

async fn openai_error_to_anthropic(response: Response) -> Response {
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .map(|collected| collected.to_bytes())
        .unwrap_or_default();
    let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let message = parsed
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("Auto Router request failed");
    anthropic_error(status, message)
}

fn anthropic_error(status: StatusCode, message: &str) -> Response {
    let error_type = match status {
        StatusCode::BAD_REQUEST => "invalid_request_error",
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => "authentication_error",
        StatusCode::TOO_MANY_REQUESTS => "rate_limit_error",
        _ => "api_error",
    };
    anthropic_response(
        status,
        "application/json",
        Body::from(
            json!({
                "type": "error",
                "error": {"type": error_type, "message": message}
            })
            .to_string(),
        ),
        None,
    )
}

fn anthropic_response(
    status: StatusCode,
    content_type: &'static str,
    body: Body,
    routed_via: Option<HeaderValue>,
) -> Response {
    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-cache");
    if let Some(routed_via) = routed_via {
        response = response.header("X-EchoBird-Routed-Via", routed_via);
    }
    response.body(body).unwrap_or_else(|_| {
        openai_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to build Anthropic router response",
        )
    })
}

/// Outcome of picking the next candidate for one attempt.
enum Selection {
    /// A candidate was picked and its account quota slot reserved. The guard
    /// returns the slot when the attempt ends. Boxed because a `Candidate` is
    /// an order of magnitude bigger than the other variants.
    Ready(Box<Candidate>, AttemptGuard),
    /// Every candidate that is not cooling down sits on an account with no quota
    /// left right now; the duration is the earliest a slot frees up.
    RateLimited(Duration),
    /// Nothing left to try for this request.
    Exhausted,
}

/// One reserved slot of [`AccountLimit`] quota, held for the duration of an
/// attempt. Dropping it always returns the in-flight slot; a request that never
/// reached the upstream also gives its send timestamp back.
struct AttemptGuard {
    route_memory: Arc<Mutex<RouteMemory>>,
    account_key: String,
    reserved_at_ms: u64,
    rollback_on_drop: bool,
}

impl AttemptGuard {
    /// The request failed before reaching the upstream, so it spent no quota.
    fn rollback(&mut self) {
        self.rollback_on_drop = true;
    }
}

impl Drop for AttemptGuard {
    fn drop(&mut self) {
        let Ok(mut memory) = self.route_memory.lock() else {
            return;
        };
        let Some(account) = memory.accounts.get_mut(&self.account_key) else {
            return;
        };
        account.inflight = account.inflight.saturating_sub(1);
        if self.rollback_on_drop {
            if let Some(index) = account
                .sent_ms
                .iter()
                .position(|sent| *sent == self.reserved_at_ms)
            {
                account.sent_ms.remove(index);
            }
        }
    }
}

struct CandidatePlan {
    ready: Vec<Candidate>,
    /// Earliest instant at which one of the skipped candidates could be retried.
    retry_at_ms: Option<u64>,
}

/// Rank the candidates that are usable right now.
///
/// The keys, in order of weight:
///
/// 1. account load — the idlest upstream account takes the next request, so a
///    pool of keys is spread out instead of drained one key at a time;
/// 2. least recently picked — models behind one API key share a quota bucket and
///    therefore tie on load, so recency is what rotates traffic between them
///    rather than letting a single model absorb the budget until it runs dry;
/// 3. the last-known-good pick, which only breaks a full tie now (a cold start,
///    or a pool that has not been used yet) so a conversation still tends to stay
///    on one model when nothing argues against it.
///
/// Candidates whose account is out of quota are skipped and reported through
/// `retry_at_ms`, which lets the caller tell the client when to come back instead
/// of failing hard.
fn plan_candidates(
    memory: &mut RouteMemory,
    candidates: &[Candidate],
    attempted: &HashSet<String>,
    now: u64,
) -> CandidatePlan {
    let preferred = memory.last_success_id.clone();
    let mut ranked: Vec<(f64, u64, u8, Candidate)> = Vec::new();
    let mut retry_at_ms: Option<u64> = None;

    // Budgets declared in the model config, per account. Models sharing a key
    // spend from one quota, so the strictest declaration wins.
    let mut declared_rpm: HashMap<&str, DeclaredRpm> = HashMap::new();
    for candidate in candidates {
        // The UI sends `-1` for "paid, no cap" and a positive number for a cap;
        // `0` and any other negative never survive `normalize_router_rpm`, and
        // `None` means the field was left blank.
        let declared = match candidate.router_rpm {
            Some(ROUTER_RPM_UNLIMITED) => DeclaredRpm::Unlimited,
            Some(rpm) if rpm > 0 => DeclaredRpm::Capped(rpm.unsigned_abs()),
            _ => continue,
        };
        declared_rpm
            .entry(candidate.account_key.as_str())
            .and_modify(|current| *current = current.strictest(declared))
            .or_insert(declared);
    }

    for candidate in candidates {
        if attempted.contains(&candidate.internal_id) {
            continue;
        }
        let cooling_down = memory
            .candidates
            .get(&candidate.internal_id)
            .and_then(|health| health.cooldown_until_ms)
            .is_some_and(|until| until > now);
        if cooling_down {
            continue;
        }
        let last_picked_ms = memory
            .last_picked_ms
            .get(&candidate.internal_id)
            .copied()
            .unwrap_or(0);

        let account = memory
            .accounts
            .entry(candidate.account_key.clone())
            .or_default();
        // Re-derived on every request so editing (or clearing) the budget in the
        // UI applies immediately, without touching the store.
        account.declared_rpm = declared_rpm
            .get(candidate.account_key.as_str())
            .copied()
            .unwrap_or_default();
        account.prune(now);
        if let Some(retry_at) = account.retry_at(now) {
            retry_at_ms = Some(retry_at_ms.map_or(retry_at, |current: u64| current.min(retry_at)));
            continue;
        }

        let sticky = preferred.as_deref() == Some(candidate.internal_id.as_str())
            && account.inflight == 0
            && account.sticky_headroom();
        ranked.push((
            account.load(),
            last_picked_ms,
            u8::from(!sticky),
            candidate.clone(),
        ));
    }

    // `sort_by` is stable, so candidates tying on every key keep the configured
    // order.
    ranked.sort_by(|left, right| {
        left.0
            .partial_cmp(&right.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(left.1.cmp(&right.1))
            .then(left.2.cmp(&right.2))
    });

    CandidatePlan {
        ready: ranked
            .into_iter()
            .map(|(_, _, _, candidate)| candidate)
            .collect(),
        retry_at_ms,
    }
}

/// Pick and reserve the next candidate to try, atomically under the memory lock
/// so concurrent requests cannot all spend the same free slot.
fn select_candidate(
    state: &AppState,
    candidates: &[Candidate],
    attempted: &HashSet<String>,
) -> Selection {
    let Ok(mut memory) = state.route_memory.lock() else {
        return Selection::Exhausted;
    };
    let now = now_ms();

    let fingerprints: HashMap<&str, &str> = candidates
        .iter()
        .map(|candidate| {
            (
                candidate.internal_id.as_str(),
                candidate.fingerprint.as_str(),
            )
        })
        .collect();
    let previous_len = memory.candidates.len();
    memory.candidates.retain(|candidate_id, health| {
        fingerprints
            .get(candidate_id.as_str())
            .is_some_and(|fingerprint| health.fingerprint == *fingerprint)
    });
    let pruned = memory.candidates.len() != previous_len;

    let plan = plan_candidates(&mut memory, candidates, attempted, now);
    let Some(candidate) = plan.ready.into_iter().next() else {
        if pruned {
            persist_route_memory(state, &memory);
        }
        return match plan.retry_at_ms {
            Some(retry_at) => {
                Selection::RateLimited(Duration::from_millis(retry_at.saturating_sub(now)))
            }
            None => Selection::Exhausted,
        };
    };

    if let Some(account) = memory.accounts.get_mut(&candidate.account_key) {
        account.sent_ms.push(now);
        account.inflight = account.inflight.saturating_add(1);
    }
    // Stamp the pick even though the attempt may still fail: rotation is about
    // spreading the budget, not about rewarding successes.
    memory
        .last_picked_ms
        .insert(candidate.internal_id.clone(), now);
    if pruned {
        persist_route_memory(state, &memory);
    }
    let guard = AttemptGuard {
        route_memory: state.route_memory.clone(),
        account_key: candidate.account_key.clone(),
        reserved_at_ms: now,
        rollback_on_drop: false,
    };
    drop(memory);
    Selection::Ready(Box::new(candidate), guard)
}

fn mark_success(state: &AppState, candidate: &Candidate) {
    let Ok(mut memory) = state.route_memory.lock() else {
        return;
    };
    let removed_failure = memory.candidates.remove(&candidate.internal_id).is_some();
    let changed_preferred = memory.last_success_id.as_deref() != Some(&candidate.internal_id);
    if changed_preferred {
        memory.last_success_id = Some(candidate.internal_id.clone());
    }
    if removed_failure || changed_preferred {
        persist_route_memory(state, &memory);
    }
}

fn mark_failure(
    state: &AppState,
    candidate: &Candidate,
    failure: FailureClass,
    retry_after: Option<Duration>,
) {
    let Ok(mut memory) = state.route_memory.lock() else {
        return;
    };
    let entry = memory
        .candidates
        .entry(candidate.internal_id.clone())
        .or_default();
    if entry.fingerprint != candidate.fingerprint {
        *entry = CandidateHealth {
            fingerprint: candidate.fingerprint.clone(),
            ..CandidateHealth::default()
        };
    }
    entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
    entry.failure = Some(failure);
    let now = now_ms();
    let cooldown = cooldown_for_failure(failure, retry_after, entry.consecutive_failures);
    entry.cooldown_until_ms = Some(now.saturating_add(duration_ms(cooldown)));

    if failure == FailureClass::RateLimit {
        // An explicit, longer-than-a-window `Retry-After` is an account-level
        // instruction: every model behind this key is capped, not just the one
        // that happened to be tried, so block them together instead of walking
        // through the rest of the account one 429 at a time.
        if let Some(retry_after) = retry_after.filter(|wait| *wait >= ACCOUNT_RETRY_AFTER_FLOOR) {
            let until = now.saturating_add(duration_ms(retry_after));
            let account = memory
                .accounts
                .entry(candidate.account_key.clone())
                .or_default();
            if account
                .next_allowed_ms
                .map_or(true, |current| current < until)
            {
                account.next_allowed_ms = Some(until);
            }
        }
    }
    persist_route_memory(state, &memory);
}

fn cooldown_for_failure(
    failure: FailureClass,
    retry_after: Option<Duration>,
    consecutive_failures: u32,
) -> Duration {
    match failure {
        FailureClass::Authentication | FailureClass::Billing => AUTH_OR_BILLING_COOLDOWN,
        FailureClass::ModelUnavailable => MODEL_UNAVAILABLE_COOLDOWN,
        FailureClass::RateLimit => retry_after
            .unwrap_or(RATE_LIMIT_COOLDOWN)
            .min(MAX_RETRY_AFTER),
        FailureClass::Transient => {
            let multiplier = 1_u32 << consecutive_failures.saturating_sub(1).min(3);
            TRANSIENT_COOLDOWN
                .saturating_mul(multiplier)
                .min(MAX_TRANSIENT_COOLDOWN)
        }
    }
}

fn remaining_timeout(started: Instant, budget: Duration, cap: Duration) -> Option<Duration> {
    budget
        .checked_sub(started.elapsed())
        .map(|remaining| remaining.min(cap))
        .filter(|remaining| !remaining.is_zero())
}

fn classify_failure(status: StatusCode, body: &[u8]) -> Option<FailureClass> {
    let message = String::from_utf8_lossy(body).to_ascii_lowercase();
    if [
        "content policy",
        "safety policy",
        "content moderation",
        "blocked by safety",
    ]
    .iter()
    .any(|pattern| message.contains(pattern))
    {
        return None;
    }
    if [
        "model_not_found",
        "model not found",
        "model does not exist",
        "model is not available",
        "model unavailable",
        "unknown model",
        "invalid model",
        "model_not_supported",
        "model has been deprecated",
        "model has been decommissioned",
        "has no provider supported",
        "no provider is available",
        "does not support tool",
        "tools are not supported",
        "tool use is not supported",
        "function calling is not supported",
    ]
    .iter()
    .any(|pattern| message.contains(pattern))
    {
        return Some(FailureClass::ModelUnavailable);
    }
    if [
        "insufficient quota",
        "insufficient_quota",
        "quota exceeded",
        "credit balance",
        "credits exhausted",
    ]
    .iter()
    .any(|pattern| message.contains(pattern))
    {
        return Some(FailureClass::Billing);
    }
    let status_failure = match status.as_u16() {
        401 | 403 => Some(FailureClass::Authentication),
        402 => Some(FailureClass::Billing),
        // 410 Gone is 404's permanent twin: the endpoint or model is gone, which
        // free-model pools hit constantly. Left unclassified it would be passed
        // straight through and the remaining candidates would never be tried.
        404 | 410 => Some(FailureClass::ModelUnavailable),
        408 | 409 => Some(FailureClass::Transient),
        429 => Some(FailureClass::RateLimit),
        500..=599 => Some(FailureClass::Transient),
        _ => None,
    };
    if status_failure.is_some() {
        return status_failure;
    }
    if ["rate limit", "rate_limit", "too many requests"]
        .iter()
        .any(|pattern| message.contains(pattern))
    {
        return Some(FailureClass::RateLimit);
    }
    None
}

/// Header names different OpenAI-compatible gateways use for the request quota.
const LIMIT_HEADERS: [&str; 3] = [
    "x-ratelimit-limit-requests",
    "x-ratelimit-limit",
    "ratelimit-limit",
];
const REMAINING_HEADERS: [&str; 3] = [
    "x-ratelimit-remaining-requests",
    "x-ratelimit-remaining",
    "ratelimit-remaining",
];
const RESET_HEADERS: [&str; 3] = [
    "x-ratelimit-reset-requests",
    "x-ratelimit-reset",
    "ratelimit-reset",
];

fn header_value<'a>(headers: &'a HeaderMap, names: &[&str]) -> Option<&'a str> {
    names
        .iter()
        .find_map(|name| headers.get(*name))
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

/// Fold a response's rate limit headers into the account ledger.
///
/// Providers that advertise their limits (`x-ratelimit-limit-requests`, …) get
/// proactive rotation with no configuration at all. Providers that stay silent
/// are never throttled on a guess, which keeps paid keys at full throughput.
fn learn_account_limits(state: &AppState, candidate: &Candidate, headers: &HeaderMap) {
    let limit = header_value(headers, &LIMIT_HEADERS)
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|limit| (1..=100_000).contains(limit));
    let remaining =
        header_value(headers, &REMAINING_HEADERS).and_then(|value| value.parse::<u32>().ok());
    let reset = header_value(headers, &RESET_HEADERS).and_then(parse_reset_duration);
    if limit.is_none() && remaining.is_none() && reset.is_none() {
        return;
    }

    let now = now_ms();
    let Ok(mut memory) = state.route_memory.lock() else {
        return;
    };
    let account = memory
        .accounts
        .entry(candidate.account_key.clone())
        .or_default();

    let mut persist = false;
    if let Some(limit) = limit {
        if account.learned_rpm != Some(limit) {
            account.learned_rpm = Some(limit);
            persist = true;
        }
    }
    if let Some(remaining) = remaining {
        apply_remaining(account, remaining, now);
    }
    if let (Some(0), Some(reset)) = (remaining, reset) {
        // The provider says the quota is spent: block the whole account (every
        // model sharing this key) until the window resets.
        let until = now.saturating_add(duration_ms(reset));
        if account
            .next_allowed_ms
            .map_or(true, |current| current < until)
        {
            account.next_allowed_ms = Some(until);
            persist = true;
        }
    }
    if persist {
        persist_route_memory(state, &memory);
    }
}

/// Reconcile the local window with an authoritative `remaining` count, so a
/// provider that publishes it keeps the ledger honest across restarts (when the
/// in-memory send timestamps are gone).
///
/// Uses the provider's own advertised limit, not [`AccountLimit::known_rpm`]:
/// `remaining` is counted in the provider's units, so mixing it with a stricter
/// user-declared budget would invent sends that never happened.
fn apply_remaining(account: &mut AccountLimit, remaining: u32, now: u64) {
    let Some(rpm) = account.learned_rpm else {
        return;
    };
    if remaining >= rpm {
        return;
    }
    let used = (rpm - remaining) as usize;
    account.prune(now);
    match account.sent_ms.len().cmp(&used) {
        std::cmp::Ordering::Equal => {}
        std::cmp::Ordering::Less => {
            // We under-counted; record the missing sends as of now so they still
            // hold a full window, which throttles conservatively rather than not
            // at all.
            let missing = used - account.sent_ms.len();
            account.sent_ms.extend(std::iter::repeat(now).take(missing));
        }
        std::cmp::Ordering::Greater => {
            let excess = account.sent_ms.len() - used;
            account.sent_ms.drain(..excess);
        }
    }
    if account.sent_ms.len() > MAX_WINDOW_SLOTS {
        let excess = account.sent_ms.len() - MAX_WINDOW_SLOTS;
        account.sent_ms.drain(..excess);
    }
}

/// Parse a `x-ratelimit-reset*` value. Providers use either a Go-style duration
/// (`1s`, `6m0s`, `500ms`), a bare number of seconds, or an absolute epoch.
fn parse_reset_duration(value: &str) -> Option<Duration> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    if value.chars().any(|ch| ch.is_ascii_alphabetic()) {
        let seconds = parse_unit_duration(value)?;
        return bounded_duration(seconds);
    }

    let number = value.parse::<f64>().ok()?;
    if !number.is_finite() || number < 0.0 {
        return None;
    }
    let now = now_ms();
    if number >= 1e12 {
        return Some(Duration::from_millis((number as u64).saturating_sub(now)));
    }
    if number >= 1e9 {
        return Some(Duration::from_secs(
            (number as u64).saturating_sub(now / 1_000),
        ));
    }
    bounded_duration(number)
}

fn bounded_duration(seconds: f64) -> Option<Duration> {
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    Some(Duration::from_secs_f64(
        seconds.min(MAX_RETRY_AFTER.as_secs_f64()),
    ))
}

/// Sum a Go-style duration string into seconds.
fn parse_unit_duration(value: &str) -> Option<f64> {
    let mut total = 0_f64;
    let mut number = String::new();
    let mut unit = String::new();
    for ch in value.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            if !unit.is_empty() {
                total += unit_seconds(&number, &unit)?;
                number.clear();
                unit.clear();
            }
            number.push(ch);
        } else {
            unit.push(ch);
        }
    }
    if !unit.is_empty() {
        total += unit_seconds(&number, &unit)?;
    }
    Some(total)
}

fn unit_seconds(number: &str, unit: &str) -> Option<f64> {
    let value = number.parse::<f64>().ok()?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    let scale = match unit {
        "ns" => 1e-9,
        "us" | "µs" | "μs" => 1e-6,
        "ms" => 1e-3,
        "s" => 1.0,
        "m" => 60.0,
        "h" => 3600.0,
        _ => return None,
    };
    Some(value * scale)
}

fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    retry_after_header(headers).or_else(|| {
        header_value(headers, &RESET_HEADERS)
            .and_then(parse_reset_duration)
            .map(|wait| wait.min(MAX_RETRY_AFTER))
    })
}

fn retry_after_header(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get(header::RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds).min(MAX_RETRY_AFTER));
    }

    let retry_at = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    let wait = retry_at.signed_duration_since(chrono::Utc::now());
    Some(wait.to_std().unwrap_or_default().min(MAX_RETRY_AFTER))
}

fn chat_completions_url(base_url: &str) -> String {
    let base_url = base_url.trim_end_matches('/');
    if base_url.ends_with("/chat/completions") {
        base_url.to_string()
    } else {
        format!("{base_url}/chat/completions")
    }
}

/// Anthropic Messages endpoint for a candidate that only has an Anthropic URL.
///
/// Mirrors the frontend's `normalizeAnthropicUrl`: stored bases are either the
/// bare host (`https://api.anthropic.com`), an `/v1` root, or the full endpoint
/// pasted straight from a provider's docs.
fn messages_url(base_url: &str) -> String {
    let base_url = base_url.trim_end_matches('/');
    if base_url.ends_with("/messages") {
        base_url.to_string()
    } else if base_url.ends_with("/v1") {
        format!("{base_url}/messages")
    } else {
        format!("{base_url}/v1/messages")
    }
}

fn is_authorized(headers: &HeaderMap, expected_key: &str) -> bool {
    let bearer_matches = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|key| key == expected_key);
    bearer_matches
        || headers
            .get("x-api-key")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|key| key == expected_key)
}

fn routed_via(candidate: &Candidate) -> HeaderValue {
    let value = format!("{}/{}", candidate.internal_id, candidate.model_id);
    HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static("echobird/unknown"))
}

fn upstream_response(
    status: StatusCode,
    content_type: Option<HeaderValue>,
    cache_control: Option<HeaderValue>,
    body: Bytes,
    candidate: &Candidate,
) -> Response {
    response_with_headers(
        status,
        content_type,
        cache_control,
        Body::from(body),
        candidate,
    )
}

fn streaming_response(
    content_type: Option<HeaderValue>,
    cache_control: Option<HeaderValue>,
    body: Body,
    candidate: &Candidate,
) -> Response {
    response_with_headers(StatusCode::OK, content_type, cache_control, body, candidate)
}

fn response_with_headers(
    status: StatusCode,
    content_type: Option<HeaderValue>,
    cache_control: Option<HeaderValue>,
    body: Body,
    candidate: &Candidate,
) -> Response {
    let mut response = Response::builder()
        .status(status)
        .header("X-EchoBird-Routed-Via", routed_via(candidate));
    if let Some(content_type) = content_type {
        response = response.header(header::CONTENT_TYPE, content_type);
    }
    if let Some(cache_control) = cache_control {
        response = response.header(header::CACHE_CONTROL, cache_control);
    }
    response.body(body).unwrap_or_else(|_| {
        openai_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to build router response",
        )
    })
}

fn openai_error(status: StatusCode, message: &str) -> Response {
    json_error(status, message, None, None)
}

fn json_error(
    status: StatusCode,
    message: &str,
    attempts: Option<Value>,
    retry_after: Option<Duration>,
) -> Response {
    let mut error = json!({
        "message": message,
        "type": "echobird_router_error",
        "code": status.as_u16(),
    });
    if let Some(attempts) = attempts {
        error["attempts"] = attempts;
    }
    let mut response = (status, Json(json!({ "error": error }))).into_response();
    if let Some(retry_after) = retry_after {
        let seconds = retry_after.as_secs().max(1).to_string();
        if let Ok(value) = HeaderValue::from_str(&seconds) {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::routing::post;
    use http_body_util::BodyExt;
    use tokio_stream::wrappers::ReceiverStream;

    use super::*;

    fn candidate(addr: SocketAddr, id: &str) -> Candidate {
        let model_id = format!("{id}-model");
        let base_url = format!("http://{addr}/v1");
        let api_key = "upstream-key".to_string();
        Candidate {
            internal_id: id.to_string(),
            fingerprint: candidate_fingerprint(&model_id, &base_url, &api_key),
            account_key: account_key(&base_url, &api_key),
            model_id,
            base_url,
            protocol: UpstreamProtocol::OpenAi,
            api_key,
            router_group: None,
            router_rpm: None,
        }
    }

    /// A candidate that only has an Anthropic URL: the router must speak
    /// `/v1/messages` to it and translate the reply back to OpenAI.
    fn anthropic_candidate(addr: SocketAddr, id: &str) -> Candidate {
        let model_id = format!("{id}-model");
        let base_url = format!("http://{addr}");
        let api_key = "upstream-key".to_string();
        Candidate {
            internal_id: id.to_string(),
            fingerprint: candidate_fingerprint(&model_id, &base_url, &api_key),
            account_key: account_key(&base_url, &api_key),
            model_id,
            base_url,
            protocol: UpstreamProtocol::Anthropic,
            api_key,
            router_group: None,
            router_rpm: None,
        }
    }

    /// Candidates a fresh request would be allowed to use, in order, without
    /// reserving anything.
    fn ready_order(state: &AppState, candidates: Vec<Candidate>) -> Vec<Candidate> {
        let mut memory = state.route_memory.lock().unwrap();
        plan_candidates(&mut memory, &candidates, &HashSet::new(), now_ms()).ready
    }

    /// Pick the next candidate the way a request does, handing the reserved slot
    /// straight back so the pick itself is all the ledger ends up remembering.
    fn pick(state: &AppState, candidates: &[Candidate]) -> Candidate {
        match select_candidate(state, candidates, &HashSet::new()) {
            Selection::Ready(candidate, guard) => {
                drop(guard);
                *candidate
            }
            _ => panic!("expected a usable candidate"),
        }
    }

    /// Make the next `sends` requests to `candidate` look like they already
    /// happened, which is how the ledger sees them after a burst.
    fn record_sends(state: &AppState, candidate: &Candidate, sends: usize, at_ms: u64) {
        let mut memory = state.route_memory.lock().unwrap();
        let account = memory
            .accounts
            .entry(candidate.account_key.clone())
            .or_default();
        for _ in 0..sends {
            account.sent_ms.push(at_ms);
        }
    }

    /// Pretend the provider advertised an RPM budget for this account, which is
    /// what `x-ratelimit-limit-requests` does at runtime.
    fn record_learned_rpm(state: &AppState, candidate: &Candidate, rpm: u32) {
        let mut memory = state.route_memory.lock().unwrap();
        memory
            .accounts
            .entry(candidate.account_key.clone())
            .or_default()
            .learned_rpm = Some(rpm);
    }

    /// Save a candidate with a user-declared RPM budget, as the model config does.
    fn with_declared_rpm(mut candidate: Candidate, rpm: u32) -> Candidate {
        candidate.router_rpm = Some(rpm as i32);
        candidate
    }

    /// Same, but marked `-1`: a paid key the router must not throttle at all.
    fn with_unlimited_rpm(mut candidate: Candidate) -> Candidate {
        candidate.router_rpm = Some(ROUTER_RPM_UNLIMITED);
        candidate
    }

    #[test]
    fn group_filter_keeps_only_matching_candidates() {
        let addr = SocketAddr::from(([127, 0, 0, 1], 1));
        let mut first = candidate(addr, "first");
        first.router_group = Some("Sonnet".to_string());
        let second = candidate(addr, "second");

        let scoped = candidates_in_group(vec![first.clone(), second], "sonnet");
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].internal_id, "first");

        assert!(candidates_in_group(vec![first], "opus").is_empty());
    }

    fn inbound_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer local-key"),
        );
        headers
    }

    async fn spawn_mock(app: Router) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        addr
    }

    #[test]
    fn route_activity_tracks_the_latest_in_flight_candidate() {
        let mut activity = RouteActivity::default();
        let first = activity.begin("first", 10);
        let second = activity.begin("second", 20);

        let snapshot = activity.snapshot();
        assert_eq!(snapshot.candidate_id.as_deref(), Some("second"));
        assert!(snapshot.active);

        activity.end(second, 30);
        let snapshot = activity.snapshot();
        assert_eq!(snapshot.candidate_id.as_deref(), Some("first"));
        assert!(snapshot.active);

        activity.end(first, 40);
        let snapshot = activity.snapshot();
        assert_eq!(snapshot.candidate_id.as_deref(), Some("first"));
        assert!(!snapshot.active);
        assert_eq!(snapshot.sequence, 4);
        assert_eq!(snapshot.updated_at_ms, 40);
    }

    #[test]
    fn classifies_only_failover_worthy_statuses() {
        assert_eq!(
            classify_failure(StatusCode::UNAUTHORIZED, b""),
            Some(FailureClass::Authentication)
        );
        assert_eq!(
            classify_failure(StatusCode::TOO_MANY_REQUESTS, b""),
            Some(FailureClass::RateLimit)
        );
        assert_eq!(
            classify_failure(
                StatusCode::TOO_MANY_REQUESTS,
                br#"{"error":{"code":"insufficient_quota"}}"#
            ),
            Some(FailureClass::Billing)
        );
        assert_eq!(
            classify_failure(StatusCode::REQUEST_TIMEOUT, b""),
            Some(FailureClass::Transient)
        );
        assert_eq!(
            classify_failure(StatusCode::BAD_GATEWAY, b""),
            Some(FailureClass::Transient)
        );
        assert_eq!(
            classify_failure(StatusCode::BAD_REQUEST, b"bad input"),
            None
        );
        assert_eq!(
            classify_failure(StatusCode::FORBIDDEN, b"blocked by safety policy"),
            None
        );
        assert_eq!(
            classify_failure(StatusCode::PAYLOAD_TOO_LARGE, b"too large"),
            None
        );
        assert_eq!(
            classify_failure(StatusCode::BAD_REQUEST, b"model_not_found"),
            Some(FailureClass::ModelUnavailable)
        );
        assert_eq!(
            classify_failure(StatusCode::BAD_REQUEST, b"unknown model"),
            Some(FailureClass::ModelUnavailable)
        );
        assert_eq!(
            classify_failure(StatusCode::BAD_REQUEST, b"insufficient quota"),
            Some(FailureClass::Billing)
        );
    }

    #[test]
    fn auto_aliases_cover_current_and_legacy_names() {
        assert!(is_auto_alias("auto"));
        assert!(is_auto_alias("echobird/auto"));
        assert!(is_auto_alias("echobird-auto"));
        assert!(is_auto_alias("smart-router"));
        // Anything else is treated as a group name, not an auto alias.
        assert!(!is_auto_alias("other-model"));
    }

    #[test]
    fn appends_chat_completions_once() {
        assert_eq!(
            chat_completions_url("https://example.com/v1"),
            "https://example.com/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("https://example.com/v1/chat/completions"),
            "https://example.com/v1/chat/completions"
        );
    }

    #[test]
    fn builds_the_anthropic_messages_endpoint() {
        assert_eq!(
            messages_url("https://api.anthropic.com"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            messages_url("https://gateway.example.com/v1"),
            "https://gateway.example.com/v1/messages"
        );
        assert_eq!(
            messages_url("https://gateway.example.com/v1/messages/"),
            "https://gateway.example.com/v1/messages"
        );
    }

    #[test]
    fn gone_responses_fail_over_like_a_missing_model() {
        // Free-model pools retire endpoints constantly, and an unclassified 410
        // is returned to the client with the rest of the pool left untried.
        assert_eq!(
            classify_failure(StatusCode::GONE, b"{\"error\":{\"message\":\"gone\"}}"),
            Some(FailureClass::ModelUnavailable)
        );
    }

    #[test]
    fn a_400_stops_the_search_unless_it_names_a_dead_model() {
        // Retrying a genuinely bad request elsewhere would only burn the
        // budget, so a plain 400 ends the search — but when it names a model
        // the provider cannot serve, the candidate is what is broken.
        assert_eq!(
            classify_failure(StatusCode::BAD_REQUEST, b"bad input"),
            None
        );
        assert_eq!(
            classify_failure(
                StatusCode::BAD_REQUEST,
                b"{\"error\":{\"message\":\"Model id : MiniMax/MiniMax-M3 , has no provider supported\"}}"
            ),
            Some(FailureClass::ModelUnavailable)
        );
    }

    #[tokio::test]
    async fn routes_to_an_anthropic_only_candidate() {
        let upstream = Router::new().route(
            "/v1/messages",
            post(|headers: HeaderMap, Json(body): Json<Value>| async move {
                assert_eq!(headers["x-api-key"], "upstream-key");
                assert_eq!(headers["anthropic-version"], "2023-06-01");
                assert_eq!(body["model"], "anthropic-model");
                // Anthropic requires max_tokens even when the OpenAI body omits it.
                assert!(body["max_tokens"].as_u64().is_some());
                Json(json!({
                    "id": "msg_anthropic",
                    "type": "message",
                    "role": "assistant",
                    "model": body["model"],
                    "content": [{"type": "text", "text": "hello"}],
                    "stop_reason": "end_turn",
                    "usage": {"input_tokens": 3, "output_tokens": 2}
                }))
            }),
        );
        let addr = spawn_mock(upstream).await;
        let state = AppState::for_tests().unwrap();
        let response = route_chat_with_candidates(
            &state,
            &inbound_headers(),
            json!({
                "model": SMART_ROUTER_MODEL_ID,
                "messages": [{"role": "user", "content": "hi"}]
            }),
            vec![anthropic_candidate(addr, "anthropic")],
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let data: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(data["choices"][0]["message"]["content"], "hello");
        assert_eq!(data["choices"][0]["finish_reason"], "stop");
        assert_eq!(data["usage"]["total_tokens"], 5);
    }

    #[tokio::test]
    async fn translates_an_anthropic_stream_for_openai_clients() {
        let upstream = Router::new().route(
            "/v1/messages",
            post(|| async move {
                let events = [
                    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_stream\",\"model\":\"claude-real\"}}\n\n",
                    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hel\"}}\n\n",
                    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n",
                    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n",
                    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
                ];
                let stream = tokio_stream::iter(
                    events
                        .into_iter()
                        .map(|event| Ok::<Bytes, Infallible>(Bytes::from(event.to_string()))),
                );
                Body::from_stream(stream)
            }),
        );
        let addr = spawn_mock(upstream).await;
        let state = AppState::for_tests().unwrap();
        let response = route_chat_with_candidates(
            &state,
            &inbound_headers(),
            json!({
                "model": SMART_ROUTER_MODEL_ID,
                "stream": true,
                "messages": [{"role": "user", "content": "hi"}]
            }),
            vec![anthropic_candidate(addr, "anthropic")],
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("\"content\":\"hel\""), "{text}");
        assert!(text.contains("\"content\":\"lo\""), "{text}");
        assert!(text.contains("\"finish_reason\":\"stop\""), "{text}");
        assert!(text.contains("data: [DONE]"), "{text}");
    }

    #[tokio::test]
    async fn sends_openrouter_app_attribution_headers() {
        let upstream = Router::new().route(
            "/v1/chat/completions",
            post(|headers: HeaderMap, Json(body): Json<Value>| async move {
                assert_eq!(headers["HTTP-Referer"], "https://echobird.ai");
                assert_eq!(headers["X-OpenRouter-Title"], "SmartRouter");
                assert_eq!(
                    headers["X-OpenRouter-Categories"],
                    "programming-app,personal-agent"
                );
                Json(json!({
                    "id": "chatcmpl-attribution",
                    "model": body["model"],
                    "choices": [{"message": {"role": "assistant", "content": "ok"}}]
                }))
            }),
        );
        let addr = spawn_mock(upstream).await;
        let state = AppState::for_tests().unwrap();
        let response = route_chat_with_candidates(
            &state,
            &inbound_headers(),
            json!({"model": SMART_ROUTER_MODEL_ID, "messages": []}),
            vec![candidate(addr, "openrouter")],
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn route_memory_survives_round_trip_and_skips_cooldown() {
        let state = AppState::for_tests().unwrap();
        let addr = SocketAddr::from(([127, 0, 0, 1], 1));
        let first = candidate(addr, "first");
        let second = candidate(addr, "second");

        mark_success(&state, &second);
        let ordered = ready_order(&state, vec![first.clone(), second.clone()]);
        assert_eq!(ordered[0].internal_id, "second");

        mark_failure(&state, &second, FailureClass::RateLimit, None);
        let serialized = {
            let memory = state.route_memory.lock().unwrap();
            serde_json::to_string(&*memory).unwrap()
        };
        let restored: RouteMemory = serde_json::from_str(&serialized).unwrap();
        let restored_state = AppState {
            http_client: state.http_client.clone(),
            route_memory: Arc::new(Mutex::new(restored)),
            route_memory_path: None,
        };
        let available = ready_order(&restored_state, vec![first, second]);
        assert_eq!(available.len(), 1);
        assert_eq!(available[0].internal_id, "first");
    }

    #[test]
    fn cooldowns_match_failure_type_and_cap_retry_after() {
        assert_eq!(
            cooldown_for_failure(FailureClass::RateLimit, None, 1),
            Duration::from_secs(60)
        );
        assert_eq!(
            cooldown_for_failure(FailureClass::Transient, None, 4),
            Duration::from_secs(4 * 60)
        );
        assert_eq!(
            cooldown_for_failure(FailureClass::ModelUnavailable, None, 1),
            Duration::from_secs(6 * 60 * 60)
        );
        assert_eq!(
            cooldown_for_failure(FailureClass::RateLimit, Some(Duration::MAX), 1),
            MAX_RETRY_AFTER
        );
    }

    #[test]
    fn parses_retry_after_seconds_and_http_dates() {
        let mut headers = HeaderMap::new();
        headers.insert(header::RETRY_AFTER, HeaderValue::from_static("3600"));
        assert_eq!(retry_after(&headers), Some(Duration::from_secs(3600)));

        let retry_at = chrono::Utc::now() + chrono::Duration::seconds(90);
        let http_date = retry_at.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
        headers.insert(
            header::RETRY_AFTER,
            HeaderValue::from_str(&http_date).unwrap(),
        );
        let parsed = retry_after(&headers).unwrap();
        assert!(parsed >= Duration::from_secs(88));
        assert!(parsed <= Duration::from_secs(90));
    }

    #[test]
    fn spent_account_is_skipped_and_reports_when_it_frees() {
        let state = AppState::for_tests().unwrap();
        let model = candidate(SocketAddr::from(([127, 0, 0, 1], 1)), "free");
        record_learned_rpm(&state, &model, 20);
        record_sends(&state, &model, 20, now_ms());

        assert!(ready_order(&state, vec![model.clone()]).is_empty());
        match select_candidate(&state, std::slice::from_ref(&model), &HashSet::new()) {
            Selection::RateLimited(wait) => {
                assert!(wait >= Duration::from_secs(55), "wait was {wait:?}");
                assert!(wait <= Duration::from_secs(61), "wait was {wait:?}");
            }
            _ => panic!("a spent account must not be selected"),
        }
    }

    #[test]
    fn declared_budget_throttles_a_provider_that_advertises_nothing() {
        let state = AppState::for_tests().unwrap();
        let model = with_declared_rpm(candidate(SocketAddr::from(([127, 0, 0, 1], 1)), "free"), 20);

        // The first selection picks the declaration up from the model config.
        assert_eq!(ready_order(&state, vec![model.clone()]).len(), 1);
        record_sends(&state, &model, 19, now_ms());
        assert_eq!(ready_order(&state, vec![model.clone()]).len(), 1);

        record_sends(&state, &model, 1, now_ms());
        assert!(ready_order(&state, vec![model.clone()]).is_empty());
    }

    #[test]
    fn strictest_budget_wins_when_models_share_a_key() {
        let state = AppState::for_tests().unwrap();
        let addr = SocketAddr::from(([127, 0, 0, 1], 1));
        let strict = with_declared_rpm(candidate(addr, "strict"), 10);
        let loose = with_declared_rpm(candidate(addr, "loose"), 60);
        assert_eq!(strict.account_key, loose.account_key);

        assert_eq!(
            ready_order(&state, vec![strict.clone(), loose.clone()]).len(),
            2
        );
        record_sends(&state, &strict, 10, now_ms());
        // Both models are parked: they spend from the one shared quota.
        assert!(ready_order(&state, vec![strict, loose]).is_empty());
    }

    #[test]
    fn learned_limit_is_capped_by_the_declared_budget() {
        let state = AppState::for_tests().unwrap();
        let model = with_declared_rpm(candidate(SocketAddr::from(([127, 0, 0, 1], 1)), "free"), 10);
        // The provider claims 60, the user asked for 10: the stricter one holds.
        record_learned_rpm(&state, &model, 60);

        assert_eq!(ready_order(&state, vec![model.clone()]).len(), 1);
        record_sends(&state, &model, 10, now_ms());
        assert!(ready_order(&state, vec![model]).is_empty());
    }

    #[test]
    fn unlimited_declaration_beats_an_advertised_limit() {
        let state = AppState::for_tests().unwrap();
        let model = with_unlimited_rpm(candidate(SocketAddr::from(([127, 0, 0, 1], 1)), "paid"));
        // The provider claims 60 and the user answered `-1`: the router stops
        // counting, leaving 429 responses as the only brake.
        record_learned_rpm(&state, &model, 60);

        record_sends(&state, &model, 500, now_ms());
        assert_eq!(ready_order(&state, vec![model]).len(), 1);
    }

    #[test]
    fn a_cap_on_a_shared_key_outranks_an_unlimited_keymate() {
        let state = AppState::for_tests().unwrap();
        let addr = SocketAddr::from(([127, 0, 0, 1], 1));
        let capped = with_declared_rpm(candidate(addr, "free"), 10);
        let unlimited = with_unlimited_rpm(candidate(addr, "paid"));
        assert_eq!(capped.account_key, unlimited.account_key);

        assert_eq!(
            ready_order(&state, vec![capped.clone(), unlimited.clone()]).len(),
            2
        );
        record_sends(&state, &capped, 10, now_ms());
        // One model of the key says "no limit", the other says 10: the quota
        // they share still runs out for both.
        assert!(ready_order(&state, vec![capped, unlimited]).is_empty());
    }

    #[test]
    fn clearing_the_declared_budget_stops_throttling() {
        let state = AppState::for_tests().unwrap();
        let model = with_declared_rpm(candidate(SocketAddr::from(([127, 0, 0, 1], 1)), "free"), 5);
        assert_eq!(ready_order(&state, vec![model.clone()]).len(), 1);
        record_sends(&state, &model, 5, now_ms());
        assert!(ready_order(&state, vec![model.clone()]).is_empty());

        // Same model, RPM field emptied in the UI: no declaration, no throttling.
        let cleared = candidate(SocketAddr::from(([127, 0, 0, 1], 1)), "free");
        assert_eq!(ready_order(&state, vec![cleared]).len(), 1);
    }

    #[test]
    fn unadvertised_limits_are_never_throttled_locally() {
        let state = AppState::for_tests().unwrap();
        let model = candidate(SocketAddr::from(([127, 0, 0, 1], 1)), "paid");
        // A paid key advertises nothing, so however much it has sent it must
        // stay usable: the router only throttles what the provider told it.
        record_sends(&state, &model, 500, now_ms());
        assert_eq!(ready_order(&state, vec![model]).len(), 1);
    }

    #[test]
    fn an_idle_account_outranks_the_last_good_pick() {
        let state = AppState::for_tests().unwrap();
        let preferred = candidate(SocketAddr::from(([127, 0, 0, 1], 1)), "preferred");
        let other = candidate(SocketAddr::from(([127, 0, 0, 1], 2)), "other");
        {
            let mut memory = state.route_memory.lock().unwrap();
            memory.last_success_id = Some("preferred".to_string());
        }
        record_learned_rpm(&state, &preferred, 10);
        record_learned_rpm(&state, &other, 10);

        // Both accounts are untouched, so the tie-break keeps the last-good pick.
        let ordered = ready_order(&state, vec![preferred.clone(), other.clone()]);
        assert_eq!(ordered[0].internal_id, "preferred");

        // A single send is enough for the idle account to take over: load now
        // outranks the sticky pick, so the pool spreads instead of draining one
        // account while an idle one sits unused.
        record_sends(&state, &preferred, 1, now_ms());
        let ordered = ready_order(&state, vec![preferred, other]);
        assert_eq!(ordered[0].internal_id, "other");
    }

    #[test]
    fn models_sharing_a_key_rotate_instead_of_draining_one() {
        let state = AppState::for_tests().unwrap();
        let addr = SocketAddr::from(([127, 0, 0, 1], 1));
        let first = with_declared_rpm(candidate(addr, "first"), 20);
        let second = with_declared_rpm(candidate(addr, "second"), 20);
        // One key, one 20 RPM bucket: load cannot tell the two apart, so recency
        // is the only thing keeping both models in play.
        assert_eq!(first.account_key, second.account_key);
        let pool = vec![first.clone(), second.clone()];

        assert_eq!(ready_order(&state, pool.clone())[0].internal_id, "first");
        assert_eq!(pick(&state, &pool).internal_id, "first");
        // The pick was stamped, which hands the next request to its key-mate.
        assert_eq!(ready_order(&state, pool.clone())[0].internal_id, "second");
        std::thread::sleep(Duration::from_millis(2));
        assert_eq!(pick(&state, &pool).internal_id, "second");
        // And back again: a two-model key alternates, neither is drained first.
        assert_eq!(ready_order(&state, pool)[0].internal_id, "first");
    }

    #[test]
    fn in_flight_requests_do_not_stack_on_the_sticky_candidate() {
        let state = AppState::for_tests().unwrap();
        let preferred = candidate(SocketAddr::from(([127, 0, 0, 1], 1)), "preferred");
        let other = candidate(SocketAddr::from(([127, 0, 0, 1], 2)), "other");
        {
            let mut memory = state.route_memory.lock().unwrap();
            memory.last_success_id = Some("preferred".to_string());
            memory
                .accounts
                .entry(preferred.account_key.clone())
                .or_default()
                .inflight = 1;
        }

        let ordered = ready_order(&state, vec![preferred, other]);
        assert_eq!(ordered[0].internal_id, "other");
    }

    #[test]
    fn response_headers_teach_the_ledger_the_account_quota() {
        let state = AppState::for_tests().unwrap();
        let model = candidate(SocketAddr::from(([127, 0, 0, 1], 1)), "free");
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-limit-requests", HeaderValue::from_static("20"));
        headers.insert(
            "x-ratelimit-remaining-requests",
            HeaderValue::from_static("19"),
        );

        learn_account_limits(&state, &model, &headers);

        {
            let memory = state.route_memory.lock().unwrap();
            let account = memory.accounts.get(&model.account_key).unwrap();
            assert_eq!(account.learned_rpm, Some(20));
            // `remaining` 19 of 20 admits the response's own send.
            assert_eq!(account.live_sends(), 1);
        }

        // Exhausting the advertised quota parks the account before any 429.
        record_sends(&state, &model, 19, now_ms());
        assert!(ready_order(&state, vec![model]).is_empty());
    }

    #[test]
    fn learned_limit_survives_a_restart_while_the_window_does_not() {
        let state = AppState::for_tests().unwrap();
        let model = candidate(SocketAddr::from(([127, 0, 0, 1], 1)), "free");
        {
            let mut memory = state.route_memory.lock().unwrap();
            let account = memory
                .accounts
                .entry(model.account_key.clone())
                .or_default();
            account.learned_rpm = Some(20);
            account.sent_ms.push(now_ms());
        }

        let serialized = {
            let memory = state.route_memory.lock().unwrap();
            serde_json::to_string(&*memory).unwrap()
        };
        let restored: RouteMemory = serde_json::from_str(&serialized).unwrap();
        let account = restored.accounts.get(&model.account_key).unwrap();
        assert_eq!(account.learned_rpm, Some(20));
        assert!(account.sent_ms.is_empty());
        assert_eq!(account.inflight, 0);
    }

    #[test]
    fn long_retry_after_blocks_every_model_sharing_the_key() {
        let state = AppState::for_tests().unwrap();
        let addr = SocketAddr::from(([127, 0, 0, 1], 1));
        let first = candidate(addr, "first");
        let second = candidate(addr, "second");
        assert_eq!(first.account_key, second.account_key);

        mark_failure(
            &state,
            &first,
            FailureClass::RateLimit,
            Some(Duration::from_secs(3600)),
        );
        assert!(ready_order(&state, vec![first, second]).is_empty());

        // A short Retry-After is one model's hiccup, not an account-wide cap.
        let other = candidate(SocketAddr::from(([127, 0, 0, 1], 2)), "other");
        mark_failure(
            &state,
            &other,
            FailureClass::RateLimit,
            Some(Duration::from_secs(2)),
        );
        let memory = state.route_memory.lock().unwrap();
        let account_blocked = memory
            .accounts
            .get(&other.account_key)
            .and_then(|account| account.next_allowed_ms)
            .is_some();
        assert!(!account_blocked);
    }

    #[test]
    fn accounts_are_keyed_by_upstream_not_by_model() {
        let addr = SocketAddr::from(([127, 0, 0, 1], 1));
        let router_model = candidate(addr, "free-a");
        let mut same_host_other_model = candidate(addr, "free-b");
        same_host_other_model.router_group = Some("group".to_string());
        assert_eq!(router_model.account_key, same_host_other_model.account_key);

        let mut other_path = router_model.clone();
        other_path.base_url = format!("http://{addr}/v1beta");
        assert_eq!(
            account_key(&other_path.base_url, &other_path.api_key),
            router_model.account_key
        );

        let mut other_key = router_model.clone();
        other_key.api_key = "another-key".to_string();
        assert_ne!(
            account_key(&other_key.base_url, &other_key.api_key),
            router_model.account_key
        );
    }

    #[test]
    fn parses_provider_reset_durations() {
        assert_eq!(parse_reset_duration("1s"), Some(Duration::from_secs(1)));
        assert_eq!(
            parse_reset_duration("500ms"),
            Some(Duration::from_millis(500))
        );
        assert_eq!(parse_reset_duration("6m0s"), Some(Duration::from_secs(360)));
        assert_eq!(
            parse_reset_duration("1.5s"),
            Some(Duration::from_millis(1500))
        );
        assert_eq!(parse_reset_duration("30"), Some(Duration::from_secs(30)));
        assert_eq!(parse_reset_duration(""), None);
        assert_eq!(parse_reset_duration("soon"), None);

        // An absolute epoch must not be read as a multi-year block.
        let reset = parse_reset_duration(&(now_ms() + 5_000).to_string()).unwrap();
        assert!(reset >= Duration::from_secs(4), "reset was {reset:?}");
        assert!(reset <= Duration::from_secs(6), "reset was {reset:?}");
    }

    #[tokio::test]
    async fn last_slot_in_the_window_is_still_usable() {
        let upstream = Router::new().route(
            "/v1/chat/completions",
            post(|Json(body): Json<Value>| async move {
                Json(json!({
                    "id": "chatcmpl-test",
                    "model": body["model"],
                    "choices": [{"message": {"role": "assistant", "content": "ok"}}]
                }))
            }),
        );
        let addr = spawn_mock(upstream).await;
        let model = candidate(addr, "free");
        let state = AppState::for_tests().unwrap();
        record_learned_rpm(&state, &model, 20);
        record_sends(&state, &model, 19, now_ms());

        let response = route_chat_with_candidates(
            &state,
            &inbound_headers(),
            json!({"model": SMART_ROUTER_MODEL_ID, "messages": []}),
            vec![model.clone()],
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let memory = state.route_memory.lock().unwrap();
        let account = memory.accounts.get(&model.account_key).unwrap();
        assert_eq!(account.inflight, 0, "the reservation must be released");
        assert_eq!(account.live_sends(), 20);
    }

    #[tokio::test]
    async fn exhausted_accounts_answer_429_with_retry_after() {
        let upstream = Router::new().route(
            "/v1/chat/completions",
            post(|| async { Json(json!({"choices": []})) }),
        );
        let addr = spawn_mock(upstream).await;
        let model = candidate(addr, "free");
        let state = AppState::for_tests().unwrap();
        record_learned_rpm(&state, &model, 20);
        record_sends(&state, &model, 20, now_ms());

        let response = route_chat_with_candidates(
            &state,
            &inbound_headers(),
            json!({"model": SMART_ROUTER_MODEL_ID, "messages": []}),
            vec![model],
        )
        .await;

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let retry_after = response.headers()[header::RETRY_AFTER]
            .to_str()
            .unwrap()
            .parse::<u64>()
            .unwrap();
        assert!(
            (55..=60).contains(&retry_after),
            "retry-after was {retry_after}"
        );
    }

    #[test]
    fn expired_transient_cooldown_keeps_failure_streak() {
        let state = AppState::for_tests().unwrap();
        let model = candidate(SocketAddr::from(([127, 0, 0, 1], 1)), "transient");

        mark_failure(&state, &model, FailureClass::Transient, None);
        {
            let mut memory = state.route_memory.lock().unwrap();
            memory
                .candidates
                .get_mut(&model.internal_id)
                .unwrap()
                .cooldown_until_ms = Some(now_ms().saturating_sub(1));
        }
        assert_eq!(ready_order(&state, vec![model.clone()]).len(), 1);

        mark_failure(&state, &model, FailureClass::Transient, None);
        let memory = state.route_memory.lock().unwrap();
        let health = memory.candidates.get(&model.internal_id).unwrap();
        assert_eq!(health.consecutive_failures, 2);
        let remaining = health.cooldown_until_ms.unwrap().saturating_sub(now_ms());
        assert!(remaining >= Duration::from_secs(59).as_millis() as u64);
    }

    #[tokio::test]
    async fn reaches_twentieth_candidate_after_nineteen_fast_failures() {
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        let upstream = Router::new().route(
            "/v1/chat/completions",
            post(move |Json(body): Json<Value>| {
                let attempt = counter.fetch_add(1, Ordering::SeqCst) + 1;
                async move {
                    if attempt < MAX_ROUTE_CANDIDATES {
                        StatusCode::TOO_MANY_REQUESTS.into_response()
                    } else {
                        Json(json!({
                            "id": "chatcmpl-test",
                            "model": body["model"],
                            "choices": [{"message": {"role": "assistant", "content": "ok"}}]
                        }))
                        .into_response()
                    }
                }
            }),
        );
        let addr = spawn_mock(upstream).await;
        let candidates = (0..MAX_ROUTE_CANDIDATES)
            .map(|index| candidate(addr, &format!("candidate-{index}")))
            .collect();
        let state = AppState::for_tests().unwrap();
        let response = route_chat_with_candidates(
            &state,
            &inbound_headers(),
            json!({"model": SMART_ROUTER_MODEL_ID, "messages": []}),
            candidates,
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(hits.load(Ordering::SeqCst), MAX_ROUTE_CANDIDATES);
        assert_eq!(
            response.headers()["X-EchoBird-Routed-Via"],
            "candidate-19/candidate-19-model"
        );
    }

    #[tokio::test]
    async fn fallback_budget_stops_starting_more_candidates() {
        let slow = Router::new().route(
            "/v1/chat/completions",
            post(|| async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                StatusCode::TOO_MANY_REQUESTS
            }),
        );
        let slow_addr = spawn_mock(slow).await;

        let second_hits = Arc::new(AtomicUsize::new(0));
        let second_counter = second_hits.clone();
        let second = Router::new().route(
            "/v1/chat/completions",
            post(move || {
                second_counter.fetch_add(1, Ordering::SeqCst);
                async { StatusCode::OK }
            }),
        );
        let second_addr = spawn_mock(second).await;
        let state = AppState::for_tests().unwrap();
        let response = route_chat_with_policy(
            &state,
            &inbound_headers(),
            json!({"model": SMART_ROUTER_MODEL_ID, "messages": []}),
            vec![
                candidate(slow_addr, "slow"),
                candidate(second_addr, "second"),
            ],
            RoutePolicy {
                max_attempts: MAX_ROUTE_CANDIDATES,
                time_budget: Duration::from_millis(30),
                header_timeout: Duration::from_secs(1),
                first_byte_timeout: Duration::from_secs(1),
            },
        )
        .await;

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(second_hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn success_body_cannot_outlive_routing_budget() {
        let hanging = Router::new().route(
            "/v1/chat/completions",
            post(|| async {
                Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from_stream(stream::pending::<
                        Result<Bytes, Infallible>,
                    >()))
                    .unwrap()
            }),
        );
        let addr = spawn_mock(hanging).await;
        let state = AppState::for_tests().unwrap();
        let routed = tokio::time::timeout(
            Duration::from_millis(500),
            route_chat_with_policy(
                &state,
                &inbound_headers(),
                json!({"model": SMART_ROUTER_MODEL_ID, "messages": []}),
                vec![candidate(addr, "hanging")],
                RoutePolicy {
                    max_attempts: 1,
                    time_budget: Duration::from_millis(30),
                    header_timeout: Duration::from_secs(1),
                    first_byte_timeout: Duration::from_secs(1),
                },
            ),
        )
        .await
        .expect("router exceeded its own budget");

        assert_eq!(routed.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn falls_back_after_rate_limit() {
        let first_hits = Arc::new(AtomicUsize::new(0));
        let first_counter = first_hits.clone();
        let first = Router::new().route(
            "/v1/chat/completions",
            post(move || {
                first_counter.fetch_add(1, Ordering::SeqCst);
                async { StatusCode::TOO_MANY_REQUESTS }
            }),
        );
        let first_addr = spawn_mock(first).await;

        let second_hits = Arc::new(AtomicUsize::new(0));
        let second_counter = second_hits.clone();
        let second = Router::new().route(
            "/v1/chat/completions",
            post(move |Json(body): Json<Value>| {
                second_counter.fetch_add(1, Ordering::SeqCst);
                async move {
                    Json(json!({
                        "id": "chatcmpl-test",
                        "model": body["model"],
                        "choices": [{"message": {"role": "assistant", "content": "ok"}}]
                    }))
                }
            }),
        );
        let second_addr = spawn_mock(second).await;

        let state = AppState::for_tests().unwrap();
        let response = route_chat_with_candidates(
            &state,
            &inbound_headers(),
            json!({"model": SMART_ROUTER_MODEL_ID, "messages": [], "stream": false}),
            vec![
                candidate(first_addr, "first"),
                candidate(second_addr, "second"),
            ],
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["X-EchoBird-Routed-Via"],
            "second/second-model"
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["model"], "second-model");
        assert_eq!(first_hits.load(Ordering::SeqCst), 1);
        assert_eq!(second_hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn never_falls_back_after_first_stream_chunk() {
        let first = Router::new().route(
            "/v1/chat/completions",
            post(|| async {
                let (tx, rx) = tokio::sync::mpsc::channel(2);
                tokio::spawn(async move {
                    tx.send(Ok::<Bytes, std::io::Error>(Bytes::from_static(
                        b"data: first\n\n",
                    )))
                    .await
                    .unwrap();
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    let _ = tx.send(Err(std::io::Error::other("stream failed"))).await;
                });
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from_stream(ReceiverStream::new(rx)))
                    .unwrap()
            }),
        );
        let first_addr = spawn_mock(first).await;

        let second_hits = Arc::new(AtomicUsize::new(0));
        let second_counter = second_hits.clone();
        let second = Router::new().route(
            "/v1/chat/completions",
            post(move || {
                second_counter.fetch_add(1, Ordering::SeqCst);
                async { Json(json!({"choices": []})) }
            }),
        );
        let second_addr = spawn_mock(second).await;

        let state = AppState::for_tests().unwrap();
        let response = route_chat_with_candidates(
            &state,
            &inbound_headers(),
            json!({"model": SMART_ROUTER_MODEL_ID, "messages": [], "stream": true}),
            vec![
                candidate(first_addr, "first"),
                candidate(second_addr, "second"),
            ],
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["X-EchoBird-Routed-Via"],
            "first/first-model"
        );
        let collected = response.into_body().collect().await;
        assert!(collected.is_err());
        assert_eq!(second_hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn falls_back_when_success_stream_has_no_first_byte() {
        let first = Router::new().route(
            "/v1/chat/completions",
            post(|| async {
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::empty())
                    .unwrap()
            }),
        );
        let first_addr = spawn_mock(first).await;
        let second = Router::new().route(
            "/v1/chat/completions",
            post(|| async {
                let chunks = stream::once(async {
                    Ok::<Bytes, Infallible>(Bytes::from_static(b"data: second\n\n"))
                });
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from_stream(chunks))
                    .unwrap()
            }),
        );
        let second_addr = spawn_mock(second).await;

        let state = AppState::for_tests().unwrap();
        let response = route_chat_with_candidates(
            &state,
            &inbound_headers(),
            json!({"model": SMART_ROUTER_MODEL_ID, "messages": [], "stream": true}),
            vec![
                candidate(first_addr, "first"),
                candidate(second_addr, "second"),
            ],
        )
        .await;

        assert_eq!(
            response.headers()["X-EchoBird-Routed-Via"],
            "second/second-model"
        );
    }

    #[tokio::test]
    async fn anthropic_request_falls_back_and_preserves_tool_use() {
        let first = Router::new().route(
            "/v1/chat/completions",
            post(|| async { StatusCode::TOO_MANY_REQUESTS }),
        );
        let first_addr = spawn_mock(first).await;
        let second = Router::new().route(
            "/v1/chat/completions",
            post(|Json(body): Json<Value>| async move {
                assert_eq!(body["model"], "second-model");
                assert_eq!(body["tool_choice"], "required");
                Json(json!({
                    "id": "chatcmpl-tool",
                    "model": body["model"],
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [{
                                "id": "call_weather",
                                "type": "function",
                                "function": {"name": "weather", "arguments": "{\"city\":\"Shanghai\"}"}
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }],
                    "usage": {"prompt_tokens": 12, "completion_tokens": 8}
                }))
            }),
        );
        let second_addr = spawn_mock(second).await;

        let state = AppState::for_tests().unwrap();
        let response = route_messages_with_candidates(
            &state,
            &inbound_headers(),
            json!({
                "model": "claude-opus-5",
                "max_tokens": 1024,
                "messages": [{"role": "user", "content": "weather?"}],
                "tools": [{
                    "name": "weather",
                    "description": "Get weather",
                    "input_schema": {"type": "object", "properties": {"city": {"type": "string"}}}
                }],
                "tool_choice": {"type": "any"}
            }),
            vec![
                candidate(first_addr, "first"),
                candidate(second_addr, "second"),
            ],
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["X-EchoBird-Routed-Via"],
            "second/second-model"
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["type"], "message");
        assert_eq!(body["stop_reason"], "tool_use");
        assert_eq!(body["content"][0]["type"], "tool_use");
        assert_eq!(body["content"][0]["name"], "weather");
        assert_eq!(body["content"][0]["input"]["city"], "Shanghai");
    }

    #[tokio::test]
    async fn anthropic_stream_converts_text_and_tool_events() {
        let upstream = Router::new().route(
            "/v1/chat/completions",
            post(|| async {
                let chunks = stream::iter([
                    Ok::<Bytes, Infallible>(Bytes::from_static(
                        b"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"},\"finish_reason\":null}]}\n\n",
                    )),
                    Ok(Bytes::from_static(
                        b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\"}}]},\"finish_reason\":null}]}\n\n",
                    )),
                    Ok(Bytes::from_static(
                        b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"a.txt\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"completion_tokens\":9}}\n\n",
                    )),
                    Ok(Bytes::from_static(b"data: [DONE]\n\n")),
                ]);
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from_stream(chunks))
                    .unwrap()
            }),
        );
        let upstream_addr = spawn_mock(upstream).await;

        let state = AppState::for_tests().unwrap();
        let response = route_messages_with_candidates(
            &state,
            &inbound_headers(),
            json!({
                "model": "claude-opus-5",
                "max_tokens": 1024,
                "stream": true,
                "messages": [{"role": "user", "content": "read a file"}]
            }),
            vec![candidate(upstream_addr, "stream")],
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("event: message_start"));
        assert!(body.contains("\"type\":\"text_delta\""));
        assert!(body.contains("\"text\":\"hello\""));
        assert!(body.contains("\"type\":\"tool_use\""));
        assert!(body.contains("\"partial_json\":\"{\\\"path\\\":\\\"a.txt\\\"}\""));
        assert!(body.contains("\"stop_reason\":\"tool_use\""));
        assert!(body.contains("event: message_stop"));
    }
}
