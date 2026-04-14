//! Setup wizard — runs on first `canopy` invocation (or `canopy setup`).
//!
//! Fetches the platform registry from GitHub, detects installed platforms
//! by config file existence, configures MCP, starts daemon, installs service.

use anyhow::{Context, Result};
use inquire::MultiSelect;
use serde::Deserialize;
use std::io::{self, Write};
use std::path::Path;

const REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/UniverLab/canopy-registry/main/platforms.json";

/// How often to refresh the registry in the background (24 hours).
const REGISTRY_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

#[derive(Deserialize, Clone)]
pub struct RegistryRaw {
    pub platforms: Vec<Platform>,
}

#[derive(Deserialize, Clone)]
pub struct Platform {
    pub name: String,
    pub config_path: String,
    #[serde(default)]
    pub config_format: Option<String>,
    #[serde(alias = "servers_key")]
    pub mcp_servers_key: Vec<String>,
    #[serde(default)]
    pub canopy_entry_key: String,
    pub canopy_entry: serde_json::Value,
    #[serde(default)]
    pub deprecated_keys: Vec<String>,
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

#[allow(dead_code)]
pub fn is_configured() -> bool {
    dirs::home_dir()
        .map(|h| h.join(".canopy/.configured").exists())
        .unwrap_or(false)
}

/// Fetch the platform registry (public for use by config commands).
pub fn fetch_registry_raw() -> Result<RegistryRaw> {
    fetch_registry()
}

pub fn run_setup() -> Result<()> {
    print_banner();

    let home = dirs::home_dir().context("No home directory")?;

    print!("  Fetching platform registry... ");
    io::stdout().flush()?;
    let mut registry = fetch_registry()?;

    for p in &mut registry.platforms {
        if p.canopy_entry_key.is_empty() && p.mcp_servers_key.len() > 1 {
            p.canopy_entry_key = p.mcp_servers_key.pop().unwrap();
        }
    }

    println!("\x1b[32m✓\x1b[0m {} platform(s)", registry.platforms.len());
    println!();

    let detected: Vec<&Platform> = registry
        .platforms
        .iter()
        .filter(|p| home.join(&p.config_path).exists())
        .collect();

    if detected.is_empty() {
        println!("  No supported platforms detected.");
        println!(
            "  Supported: {}",
            registry
                .platforms
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!();
    } else {
        println!("  Detected platforms:");
        for p in &detected {
            println!("    \x1b[32m✓\x1b[0m {}", p.name);
        }
        println!();
    }

    let selected = select_platforms(&detected)?;

    for p in &selected {
        let path = home.join(&p.config_path);
        let is_toml = p.config_format.as_deref() == Some("toml");

        if !is_toml {
            let servers_parent = &p.mcp_servers_key[0];
            for old_key in &p.deprecated_keys {
                if let Ok(true) = remove_json_key(&path, servers_parent, old_key) {
                    println!("  🗑  Removed old '{}' from {}", old_key, p.name);
                }
            }
        }

        let entry = sanitize_canopy_entry(&p.name, p.canopy_entry.clone());
        let result = if is_toml {
            upsert_toml_key(&path, &p.mcp_servers_key[0], &p.canopy_entry_key, &entry)
        } else {
            let mut key_refs: Vec<&str> = p.mcp_servers_key.iter().map(|s| s.as_str()).collect();
            key_refs.push(&p.canopy_entry_key);
            upsert_json_key(&path, &key_refs, &entry)
        };

        match result {
            Ok(true) => println!("  \x1b[32m✅\x1b[0m Configured MCP for {}", p.name),
            Ok(false) => println!("  \x1b[33m⏭\x1b[0m  {} already configured", p.name),
            Err(e) => println!("  \x1b[31m❌\x1b[0m Failed to configure {}: {}", p.name, e),
        }
    }
    println!();

    print!("  Saving CLI configuration... ");
    io::stdout().flush()?;
    let platforms_with_cli: Vec<PlatformWithCli> = selected
        .iter()
        .map(|p| p.to_platform_with_cli())
        .filter(|p| p.cli.is_some())
        .collect();

    let cli_registry =
        crate::domain::cli_config::CliRegistry::detect_available(&platforms_with_cli);
    let canopy_dir = home.join(".canopy");
    std::fs::create_dir_all(&canopy_dir)?;

    match cli_registry.save(&canopy_dir.join("cli_config.json")) {
        Ok(_) => {
            println!(
                "\x1b[32m✅\x1b[0m {} CLI(s) saved",
                cli_registry.available_clis.len()
            );
        }
        Err(e) => println!("\x1b[33m⚠\x1b[0m  Failed to save CLI config: {}", e),
    }

    // Start daemon
    print!("  Starting daemon... ");
    io::stdout().flush()?;
    match start_daemon_if_needed() {
        Ok(true) => println!("\x1b[32m✅\x1b[0m started"),
        Ok(false) => println!("\x1b[32m✅\x1b[0m already running"),
        Err(e) => println!("\x1b[31m❌\x1b[0m {e}"),
    }

    // Install service
    print!("  Installing system service... ");
    io::stdout().flush()?;
    match install_service_if_needed() {
        Ok(true) => println!("\x1b[32m✅\x1b[0m installed"),
        Ok(false) => println!("\x1b[32m✅\x1b[0m already installed"),
        Err(e) => println!("\x1b[31m❌\x1b[0m {e}"),
    }

    // Mark configured
    let marker = home.join(".canopy/.configured");
    std::fs::create_dir_all(marker.parent().unwrap())?;
    std::fs::write(&marker, chrono::Utc::now().to_rfc3339())?;

    println!();
    println!("  \x1b[1;32m✅ canopy is ready!\x1b[0m");
    println!("  Launching TUI...");
    println!();

    Ok(())
}

fn fetch_registry() -> Result<RegistryRaw> {
    let response = reqwest::blocking::Client::new()
        .get(REGISTRY_URL)
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
    if current.get(leaf) == Some(value) {
        return Ok(false);
    }

    current[leaf] = value.clone();

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(&root)? + "\n")?;
    Ok(true)
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

    // Already configured — check if the section header exists
    if content.contains(&table_header) {
        return Ok(false);
    }

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
                    // For arrays/objects, serialize as inline TOML via serde
                    let toml_val: toml::Value = serde_json::from_value(v.clone())?;
                    fragment.push_str(&format!("{k} = {toml_val}\n"));
                }
            }
        }
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut out = content;
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
pub fn run_setup_silent() -> Result<()> {
    let home = dirs::home_dir().context("No home directory")?;
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
        .filter(|p| home.join(&p.config_path).exists())
        .collect();

    // Configure MCP for all detected platforms
    for p in &detected {
        let path = home.join(&p.config_path);
        let is_toml = p.config_format.as_deref() == Some("toml");

        if !is_toml {
            let servers_parent = &p.mcp_servers_key[0];
            for old_key in &p.deprecated_keys {
                let _ = remove_json_key(&path, servers_parent, old_key);
            }
        }

        let entry = sanitize_canopy_entry(&p.name, p.canopy_entry.clone());
        if is_toml {
            let _ = upsert_toml_key(&path, &p.mcp_servers_key[0], &p.canopy_entry_key, &entry);
        } else {
            let mut key_refs: Vec<&str> = p.mcp_servers_key.iter().map(|s| s.as_str()).collect();
            key_refs.push(&p.canopy_entry_key);
            let _ = upsert_json_key(&path, &key_refs, &entry);
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

    Ok(())
}

/// Sanitize a platform's `canopy_entry` by stripping keys that the CLI's
/// MCP config schema does not support.  This protects against registry
/// entries that include keys valid for one CLI but invalid for another
/// (e.g. `"tools"` is supported by copilot but rejected by gemini).
fn sanitize_canopy_entry(name: &str, mut entry: serde_json::Value) -> serde_json::Value {
    // Gemini does not support "tools" in mcpServers entries.
    if name == "gemini" {
        if let Some(obj) = entry.as_object_mut() {
            obj.remove("tools");
        }
    }
    entry
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
        .filter(|p| home.join(&p.config_path).exists())
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
