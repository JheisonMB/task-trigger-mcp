//! Registry-driven CLI configuration.
//!
//! All CLI definitions come from the canopy registry (`platforms.json`).
//! During setup, available CLIs are detected and saved to `~/.canopy/cli_config.json`.
//! The executor uses this saved config to build commands dynamically --
//! no hard-coded strategies needed.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Complete CLI definition from the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliConfig {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub binary: String,
    #[serde(default)]
    pub headless_mode: String,
    #[serde(default)]
    pub model_flag: Option<String>,
    #[serde(default)]
    pub supports_working_dir: bool,
    #[serde(default)]
    pub working_dir_flag: Option<String>,
    #[serde(default)]
    pub env_vars: std::collections::HashMap<String, String>,
    /// Arguments to pass when launching in interactive (TUI) mode.
    #[serde(default)]
    pub interactive_args: Option<String>,
    /// RGB accent color for this CLI's agents in the TUI.
    #[serde(default)]
    pub accent_color: Option<[u8; 3]>,
}

/// Persisted CLI configuration for available CLIs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliRegistry {
    /// Version of the config format
    pub version: u32,
    /// Available CLIs detected during setup
    pub available_clis: Vec<CliConfig>,
}

impl CliConfig {
    /// Check if this CLI is available in PATH.
    pub fn is_available(&self) -> bool {
        which::which(&self.binary).is_ok()
    }
}

impl CliRegistry {
    /// Create a new registry with the current config version.
    pub fn new() -> Self {
        Self {
            version: 2,
            available_clis: Vec::new(),
        }
    }

    /// Detect which CLIs from a list are available in PATH.
    pub fn detect_available(platforms: &[crate::setup::PlatformWithCli]) -> Self {
        let mut registry = Self::new();

        for platform in platforms {
            if let Some(ref cli) = platform.cli {
                if cli.is_available() {
                    registry.available_clis.push(cli.clone());
                }
            }
        }

        registry
    }

    /// Save this configuration to a file.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, content)
    }

    /// Load configuration from a file.
    pub fn load(path: &Path) -> Option<Self> {
        let content = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Get a CLI config by name.
    pub fn get(&self, name: &str) -> Option<&CliConfig> {
        self.available_clis.iter().find(|c| c.name == name)
    }

    /// Get all available CLI names.
    #[allow(dead_code)]
    pub fn names(&self) -> Vec<&str> {
        self.available_clis
            .iter()
            .map(|c| c.name.as_str())
            .collect()
    }
}

impl Default for CliRegistry {
    fn default() -> Self {
        Self::new()
    }
}
