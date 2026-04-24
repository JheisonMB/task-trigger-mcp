use anyhow::{Context, Result};
use inquire::MultiSelect;
use serde::Deserialize;
use std::io::{self, Write};
use std::path::Path;

const REGISTRY_BASE_URL: &str = "https://raw.githubusercontent.com/UniverLab/canopy-registry/main/";

const REGISTRY_LEGACY_URL: &str =
    "https://raw.githubusercontent.com/UniverLab/canopy-registry/main/platforms.json";

/// How often to refresh the registry in the background (24 hours).
const REGISTRY_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

#[derive(Clone)]
pub struct RegistryRaw {
    pub platforms: Vec<Platform>,
    pub canonical_servers: CanonicalServers,
}

/// Canonical MCP server definitions from `servers.toml`.
#[derive(Deserialize, Clone, Default)]
pub struct CanonicalServers {
    #[serde(default)]
    pub servers: std::collections::HashMap<String, serde_json::Value>,
}

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

#[derive(Deserialize, Clone)]
pub struct Platform {
    pub name: String,
    pub config_path: String,
    #[serde(default)]
    pub config_format: Option<String>,
    /// When true, TOML uses `[[section]]` array-of-tables with `name = "key"`
    /// instead of the default `[section.key]` table format.
    #[serde(default)]
    pub toml_array_format: bool,
    /// How `command` + `args` are represented:
    /// - `"separate"` (default): `"command": "x", "args": [...]`
    /// - `"merged"`: `"command": ["x", ...args]` (single array)
    #[serde(default = "default_command_format")]
    pub command_format: String,
    #[serde(alias = "servers_key")]
    pub mcp_servers_key: Vec<String>,
    #[serde(default)]
    pub deprecated_keys: Vec<String>,
    /// Keys that this platform's MCP schema does not support.
    /// Used to sanitize configs when syncing across platforms.
    #[serde(default)]
    pub unsupported_keys: Vec<String>,
    /// Translation map from Canopy's standard field names to this platform's names.
    /// e.g. `{"env": "environment"}`.
    #[serde(default)]
    pub fields_mapping: std::collections::HashMap<String, String>,
    /// Fields that are required by this platform, with their allowed values.
    /// e.g. `{"type": ["http", "remote"]}`.
    #[serde(default)]
    pub required_fields: std::collections::HashMap<String, Vec<String>>,
    /// Per-server extra fields merged into the adapted config.
    /// e.g. `server_extras.canopy = { tools = ["*"] }`.
    #[serde(default)]
    pub server_extras: std::collections::HashMap<String, serde_json::Value>,
    /// Path to the platform's skills directory (relative to home).
    /// e.g. `".kiro/skills"`.
    #[serde(default)]
    pub skills_dir: Option<String>,
    #[serde(default)]
    pub cli: Option<serde_json::Value>,
}

fn default_command_format() -> String {
    "separate".to_string()
}

/// Platform with parsed CLI config (for saving to .canopy/)
pub struct PlatformWithCli {
    #[allow(dead_code)]
    pub name: String,
    #[allow(dead_code)]
    pub config_path: String,
    pub cli: Option<crate::domain::cli_config::CliConfig>,
}

impl Platform {
    fn to_platform_with_cli(&self) -> PlatformWithCli {
        let cli = self.cli.as_ref().and_then(|v| {
            serde_json::from_value::<crate::domain::cli_config::CliConfig>(v.clone())
                .map(|mut c| {
                    c.name = self.name.clone();
                    c
                })
                .ok()
        });

        PlatformWithCli {
            name: self.name.clone(),
            config_path: self.config_path.clone(),
            cli,
        }
    }
}

/// Check if a platform is available by detecting its CLI binary in PATH.
pub fn is_platform_available(p: &Platform) -> bool {
    p.cli
        .as_ref()
        .and_then(|v| v.get("binary").and_then(|b| b.as_str()))
        .map(|binary| which::which(binary).is_ok())
        .unwrap_or(false)
}

/// Resolve the actual config file path for a platform.
///
/// Handles the `.json` ↔ `.jsonc` ambiguity (e.g. opencode supports both).
/// Returns the existing file if found, falling back to an alternate extension,
/// and finally the registry default.
fn resolve_config_path(home: &Path, config_path: &str) -> std::path::PathBuf {
    let primary = home.join(config_path);
    if primary.exists() {
        return primary;
    }

    // Try alternate JSON extension
    let ext = primary.extension().and_then(|e| e.to_str()).unwrap_or("");
    let alternate = match ext {
        "jsonc" => primary.with_extension("json"),
        "json" => primary.with_extension("jsonc"),
        _ => return primary,
    };
    if alternate.exists() {
        return alternate;
    }

    primary
}

/// Load the saved filesystem root path for the MCP filesystem server.
/// Returns home dir as default if not yet configured.
fn load_mcp_fs_root(home: &Path) -> String {
    let canopy_dir = home.join(".canopy");
    let config = crate::domain::canopy_config::CanopyConfig::load(&canopy_dir);
    config.mcp_filesystem_root
}

/// Persist the chosen filesystem root path for reuse across setups and updates.
fn save_mcp_fs_root(home: &Path, root: &str) {
    let canopy_dir = home.join(".canopy");
    let mut config = crate::domain::canopy_config::CanopyConfig::load(&canopy_dir);
    config.mcp_filesystem_root = root.to_string();
    let _ = config.save(&canopy_dir);
}
fn is_binary_available(binary: &str) -> bool {
    which::which(binary).is_ok()
}

/// Ensure MCP runtime dependencies (npx, uvx) are available.
/// Attempts to install missing ones automatically.
/// Returns a summary message for the wizard.
fn ensure_mcp_dependencies() -> String {
    let mut installed = Vec::new();
    let mut already = Vec::new();
    let mut failed = Vec::new();

    // npx comes with Node.js/npm
    if is_binary_available("npx") {
        already.push("npx");
    } else {
        println!("  \x1b[33m⚠\x1b[0m  npx not found. Attempting to install Node.js...");
        if try_install_node() {
            installed.push("npx (via Node.js)");
        } else {
            failed.push("npx — install Node.js: https://nodejs.org");
        }
    }

    // uvx comes with uv (Python package manager)
    if is_binary_available("uvx") {
        already.push("uvx");
    } else {
        println!("  \x1b[33m⚠\x1b[0m  uvx not found. Attempting to install uv...");
        if try_install_uv() {
            installed.push("uvx (via uv)");
        } else {
            failed.push("uvx — install uv: https://docs.astral.sh/uv");
        }
    }

    let mut parts = Vec::new();
    if !already.is_empty() {
        parts.push(format!("{} present", already.join(", ")));
    }
    if !installed.is_empty() {
        parts.push(format!("{} installed", installed.join(", ")));
    }
    if !failed.is_empty() {
        return format!(
            "\x1b[31m✗\x1b[0m Dependencies: missing — {}",
            failed.join("; ")
        );
    }

    format!("\x1b[32m✓\x1b[0m Dependencies: {}", parts.join(", "))
}

/// Ensure MCP dependencies silently (no prompts). Returns true if all ok.
fn ensure_mcp_dependencies_silent() -> bool {
    let has_npx = is_binary_available("npx") || try_install_node();
    let has_uvx = is_binary_available("uvx") || try_install_uv();
    has_npx && has_uvx
}

/// Try to install Node.js (which provides npx).
fn try_install_node() -> bool {
    #[cfg(unix)]
    {
        // Try nvm if available
        if let Ok(home) = std::env::var("HOME") {
            let nvm_dir = format!("{home}/.nvm");
            if std::path::Path::new(&nvm_dir).exists() {
                let status = std::process::Command::new("bash")
                    .args([
                        "-c",
                        &format!("source {nvm_dir}/nvm.sh && nvm install --lts 2>/dev/null"),
                    ])
                    .status();
                if status.map(|s| s.success()).unwrap_or(false) {
                    return true;
                }
            }
        }
        // Try apt (Debian/Ubuntu)
        if is_binary_available("apt-get") {
            let status = std::process::Command::new("sudo")
                .args(["apt-get", "install", "-y", "nodejs", "npm"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            if status.map(|s| s.success()).unwrap_or(false) {
                return is_binary_available("npx");
            }
        }
        // Try brew
        if is_binary_available("brew") {
            let status = std::process::Command::new("brew")
                .args(["install", "node"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            if status.map(|s| s.success()).unwrap_or(false) {
                return is_binary_available("npx");
            }
        }
    }
    false
}

/// Try to install uv (which provides uvx).
fn try_install_uv() -> bool {
    #[cfg(unix)]
    {
        // Official installer: curl -LsSf https://astral.sh/uv/install.sh | sh
        let status = std::process::Command::new("bash")
            .args([
                "-c",
                "curl -LsSf https://astral.sh/uv/install.sh 2>/dev/null | sh 2>/dev/null",
            ])
            .stdout(std::process::Stdio::null())
            .status();
        if status.map(|s| s.success()).unwrap_or(false) {
            // uv installs to ~/.local/bin — may not be in PATH yet for this process
            if let Ok(home) = std::env::var("HOME") {
                let uv_bin = format!("{home}/.local/bin");
                if let Ok(path) = std::env::var("PATH") {
                    if !path.contains(&uv_bin) {
                        std::env::set_var("PATH", format!("{uv_bin}:{path}"));
                    }
                }
            }
            return is_binary_available("uvx");
        }
    }
    false
}

#[allow(dead_code)]
pub fn is_configured() -> bool {
    dirs::home_dir()
        .map(|h| {
            let config = crate::domain::canopy_config::CanopyConfig::load(&h.join(".canopy"));
            config.is_configured()
        })
        .unwrap_or(false)
}

/// Fetch the platform registry (public for use by config commands).
#[allow(dead_code)]
pub fn fetch_registry_raw() -> Result<RegistryRaw> {
    fetch_registry()
}

pub fn run_setup() -> Result<()> {
    let mut wiz = WizardState::new();
    let home = dirs::home_dir().context("No home directory")?;

    // ── Step 1: Fetch registry ──────────────────────────────────
    clear_wizard_screen()?;
    print_banner();
    print!("  Fetching platform registry... ");
    io::stdout().flush()?;
    let mut registry = fetch_registry()?;

    // Legacy v5 compat: no longer needed with v6
    let _ = &mut registry;
    println!("\x1b[32m✓\x1b[0m");

    let detected: Vec<&Platform> = registry
        .platforms
        .iter()
        .filter(|p| is_platform_available(p))
        .collect();

    let detected_names: Vec<&str> = detected.iter().map(|p| p.name.as_str()).collect();
    wiz.add(format!(
        "\x1b[32m✓\x1b[0m Fetched registry — {} detected: {}",
        detected.len(),
        if detected_names.is_empty() {
            "(none)".to_string()
        } else {
            detected_names.join(", ")
        }
    ));

    // ── Step 2: Select platforms ─────────────────────────────────
    wiz.render()?;
    if detected.is_empty() {
        println!(
            "  No supported platforms detected. Supported: {}",
            registry
                .platforms
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!();
    }

    let selected = select_platforms(&detected)?;
    let selected_names: Vec<&str> = selected.iter().map(|p| p.name.as_str()).collect();
    wiz.add(format!(
        "\x1b[32m✓\x1b[0m Platforms: {}",
        if selected_names.is_empty() {
            "(none)".to_string()
        } else {
            selected_names.join(", ")
        }
    ));

    // ── Step 2.5: Verify MCP runtime dependencies ─────────────
    wiz.render()?;
    let dep_msg = ensure_mcp_dependencies();
    wiz.add(dep_msg);

    // ── Step 3: Install MCP servers + show matrix ───────────────
    if !selected.is_empty() {
        let sync_summary = run_sync_step(&mut wiz, &home, &selected, &registry.canonical_servers)?;
        if let Some(s) = sync_summary {
            wiz.add(s);
        }
    }

    // ── Step 4: Save CLI configuration ──────────────────────────
    let platforms_with_cli: Vec<PlatformWithCli> = selected
        .iter()
        .map(|p| p.to_platform_with_cli())
        .filter(|p| p.cli.is_some())
        .collect();

    let cli_registry =
        crate::domain::cli_config::CliRegistry::detect_available(&platforms_with_cli);
    let canopy_dir = home.join(".canopy");
    std::fs::create_dir_all(&canopy_dir)?;

    // ── Step 5: MCP Manager (sync/add/remove) ───────────────────
    if !selected.is_empty() {
        let sync_summary = run_sync_step(&mut wiz, &home, &selected, &registry.canonical_servers)?;
        if let Some(s) = sync_summary {
            wiz.add(s);
        }
    }

    // ── Step 5.5: Essential Skills ───────────────────────────────
    wiz.render()?;
    let skills_step = run_essential_skills_step(&home, &selected);
    wiz.add(skills_step);

    // ── Step 6: Daemon + service ────────────────────────────────
    wiz.render()?;

    // Always restart daemon to pick up new MCP configs
    let _ = stop_daemon();
    let daemon_msg = match start_daemon_if_needed() {
        Ok(true) => "\x1b[32m✓\x1b[0m Daemon: (re)started",
        Ok(false) => "\x1b[32m✓\x1b[0m Daemon: already running",
        Err(_) => "\x1b[31m✗\x1b[0m Daemon: failed to start",
    };
    wiz.add(daemon_msg.to_string());

    let service_msg = match install_service_if_needed() {
        Ok(true) => "\x1b[32m✓\x1b[0m Service: installed",
        Ok(false) => "\x1b[32m✓\x1b[0m Service: already installed",
        Err(_) => "\x1b[31m✗\x1b[0m Service: failed to install",
    };
    wiz.add(service_msg.to_string());

    // ── Save unified config ──────────────────────────────────────
    let mut config = crate::domain::canopy_config::CanopyConfig::load(&canopy_dir);
    config.mark_configured();
    config.clis = cli_registry.available_clis;
    let config_step = match config.save(&canopy_dir) {
        Ok(_) => format!(
            "\x1b[32m✓\x1b[0m Config: {} CLI(s) saved to config.toml",
            config.clis.len()
        ),
        Err(e) => format!("\x1b[33m⚠\x1b[0m Config: {e}"),
    };
    wiz.add(config_step);

    // ── Final summary ───────────────────────────────────────────
    wiz.render()?;
    println!("  \x1b[1;32m✅ Setup complete! canopy is ready.\x1b[0m");
    println!("  Run \x1b[1mcanopy\x1b[0m or \x1b[1mcanopy tui\x1b[0m to launch the interface.");
    println!();

    Ok(())
}

/// Fetch the per-platform registry (v6 TOML, v5 JSON fallback).
fn fetch_registry() -> Result<RegistryRaw> {
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

pub(crate) const BANNER: &str = r#"                                                     
  ██████   ██████   ████████    ██████  ████████  █████ ████
 ███░░███ ░░░░░███ ░░███░░███  ███░░███░░███░░███░░███ ░███ 
░███ ░░░   ███████  ░███ ░███ ░███ ░███ ░███ ░███ ░███ ░███ 
░███  ███ ███░░███  ░███ ░███ ░███ ░███ ░███ ░███ ░███ ░███ 
░░██████ ░░████████ ████ █████░░██████  ░███████  ░░███████ 
 ░░░░░░   ░░░░░░░░ ░░░░ ░░░░░  ░░░░░░   ░███░░░    ░░░░░███ 
                                        ░███       ███ ░███ 
                                        █████     ░░██████  
                                       ░░░░░       ░░░░░░   
"#;

fn print_banner() {
    println!("\x1b[32m{BANNER}\x1b[0m");
    println!("  \x1b[1mAgent Hub — Setup Wizard\x1b[0m");
    println!("  ─────────────────────────────────────────────");
    println!();
}

/// Tracks completed wizard steps so we can re-render a clean summary
/// after clearing the screen between interactive phases.
struct WizardState {
    steps: Vec<String>,
}

impl WizardState {
    fn new() -> Self {
        Self { steps: vec![] }
    }

    fn add(&mut self, summary: String) {
        self.steps.push(summary);
    }

    /// Clear screen → banner → all completed step summaries.
    fn render(&self) -> Result<()> {
        clear_wizard_screen()?;
        print_banner();
        for step in &self.steps {
            println!("  {step}");
        }
        if !self.steps.is_empty() {
            println!();
        }
        Ok(())
    }
}

fn select_platforms<'a>(detected: &[&'a Platform]) -> Result<Vec<&'a Platform>> {
    if detected.is_empty() {
        println!("  Press Enter to continue...");
        let mut buf = String::new();
        io::stdin().read_line(&mut buf)?;
        return Ok(vec![]);
    }

    let platform_names: Vec<&str> = detected.iter().map(|p| p.name.as_str()).collect();
    let all_indices: Vec<usize> = (0..detected.len()).collect();

    let selected = MultiSelect::new("Select platforms to configure:", platform_names)
        .with_default(&all_indices)
        .with_help_message("space: toggle | enter: confirm | ↑↓: navigate")
        .prompt()
        .map_err(|e| anyhow::anyhow!("Selection cancelled: {}", e))?;

    Ok(selected
        .iter()
        .filter_map(|name| detected.iter().find(|p| p.name == *name).copied())
        .collect())
}

fn upsert_json_key(path: &Path, keys: &[&str], value: &serde_json::Value) -> Result<bool> {
    let mut root: serde_json::Value = if path.exists() {
        let content = std::fs::read_to_string(path)?;
        let clean = strip_jsonc_comments(&content);
        serde_json::from_str(&clean).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let mut current = &mut root;
    for &key in &keys[..keys.len() - 1] {
        if !current.get(key).is_some_and(|v| v.is_object()) {
            current[key] = serde_json::json!({});
        }
        current = &mut current[key];
    }

    let leaf = keys[keys.len() - 1];
    current[leaf] = value.clone();

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(&root)? + "\n")?;
    Ok(true)
}

/// Remove a `[section.entry_key]` TOML block from string content.
fn remove_toml_key_section_str(content: &str, table_header: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut in_target = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if trimmed == table_header {
                in_target = true;
                continue;
            }
            in_target = false;
        }
        if !in_target {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Upsert a TOML config key for platforms like Codex that use config.toml.
///
/// Writes `[{section}.{entry_key}]` with the fields from `value` (a JSON object).
/// Example output: `[mcp_servers.canopy]\nurl = "http://localhost:7755/mcp"\n`
fn upsert_toml_key(
    path: &Path,
    section: &str,
    entry_key: &str,
    value: &serde_json::Value,
) -> Result<bool> {
    let table_header = format!("[{section}.{entry_key}]");

    let content = if path.exists() {
        std::fs::read_to_string(path)?
    } else {
        String::new()
    };

    // Remove existing [section.entry_key] (if any) so we always write a fresh one
    let mut base = remove_toml_key_section_str(&content, &table_header);

    // Remove conflicting [[section]] array-of-tables entries (e.g. from old format)
    base = remove_conflicting_toml_arrays(&base, section);

    // Build the TOML fragment from the JSON value
    let mut fragment = format!("\n{table_header}\n");
    if let Some(obj) = value.as_object() {
        for (k, v) in obj {
            match v {
                serde_json::Value::String(s) => {
                    fragment.push_str(&format!("{k} = \"{s}\"\n"));
                }
                serde_json::Value::Bool(b) => {
                    fragment.push_str(&format!("{k} = {b}\n"));
                }
                serde_json::Value::Number(n) => {
                    fragment.push_str(&format!("{k} = {n}\n"));
                }
                _ => {
                    let toml_val: toml::Value = serde_json::from_value(v.clone())?;
                    fragment.push_str(&format!("{k} = {toml_val}\n"));
                }
            }
        }
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut out = base;
    out.push_str(&fragment);
    std::fs::write(path, out)?;
    Ok(true)
}

fn remove_json_key(path: &Path, parent_key: &str, key: &str) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let content = std::fs::read_to_string(path)?;
    let clean = strip_jsonc_comments(&content);
    let mut root: serde_json::Value = serde_json::from_str(&clean).unwrap_or(serde_json::json!({}));

    let Some(parent) = root.get_mut(parent_key).and_then(|v| v.as_object_mut()) else {
        return Ok(false);
    };

    if parent.remove(key).is_some() {
        std::fs::write(path, serde_json::to_string_pretty(&root)? + "\n")?;
        return Ok(true);
    }
    Ok(false)
}

/// Upsert a TOML entry using `[[section]]` array-of-tables format (e.g. mistral).
///
/// Each entry is identified by `name = "entry_key"` within the array.
/// Example: `[[mcp_servers]]\nname = "fetch"\ncommand = "uvx"\n`
fn upsert_toml_array(
    path: &Path,
    section: &str,
    entry_key: &str,
    value: &serde_json::Value,
) -> Result<bool> {
    let array_header = format!("[[{section}]]");
    let name_line = format!("name = \"{entry_key}\"");

    let content = if path.exists() {
        std::fs::read_to_string(path)?
    } else {
        String::new()
    };

    // Remove existing entry (if any) so we always write a fresh one
    let mut base = if content.contains(&name_line) {
        remove_toml_array_entry_str(&content, &array_header, &name_line)
    } else {
        content
    };

    // Remove any `{section} = []` scalar that conflicts with [[section]] array-of-tables
    let scalar_prefix = format!("{section} =");
    let scalar_prefix2 = format!("{section}=");
    base = base
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.starts_with(&scalar_prefix) && !t.starts_with(&scalar_prefix2)
        })
        .collect::<Vec<_>>()
        .join("\n");
    if !base.is_empty() && !base.ends_with('\n') {
        base.push('\n');
    }

    // Remove conflicting [section.xxx] key-table entries (left over from old format)
    base = remove_conflicting_toml_tables(&base, section);

    // Remove stray [[section]] headers not followed by a name = "..." line
    base = remove_stray_toml_array_headers(&base, &array_header);

    // Build the TOML fragment
    let mut fragment = format!("\n{array_header}\nname = \"{entry_key}\"\n");
    if let Some(obj) = value.as_object() {
        for (k, v) in obj {
            match v {
                serde_json::Value::String(s) => {
                    fragment.push_str(&format!("{k} = \"{s}\"\n"));
                }
                serde_json::Value::Bool(b) => {
                    fragment.push_str(&format!("{k} = {b}\n"));
                }
                serde_json::Value::Number(n) => {
                    fragment.push_str(&format!("{k} = {n}\n"));
                }
                _ => {
                    let toml_val: toml::Value = serde_json::from_value(v.clone())?;
                    fragment.push_str(&format!("{k} = {toml_val}\n"));
                }
            }
        }
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut out = base;
    out.push_str(&fragment);
    std::fs::write(path, out)?;
    Ok(true)
}

/// Remove a `[[section]]` array entry from string content.
/// The entry is identified by the line `name = "entry_key"` immediately following the header.
fn remove_toml_array_entry_str(content: &str, array_header: &str, name_line: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut in_target = false;
    let mut pending_header: Option<String> = None;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed == array_header {
            // A new [[section]] header ends any previous target block
            if in_target {
                in_target = false;
            }
            pending_header = Some(line.to_string());
            continue;
        }

        if let Some(ref header) = pending_header {
            if trimmed == name_line {
                in_target = true;
                pending_header = None;
                continue;
            }
            out.push_str(header);
            out.push('\n');
            pending_header = None;
        }

        if in_target {
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                in_target = false;
                out.push_str(line);
                out.push('\n');
            }
            continue;
        }

        out.push_str(line);
        out.push('\n');
    }

    // Flush any buffered header not yet written
    if let Some(header) = pending_header {
        out.push_str(&header);
        out.push('\n');
    }

    out
}

/// Remove `[[section]]` header lines that are not followed by a `name = "..."` line.
/// Fixes stray empty headers accumulated from previous corrupt writes.
fn remove_stray_toml_array_headers(content: &str, array_header: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let mut out = String::with_capacity(content.len());
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() == array_header {
            let next_non_empty = (i + 1..lines.len()).find(|&j| !lines[j].trim().is_empty());
            let is_valid = next_non_empty
                .map(|j| lines[j].trim().starts_with("name ="))
                .unwrap_or(false);
            if is_valid {
                out.push_str(lines[i]);
                out.push('\n');
            }
            // else: stray header without content — drop it
        } else {
            out.push_str(lines[i]);
            out.push('\n');
        }
        i += 1;
    }
    out
}

/// Remove all `[section.xxx]` key-table entries that conflict with `[[section]]` format.
/// This handles migration from the older `[mcp_servers.canopy]` format to `[[mcp_servers]]`.
fn remove_conflicting_toml_tables(content: &str, section: &str) -> String {
    let prefix = format!("[{section}.");
    let mut out = String::with_capacity(content.len());
    let mut in_conflict = false;

    for line in content.lines() {
        let trimmed = line.trim();

        // Detect a conflicting [section.xxx] header (single-bracket, not [[...]])
        if trimmed.starts_with(&prefix)
            && trimmed.ends_with(']')
            && !trimmed.starts_with(&format!("[[{section}."))
        {
            in_conflict = true;
            continue;
        }

        // Any new section header ends the conflicting block
        if in_conflict && trimmed.starts_with('[') {
            in_conflict = false;
        }

        if !in_conflict {
            out.push_str(line);
            out.push('\n');
        }
    }

    out
}

/// Remove all `[[section]]` array-of-tables blocks for a given section.
/// Counterpart to `remove_conflicting_toml_tables`: cleans up array entries
/// when switching a platform to `[section.name]` key-table format.
fn remove_conflicting_toml_arrays(content: &str, section: &str) -> String {
    let header = format!("[[{section}]]");
    let mut out = String::with_capacity(content.len());
    let mut in_array = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed == header {
            in_array = true;
            continue;
        }

        // Any new section header ends the array block
        if in_array && trimmed.starts_with('[') {
            in_array = false;
        }

        if !in_array {
            out.push_str(line);
            out.push('\n');
        }
    }

    out
}

pub(crate) fn strip_jsonc_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;

    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(&next) = chars.peek() {
                    out.push(next);
                    chars.next();
                }
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
            out.push(c);
        } else if c == '/' {
            match chars.peek() {
                Some('/') => {
                    for ch in chars.by_ref() {
                        if ch == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                }
                Some('*') => {
                    chars.next();
                    while let Some(ch) = chars.next() {
                        if ch == '*' && chars.peek() == Some(&'/') {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => out.push(c),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Stop the daemon if it is running.
fn stop_daemon() -> Result<()> {
    let data_dir = crate::ensure_data_dir()?;
    let pid_path = data_dir.join("daemon.pid");
    if let Ok(pid_str) = std::fs::read_to_string(&pid_path) {
        if let Ok(pid) = pid_str.trim().parse::<i32>() {
            #[cfg(unix)]
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
            // Wait for process to stop (up to 3s)
            for _ in 0..12 {
                std::thread::sleep(std::time::Duration::from_millis(250));
                if !is_process_running(pid as u32) {
                    break;
                }
            }
            let _ = std::fs::remove_file(&pid_path);
        }
    }
    Ok(())
}

fn start_daemon_if_needed() -> Result<bool> {
    let data_dir = crate::ensure_data_dir()?;
    let pid_path = data_dir.join("daemon.pid");

    if let Ok(pid_str) = std::fs::read_to_string(&pid_path) {
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            if is_process_running(pid) {
                return Ok(false);
            }
        }
    }

    let exe = std::env::current_exe()?;
    let log_path = data_dir.join("daemon.log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let log_err = log_file.try_clone()?;

    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("serve")
        .stdout(log_file)
        .stderr(log_err)
        .stdin(std::process::Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }

    let child = cmd.spawn()?;
    std::thread::sleep(std::time::Duration::from_millis(500));

    if is_process_running(child.id()) {
        Ok(true)
    } else {
        anyhow::bail!("Daemon failed to start")
    }
}

fn install_service_if_needed() -> Result<bool> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let home = dirs::home_dir().context("No home directory")?;

    #[cfg(target_os = "macos")]
    {
        if home.join("Library/LaunchAgents/com.canopy.plist").exists() {
            return Ok(false);
        }
    }

    #[cfg(target_os = "linux")]
    {
        if home.join(".config/systemd/user/canopy.service").exists() {
            return Ok(false);
        }
    }

    let exe = std::env::current_exe()?;
    crate::daemon::service_install::install_service(&exe, 7755)?;
    Ok(true)
}

fn is_process_running(pid: u32) -> bool {
    crate::daemon::process::is_process_running(pid)
}

/// Check if auto-setup should run (no CLI config found).
pub fn needs_setup() -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    let config = crate::domain::canopy_config::CanopyConfig::load(&home.join(".canopy"));
    !config.is_configured()
}

/// Run setup silently (no prompts, auto-detect all platforms).
#[allow(dead_code)]
pub fn run_setup_silent() -> Result<()> {
    let home = dirs::home_dir().context("No home directory")?;

    ensure_mcp_dependencies_silent();

    let registry = fetch_registry()?;

    let detected: Vec<&Platform> = registry
        .platforms
        .iter()
        .filter(|p| is_platform_available(p))
        .collect();

    run_install_our_servers(&home, &detected, &registry.canonical_servers)?;

    // Save CLI config
    let platforms_with_cli: Vec<PlatformWithCli> = detected
        .iter()
        .map(|p| p.to_platform_with_cli())
        .filter(|p| p.cli.is_some())
        .collect();

    let cli_registry =
        crate::domain::cli_config::CliRegistry::detect_available(&platforms_with_cli);
    let canopy_dir = home.join(".canopy");
    std::fs::create_dir_all(&canopy_dir)?;

    // Save unified config
    let mut config = crate::domain::canopy_config::CanopyConfig::load(&canopy_dir);
    config.mark_configured();
    config.clis = cli_registry.available_clis;
    config.save(&canopy_dir)?;

    Ok(())
}

/// Refresh the registry config in the background if it's older than 24h.
/// Returns true if a refresh was performed.
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

// ── Recommended MCP servers ───────────────────────────────────────────────

/// Replace `{filesystem_dir}` and `{home}` placeholders in a JSON value tree.
fn substitute_placeholders(value: &mut serde_json::Value, home: &str, fs_dir: &str) {
    match value {
        serde_json::Value::String(s) if s.contains("{filesystem_dir}") || s.contains("{home}") => {
            *s = s
                .replace("{filesystem_dir}", fs_dir)
                .replace("{home}", home);
        }
        serde_json::Value::String(_) => {}
        serde_json::Value::Array(arr) => {
            for item in arr {
                substitute_placeholders(item, home, fs_dir);
            }
        }
        serde_json::Value::Object(map) => {
            for val in map.values_mut() {
                substitute_placeholders(val, home, fs_dir);
            }
        }
        _ => {}
    }
}

/// Translate a canonical server config to a target platform's format.
///
/// Applies in order:
/// 1. `command_format` — merge `command` + `args` into single array if "merged"
/// 2. `fields_mapping` — rename fields (e.g. `env` → `environment`)
/// 3. `required_fields` — inject missing required fields with default values
/// 4. `server_extras` — merge per-server platform-specific fields
/// 5. `unsupported_keys` — strip fields the platform doesn't support
pub fn adapt_config(
    config: &serde_json::Value,
    platform: &Platform,
    server_name: &str,
) -> serde_json::Value {
    let Some(obj) = config.as_object() else {
        return config.clone();
    };

    let mut adapted = serde_json::Map::new();

    // Step 1: handle command_format
    if platform.command_format == "merged" {
        // Merge command + args into a single "command" array
        let has_command = obj.get("command").and_then(|v| v.as_str()).is_some();
        let has_args = obj.contains_key("args");

        if has_command && has_args {
            let cmd = obj["command"].as_str().unwrap();
            let mut merged = vec![serde_json::Value::String(cmd.to_string())];
            if let Some(args) = obj["args"].as_array() {
                merged.extend(args.iter().cloned());
            }
            adapted.insert("command".to_string(), serde_json::Value::Array(merged));

            // Copy all other fields except command and args
            for (k, v) in obj {
                if k != "command" && k != "args" {
                    adapted.insert(k.clone(), v.clone());
                }
            }
        } else {
            // No merge needed — copy all fields
            for (k, v) in obj {
                adapted.insert(k.clone(), v.clone());
            }
        }
    } else {
        // "separate" (default) — copy fields as-is
        for (k, v) in obj {
            adapted.insert(k.clone(), v.clone());
        }
    }

    // Step 2: apply field rename mapping
    let mut renamed = serde_json::Map::new();
    for (k, v) in adapted {
        let target_key = platform.fields_mapping.get(&k).cloned().unwrap_or(k);
        renamed.insert(target_key, v);
    }
    adapted = renamed;

    // Step 3: inject required fields using index convention
    // required_fields values: [0] = url-based value, [1] = command-based value
    let type_idx = infer_server_type_index(&adapted);
    for (field, allowed) in &platform.required_fields {
        if let Some(idx) = type_idx {
            if let Some(value) = allowed.get(idx) {
                // Always set — overwrite stale values from cross-platform sync
                adapted.insert(field.clone(), serde_json::Value::String(value.clone()));
            }
            // Index out of bounds (e.g. Gemini only has [0]) → skip injection
        } else if !adapted.contains_key(field) {
            // No inference possible — use first value as default
            if let Some(value) = allowed.first() {
                adapted.insert(field.clone(), serde_json::Value::String(value.clone()));
            }
        }
    }

    // Step 4: merge server_extras for this server
    if let Some(extras) = platform.server_extras.get(server_name) {
        if let Some(extras_obj) = extras.as_object() {
            for (k, v) in extras_obj {
                adapted.insert(k.clone(), v.clone());
            }
        }
    }

    // Step 5: strip unsupported keys
    for key in &platform.unsupported_keys {
        adapted.remove(key);
    }

    serde_json::Value::Object(adapted)
}

/// Infer server type index from canonical fields.
/// Returns `0` for url-based (http/remote) or `1` for command-based (stdio/local).
fn infer_server_type_index(config: &serde_json::Map<String, serde_json::Value>) -> Option<usize> {
    if config.contains_key("url") {
        Some(0)
    } else if config.contains_key("command") {
        Some(1)
    } else {
        None
    }
}

/// Interactive directory browser using `inquire::Select`.
/// Lets the user navigate the filesystem and select a directory.
/// Interactive directory picker using arrow-key navigation.
///
/// Keys: ↑↓ navigate  →  enter directory  ←  go up  Enter  confirm  Esc  cancel
fn browse_directory(start_dir: &str) -> String {
    use ratatui::crossterm::event::{read, Event, KeyCode, KeyEventKind};
    use ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode};

    fn list_subdirs(path: &std::path::Path) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(path) else {
            return Vec::new();
        };
        let mut dirs: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_type().map(|t| t.is_dir()).unwrap_or(false)
                    || e.file_type().map(|t| t.is_symlink()).unwrap_or(false) && e.path().is_dir()
            })
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    None
                } else {
                    Some(name)
                }
            })
            .collect();
        dirs.sort();
        dirs
    }

    let mut current = std::path::PathBuf::from(start_dir);
    if !current.is_dir() {
        current = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
    }

    let mut cursor: usize = 0;
    let visible: usize = 10;
    // Fixed row count: header + hint + visible slots + status line
    let total_rows = 2 + visible + 1;

    let _ = enable_raw_mode();

    // Reserve lines for the drawing area
    for _ in 0..total_rows {
        print!("\r\n");
    }

    loop {
        let subdirs = list_subdirs(&current);

        // Clamp cursor
        if !subdirs.is_empty() && cursor >= subdirs.len() {
            cursor = subdirs.len().saturating_sub(1);
        }

        let scroll = if cursor >= visible {
            cursor - visible + 1
        } else {
            0
        };
        let has_above = scroll > 0;
        let has_below = !subdirs.is_empty() && scroll + visible < subdirs.len();

        // Move up to start of our drawing area
        print!("\x1b[{total_rows}A");

        // Path header
        print!("\r\x1b[2K  \x1b[36m»\x1b[0m  {}\r\n", current.display());
        // Hint bar
        print!(
            "\r\x1b[2K  \x1b[90m↑↓ navigate  → enter  ← back  Enter confirm  Esc cancel\x1b[0m\r\n"
        );

        // Directory list (always render exactly `visible` rows)
        if subdirs.is_empty() {
            print!("\r\x1b[2K  \x1b[90m(empty — Enter to confirm, ← to go up)\x1b[0m\r\n");
            for _ in 1..visible {
                print!("\r\x1b[2K\r\n");
            }
        } else {
            let mut drawn = 0usize;
            for (i, name) in subdirs.iter().enumerate().skip(scroll).take(visible) {
                if i == cursor {
                    print!("\r\x1b[2K  \x1b[1;32m▶\x1b[0m \x1b[7m {name} \x1b[0m\r\n");
                } else {
                    print!("\r\x1b[2K    {name}\r\n");
                }
                drawn += 1;
            }
            // Fill remaining visible slots with blank lines
            for _ in drawn..visible {
                print!("\r\x1b[2K\r\n");
            }
        }

        // Status line: scroll indicators + item count
        if subdirs.is_empty() {
            print!("\r\x1b[2K  \x1b[90m0 items\x1b[0m\r\n");
        } else {
            let up = if has_above { "↑ " } else { "  " };
            let dn = if has_below { " ↓" } else { "  " };
            print!(
                "\r\x1b[2K  \x1b[90m{up}{}/{}{dn}\x1b[0m\r\n",
                cursor + 1,
                subdirs.len()
            );
        }

        let _ = io::stdout().flush();

        match read() {
            Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => match k.code {
                KeyCode::Enter => {
                    let _ = disable_raw_mode();
                    print!("\r\n");
                    let _ = io::stdout().flush();
                    return current.to_string_lossy().to_string();
                }
                KeyCode::Esc => {
                    let _ = disable_raw_mode();
                    print!("\r\n");
                    let _ = io::stdout().flush();
                    return start_dir.to_string();
                }
                KeyCode::Up => {
                    cursor = cursor.saturating_sub(1);
                }
                KeyCode::Down if !subdirs.is_empty() && cursor + 1 < subdirs.len() => {
                    cursor += 1;
                }
                KeyCode::Down => {}
                KeyCode::Right | KeyCode::Char('l') => {
                    if let Some(name) = subdirs.get(cursor) {
                        current = current.join(name);
                        cursor = 0;
                    }
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    if let Some(parent) = current.parent() {
                        current = parent.to_path_buf();
                        cursor = 0;
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
}

/// Extract all MCP server configs from the selected platforms.
fn extract_all_mcp_configs(
    home: &Path,
    selected: &[&Platform],
) -> Vec<crate::config::PlatformMcpConfig> {
    let mut configs = Vec::new();
    for p in selected {
        let config_path = resolve_config_path(home, &p.config_path);
        if !config_path.exists() {
            configs.push(crate::config::PlatformMcpConfig {
                platform: p.name.clone(),
                config_path: config_path.to_string_lossy().to_string(),
                servers: Vec::new(),
            });
            continue;
        }
        match crate::config::McpConfigRegistry::extract_from_platform(
            &p.name,
            &config_path,
            &p.mcp_servers_key,
        ) {
            Ok(cfg) => configs.push(cfg),
            Err(_) => configs.push(crate::config::PlatformMcpConfig {
                platform: p.name.clone(),
                config_path: config_path.to_string_lossy().to_string(),
                servers: Vec::new(),
            }),
        }
    }
    configs
}

fn print_mcp_matrix(all_configs: &[crate::config::PlatformMcpConfig]) {
    use std::collections::BTreeSet;

    if all_configs.is_empty() {
        return;
    }

    let mut all_servers: BTreeSet<String> = all_configs
        .iter()
        .flat_map(|c| c.servers.iter().map(|s| s.name.clone()))
        .collect();
    // Always show our 4 core servers in the matrix
    for s in &["canopy", "fetch", "filesystem"] {
        all_servers.insert(s.to_string());
    }

    let server_col = 20usize;
    let cell_col = 3usize;
    let total_width = 2 + server_col + 1 + (all_configs.len() * (cell_col + 1));

    println!("  MCP overview:");
    println!(
        "  {:<server_col$} {}",
        "Server",
        (1..=all_configs.len())
            .map(|i| format!("{:>cell_col$}", i, cell_col = cell_col))
            .collect::<Vec<_>>()
            .join(" "),
        server_col = server_col
    );
    println!("  {:─<width$}", "", width = total_width.max(34));
    for server_name in &all_servers {
        let mut row = format!("  {:<server_col$}", server_name, server_col = server_col);
        for config in all_configs {
            let has = config.servers.iter().any(|s| s.name == *server_name);
            let icon = if has {
                "\x1b[32m✓\x1b[0m"
            } else {
                "\x1b[31m✗\x1b[0m"
            };
            // Manual padding: ANSI codes break format width, so pad explicitly
            row.push_str(&format!(" {}{}", " ".repeat(cell_col - 1), icon));
        }
        println!("{}", row);
    }
    println!();
    println!("  Platforms:");
    for (idx, cfg) in all_configs.iter().enumerate() {
        println!("    {:>2}: {}", idx + 1, cfg.platform);
    }
}

fn clear_wizard_screen() -> Result<()> {
    print!("\x1b[2J\x1b[H");
    io::stdout().flush()?;
    Ok(())
}

fn apply_upsert_to_platform(
    platform: &Platform,
    config_path: &Path,
    server_name: &str,
    config: &serde_json::Value,
) -> Result<bool> {
    let is_toml = platform.config_format.as_deref() == Some("toml");
    if is_toml {
        if platform.toml_array_format {
            upsert_toml_array(
                config_path,
                &platform.mcp_servers_key.join("."),
                server_name,
                config,
            )
        } else {
            upsert_toml_key(
                config_path,
                platform
                    .mcp_servers_key
                    .first()
                    .map(|s| s.as_str())
                    .unwrap_or("mcpServers"),
                server_name,
                config,
            )
        }
    } else {
        let mut key_refs: Vec<&str> = platform
            .mcp_servers_key
            .iter()
            .map(|s| s.as_str())
            .collect();
        key_refs.push(server_name);
        upsert_json_key(config_path, &key_refs, config)
    }
}

/// Install/update canopy + recommended MCP servers on all selected platforms.
/// Translates canonical server definitions using each platform's rules.
fn run_install_our_servers(
    home: &Path,
    selected: &[&Platform],
    canonical: &CanonicalServers,
) -> Result<()> {
    let has_filesystem = canonical.servers.contains_key("filesystem");

    let fs_dir = if has_filesystem {
        let current_fs = load_mcp_fs_root(home);
        println!();
        println!("  \x1b[36mFilesystem MCP root directory\x1b[0m");
        println!("  Agents will have read/write access to everything inside this directory.");
        println!("  Choose a project folder or workspace root.");
        println!("  Current: \x1b[33m{}\x1b[0m", current_fs);
        println!();
        let chosen = browse_directory(&current_fs);
        save_mcp_fs_root(home, &chosen);
        chosen
    } else {
        load_mcp_fs_root(home)
    };

    let home_str = home.to_string_lossy().to_string();

    for p in selected {
        let config_path = resolve_config_path(home, &p.config_path);
        let is_toml = p.config_format.as_deref() == Some("toml");

        // Create config file if missing
        if !config_path.exists() {
            if let Some(parent) = config_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let initial = if is_toml {
                String::new()
            } else {
                format!(
                    "{{\"{}\": {{}}}}\n",
                    p.mcp_servers_key
                        .first()
                        .map(|s| s.as_str())
                        .unwrap_or("mcpServers")
                )
            };
            let _ = std::fs::write(&config_path, &initial);
        }

        // Remove deprecated JSON keys (cleanup only, don't touch other entries)
        if !is_toml {
            let servers_parent = p
                .mcp_servers_key
                .first()
                .map(|s| s.as_str())
                .unwrap_or("mcpServers");
            for old_key in &p.deprecated_keys {
                let _ = remove_json_key(&config_path, servers_parent, old_key);
            }
        }

        // Translate and write each canonical server for this platform
        for (server_name, template) in &canonical.servers {
            let mut config = template.clone();
            substitute_placeholders(&mut config, &home_str, &fs_dir);
            let adapted = adapt_config(&config, p, server_name);
            if let Err(e) = apply_upsert_to_platform(p, &config_path, server_name, &adapted) {
                eprintln!(
                    "  \x1b[33m⚠\x1b[0m  Failed to write {server_name} for {}: {e}",
                    p.name
                );
            }
        }
    }

    Ok(())
}

// ── Public thin wrappers for mcp_wizard ────────────────────────────────────

/// Public wrapper for [`upsert_json_key`].
pub fn upsert_json_key_pub(path: &Path, keys: &[&str], value: &serde_json::Value) -> Result<bool> {
    upsert_json_key(path, keys, value)
}

/// Public wrapper for [`upsert_toml_key`].
pub fn upsert_toml_key_pub(
    path: &Path,
    section: &str,
    entry_key: &str,
    value: &serde_json::Value,
) -> Result<bool> {
    upsert_toml_key(path, section, entry_key, value)
}

/// Public wrapper for [`upsert_toml_array`].
pub fn upsert_toml_array_pub(
    path: &Path,
    section: &str,
    entry_key: &str,
    value: &serde_json::Value,
) -> Result<bool> {
    upsert_toml_array(path, section, entry_key, value)
}

/// Public wrapper for [`remove_json_key`].
pub fn remove_json_key_pub(path: &Path, parent_key: &str, key: &str) -> Result<bool> {
    remove_json_key(path, parent_key, key)
}

/// Remove a server entry from a TOML platform config, handling both key-table
/// and array-of-tables formats.
pub fn remove_toml_server_pub(platform: &Platform, path: &Path, server_name: &str) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let content = std::fs::read_to_string(path)?;

    let updated = if platform.toml_array_format {
        let section = platform.mcp_servers_key.join(".");
        let array_header = format!("[[{section}]]");
        let name_line = format!("name = \"{server_name}\"");
        if !content.contains(&name_line) {
            return Ok(false);
        }
        remove_toml_array_entry_str(&content, &array_header, &name_line)
    } else {
        let section = platform
            .mcp_servers_key
            .first()
            .cloned()
            .unwrap_or_else(|| "mcpServers".to_string());
        let table_header = format!("[{section}.{server_name}]");
        if !content.contains(&table_header) {
            return Ok(false);
        }
        remove_toml_key_section_str(&content, &table_header)
    };

    if updated == content {
        return Ok(false);
    }

    std::fs::write(path, updated)?;
    Ok(true)
}

/// Run the interactive MCP setup/management step.
fn run_sync_step(
    wiz: &mut WizardState,
    home: &Path,
    selected: &[&Platform],
    canonical: &CanonicalServers,
) -> Result<Option<String>> {
    if selected.is_empty() {
        return Ok(None);
    }

    wiz.render()?;
    println!("  \x1b[1mMCP Manager\x1b[0m");
    println!("  ─────────────────────────────────────────────");
    println!();

    run_install_our_servers(home, selected, canonical)?;

    let all_configs = extract_all_mcp_configs(home, selected);
    if !all_configs.is_empty() {
        print_mcp_matrix(&all_configs);
    }

    Ok(Some("\x1b[32m✓\x1b[0m MCP servers updated".to_string()))
}

/// Download the UniverLab Essential Skills pack and create platform symlinks.
///
/// Runs silently on failure so a network error never blocks setup completion.
fn run_essential_skills_step(home: &Path, selected: &[&Platform]) -> String {
    // Ensure global skills directory exists
    if crate::skills_module::ensure_global_skills_dir().is_err() {
        return "\x1b[33m⚠\x1b[0m Skills: could not create ~/.agents/skills/".to_string();
    }

    // Download Essential Pack from GitHub (best-effort)
    let downloaded = crate::skills_module::download_essential_pack().unwrap_or_else(|e| {
        tracing::warn!("Essential skills download failed: {e}");
        0
    });

    // Create platform symlinks for all selected platforms that have skills_dir
    let symlinks =
        crate::skills_module::create_platform_symlinks(home, selected).unwrap_or_else(|e| {
            tracing::warn!("Skills symlink creation failed: {e}");
            vec![]
        });

    if downloaded == 0 && symlinks.is_empty() {
        // Check if we actually have any skills installed
        let global_dir = dirs::home_dir()
            .map(|h| h.join(".agents/skills"))
            .unwrap_or_default();
        let has_skills = global_dir.exists()
            && std::fs::read_dir(&global_dir)
                .map(|mut d| d.next().is_some())
                .unwrap_or(false);
        if has_skills {
            "\x1b[32m✓\x1b[0m Skills: up to date".to_string()
        } else {
            "\x1b[33m⚠\x1b[0m Skills: no packs available (repo not found)".to_string()
        }
    } else if downloaded > 0 {
        format!(
            "\x1b[32m✓\x1b[0m Skills: {} pack(s) downloaded, {} symlink(s) created",
            downloaded,
            symlinks.len()
        )
    } else {
        format!(
            "\x1b[32m✓\x1b[0m Skills: {} symlink(s) created",
            symlinks.len()
        )
    }
}
