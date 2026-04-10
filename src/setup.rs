//! Setup wizard — runs on first `canopy` invocation (or `canopy setup`).
//!
//! Fetches the platform registry from GitHub, detects installed platforms
//! by config file existence, configures MCP, starts daemon, installs service.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::io::{self, Write};
use std::path::Path;

const REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/UniverLab/canopy-registry/main/platforms.json";

// ── Registry types ───────────────────────────────────────────────

#[derive(Deserialize)]
struct Registry {
    platforms: Vec<Platform>,
}

#[derive(Deserialize)]
struct Platform {
    name: String,
    config_path: String,
    servers_key: Vec<String>,
    canopy_entry: serde_json::Value,
    #[serde(default)]
    deprecated_keys: Vec<String>,
}

// ── Public API ───────────────────────────────────────────────────

pub fn is_configured() -> bool {
    dirs::home_dir()
        .map(|h| h.join(".canopy/.configured").exists())
        .unwrap_or(false)
}

pub fn run_setup() -> Result<()> {
    print_banner();

    let home = dirs::home_dir().context("No home directory")?;

    // Fetch registry (remote with local fallback)
    print!("  Fetching platform registry... ");
    io::stdout().flush()?;
    let registry = fetch_registry()?;
    println!("\x1b[32m✓\x1b[0m {} platform(s)", registry.platforms.len());
    println!();

    // Detect which platforms have config files
    let detected: Vec<&Platform> = registry
        .platforms
        .iter()
        .filter(|p| home.join(&p.config_path).exists())
        .collect();

    if detected.is_empty() {
        println!("  No supported platforms detected.");
        println!("  Supported: {}", registry.platforms.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", "));
        println!();
    } else {
        println!("  Detected platforms:");
        for p in &detected {
            println!("    \x1b[32m✓\x1b[0m {}", p.name);
        }
        println!();
    }

    // All detected selected by default — user can deselect
    let selected = select_platforms(&detected)?;

    // Configure MCP + remove deprecated keys
    for p in &selected {
        let path = home.join(&p.config_path);
        let servers_parent = &p.servers_key[0];

        for old_key in &p.deprecated_keys {
            if let Ok(true) = remove_json_key(&path, servers_parent, old_key) {
                println!("  🗑  Removed old '{}' from {}", old_key, p.name);
            }
        }

        let key_refs: Vec<&str> = p.servers_key.iter().map(|s| s.as_str()).collect();
        match upsert_json_key(&path, &key_refs, &p.canopy_entry) {
            Ok(true) => println!("  \x1b[32m✅\x1b[0m Configured MCP for {}", p.name),
            Ok(false) => println!("  \x1b[33m⏭\x1b[0m  {} already configured", p.name),
            Err(e) => println!("  \x1b[31m❌\x1b[0m Failed to configure {}: {}", p.name, e),
        }
    }
    println!();

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

// ── Registry fetch ───────────────────────────────────────────────

/// Fetch registry directly from the repo.
fn fetch_registry() -> Result<Registry> {
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

// ── Banner ───────────────────────────────────────────────────────

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

// ── Platform selection ───────────────────────────────────────────

fn select_platforms<'a>(detected: &[&'a Platform]) -> Result<Vec<&'a Platform>> {
    if detected.is_empty() {
        println!("  Press Enter to continue...");
        let mut buf = String::new();
        io::stdin().read_line(&mut buf)?;
        return Ok(vec![]);
    }

    println!("  All detected platforms will be configured.");
    println!("  To exclude, type numbers to deselect (e.g. '1' or '0,2').");
    println!("  Press Enter to configure all.\n");

    for (i, p) in detected.iter().enumerate() {
        println!("    [\x1b[32m{i}\x1b[0m] {}", p.name);
    }

    print!("\n  deselect> ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();

    if input.is_empty() {
        return Ok(detected.to_vec());
    }

    let exclude: Vec<usize> = input
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();

    Ok(detected
        .iter()
        .enumerate()
        .filter(|(i, _)| !exclude.contains(i))
        .map(|(_, p)| *p)
        .collect())
}

// ── JSON helpers ─────────────────────────────────────────────────

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

fn remove_json_key(path: &Path, parent_key: &str, key: &str) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let content = std::fs::read_to_string(path)?;
    let clean = strip_jsonc_comments(&content);
    let mut root: serde_json::Value =
        serde_json::from_str(&clean).unwrap_or(serde_json::json!({}));

    let Some(parent) = root.get_mut(parent_key).and_then(|v| v.as_object_mut()) else {
        return Ok(false);
    };

    if parent.remove(key).is_some() {
        std::fs::write(path, serde_json::to_string_pretty(&root)? + "\n")?;
        return Ok(true);
    }
    Ok(false)
}

fn strip_jsonc_comments(input: &str) -> String {
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

// ── Daemon & service ─────────────────────────────────────────────

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
