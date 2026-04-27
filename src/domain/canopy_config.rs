//! Unified canopy configuration (`~/.canopy/config.toml`).

use serde::{Deserialize, Serialize};
use std::path::Path;

use super::cli_config::CliConfig;

/// Top-level canopy configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CanopyConfig {
    /// RFC 3339 timestamp of when setup was last completed.
    /// If `None`, setup has not been run yet.
    #[serde(default)]
    pub configured_at: Option<String>,

    /// Root directory for the MCP filesystem server.
    #[serde(default = "default_mcp_root")]
    pub mcp_filesystem_root: String,

    /// Available CLIs detected during setup.
    #[serde(default)]
    pub clis: Vec<CliConfig>,

    /// Temperature unit used by sysinfo widgets.
    #[serde(default)]
    pub temperature_unit: TemperatureUnit,
}

/// Preferred unit for temperature display.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TemperatureUnit {
    #[default]
    Celsius,
    Fahrenheit,
}

fn default_mcp_root() -> String {
    dirs::home_dir()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|| "/".to_string())
}

impl CanopyConfig {
    /// Load config from `~/.canopy/config.toml`. Returns default if not found.
    pub fn load(canopy_dir: &Path) -> Self {
        let config_path = canopy_dir.join("config.toml");
        std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|content| toml::from_str::<CanopyConfig>(&content).ok())
            .unwrap_or_default()
    }

    /// Save config to `~/.canopy/config.toml`.
    pub fn save(&self, canopy_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(canopy_dir)?;
        let content = toml::to_string_pretty(self).unwrap_or_default();
        std::fs::write(canopy_dir.join("config.toml"), content)
    }

    /// Whether setup has been completed.
    pub fn is_configured(&self) -> bool {
        self.configured_at.is_some()
    }

    /// Mark setup as completed (sets `configured_at` to now).
    pub fn mark_configured(&mut self) {
        self.configured_at = Some(chrono::Utc::now().to_rfc3339());
    }

    /// Get a CLI config by name.
    pub fn get_cli(&self, name: &str) -> Option<&CliConfig> {
        self.clis.iter().find(|c| c.name == name)
    }

    /// Get all available CLI names.
    pub fn cli_names(&self) -> Vec<&str> {
        self.clis.iter().map(|c| c.name.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_default_config() {
        let config = CanopyConfig::default();
        assert!(!config.is_configured());
        assert!(config.clis.is_empty());
        assert_eq!(config.temperature_unit, TemperatureUnit::Celsius);
    }

    #[test]
    fn test_save_and_load() {
        let dir = TempDir::new().unwrap();
        let canopy_dir = dir.path().join(".canopy");

        let mut config = CanopyConfig::default();
        config.mark_configured();
        config.mcp_filesystem_root = "/custom/path".to_string();
        config.temperature_unit = TemperatureUnit::Fahrenheit;

        config.save(&canopy_dir).unwrap();

        let loaded = CanopyConfig::load(&canopy_dir);
        assert!(loaded.is_configured());
        assert_eq!(loaded.mcp_filesystem_root, "/custom/path");
        assert_eq!(loaded.temperature_unit, TemperatureUnit::Fahrenheit);
    }

    #[test]
    fn test_load_missing_returns_default() {
        let dir = TempDir::new().unwrap();
        let canopy_dir = dir.path().join(".canopy");

        let config = CanopyConfig::load(&canopy_dir);
        assert!(!config.is_configured());
        assert!(config.clis.is_empty());
    }

    #[test]
    fn test_get_cli() {
        let mut config = CanopyConfig::default();
        config.clis.push(CliConfig {
            name: "opencode".to_string(),
            binary: "opencode".to_string(),
            ..Default::default()
        });

        assert!(config.get_cli("opencode").is_some());
        assert!(config.get_cli("nonexistent").is_none());
    }

    #[test]
    fn test_cli_names() {
        let mut config = CanopyConfig::default();
        config.clis.push(CliConfig {
            name: "opencode".to_string(),
            ..Default::default()
        });
        config.clis.push(CliConfig {
            name: "kiro".to_string(),
            ..Default::default()
        });

        let names = config.cli_names();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"opencode"));
        assert!(names.contains(&"kiro"));
    }
}
