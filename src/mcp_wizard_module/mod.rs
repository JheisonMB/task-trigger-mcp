//! Interactive MCP management wizard — `canopy mcp`.
//!
//! Provides three operations:
//!  - **Sync**: scan all detected platforms and replicate MCPs found in any of
//!    them across every other platform (format-converted).
//!  - **Add**: prompt for Name / URL / Type and write the entry to every
//!    detected platform simultaneously.
//!  - **Remove**: show a unified server list and remove the chosen entry from
//!    every platform it appears in.

use anyhow::{Context, Result};
use inquire::{Select, Text};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::setup_module::{self as setup, Platform};

// ── Public entry point ─────────────────────────────────────────────────────

/// Run the interactive `canopy mcp` wizard.
pub fn run_mcp_wizard() -> Result<()> {
    let home = dirs::home_dir().context("No home directory")?;

    print!("\x1b[2J\x1b[H");
    io::stdout().flush()?;
    print_mcp_banner();

    // Fetch registry (uses the same cached logic as setup)
    print!("  Fetching platform registry… ");
    io::stdout().flush()?;
    let registry = setup::fetch_registry_raw().context("Failed to fetch registry")?;
    println!("\x1b[32m✓\x1b[0m");

    let detected: Vec<&Platform> = registry
        .platforms
        .iter()
        .filter(|p| setup::is_platform_available(p))
        .collect();

    if detected.is_empty() {
        println!(
            "  \x1b[33m⚠\x1b[0m  No supported platforms detected ({}). Install one and re-run.",
            registry
                .platforms
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        return Ok(());
    }

    println!(
        "  Platforms detected: \x1b[32m{}\x1b[0m",
        detected
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!();

    // Show current MCP matrix
    let pre_configs = collect_all_platform_configs(&home, &detected);
    print_mcp_table(&detected, &pre_configs);

    let action = Select::new(
        "What would you like to do?",
        vec![
            "Sync — replicate MCPs across all platforms",
            "Add — register a new MCP server everywhere",
            "Remove — delete an MCP server from all platforms",
        ],
    )
    .with_help_message("↑↓ navigate | Enter select | Esc cancel")
    .prompt()
    .map_err(|e| anyhow::anyhow!("Cancelled: {}", e))?;

    match action {
        a if a.starts_with("Sync") => run_sync(&home, &detected),
        a if a.starts_with("Add") => run_add(&home, &detected),
        a if a.starts_with("Remove") => run_remove(&home, &detected),
        _ => Ok(()),
    }
}

// ── Sync ───────────────────────────────────────────────────────────────────

/// Collect all MCP servers from every platform then replicate each missing one
/// to every platform that does not yet have it.
fn run_sync(home: &Path, detected: &[&Platform]) -> Result<()> {
    println!();
    println!("  \x1b[1mGlobal MCP Sync\x1b[0m");
    println!("  ─────────────────────────────────────────────");
    println!("  Scanning platform configs…");

    let all_configs = collect_all_platform_configs(home, detected);

    // Build a unified server map: name → (config_json, source_platform)
    let mut unified: BTreeMap<String, (serde_json::Value, &str)> = BTreeMap::new();
    for (platform, servers) in &all_configs {
        for (name, config) in servers {
            unified
                .entry(name.clone())
                .or_insert_with(|| (config.clone(), platform.as_str()));
        }
    }

    if unified.is_empty() {
        println!("  \x1b[33m⚠\x1b[0m  No MCP servers found in any platform config.");
        return Ok(());
    }

    println!();
    print_mcp_table(detected, &all_configs);

    // Show which servers are missing where
    let mut missing: Vec<(String, String)> = Vec::new(); // (platform, server)
    for p in detected {
        let existing: BTreeSet<String> = all_configs
            .get(&p.name)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        for name in unified.keys() {
            if !existing.contains(name) {
                missing.push((p.name.clone(), name.clone()));
            }
        }
    }

    if missing.is_empty() {
        println!("  \x1b[32m✓\x1b[0m All platforms already in sync.");
        return Ok(());
    }

    let missing_count = missing.len();
    println!("  \x1b[33m{missing_count}\x1b[0m server(s) to replicate. Proceed? [Y/n] ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    if input.trim().to_lowercase() == "n" {
        println!("  Cancelled.");
        return Ok(());
    }

    // Apply
    let mut applied = 0usize;
    let mut errors = 0usize;
    for p in detected {
        let config_path = resolve_platform_config_path(home, p);
        let existing: BTreeSet<String> = all_configs
            .get(&p.name)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        for (server_name, (config, _source)) in &unified {
            if existing.contains(server_name) {
                continue;
            }
            // Adapt config to the target platform's field names
            let adapted = setup::adapt_config(config, p, server_name);
            match apply_server_to_platform(p, &config_path, server_name, &adapted) {
                Ok(_) => applied += 1,
                Err(e) => {
                    eprintln!("    \x1b[31m✗\x1b[0m {}/{}: {}", p.name, server_name, e);
                    errors += 1;
                }
            }
        }
    }

    println!();
    if errors == 0 {
        println!("  \x1b[32m✓\x1b[0m Sync complete — {applied} server(s) replicated.");
    } else {
        println!("  \x1b[33m⚠\x1b[0m Sync partial — {applied} synced, {errors} failed.");
    }

    // Show updated matrix
    println!();
    let post_configs = collect_all_platform_configs(home, detected);
    print_mcp_table(detected, &post_configs);

    Ok(())
}

// ── Add ────────────────────────────────────────────────────────────────────

/// Interactively collect Name / URL / Type and inject into every detected platform.
fn run_add(home: &Path, detected: &[&Platform]) -> Result<()> {
    println!();
    println!("  \x1b[1mAdd MCP Server\x1b[0m");
    println!("  ─────────────────────────────────────────────");

    let name = Text::new("Server name (e.g. \"github\"):")
        .prompt()
        .map_err(|e| anyhow::anyhow!("Cancelled: {}", e))?;
    let name = name.trim().to_string();
    if name.is_empty() {
        println!("  \x1b[31m✗\x1b[0m Server name is required.");
        return Ok(());
    }

    let url = Text::new("Server URL (e.g. \"https://example.com/mcp\"):")
        .prompt()
        .map_err(|e| anyhow::anyhow!("Cancelled: {}", e))?;
    let url = url.trim().to_string();

    let server_type = Select::new("Server type:", vec!["http", "remote", "stdio"])
        .prompt()
        .map_err(|e| anyhow::anyhow!("Cancelled: {}", e))?;

    // Build a generic Canopy-standard config object
    let config = serde_json::json!({
        "type": server_type,
        "url": url,
    });

    println!();
    println!("  Installing \x1b[1m{name}\x1b[0m ({server_type}) on all platforms…");

    let mut ok = 0usize;
    let mut fail = 0usize;
    for p in detected {
        let config_path = resolve_platform_config_path(home, p);
        let adapted = setup::adapt_config(&config, p, &name);
        match apply_server_to_platform(p, &config_path, &name, &adapted) {
            Ok(_) => {
                println!("    \x1b[32m✓\x1b[0m {}", p.name);
                ok += 1;
            }
            Err(e) => {
                println!("    \x1b[31m✗\x1b[0m {}: {e}", p.name);
                fail += 1;
            }
        }
    }

    println!();
    if fail == 0 {
        println!("  \x1b[32m✓\x1b[0m '{name}' added to {ok} platform(s).");
    } else {
        println!("  \x1b[33m⚠\x1b[0m '{name}' added to {ok}, failed on {fail} platform(s).");
    }

    // Show updated matrix
    println!();
    let post_configs = collect_all_platform_configs(home, detected);
    print_mcp_table(detected, &post_configs);

    Ok(())
}

// ── Remove ─────────────────────────────────────────────────────────────────

/// Show every unique MCP server found in any platform and remove the chosen
/// one from every platform where it exists.
fn run_remove(home: &Path, detected: &[&Platform]) -> Result<()> {
    println!();
    println!("  \x1b[1mRemove MCP Server\x1b[0m");
    println!("  ─────────────────────────────────────────────");

    let all_configs = collect_all_platform_configs(home, detected);

    // Collect unique server names
    let all_names: BTreeSet<String> = all_configs
        .values()
        .flat_map(|m| m.keys().cloned())
        .collect();

    if all_names.is_empty() {
        println!("  \x1b[33m⚠\x1b[0m  No MCP servers found.");
        return Ok(());
    }

    let choices: Vec<String> = all_names.into_iter().collect();
    let selected = Select::new("Select server to remove:", choices)
        .with_help_message("Enter to confirm | Esc to cancel")
        .prompt()
        .map_err(|e| anyhow::anyhow!("Cancelled: {}", e))?;

    let target_platforms: Vec<&&Platform> = detected
        .iter()
        .filter(|p| {
            all_configs
                .get(&p.name)
                .map(|m| m.contains_key(&selected))
                .unwrap_or(false)
        })
        .collect();

    println!();
    println!(
        "  Will remove \x1b[1m{selected}\x1b[0m from: {}",
        target_platforms
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    print!("  Proceed? [Y/n] ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    if input.trim().to_lowercase() == "n" {
        println!("  Cancelled.");
        return Ok(());
    }

    let mut ok = 0usize;
    let mut fail = 0usize;
    for p in &target_platforms {
        let config_path = resolve_platform_config_path(home, p);
        match remove_server_from_platform(p, &config_path, &selected) {
            Ok(true) => {
                println!("    \x1b[32m✓\x1b[0m {}", p.name);
                ok += 1;
            }
            Ok(false) => {
                println!("    \x1b[33m–\x1b[0m {} (not found, skipped)", p.name);
            }
            Err(e) => {
                println!("    \x1b[31m✗\x1b[0m {}: {e}", p.name);
                fail += 1;
            }
        }
    }

    println!();
    if fail == 0 {
        println!("  \x1b[32m✓\x1b[0m '{selected}' removed from {ok} platform(s).");
    } else {
        println!("  \x1b[33m⚠\x1b[0m Removed from {ok}, failed on {fail}.");
    }

    // Show updated matrix
    println!();
    let post_configs = collect_all_platform_configs(home, detected);
    print_mcp_table(detected, &post_configs);

    Ok(())
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Collect servers from each platform: platform_name → {server_name → config}.
fn collect_all_platform_configs(
    home: &Path,
    detected: &[&Platform],
) -> BTreeMap<String, BTreeMap<String, serde_json::Value>> {
    let mut result = BTreeMap::new();
    for p in detected {
        let config_path = resolve_platform_config_path(home, p);
        if !config_path.exists() {
            result.insert(p.name.clone(), BTreeMap::new());
            continue;
        }
        match crate::config::McpConfigRegistry::extract_from_platform(
            &p.name,
            &config_path,
            &p.mcp_servers_key,
        ) {
            Ok(cfg) => {
                let map: BTreeMap<String, serde_json::Value> = cfg
                    .servers
                    .into_iter()
                    .map(|s| (s.name, s.config))
                    .collect();
                result.insert(p.name.clone(), map);
            }
            Err(e) => {
                eprintln!(
                    "  \x1b[33m⚠\x1b[0m Warning: could not read {} config: {}",
                    p.name, e
                );
                result.insert(p.name.clone(), BTreeMap::new());
            }
        }
    }
    result
}

/// Resolve the config file path for a platform (mirrors `setup::resolve_config_path`).
fn resolve_platform_config_path(home: &Path, p: &Platform) -> PathBuf {
    let primary = home.join(&p.config_path);
    if primary.exists() {
        return primary;
    }
    let ext = primary.extension().and_then(|e| e.to_str()).unwrap_or("");
    let alternate = match ext {
        "jsonc" => primary.with_extension("json"),
        "json" => primary.with_extension("jsonc"),
        _ => return primary,
    };
    if alternate.exists() {
        alternate
    } else {
        primary
    }
}

/// Write a server config entry to a platform's config file.
fn apply_server_to_platform(
    platform: &Platform,
    config_path: &Path,
    server_name: &str,
    config: &serde_json::Value,
) -> Result<bool> {
    // Ensure the config file exists
    if !config_path.exists() {
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let is_toml = platform.config_format.as_deref() == Some("toml");
        let initial = if is_toml {
            String::new()
        } else {
            format!(
                "{{\"{}\": {{}}}}\n",
                platform
                    .mcp_servers_key
                    .first()
                    .unwrap_or(&"mcpServers".to_string())
            )
        };
        std::fs::write(config_path, initial)?;
    }

    let is_toml = platform.config_format.as_deref() == Some("toml");
    if is_toml {
        if platform.toml_array_format {
            setup::upsert_toml_array_pub(
                config_path,
                &platform.mcp_servers_key.join("."),
                server_name,
                config,
            )
        } else {
            setup::upsert_toml_key_pub(
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
        setup::upsert_json_key_pub(config_path, &key_refs, config)
    }
}

/// Remove a server entry from a platform config file.
fn remove_server_from_platform(
    platform: &Platform,
    config_path: &Path,
    server_name: &str,
) -> Result<bool> {
    if !config_path.exists() {
        return Ok(false);
    }

    let is_toml = platform.config_format.as_deref() == Some("toml");
    if is_toml {
        setup::remove_toml_server_pub(platform, config_path, server_name)
    } else {
        let parent_key = platform
            .mcp_servers_key
            .first()
            .cloned()
            .unwrap_or_else(|| "mcpServers".to_string());
        setup::remove_json_key_pub(config_path, &parent_key, server_name)
    }
}

// ── Banner ─────────────────────────────────────────────────────────────────

use crate::shared::banner;

fn print_mcp_banner() {
    banner::print_banner_with_gradient("Agent Hub — MCP Manager");
    // Removed duplicate line - banner function already prints the separator line
}

// ── Matrix table ───────────────────────────────────────────────────────────

/// Print a table: platforms as columns, MCP servers as rows, ✓/✗ cells.
fn print_mcp_table(
    detected: &[&Platform],
    all_configs: &BTreeMap<String, BTreeMap<String, serde_json::Value>>,
) {
    // Gather all unique server names
    let all_servers: BTreeSet<String> = all_configs
        .values()
        .flat_map(|m| m.keys().cloned())
        .collect();

    if all_servers.is_empty() {
        println!("  \x1b[90mNo MCP servers configured.\x1b[0m");
        println!();
        return;
    }

    // Column widths
    let name_col = all_servers
        .iter()
        .map(|s| s.len())
        .max()
        .unwrap_or(6)
        .max(6);
    let plat_col = detected
        .iter()
        .map(|p| p.name.len())
        .max()
        .unwrap_or(4)
        .max(4);

    // Header row
    print!("  {:<name_col$}", "Server");
    for p in detected {
        print!("  {:>plat_col$}", p.name);
    }
    println!();

    // Separator
    let total_w = name_col + detected.len() * (plat_col + 2);
    println!("  {:─<total_w$}", "");

    // Data rows
    for server in &all_servers {
        print!("  {:<name_col$}", server);
        for p in detected {
            let has = all_configs
                .get(&p.name)
                .map(|m| m.contains_key(server))
                .unwrap_or(false);
            let icon = if has {
                "\x1b[32m ✓\x1b[0m"
            } else {
                "\x1b[31m ✗\x1b[0m"
            };
            // pad before icon (ANSI codes break format width)
            let pad = plat_col.saturating_sub(1);
            print!("  {}{}", " ".repeat(pad), icon);
        }
        println!();
    }
    println!();
}
