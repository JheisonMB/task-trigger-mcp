use crate::setup_module::models::{
    is_binary_available, resolve_config_path, CanonicalServers, Platform, RegistryRaw,
};
use crate::setup_module::PlatformWithCli;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

/// Lightweight index for the per-platform registry (v6).
#[derive(Deserialize)]
struct RegistryIndex {
    #[allow(dead_code)]
    version: u32,
    platforms: Vec<IndexEntry>,
}

#[derive(Deserialize)]
struct IndexEntry {
    name: String,
    binary: String,
}

/// Legacy index (v5, JSON).
#[derive(Deserialize)]
struct LegacyRegistryIndex {
    #[allow(dead_code)]
    version: u32,
    platforms: Vec<IndexEntry>,
}

const REGISTRY_BASE_URL: &str = "https://raw.githubusercontent.com/UniverLab/canopy-registry/main/";

const REGISTRY_LEGACY_URL: &str =
    "https://raw.githubusercontent.com/UniverLab/canopy-registry/main/platforms.json";

/// How often to refresh the registry in the background (24 hours).
const REGISTRY_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// Fetch the platform registry (public for use by config commands).
#[allow(dead_code)]
pub fn fetch_registry_raw() -> Result<RegistryRaw> {
    fetch_registry()
}

pub(crate) fn fetch_registry() -> Result<RegistryRaw> {
    let client = reqwest::blocking::Client::new();

    // Try v6 (TOML) first
    if let Some(reg) = try_fetch_v6(&client) {
        return Ok(reg);
    }

    // Try v5 (JSON per-platform)
    if let Some(reg) = try_fetch_v5(&client) {
        return Ok(reg);
    }

    // Fallback: legacy monolithic platforms.json (v4)
    let response = client
        .get(REGISTRY_LEGACY_URL)
        .header("User-Agent", "canopy")
        .send()
        .context("Failed to connect to platform registry")?;

    if !response.status().is_success() {
        anyhow::bail!("Registry returned HTTP {}", response.status());
    }

    #[derive(Deserialize)]
    struct LegacyRaw {
        platforms: Vec<Platform>,
    }

    let legacy: LegacyRaw = response.json().context("Invalid registry JSON")?;
    Ok(RegistryRaw {
        platforms: legacy.platforms,
        canonical_servers: CanonicalServers::default(),
    })
}

/// Try fetching registry v6 (TOML index + servers + platforms).
fn try_fetch_v6(client: &reqwest::blocking::Client) -> Option<RegistryRaw> {
    let index_resp = client
        .get(format!("{REGISTRY_BASE_URL}index.toml"))
        .header("User-Agent", "canopy")
        .send()
        .ok()?;

    if !index_resp.status().is_success() {
        return None;
    }

    let index_text = index_resp.text().ok()?;
    let index: RegistryIndex = toml::from_str(&index_text).ok()?;

    // Fetch canonical servers
    let servers_resp = client
        .get(format!("{REGISTRY_BASE_URL}servers.toml"))
        .header("User-Agent", "canopy")
        .send()
        .ok()?;

    let canonical_servers: CanonicalServers = if servers_resp.status().is_success() {
        let text = servers_resp.text().ok()?;
        toml::from_str(&text).unwrap_or_default()
    } else {
        CanonicalServers::default()
    };

    // Fetch platform files (only for installed binaries)
    let needed: Vec<&IndexEntry> = index
        .platforms
        .iter()
        .filter(|e| is_binary_available(&e.binary))
        .collect();

    let mut platforms = Vec::new();
    for entry in &needed {
        let url = format!("{REGISTRY_BASE_URL}platforms/{}.toml", entry.name);
        match client
            .get(&url)
            .header("User-Agent", "canopy")
            .send()
            .and_then(|r| r.text())
        {
            Ok(text) => match toml::from_str::<Platform>(&text) {
                Ok(p) => platforms.push(p),
                Err(e) => {
                    tracing::warn!("Failed to parse platform '{}': {e}", entry.name);
                }
            },
            Err(e) => {
                tracing::warn!("Failed to fetch platform '{}': {e}", entry.name);
            }
        }
    }

    if platforms.is_empty() {
        return None;
    }

    Some(RegistryRaw {
        platforms,
        canonical_servers,
    })
}

/// Try fetching registry v5 (JSON per-platform).
fn try_fetch_v5(client: &reqwest::blocking::Client) -> Option<RegistryRaw> {
    let resp = client
        .get(format!("{REGISTRY_BASE_URL}index.json"))
        .header("User-Agent", "canopy")
        .send()
        .ok()?;

    if !resp.status().is_success() {
        return None;
    }

    let index: LegacyRegistryIndex = resp.json().ok()?;

    let needed: Vec<&IndexEntry> = index
        .platforms
        .iter()
        .filter(|e| is_binary_available(&e.binary))
        .collect();

    let mut platforms = Vec::new();
    for entry in &needed {
        let url = format!("{REGISTRY_BASE_URL}platforms/{}.json", entry.name);
        match client
            .get(&url)
            .header("User-Agent", "canopy")
            .send()
            .and_then(|r| r.json::<Platform>())
        {
            Ok(p) => platforms.push(p),
            Err(e) => {
                tracing::warn!("Failed to fetch platform '{}': {e}", entry.name);
            }
        }
    }

    if platforms.is_empty() {
        return None;
    }

    Some(RegistryRaw {
        platforms,
        canonical_servers: CanonicalServers::default(),
    })
}

use crate::shared::banner;

pub(crate) fn print_banner() {
    banner::print_banner_with_gradient("Agent Hub — Setup Wizard");
}

pub fn maybe_refresh_registry() -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    let config_path = home.join(".canopy/config.toml");

    // Check if file exists and when it was last modified
    let last_modified = match std::fs::metadata(&config_path) {
        Ok(meta) => meta.modified().ok(),
        Err(_) => return false,
    };

    let needs_refresh = match last_modified {
        Some(time) => time.elapsed().unwrap_or_default() > REGISTRY_REFRESH_INTERVAL,
        None => true,
    };

    if !needs_refresh {
        return false;
    }

    // Fetch and update in background thread
    std::thread::spawn(move || {
        let _ = refresh_registry_inner(&home);
    });

    true
}

fn refresh_registry_inner(home: &Path) -> Result<()> {
    let registry = fetch_registry()?;

    let detected: Vec<&Platform> = registry
        .platforms
        .iter()
        .filter(|p| resolve_config_path(home, &p.config_path).exists())
        .collect();

    let platforms_with_cli: Vec<PlatformWithCli> = detected
        .iter()
        .map(|p| p.to_platform_with_cli())
        .filter(|p| p.cli.is_some())
        .collect();

    let cli_registry =
        crate::domain::cli_config::CliRegistry::detect_available(&platforms_with_cli);

    if !cli_registry.available_clis.is_empty() {
        let canopy_dir = home.join(".canopy");
        let mut config = crate::domain::canopy_config::CanopyConfig::load(&canopy_dir);
        config.clis = cli_registry.available_clis;
        let _ = config.save(&canopy_dir);
    }

    Ok(())
}
