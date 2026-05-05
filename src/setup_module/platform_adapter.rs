use crate::setup_module::config_manip::{
    remove_json_key, upsert_json_key, upsert_toml_array, upsert_toml_key,
};
use crate::setup_module::models::{
    load_mcp_fs_root, resolve_config_path, save_mcp_fs_root, CanonicalServers, Platform,
};
use anyhow::Result;
use std::io::{self, Write};
use std::path::Path;

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

    let mut adapted = apply_command_format(obj, &platform.command_format);

    // Step 2: apply field rename mapping
    let mut renamed = serde_json::Map::new();
    for (k, v) in adapted {
        let target_key = platform.fields_mapping.get(&k).cloned().unwrap_or(k);
        renamed.insert(target_key, v);
    }
    adapted = renamed;

    // Step 3: inject required fields
    let type_idx = infer_server_type_index(&adapted);
    for (field, allowed) in &platform.required_fields {
        if let Some(idx) = type_idx {
            if let Some(value) = allowed.get(idx) {
                adapted.insert(field.clone(), serde_json::Value::String(value.clone()));
            }
        } else if !adapted.contains_key(field) {
            if let Some(value) = allowed.first() {
                adapted.insert(field.clone(), serde_json::Value::String(value.clone()));
            }
        }
    }

    // Step 4: merge server_extras
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

/// Build the initial field map applying the platform's command_format rule.
fn apply_command_format(
    obj: &serde_json::Map<String, serde_json::Value>,
    command_format: &str,
) -> serde_json::Map<String, serde_json::Value> {
    let mut adapted = serde_json::Map::new();

    if command_format != "merged" {
        for (k, v) in obj {
            adapted.insert(k.clone(), v.clone());
        }
        return adapted;
    }

    let has_command = obj.get("command").and_then(|v| v.as_str()).is_some();
    let has_args = obj.contains_key("args");

    if has_command && has_args {
        let cmd = obj["command"].as_str().unwrap();
        let mut merged = vec![serde_json::Value::String(cmd.to_string())];
        if let Some(args) = obj["args"].as_array() {
            merged.extend(args.iter().cloned());
        }
        adapted.insert("command".to_string(), serde_json::Value::Array(merged));
        for (k, v) in obj {
            if k != "command" && k != "args" {
                adapted.insert(k.clone(), v.clone());
            }
        }
    } else {
        for (k, v) in obj {
            adapted.insert(k.clone(), v.clone());
        }
    }

    adapted
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
pub(crate) fn browse_directory(start_dir: &str) -> String {
    use ratatui::crossterm::event::{read, Event, KeyEventKind};
    use ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode};

    let mut current = std::path::PathBuf::from(start_dir);
    if !current.is_dir() {
        current = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
    }

    let mut cursor: usize = 0;
    let visible: usize = 10;
    let total_rows = 2 + visible + 1;

    let _ = enable_raw_mode();
    for _ in 0..total_rows {
        print!("\r\n");
    }

    loop {
        let subdirs = browse_list_subdirs(&current);
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

        print!("\x1b[{total_rows}A");
        print!("\r\x1b[2K  \x1b[36m»\x1b[0m  {}\r\n", current.display());
        print!(
            "\r\x1b[2K  \x1b[90m↑↓ navigate  → enter  ← back  Enter confirm  Esc cancel\x1b[0m\r\n"
        );

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
            for _ in drawn..visible {
                print!("\r\x1b[2K\r\n");
            }
        }

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
            Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => {
                match handle_browse_key(k.code, &subdirs, &mut cursor, &mut current) {
                    BrowseAction::Confirm => {
                        let _ = disable_raw_mode();
                        print!("\r\n");
                        let _ = io::stdout().flush();
                        return current.to_string_lossy().to_string();
                    }
                    BrowseAction::Cancel => {
                        let _ = disable_raw_mode();
                        print!("\r\n");
                        let _ = io::stdout().flush();
                        return start_dir.to_string();
                    }
                    BrowseAction::Continue => {}
                }
            }
            _ => {}
        }
    }
}

enum BrowseAction {
    Confirm,
    Cancel,
    Continue,
}

fn handle_browse_key(
    code: ratatui::crossterm::event::KeyCode,
    subdirs: &[String],
    cursor: &mut usize,
    current: &mut std::path::PathBuf,
) -> BrowseAction {
    use ratatui::crossterm::event::KeyCode;
    match code {
        KeyCode::Enter => BrowseAction::Confirm,
        KeyCode::Esc => BrowseAction::Cancel,
        KeyCode::Up => {
            *cursor = cursor.saturating_sub(1);
            BrowseAction::Continue
        }
        KeyCode::Down if !subdirs.is_empty() && *cursor + 1 < subdirs.len() => {
            *cursor += 1;
            BrowseAction::Continue
        }
        KeyCode::Right | KeyCode::Char('l') => {
            if let Some(name) = subdirs.get(*cursor) {
                *current = current.join(name);
                *cursor = 0;
            }
            BrowseAction::Continue
        }
        KeyCode::Left | KeyCode::Char('h') => {
            if let Some(parent) = current.parent() {
                *current = parent.to_path_buf();
                *cursor = 0;
            }
            BrowseAction::Continue
        }
        _ => BrowseAction::Continue,
    }
}

fn browse_list_subdirs(path: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut dirs: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().map(|t| t.is_dir()).unwrap_or(false)
                || (e.file_type().map(|t| t.is_symlink()).unwrap_or(false) && e.path().is_dir())
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

/// Extract all MCP server configs from the selected platforms.
pub(crate) fn extract_all_mcp_configs(
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

pub(crate) fn print_mcp_matrix(all_configs: &[crate::config::PlatformMcpConfig]) {
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

pub(crate) fn clear_wizard_screen() -> Result<()> {
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
pub(crate) fn run_install_our_servers(
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
