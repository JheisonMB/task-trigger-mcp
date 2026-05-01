use serde::Deserialize;
use std::path::Path;

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
    pub(crate) fn to_platform_with_cli(&self) -> PlatformWithCli {
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
pub(crate) fn resolve_config_path(home: &Path, config_path: &str) -> std::path::PathBuf {
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
pub(crate) fn load_mcp_fs_root(home: &Path) -> String {
    let canopy_dir = home.join(".canopy");
    let config = crate::domain::canopy_config::CanopyConfig::load(&canopy_dir);
    config.mcp_filesystem_root
}

/// Persist the chosen filesystem root path for reuse across setups and updates.
pub(crate) fn save_mcp_fs_root(home: &Path, root: &str) {
    let canopy_dir = home.join(".canopy");
    let mut config = crate::domain::canopy_config::CanopyConfig::load(&canopy_dir);
    config.mcp_filesystem_root = root.to_string();
    let _ = config.save(&canopy_dir);
}
pub(crate) fn is_binary_available(binary: &str) -> bool {
    which::which(binary).is_ok()
}

/// Ensure MCP runtime dependencies (npx, uvx) are available.
/// Attempts to install missing ones automatically.
/// Returns a summary message for the wizard.
pub(crate) fn ensure_mcp_dependencies() -> String {
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
pub(crate) fn ensure_mcp_dependencies_silent() -> bool {
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
