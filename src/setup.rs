use anyhow::{Context, Result};
use inquire::{Confirm, MultiSelect, Select, Text};
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
    /// Keys that this platform's MCP schema does not support.
    /// Used to sanitize configs when syncing across platforms.
    #[serde(default)]
    pub unsupported_keys: Vec<String>,
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
        .filter(|p| is_platform_available(p))
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

        // Create config file if it doesn't exist
        if !path.exists() {
            print!(
                "  \x1b[33m?\x1b[0m {} config not found. Create it? [y/N] ",
                p.name
            );
            io::stdout().flush()?;
            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let input = input.trim().to_lowercase();
            if input != "y" && input != "yes" {
                println!("  \x1b[33m⏭\x1b[0m  Skipping {}", p.name);
                continue;
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let initial_content = if is_toml {
                format!("[{}]\n", &p.mcp_servers_key[0])
            } else {
                format!("{{\"{}\": {{}}}}\n", &p.mcp_servers_key[0])
            };
            std::fs::write(&path, &initial_content)?;
            println!("  \x1b[32m✓\x1b[0m Created {}", path.display());
        }

        if !is_toml {
            let servers_parent = &p.mcp_servers_key[0];
            for old_key in &p.deprecated_keys {
                if let Ok(true) = remove_json_key(&path, servers_parent, old_key) {
                    println!("  🗑  Removed old '{}' from {}", old_key, p.name);
                }
            }
            if let Ok(removed) =
                sanitize_existing_json_servers(&path, &p.mcp_servers_key, &p.unsupported_keys)
            {
                if removed > 0 {
                    println!(
                        "  \x1b[32m✓\x1b[0m Sanitized {} unsupported key(s) in {}",
                        removed, p.name
                    );
                }
            }
        }

        let entry = sanitize_canopy_entry(&p.name, &p.unsupported_keys, p.canopy_entry.clone());
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

    // Sync MCP configurations across platforms
    if !selected.is_empty() {
        let _ = run_sync_step(&home, &selected);
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
    println!("  \x1b[1;32m✅ Setup complete! canopy is ready.\x1b[0m");
    println!("  Run \x1b[1mcanopy\x1b[0m or \x1b[1mcanopy tui\x1b[0m to launch the interface.");
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

fn remove_json_nested_key(path: &Path, keys: &[&str]) -> Result<bool> {
    if keys.is_empty() || !path.exists() {
        return Ok(false);
    }

    let content = std::fs::read_to_string(path)?;
    let clean = strip_jsonc_comments(&content);
    let mut root: serde_json::Value = serde_json::from_str(&clean).unwrap_or(serde_json::json!({}));

    let mut current = &mut root;
    for key in &keys[..keys.len() - 1] {
        let Some(next) = current.get_mut(*key) else {
            return Ok(false);
        };
        current = next;
    }

    let Some(obj) = current.as_object_mut() else {
        return Ok(false);
    };

    let removed = obj.remove(keys[keys.len() - 1]).is_some();
    if removed {
        std::fs::write(path, serde_json::to_string_pretty(&root)? + "\n")?;
    }
    Ok(removed)
}

fn remove_toml_key(path: &Path, section: &str, entry_key: &str) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }

    let content = std::fs::read_to_string(path)?;
    let header = format!("[{section}.{entry_key}]");
    let mut out = String::with_capacity(content.len());
    let mut in_target_section = false;
    let mut removed = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if trimmed == header {
                in_target_section = true;
                removed = true;
                continue;
            }
            in_target_section = false;
        }

        if !in_target_section {
            out.push_str(line);
            out.push('\n');
        }
    }

    if removed {
        std::fs::write(path, out)?;
    }
    Ok(removed)
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
        .filter(|p| is_platform_available(p))
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
            let _ = sanitize_existing_json_servers(&path, &p.mcp_servers_key, &p.unsupported_keys);
        }

        let entry = sanitize_canopy_entry(&p.name, &p.unsupported_keys, p.canopy_entry.clone());
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
        // Some clients (copilot, qwen) require "sse", others like "http".
        // Using "sse" is generally safer and more precise for MCP-over-HTTP.
        if matches!(platform_name, "copilot" | "qwen" | "claude" | "mistral")
            && obj.contains_key("url")
        {
            obj.insert(
                "type".to_string(),
                serde_json::Value::String("sse".to_string()),
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
        if matches!(platform_name, "copilot" | "qwen" | "claude" | "mistral")
            && obj.contains_key("url")
        {
            obj.insert(
                "type".to_string(),
                serde_json::Value::String("sse".to_string()),
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

// ── MCP Sync ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct SyncConfigEntry {
    server_name: String,
    config: serde_json::Value,
    source_platforms: Vec<String>,
}

/// Extract all MCP server configs from the selected platforms.
fn extract_all_mcp_configs(
    home: &Path,
    selected: &[&Platform],
) -> Vec<crate::config::PlatformMcpConfig> {
    let mut configs = Vec::new();
    for p in selected {
        let config_path = home.join(&p.config_path);
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

/// Collect unique server names across all platforms.
fn collect_unique_servers(
    all_configs: &[crate::config::PlatformMcpConfig],
) -> Vec<SyncConfigEntry> {
    let mut server_map: std::collections::BTreeMap<String, SyncConfigEntry> =
        std::collections::BTreeMap::new();

    for platform_cfg in all_configs {
        for server in &platform_cfg.servers {
            let entry = server_map
                .entry(server.name.clone())
                .or_insert_with(|| SyncConfigEntry {
                    server_name: server.name.clone(),
                    config: server.config.clone(),
                    source_platforms: Vec::new(),
                });
            if !entry.source_platforms.contains(&platform_cfg.platform) {
                entry.source_platforms.push(platform_cfg.platform.clone());
            }
        }
    }

    server_map.into_values().collect()
}

fn print_mcp_matrix(all_configs: &[crate::config::PlatformMcpConfig]) {
    use std::collections::BTreeSet;

    if all_configs.is_empty() {
        return;
    }

    let all_servers: BTreeSet<String> = all_configs
        .iter()
        .flat_map(|c| c.servers.iter().map(|s| s.name.clone()))
        .collect();

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
            row.push_str(&format!(" {:>cell_col$}", icon, cell_col = cell_col));
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
    removed: usize,
    skipped: usize,
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
        upsert_toml_key(
            config_path,
            &platform.mcp_servers_key[0],
            server_name,
            config,
        )
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

fn apply_remove_to_platform(
    platform: &Platform,
    config_path: &Path,
    server_name: &str,
) -> Result<bool> {
    let is_toml = platform.config_format.as_deref() == Some("toml");
    if is_toml {
        remove_toml_key(config_path, &platform.mcp_servers_key[0], server_name)
    } else {
        let mut key_refs: Vec<&str> = platform
            .mcp_servers_key
            .iter()
            .map(|s| s.as_str())
            .collect();
        key_refs.push(server_name);
        remove_json_nested_key(config_path, &key_refs)
    }
}

fn select_target_platforms(selected: &[&Platform]) -> Result<Vec<String>> {
    let platform_names: Vec<String> = selected.iter().map(|p| p.name.clone()).collect();
    let chosen = MultiSelect::new("Select target platforms:", platform_names)
        .with_all_selected_by_default()
        .prompt()
        .unwrap_or_default();
    Ok(chosen)
}

fn run_sync_action(
    home: &Path,
    selected: &[&Platform],
    unique_servers: &[SyncConfigEntry],
) -> Result<()> {
    let server_choices: Vec<String> = unique_servers
        .iter()
        .map(|s| s.server_name.clone())
        .collect();
    if server_choices.is_empty() {
        println!("  \x1b[33m⏭\x1b[0m  No servers available to sync.");
        return Ok(());
    }

    let selected_servers = MultiSelect::new("Select MCP servers to sync:", server_choices)
        .with_all_selected_by_default()
        .prompt()
        .unwrap_or_default();

    if selected_servers.is_empty() {
        println!("  \x1b[33m⏭\x1b[0m  No servers selected, skipping.");
        return Ok(());
    }

    let target_platforms = select_target_platforms(selected)?;
    if target_platforms.is_empty() {
        println!("  \x1b[33m⏭\x1b[0m  No target platforms selected, skipping.");
        return Ok(());
    }

    let mut summaries: std::collections::BTreeMap<String, OperationSummary> =
        std::collections::BTreeMap::new();
    let source_by_name: std::collections::HashMap<&str, &SyncConfigEntry> = unique_servers
        .iter()
        .map(|s| (s.server_name.as_str(), s))
        .collect();

    for platform_name in &target_platforms {
        let platform = selected
            .iter()
            .find(|p| p.name == *platform_name)
            .expect("platform should exist");
        let config_path = home.join(&platform.config_path);
        let summary = summaries.entry(platform_name.clone()).or_default();

        for server_name in &selected_servers {
            let Some(server) = source_by_name.get(server_name.as_str()) else {
                summary.failed += 1;
                continue;
            };

            let sanitized = sanitize_server_config_for_platform(
                &platform.name,
                &platform.unsupported_keys,
                server.config.clone(),
            );
            match apply_upsert_to_platform(platform, &config_path, server_name, &sanitized) {
                Ok(true) => summary.added += 1,
                Ok(false) => summary.skipped += 1,
                Err(_) => summary.failed += 1,
            }
        }
    }

    println!();
    println!("  Sync summary:");
    for (platform, s) in summaries {
        println!(
            "    {} -> added: {}, skipped: {}, failed: {}",
            platform, s.added, s.skipped, s.failed
        );
    }
    println!();
    Ok(())
}

fn run_add_action(
    home: &Path,
    selected: &[&Platform],
    unique_servers: &[SyncConfigEntry],
) -> Result<()> {
    let server_name = Text::new("New MCP server name:")
        .with_validator(|input: &str| {
            if input.trim().is_empty() {
                Ok(inquire::validator::Validation::Invalid(
                    "Name cannot be empty".into(),
                ))
            } else {
                Ok(inquire::validator::Validation::Valid)
            }
        })
        .prompt()
        .unwrap_or_default()
        .trim()
        .to_string();

    if server_name.is_empty() {
        println!("  \x1b[33m⏭\x1b[0m  Invalid name, skipping.");
        return Ok(());
    }

    let source_mode = Select::new(
        "Config source:",
        vec![
            "Copy from existing server".to_string(),
            "Paste JSON config".to_string(),
        ],
    )
    .prompt()
    .unwrap_or_else(|_| "Copy from existing server".to_string());

    let base_config = if source_mode == "Paste JSON config" {
        let raw = Text::new("Paste server config as JSON object:")
            .with_initial_value("{}")
            .prompt()
            .unwrap_or_else(|_| "{}".to_string());
        let parsed: serde_json::Value = serde_json::from_str(raw.trim())
            .map_err(|e| anyhow::anyhow!("Invalid JSON config: {}", e))?;
        if !parsed.is_object() {
            return Err(anyhow::anyhow!("Config must be a JSON object"));
        }
        parsed
    } else {
        let source_choices: Vec<String> = unique_servers
            .iter()
            .map(|s| s.server_name.clone())
            .collect();
        if source_choices.is_empty() {
            return Err(anyhow::anyhow!(
                "No existing servers available to copy from"
            ));
        }
        let template_name = Select::new("Template server:", source_choices)
            .prompt()
            .map_err(|e| anyhow::anyhow!("Selection cancelled: {}", e))?;
        unique_servers
            .iter()
            .find(|s| s.server_name == template_name)
            .map(|s| s.config.clone())
            .ok_or_else(|| anyhow::anyhow!("Template server not found"))?
    };

    let target_platforms = select_target_platforms(selected)?;
    if target_platforms.is_empty() {
        println!("  \x1b[33m⏭\x1b[0m  No target platforms selected, skipping.");
        return Ok(());
    }

    let mut summaries: std::collections::BTreeMap<String, OperationSummary> =
        std::collections::BTreeMap::new();
    for platform_name in &target_platforms {
        let platform = selected
            .iter()
            .find(|p| p.name == *platform_name)
            .expect("platform should exist");
        let config_path = home.join(&platform.config_path);
        let summary = summaries.entry(platform_name.clone()).or_default();

        let sanitized = sanitize_server_config_for_platform(
            &platform.name,
            &platform.unsupported_keys,
            base_config.clone(),
        );
        match apply_upsert_to_platform(platform, &config_path, &server_name, &sanitized) {
            Ok(true) => summary.added += 1,
            Ok(false) => summary.skipped += 1,
            Err(_) => summary.failed += 1,
        }
    }

    println!();
    println!("  Add summary (server: {}):", server_name);
    for (platform, s) in summaries {
        println!(
            "    {} -> added: {}, skipped: {}, failed: {}",
            platform, s.added, s.skipped, s.failed
        );
    }
    println!();
    Ok(())
}

fn run_remove_action(
    home: &Path,
    selected: &[&Platform],
    unique_servers: &[SyncConfigEntry],
) -> Result<()> {
    let server_choices: Vec<String> = unique_servers
        .iter()
        .map(|s| s.server_name.clone())
        .collect();
    if server_choices.is_empty() {
        println!("  \x1b[33m⏭\x1b[0m  No servers available to remove.");
        return Ok(());
    }

    let selected_servers = MultiSelect::new("Select MCP servers to remove:", server_choices)
        .prompt()
        .unwrap_or_default();
    if selected_servers.is_empty() {
        println!("  \x1b[33m⏭\x1b[0m  No servers selected, skipping.");
        return Ok(());
    }

    let confirmed = Confirm::new("Apply deletion on selected platforms?")
        .with_default(false)
        .prompt()
        .unwrap_or(false);
    if !confirmed {
        println!("  \x1b[33m⏭\x1b[0m  Deletion cancelled.");
        return Ok(());
    }

    let target_platforms = select_target_platforms(selected)?;
    if target_platforms.is_empty() {
        println!("  \x1b[33m⏭\x1b[0m  No target platforms selected, skipping.");
        return Ok(());
    }

    let mut summaries: std::collections::BTreeMap<String, OperationSummary> =
        std::collections::BTreeMap::new();
    for platform_name in &target_platforms {
        let platform = selected
            .iter()
            .find(|p| p.name == *platform_name)
            .expect("platform should exist");
        let config_path = home.join(&platform.config_path);
        let summary = summaries.entry(platform_name.clone()).or_default();

        for server_name in &selected_servers {
            match apply_remove_to_platform(platform, &config_path, server_name) {
                Ok(true) => summary.removed += 1,
                Ok(false) => summary.skipped += 1,
                Err(_) => summary.failed += 1,
            }
        }
    }

    println!();
    println!("  Remove summary:");
    for (platform, s) in summaries {
        println!(
            "    {} -> removed: {}, skipped: {}, failed: {}",
            platform, s.removed, s.skipped, s.failed
        );
    }
    println!();
    Ok(())
}

/// Run the interactive MCP setup/management step.
fn run_sync_step(home: &Path, selected: &[&Platform]) -> Result<()> {
    if selected.is_empty() {
        return Ok(());
    }

    loop {
        clear_wizard_screen()?;
        println!("  \x1b[1mMCP Manager\x1b[0m");
        println!("  ─────────────────────────────────────────────");
        println!();

        let all_configs = extract_all_mcp_configs(home, selected);
        if all_configs.is_empty() {
            return Ok(());
        }
        let unique_servers = collect_unique_servers(&all_configs);
        print_mcp_matrix(&all_configs);

        let action = Select::new(
            "MCP action:",
            vec![
                "Sync servers across platforms".to_string(),
                "Add server to platforms".to_string(),
                "Remove server from platforms".to_string(),
                "Continue setup".to_string(),
            ],
        )
        .prompt()
        .unwrap_or_else(|_| "Continue setup".to_string());

        match action.as_str() {
            "Sync servers across platforms" => {
                run_sync_action(home, selected, &unique_servers)?;
                wait_continue()?;
            }
            "Add server to platforms" => {
                run_add_action(home, selected, &unique_servers)?;
                wait_continue()?;
            }
            "Remove server from platforms" => {
                run_remove_action(home, selected, &unique_servers)?;
                wait_continue()?;
            }
            _ => break,
        }
    }

    Ok(())
}
