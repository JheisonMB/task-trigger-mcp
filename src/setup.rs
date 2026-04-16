use anyhow::{Context, Result};
use inquire::{MultiSelect, Select};
use serde::Deserialize;
use std::io::{self, Write};
use std::path::Path;

const REGISTRY_BASE_URL: &str =
    "https://raw.githubusercontent.com/UniverLab/canopy-registry/main/";

const REGISTRY_LEGACY_URL: &str =
    "https://raw.githubusercontent.com/UniverLab/canopy-registry/main/platforms.json";

/// How often to refresh the registry in the background (24 hours).
const REGISTRY_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

#[derive(Deserialize, Clone)]
pub struct RegistryRaw {
    pub platforms: Vec<Platform>,
}

/// Lightweight index for the per-platform registry (v5+).
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
    #[serde(alias = "servers_key")]
    pub mcp_servers_key: Vec<String>,
    #[serde(default)]
    pub canopy_entry_key: String,
    pub canopy_entry: serde_json::Value,
    #[serde(default)]
    pub deprecated_keys: Vec<String>,
    /// Keys that this platform's MCP schema does not support.
    /// Used to sanitize configs when syncing across platforms.
    #[serde(default)]
    pub unsupported_keys: Vec<String>,
    /// MCP servers that canopy always installs alongside its own entry.
    /// Keys are server names, values are their config templates.
    /// Supports `{filesystem_dir}` and `{home}` placeholders.
    #[serde(default)]
    pub recommended_servers: std::collections::HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub cli: Option<serde_json::Value>,
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
/// Returns "/" as default if not yet configured.
fn load_mcp_fs_root(home: &Path) -> String {
    let path = home.join(".canopy/mcp_config.json");
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(s) = v.get("filesystem_root").and_then(|v| v.as_str()) {
                if !s.is_empty() {
                    return s.to_string();
                }
            }
        }
    }
    "/".to_string()
}

/// Persist the chosen filesystem root path for reuse across setups and updates.
fn save_mcp_fs_root(home: &Path, root: &str) {
    let path = home.join(".canopy/mcp_config.json");
    let content = serde_json::json!({ "filesystem_root": root });
    let _ = std::fs::write(
        &path,
        serde_json::to_string_pretty(&content).unwrap_or_default() + "\n",
    );
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
                        &format!(
                            "source {nvm_dir}/nvm.sh && nvm install --lts 2>/dev/null"
                        ),
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
        .map(|h| h.join(".canopy/.configured").exists())
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

    for p in &mut registry.platforms {
        if p.canopy_entry_key.is_empty() && p.mcp_servers_key.len() > 1 {
            p.canopy_entry_key = p.mcp_servers_key.pop().unwrap();
        }
    }
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

    // ── Step 3: Configure MCP entries ───────────────────────────
    wiz.render()?;
    let (mut configured, mut skipped, mut failed) = (0usize, 0usize, 0usize);

    for p in &selected {
        let path = resolve_config_path(&home, &p.config_path);
        let is_toml = p.config_format.as_deref() == Some("toml");

        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let initial_content = if is_toml {
                format!("[{}]\n", &p.mcp_servers_key[0])
            } else {
                format!("{{\"{}\": {{}}}}\n", &p.mcp_servers_key[0])
            };
            std::fs::write(&path, &initial_content)?;
        }

        if !is_toml {
            let servers_parent = &p.mcp_servers_key[0];
            for old_key in &p.deprecated_keys {
                let _ = remove_json_key(&path, servers_parent, old_key);
            }
            let _ =
                sanitize_existing_json_servers(&path, &p.mcp_servers_key, &p.unsupported_keys);
        }

        let entry = sanitize_canopy_entry(&p.name, &p.unsupported_keys, p.canopy_entry.clone());
        let result = if is_toml {
            if p.toml_array_format {
                upsert_toml_array(&path, &p.mcp_servers_key.join("."), &p.canopy_entry_key, &entry)
            } else {
                upsert_toml_key(&path, &p.mcp_servers_key[0], &p.canopy_entry_key, &entry)
            }
        } else {
            let mut key_refs: Vec<&str> = p.mcp_servers_key.iter().map(|s| s.as_str()).collect();
            key_refs.push(&p.canopy_entry_key);
            upsert_json_key(&path, &key_refs, &entry)
        };

        match result {
            Ok(true) => configured += 1,
            Ok(false) => skipped += 1,
            Err(_) => failed += 1,
        }
    }

    let mut mcp_parts = vec![format!("{configured} configured")];
    if skipped > 0 {
        mcp_parts.push(format!("{skipped} skipped"));
    }
    if failed > 0 {
        mcp_parts.push(format!("{failed} failed"));
    }
    wiz.add(format!(
        "\x1b[32m✓\x1b[0m MCP entries: {}",
        mcp_parts.join(", ")
    ));

    // ── Step 3.5: Recommended MCP servers ───────────────────────
    wiz.render()?;
    let rec_summary = install_recommended_mcp_servers(&home, &selected)?;
    if let Some(s) = rec_summary {
        wiz.add(s);
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

    let cli_step = match cli_registry.save(&canopy_dir.join("cli_config.json")) {
        Ok(_) => format!(
            "\x1b[32m✓\x1b[0m CLI config: {} CLI(s) saved",
            cli_registry.available_clis.len()
        ),
        Err(e) => format!("\x1b[33m⚠\x1b[0m CLI config: {e}"),
    };
    wiz.add(cli_step);

    // ── Step 5: MCP Manager (sync/add/remove) ───────────────────
    if !selected.is_empty() {
        let sync_summary = run_sync_step(&mut wiz, &home, &selected)?;
        if let Some(s) = sync_summary {
            wiz.add(s);
        }
    }

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

    // Mark configured
    let marker = home.join(".canopy/.configured");
    std::fs::create_dir_all(marker.parent().unwrap())?;
    std::fs::write(&marker, chrono::Utc::now().to_rfc3339())?;

    // ── Final summary ───────────────────────────────────────────
    wiz.render()?;
    println!("  \x1b[1;32m✅ Setup complete! canopy is ready.\x1b[0m");
    println!("  Run \x1b[1mcanopy\x1b[0m or \x1b[1mcanopy tui\x1b[0m to launch the interface.");
    println!();

    Ok(())
}

/// Fetch the per-platform registry (v5) or fall back to legacy monolithic file.
fn fetch_registry() -> Result<RegistryRaw> {
    let client = reqwest::blocking::Client::new();

    // Try the v5 index first
    if let Ok(response) = client
        .get(format!("{REGISTRY_BASE_URL}index.json"))
        .header("User-Agent", "canopy")
        .send()
    {
        if response.status().is_success() {
            if let Ok(index) = response.json::<RegistryIndex>() {
                // Detect which binaries are available, fetch only those platform files
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

                if !platforms.is_empty() {
                    return Ok(RegistryRaw { platforms });
                }
            }
        }
    }

    // Fallback: legacy monolithic platforms.json
    let response = client
        .get(REGISTRY_LEGACY_URL)
        .header("User-Agent", "canopy")
        .send()
        .context("Failed to connect to platform registry")?;

    if !response.status().is_success() {
        anyhow::bail!("Registry returned HTTP {}", response.status());
    }

    response.json().context("Invalid registry JSON")
}

const BANNER: &str = r#"                                                     
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

    // Remove existing section (if any) so we always write a fresh one
    let base = remove_toml_key_section_str(&content, &table_header);

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

fn sanitize_existing_json_servers(
    path: &Path,
    servers_key: &[String],
    unsupported_keys: &[String],
) -> Result<usize> {
    if unsupported_keys.is_empty() || !path.exists() {
        return Ok(0);
    }

    let content = std::fs::read_to_string(path)?;
    let clean = strip_jsonc_comments(&content);
    let mut root: serde_json::Value = serde_json::from_str(&clean).unwrap_or(serde_json::json!({}));

    let mut current = &mut root;
    for key in servers_key {
        let Some(next) = current.get_mut(key) else {
            return Ok(0);
        };
        current = next;
    }

    let Some(servers_obj) = current.as_object_mut() else {
        return Ok(0);
    };

    let mut removed_count = 0usize;
    for (_, server_cfg) in servers_obj.iter_mut() {
        let Some(server_obj) = server_cfg.as_object_mut() else {
            continue;
        };
        for key in unsupported_keys {
            if server_obj.remove(key).is_some() {
                removed_count += 1;
            }
        }
    }

    if removed_count > 0 {
        std::fs::write(path, serde_json::to_string_pretty(&root)? + "\n")?;
    }

    Ok(removed_count)
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
    crate::service_install::install_service(&exe, 7755)?;
    Ok(true)
}

fn is_process_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// Check if auto-setup should run (no CLI config found).
pub fn needs_setup() -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    !home.join(".canopy/cli_config.json").exists()
}

/// Run setup silently (no prompts, auto-detect all platforms).
#[allow(dead_code)]
pub fn run_setup_silent() -> Result<()> {
    let home = dirs::home_dir().context("No home directory")?;

    // Ensure MCP runtime dependencies
    ensure_mcp_dependencies_silent();

    let mut registry = fetch_registry()?;

    for p in &mut registry.platforms {
        if p.canopy_entry_key.is_empty() && p.mcp_servers_key.len() > 1 {
            p.canopy_entry_key = p.mcp_servers_key.pop().unwrap();
        }
    }

    // Auto-detect all installed platforms
    let detected: Vec<&Platform> = registry
        .platforms
        .iter()
        .filter(|p| is_platform_available(p))
        .collect();

    // Configure MCP for all detected platforms
    for p in &detected {
        let path = resolve_config_path(&home, &p.config_path);
        let is_toml = p.config_format.as_deref() == Some("toml");

        // Auto-create config file if missing
        if !path.exists() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let initial = if is_toml {
                format!("[{}]\n", &p.mcp_servers_key[0])
            } else {
                format!("{{\"{}\": {{}}}}\n", &p.mcp_servers_key[0])
            };
            let _ = std::fs::write(&path, &initial);
        }

        if !is_toml {
            let servers_parent = &p.mcp_servers_key[0];
            for old_key in &p.deprecated_keys {
                let _ = remove_json_key(&path, servers_parent, old_key);
            }
            let _ = sanitize_existing_json_servers(&path, &p.mcp_servers_key, &p.unsupported_keys);
        }

        let entry = sanitize_canopy_entry(&p.name, &p.unsupported_keys, p.canopy_entry.clone());
        if is_toml {
            if p.toml_array_format {
                let _ = upsert_toml_array(&path, &p.mcp_servers_key.join("."), &p.canopy_entry_key, &entry);
            } else {
                let _ = upsert_toml_key(&path, &p.mcp_servers_key[0], &p.canopy_entry_key, &entry);
            }
        } else {
            let mut key_refs: Vec<&str> = p.mcp_servers_key.iter().map(|s| s.as_str()).collect();
            key_refs.push(&p.canopy_entry_key);
            let _ = upsert_json_key(&path, &key_refs, &entry);
        }

        // Install recommended servers silently (use home as default fs dir)
        let home_str = home.to_string_lossy().to_string();
        let default_dir = load_mcp_fs_root(&home);
        for (server_name, template) in &p.recommended_servers {
            let mut config = template.clone();
            substitute_placeholders(&mut config, &home_str, &default_dir);
            let sanitized = sanitize_server_config_for_platform(
                &p.name,
                &p.unsupported_keys,
                config,
            );
            let _ = apply_upsert_to_platform(p, &path, server_name, &sanitized);
        }
        // Ensure memory directory
        if p.recommended_servers.contains_key("memory") {
            let _ = std::fs::create_dir_all(home.join(".canopy/memory"));
        }
    }

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
    cli_registry.save(&canopy_dir.join("cli_config.json"))?;

    // Mark configured
    let marker = home.join(".canopy/.configured");
    std::fs::write(&marker, chrono::Utc::now().to_rfc3339())?;

    // Restart daemon so it picks up new configs
    let _ = stop_daemon();
    let _ = start_daemon_if_needed();

    Ok(())
}

/// Sanitize a platform's `canopy_entry` by stripping keys that the CLI's
/// MCP config schema does not support.  This protects against registry
/// entries that include keys valid for one CLI but invalid for another
/// (e.g. `"tools"` is supported by copilot but rejected by gemini).
fn sanitize_canopy_entry(
    platform_name: &str,
    unsupported_keys: &[String],
    mut entry: serde_json::Value,
) -> serde_json::Value {
    if let Some(obj) = entry.as_object_mut() {
        for key in unsupported_keys {
            obj.remove(key);
        }

        // Homologate transport type for HTTP servers.
        // Modern MCP clients use "remote" for HTTP-based transports.
        if matches!(platform_name, "copilot" | "qwen" | "claude" | "mistral" | "gemini")
            && obj.contains_key("url")
        {
            obj.insert(
                "type".to_string(),
                serde_json::Value::String("remote".to_string()),
            );
        }
    }
    entry
}

/// Sanitize an arbitrary MCP server config for a target platform.
/// Removes keys that the target platform does not support.
fn sanitize_server_config_for_platform(
    platform_name: &str,
    unsupported_keys: &[String],
    mut config: serde_json::Value,
) -> serde_json::Value {
    if let Some(obj) = config.as_object_mut() {
        for key in unsupported_keys {
            obj.remove(key);
        }

        // Homologate transport type for HTTP servers.
        if matches!(platform_name, "copilot" | "qwen" | "claude" | "mistral" | "gemini")
            && obj.contains_key("url")
        {
            obj.insert(
                "type".to_string(),
                serde_json::Value::String("remote".to_string()),
            );
        }
    }
    config
}

/// Refresh the registry config in the background if it's older than 24h.
/// Returns true if a refresh was performed.
pub fn maybe_refresh_registry() -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    let config_path = home.join(".canopy/cli_config.json");

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

fn refresh_registry_inner(home: &std::path::Path) -> Result<()> {
    let mut registry = fetch_registry()?;

    for p in &mut registry.platforms {
        if p.canopy_entry_key.is_empty() && p.mcp_servers_key.len() > 1 {
            p.canopy_entry_key = p.mcp_servers_key.pop().unwrap();
        }
    }

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
        cli_registry.save(&home.join(".canopy/cli_config.json"))?;
    }

    Ok(())
}

// ── Recommended MCP servers ───────────────────────────────────────────────

/// Replace `{filesystem_dir}` and `{home}` placeholders in a JSON value tree.
fn substitute_placeholders(value: &mut serde_json::Value, home: &str, fs_dir: &str) {
    match value {
        serde_json::Value::String(s) => {
            if s.contains("{filesystem_dir}") || s.contains("{home}") {
                *s = s.replace("{filesystem_dir}", fs_dir).replace("{home}", home);
            }
        }
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
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.') { None } else { Some(name) }
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
    let mut prev_rows: usize = 0;

    let _ = enable_raw_mode();

    loop {
        let subdirs = list_subdirs(&current);
        let list_rows = if subdirs.is_empty() { 1 } else { subdirs.len().min(visible) };
        let total_rows = 4 + list_rows; // blank + path + blank + hint + entries

        // Erase previous draw
        if prev_rows > 0 {
            for _ in 0..prev_rows {
                print!("\x1b[1A\x1b[2K");
            }
        }
        prev_rows = total_rows;

        // Clamp cursor
        if !subdirs.is_empty() && cursor >= subdirs.len() {
            cursor = subdirs.len().saturating_sub(1);
        }

        // Draw path header
        print!("\r\n\x1b[2K  \x1b[36m»\x1b[0m  {}\r\n", current.display());
        print!("\x1b[2K  \x1b[90m↑↓ navigate  → enter dir  ← go up  Enter select  Esc cancel\x1b[0m\r\n");

        // Draw directory list
        if subdirs.is_empty() {
            print!("\x1b[2K  \x1b[90m(no subdirectories)\x1b[0m\r\n");
        } else {
            let scroll = if cursor >= visible { cursor - visible + 1 } else { 0 };
            for (i, name) in subdirs.iter().enumerate().skip(scroll).take(visible) {
                if i == cursor {
                    print!("\x1b[2K  \x1b[1;32m▶\x1b[0m \x1b[7m {name} \x1b[0m\r\n");
                } else {
                    print!("\x1b[2K    {name}\r\n");
                }
            }
        }

        let _ = io::stdout().flush();

        match read() {
            Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => match k.code {
                KeyCode::Enter => {
                    let _ = disable_raw_mode();
                    println!("\r");
                    return current.to_string_lossy().to_string();
                }
                KeyCode::Esc => {
                    let _ = disable_raw_mode();
                    println!("\r");
                    return start_dir.to_string();
                }
                KeyCode::Up => {
                    cursor = cursor.saturating_sub(1);
                }
                KeyCode::Down => {
                    if !subdirs.is_empty() && cursor + 1 < subdirs.len() {
                        cursor += 1;
                    }
                }
                KeyCode::Right => {
                    if let Some(name) = subdirs.get(cursor) {
                        current = current.join(name);
                        cursor = 0;
                    }
                }
                KeyCode::Left => {
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

/// Install recommended MCP servers (mandatory) across all selected platforms.
/// Uses `recommended_servers` from each platform's registry entry.
fn install_recommended_mcp_servers(
    home: &Path,
    selected: &[&Platform],
) -> Result<Option<String>> {
    if selected.is_empty() {
        return Ok(None);
    }

    // Collect all unique recommended server names across platforms
    let mut all_server_names: Vec<String> = selected
        .iter()
        .flat_map(|p| p.recommended_servers.keys().cloned())
        .collect();
    all_server_names.sort();
    all_server_names.dedup();

    if all_server_names.is_empty() {
        return Ok(None);
    }

    // Check if any platform needs a filesystem directory
    let needs_fs = selected
        .iter()
        .any(|p| p.recommended_servers.contains_key("filesystem"));

    let mut fs_dir = String::new();
    if needs_fs {
        let default_dir = load_mcp_fs_root(home);
        println!();
        println!("  \x1b[36mFilesystem MCP root directory\x1b[0m");
        println!("  Agents will have read/write access to everything inside this directory.");
        println!("  Choose a project folder or workspace root (e.g. ~/Documents/Projects).");
        println!();
        fs_dir = browse_directory(&default_dir);
        save_mcp_fs_root(home, &fs_dir);
    }

    // Ensure memory directory exists
    let needs_memory = selected
        .iter()
        .any(|p| p.recommended_servers.contains_key("memory"));
    if needs_memory {
        let memory_dir = home.join(".canopy/memory");
        let _ = std::fs::create_dir_all(&memory_dir);
    }

    let home_str = home.to_string_lossy().to_string();
    let mut installed = 0usize;

    for p in selected {
        let config_path = resolve_config_path(home, &p.config_path);
        if !config_path.exists() {
            continue;
        }

        for (server_name, template) in &p.recommended_servers {
            let mut config = template.clone();
            substitute_placeholders(&mut config, &home_str, &fs_dir);

            let sanitized = sanitize_server_config_for_platform(
                &p.name,
                &p.unsupported_keys,
                config,
            );
            if apply_upsert_to_platform(p, &config_path, server_name, &sanitized).is_ok() {
                installed += 1;
            }
        }
    }

    let server_list = all_server_names.join(", ");
    let label = if installed > 0 {
        format!("{installed} installed")
    } else {
        "no changes".to_string()
    };

    Ok(Some(format!(
        "\x1b[32m✓\x1b[0m Recommended servers ({server_list}): {label}"
    )))
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
    for s in &["canopy", "fetch", "filesystem", "memory"] {
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

fn wait_continue() -> Result<()> {
    println!();
    println!("  Press Enter to continue...");
    let mut buf = String::new();
    io::stdin().read_line(&mut buf)?;
    println!();
    Ok(())
}

#[derive(Default)]
struct OperationSummary {
    added: usize,
    failed: usize,
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
                &platform.mcp_servers_key[0],
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

/// Install/update the 4 canopy servers on all selected platforms using the saved fs root.
fn run_install_our_servers(home: &Path, selected: &[&Platform]) -> Result<()> {
    let fs_dir = load_mcp_fs_root(home);
    let home_str = home.to_string_lossy().to_string();
    let mut summaries: std::collections::BTreeMap<String, OperationSummary> =
        std::collections::BTreeMap::new();

    for p in selected {
        let config_path = resolve_config_path(home, &p.config_path);
        if !config_path.exists() {
            continue;
        }
        let summary = summaries.entry(p.name.clone()).or_default();

        for (server_name, template) in &p.recommended_servers {
            let mut config = template.clone();
            substitute_placeholders(&mut config, &home_str, &fs_dir);
            let sanitized = sanitize_server_config_for_platform(&p.name, &p.unsupported_keys, config);
            match apply_upsert_to_platform(p, &config_path, server_name, &sanitized) {
                Ok(_) => summary.added += 1,
                Err(_) => summary.failed += 1,
            }
        }

        // Also write the canopy entry itself
        let entry = sanitize_canopy_entry(&p.name, &p.unsupported_keys, p.canopy_entry.clone());
        let _ = apply_upsert_to_platform(p, &config_path, &p.canopy_entry_key.clone(), &entry);
    }

    println!();
    println!("  Update summary:");
    for (platform, s) in summaries {
        println!("    {} -> updated: {}, failed: {}", platform, s.added, s.failed);
    }
    println!();
    Ok(())
}

/// Run the interactive MCP setup/management step.
fn run_sync_step(
    wiz: &mut WizardState,
    home: &Path,
    selected: &[&Platform],
) -> Result<Option<String>> {
    if selected.is_empty() {
        return Ok(None);
    }

    loop {
        wiz.render()?;
        println!("  \x1b[1mMCP Manager\x1b[0m");
        println!("  ─────────────────────────────────────────────");
        println!();

        let all_configs = extract_all_mcp_configs(home, selected);
        if all_configs.is_empty() {
            return Ok(None);
        }
        print_mcp_matrix(&all_configs);

        let action = Select::new(
            "MCP action:",
            vec![
                "Install / Update our servers on all platforms".to_string(),
                "Continue".to_string(),
            ],
        )
        .prompt()
        .unwrap_or_else(|_| "Continue".to_string());

        match action.as_str() {
            "Install / Update our servers on all platforms" => {
                run_install_our_servers(home, selected)?;
                wait_continue()?;
            }
            _ => break,
        }
    }

    Ok(Some(
        "\x1b[32m✓\x1b[0m MCP servers updated".to_string(),
    ))
}
