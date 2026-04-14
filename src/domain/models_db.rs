//! Cached model catalog from <https://models.dev>.
//!
//! Provides a flat list of AI model entries with provider metadata,
//! cached locally for fast lookup.  The catalog can be filtered by
//! CLI name so the new-agent dialog only shows relevant models.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// How long the local cache stays valid before re-fetching.
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

const API_URL: &str = "https://models.dev/api.json";

// ── Public types ────────────────────────────────────────────────────

/// A single model entry with enough info for the picker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    /// Model identifier passed to the CLI (e.g. `claude-sonnet-4-6`).
    pub id: String,
    /// Human-readable name (e.g. `Claude Sonnet 4.6`).
    pub name: String,
    /// Provider slug (e.g. `anthropic`).
    pub provider: String,
}

/// Full catalog of models.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCatalog {
    pub models: Vec<ModelEntry>,
    #[serde(with = "timestamp_serde")]
    pub fetched_at: SystemTime,
}

// ── CLI → provider mapping ──────────────────────────────────────────

/// Returns the models.dev provider slugs relevant for a given CLI.
pub fn providers_for_cli(cli: &str) -> &[&str] {
    match cli {
        "claude" => &["anthropic"],
        "codex" => &["openai"],
        "copilot" => &["openai", "anthropic", "google", "mistral", "xai", "deepseek"],
        "gemini" => &["google"],
        "qwen" => &["alibaba"],
        "kiro" => &["anthropic", "amazon", "google"],
        // opencode supports any AI-SDK provider
        "opencode" => &[
            "anthropic",
            "openai",
            "google",
            "xai",
            "deepseek",
            "mistral",
            "amazon",
        ],
        _ => &[],
    }
}

// ── Cache path ──────────────────────────────────────────────────────

fn cache_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".canopy/models_cache.json"))
}

// ── Public API ──────────────────────────────────────────────────────

/// Load the catalog from cache, fetching from the network if stale/missing.
///
/// Returns `None` only when both cache and network fail.
pub fn load_catalog() -> Option<ModelCatalog> {
    if let Some(cached) = load_from_cache() {
        if cached.fetched_at.elapsed().unwrap_or(CACHE_TTL) < CACHE_TTL {
            return Some(cached);
        }
    }
    // Cache stale or missing — try network
    fetch_and_cache().or_else(load_from_cache)
}

/// Filter catalog entries to only models relevant for `cli_name`.
pub fn models_for_cli(catalog: &ModelCatalog, cli_name: &str) -> Vec<ModelEntry> {
    let providers = providers_for_cli(cli_name);
    if providers.is_empty() {
        return catalog.models.clone();
    }
    catalog
        .models
        .iter()
        .filter(|m| providers.contains(&m.provider.as_str()))
        .cloned()
        .collect()
}

/// Filter a model list by a search query (case-insensitive substring).
pub fn filter_models(models: &[ModelEntry], query: &str) -> Vec<ModelEntry> {
    if query.is_empty() {
        return models.to_vec();
    }
    let q = query.to_lowercase();
    models
        .iter()
        .filter(|m| m.id.to_lowercase().contains(&q) || m.name.to_lowercase().contains(&q))
        .cloned()
        .collect()
}

// ── Internal: fetch ─────────────────────────────────────────────────

fn fetch_and_cache() -> Option<ModelCatalog> {
    let body: HashMap<String, ProviderRaw> = reqwest::blocking::Client::new()
        .get(API_URL)
        .timeout(Duration::from_secs(10))
        .send()
        .ok()?
        .json()
        .ok()?;

    let mut entries = Vec::new();
    for (provider_id, provider) in &body {
        for model in provider.models.values() {
            entries.push(ModelEntry {
                id: model.id.clone(),
                name: model.name.clone().unwrap_or_else(|| model.id.clone()),
                provider: provider_id.clone(),
            });
        }
    }
    entries.sort_by(|a, b| a.provider.cmp(&b.provider).then(a.id.cmp(&b.id)));

    let catalog = ModelCatalog {
        models: entries,
        fetched_at: SystemTime::now(),
    };

    save_to_cache(&catalog);
    Some(catalog)
}

// ── Internal: cache I/O ─────────────────────────────────────────────

fn load_from_cache() -> Option<ModelCatalog> {
    let path = cache_path()?;
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

fn save_to_cache(catalog: &ModelCatalog) {
    let Some(path) = cache_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(catalog) {
        let _ = std::fs::write(path, json);
    }
}

// ── Raw API types (only used for deserialization) ───────────────────

#[derive(Deserialize)]
struct ProviderRaw {
    #[serde(default)]
    models: HashMap<String, ModelRaw>,
}

#[derive(Deserialize)]
struct ModelRaw {
    id: String,
    name: Option<String>,
}

// ── Timestamp serde helper ──────────────────────────────────────────

mod timestamp_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    pub fn serialize<S: Serializer>(time: &SystemTime, ser: S) -> Result<S::Ok, S::Error> {
        let secs = time.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        secs.serialize(ser)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<SystemTime, D::Error> {
        let secs = u64::deserialize(de)?;
        Ok(UNIX_EPOCH + Duration::from_secs(secs))
    }
}
