//! Nexus Mods API client.
//!
//! Endpoints confirmed against Nexus's own node client (node-nexus-api):
//!   GET https://api.nexusmods.com/v1/users/validate
//!   GET https://api.nexusmods.com/v1/games/{game}/mods/md5_search/{md5}
//!   GET https://api.nexusmods.com/v1/games/{game}/mods/{id}
//! API key travels in the `apikey` header. Rate limits come back in
//! `x-rl-daily-remaining` / `x-rl-hourly-remaining` (+ `-reset` variants);
//! exhaustion is HTTP 429.
//!
//! The v1 API has no search-mods-by-name endpoint (still true as of
//! 2026-08); anything unresolvable stays in the Unknown list by design.
//!
//! The API key is never logged and never interpolated into error text.
//! Responses are cached on disk (mod info 24h, md5 lookups 30 days) so
//! repeated scans don't burn rate limit.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const BASE: &str = "https://api.nexusmods.com/v1";
pub const GAME_DOMAIN: &str = "skyrimspecialedition";
const MOD_INFO_TTL_SECS: i64 = 24 * 60 * 60;
const MD5_TTL_SECS: i64 = 30 * 24 * 60 * 60;

fn client() -> &'static reqwest::blocking::Client {
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("Lineage/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest client")
    })
}

// ---------------------------------------------------------------------------
// Rate limit surface
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Default)]
pub struct RateLimitInfo {
    pub daily_remaining: Option<i64>,
    pub hourly_remaining: Option<i64>,
    pub daily_reset: Option<String>,
    pub hourly_reset: Option<String>,
}

fn rate_limit_cell() -> &'static Mutex<Option<RateLimitInfo>> {
    static CELL: OnceLock<Mutex<Option<RateLimitInfo>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

fn record_rate_limit(headers: &reqwest::header::HeaderMap) -> RateLimitInfo {
    let get = |name: &str| -> Option<String> {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let info = RateLimitInfo {
        daily_remaining: get("x-rl-daily-remaining").and_then(|v| v.parse().ok()),
        hourly_remaining: get("x-rl-hourly-remaining").and_then(|v| v.parse().ok()),
        daily_reset: get("x-rl-daily-reset"),
        hourly_reset: get("x-rl-hourly-reset"),
    };
    *rate_limit_cell().lock().unwrap() = Some(info.clone());
    info
}

/// Latest rate-limit readings from any Nexus response this session.
#[tauri::command]
pub fn get_rate_limit() -> Option<RateLimitInfo> {
    rate_limit_cell().lock().unwrap().clone()
}

// ---------------------------------------------------------------------------
// Failure type — keeps 429 distinguishable so callers can degrade gracefully
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct NexusFailure {
    pub rate_limited: bool,
    /// Human message; never contains the API key.
    pub message: String,
}

impl std::fmt::Display for NexusFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

fn rate_limit_message() -> String {
    let info = rate_limit_cell().lock().unwrap().clone().unwrap_or_default();
    let reset = info
        .hourly_remaining
        .filter(|r| *r <= 0)
        .and(info.hourly_reset.clone())
        .or(info.daily_reset.clone());
    match reset {
        Some(reset) => format!(
            "Nexus rate limit reached. Requests resume at {reset}. Already-identified mods stay cached."
        ),
        None => "Nexus rate limit reached. Wait a while and rescan — already-identified mods stay cached.".to_string(),
    }
}

fn get_json<T: serde::de::DeserializeOwned>(
    url: &str,
    api_key: &str,
) -> Result<T, NexusFailure> {
    let response = client()
        .get(url)
        .header("apikey", api_key)
        .send()
        .map_err(|e| NexusFailure {
            rate_limited: false,
            // reqwest errors carry the URL, never headers — safe to include.
            message: format!("Couldn't reach Nexus Mods ({e}). Check your connection and try again."),
        })?;
    record_rate_limit(response.headers());
    let status = response.status();
    if status.as_u16() == 429 {
        return Err(NexusFailure {
            rate_limited: true,
            message: rate_limit_message(),
        });
    }
    if status.as_u16() == 401 {
        return Err(NexusFailure {
            rate_limited: false,
            message: "Nexus rejected the API key. Check the key in Settings — copy it fresh from your Nexus account page.".to_string(),
        });
    }
    if status.as_u16() == 404 {
        return Err(NexusFailure {
            rate_limited: false,
            message: "not found".to_string(),
        });
    }
    if !status.is_success() {
        return Err(NexusFailure {
            rate_limited: false,
            message: format!("Nexus Mods returned an unexpected response ({status})."),
        });
    }
    response.json::<T>().map_err(|e| NexusFailure {
        rate_limited: false,
        message: format!("Nexus sent a response Lineage couldn't read ({e})."),
    })
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ValidateResponse {
    name: Option<String>,
    is_premium: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct NexusValidation {
    pub name: String,
    pub is_premium: bool,
    pub daily_remaining: Option<i64>,
    pub hourly_remaining: Option<i64>,
}

/// Validate a candidate API key (called before the key is saved). Returns the
/// account name and remaining quota; the key itself is only sent to Nexus.
#[tauri::command]
pub async fn validate_nexus_key(key: String) -> Result<NexusValidation, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let key = key.trim().to_string();
        if key.is_empty() {
            return Err("Paste your personal API key from your Nexus Mods account settings.".to_string());
        }
        let body: ValidateResponse =
            get_json(&format!("{BASE}/users/validate"), &key).map_err(|f| f.message)?;
        let limits = rate_limit_cell().lock().unwrap().clone().unwrap_or_default();
        Ok(NexusValidation {
            name: body.name.unwrap_or_else(|| "Nexus user".into()),
            is_premium: body.is_premium.unwrap_or(false),
            daily_remaining: limits.daily_remaining,
            hourly_remaining: limits.hourly_remaining,
        })
    })
    .await
    .map_err(|e| format!("validation task failed: {e}"))?
}

// ---------------------------------------------------------------------------
// Mod info + MD5 search, with the disk cache
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NexusModInfo {
    pub mod_id: u32,
    pub name: Option<String>,
    pub summary: Option<String>,
    pub author: Option<String>,
    pub uploaded_by: Option<String>,
    pub picture_url: Option<String>,
    pub version: Option<String>,
    pub category_id: Option<i64>,
    pub available: Option<bool>,
    /// Category display name, resolved from the game info endpoint (not part
    /// of the mod response itself; filled in before caching).
    #[serde(default)]
    pub category: Option<String>,
}

// ---------------------------------------------------------------------------
// Category names (from GET /v1/games/{game}, cached 7 days)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Serialize, Clone)]
struct GameCategory {
    category_id: i64,
    name: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct GameInfo {
    #[serde(default)]
    categories: Vec<GameCategory>,
}

fn category_name(app: &tauri::AppHandle, api_key: &str, category_id: i64) -> Option<String> {
    const GAME_INFO_TTL: i64 = 7 * 24 * 60 * 60;
    let cache_file = "game-info.json";
    let info: Option<GameInfo> = match cache_read::<GameInfo>(app, cache_file, GAME_INFO_TTL) {
        Some(cached) => cached,
        None => {
            // Best effort: category names are decoration, never worth failing
            // (or rate-limit-spending retries) over.
            let fetched = get_json::<GameInfo>(&format!("{BASE}/games/{GAME_DOMAIN}"), api_key).ok();
            cache_write(app, cache_file, &fetched);
            fetched
        }
    };
    info?
        .categories
        .into_iter()
        .find(|c| c.category_id == category_id)
        .map(|c| c.name)
}

fn fill_category(app: &tauri::AppHandle, api_key: &str, info: &mut NexusModInfo) {
    if info.category.is_none() {
        info.category = info
            .category_id
            .and_then(|id| category_name(app, api_key, id));
    }
}

#[derive(Debug, Deserialize)]
struct Md5SearchRow {
    #[serde(rename = "mod")]
    mod_info: Option<NexusModInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheRecord<T> {
    fetched_at: i64,
    /// None caches a miss (e.g. md5 not on Nexus) so we don't re-ask.
    data: Option<T>,
}

fn cache_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    use tauri::Manager;
    let dir = app.path().app_config_dir().ok()?.join("cache").join("nexus");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

fn cache_read<T: serde::de::DeserializeOwned>(
    app: &tauri::AppHandle,
    file: &str,
    ttl_secs: i64,
) -> Option<Option<T>> {
    let path = cache_dir(app)?.join(file);
    let text = std::fs::read_to_string(path).ok()?;
    let record: CacheRecord<T> = serde_json::from_str(&text).ok()?;
    let age = chrono::Utc::now().timestamp() - record.fetched_at;
    (age >= 0 && age < ttl_secs).then_some(record.data)
}

fn cache_write<T: Serialize>(app: &tauri::AppHandle, file: &str, data: &Option<T>) {
    let Some(dir) = cache_dir(app) else { return };
    let record = CacheRecord::<&T> {
        fetched_at: chrono::Utc::now().timestamp(),
        data: data.as_ref(),
    };
    if let Ok(json) = serde_json::to_string(&record) {
        let _ = std::fs::write(dir.join(file), json);
    }
}

/// Mod info by id, disk-cached for 24h. `Ok(None)` = mod hidden/deleted.
pub fn mod_info(
    app: &tauri::AppHandle,
    api_key: &str,
    mod_id: u32,
) -> Result<Option<NexusModInfo>, NexusFailure> {
    let cache_file = format!("mod-{mod_id}.json");
    if let Some(cached) = cache_read::<NexusModInfo>(app, &cache_file, MOD_INFO_TTL_SECS) {
        return Ok(cached);
    }
    let url = format!("{BASE}/games/{GAME_DOMAIN}/mods/{mod_id}");
    match get_json::<NexusModInfo>(&url, api_key) {
        Ok(mut info) => {
            fill_category(app, api_key, &mut info);
            let data = Some(info);
            cache_write(app, &cache_file, &data);
            Ok(data)
        }
        Err(f) if f.message == "not found" => {
            cache_write::<NexusModInfo>(app, &cache_file, &None);
            Ok(None)
        }
        Err(f) => Err(f),
    }
}

/// MD5 lookup, disk-cached for 30 days (the hash → mod mapping is stable).
/// `Ok(None)` = the file isn't on Nexus.
pub fn md5_lookup(
    app: &tauri::AppHandle,
    api_key: &str,
    md5_hex: &str,
) -> Result<Option<NexusModInfo>, NexusFailure> {
    let cache_file = format!("md5-{md5_hex}.json");
    if let Some(cached) = cache_read::<NexusModInfo>(app, &cache_file, MD5_TTL_SECS) {
        return Ok(cached);
    }
    let url = format!("{BASE}/games/{GAME_DOMAIN}/mods/md5_search/{md5_hex}");
    match get_json::<Vec<Md5SearchRow>>(&url, api_key) {
        Ok(rows) => {
            let mut data = rows.into_iter().find_map(|r| r.mod_info);
            if let Some(info) = data.as_mut() {
                fill_category(app, api_key, info);
            }
            cache_write(app, &cache_file, &data);
            Ok(data)
        }
        // Nexus answers 404 for "no file with this hash".
        Err(f) if f.message == "not found" => {
            cache_write::<NexusModInfo>(app, &cache_file, &None);
            Ok(None)
        }
        Err(f) => Err(f),
    }
}
