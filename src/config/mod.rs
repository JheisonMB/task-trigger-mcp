//! MCP Configuration Sync — Extract and synchronize MCP servers across platforms.
//!
//! This module provides the ability to:
//! 1. **Extract** all MCP server configurations from installed platforms
//! 2. **Compare** configurations across platforms
//! 3. **Sync** selected MCPs to target platforms
//!
//! The goal is to homologate MCP configurations so all your agents
//! have the same set of MCP servers configured.

pub mod skills;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// MCP Server configuration entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerEntry {
    /// Server name/key (e.g., "canopy", "github", "filesystem")
    pub name: String,
    /// Server configuration (varies by platform format)
    pub config: serde_json::Value,
    /// Whether this server is enabled
    pub enabled: bool,
}

/// Platform MCP configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformMcpConfig {
    /// Platform name (e.g., "kiro", "opencode", "copilot", "qwen")
    pub platform: String,
    /// Path to the config file
    pub config_path: String,
    /// All MCP servers configured for this platform
    pub servers: Vec<McpServerEntry>,
}

/// Registry of all platform MCP configs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfigRegistry {
    /// Version of the config format
    pub version: u32,
    /// All platform configurations
    pub platforms: Vec<PlatformMcpConfig>,
}

impl McpConfigRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            version: 1,
            platforms: Vec::new(),
        }
    }

    /// Extract MCP configs from a platform's config file.
    pub fn extract_from_platform(
        platform_name: &str,
        config_path: &Path,
        servers_key: &[String],
    ) -> Result<PlatformMcpConfig> {
        if !config_path.exists() {
            return Err(anyhow::anyhow!(
                "Config file not found: {}",
                config_path.display()
            ));
        }

        let content = std::fs::read_to_string(config_path)?;
        let clean = crate::setup::strip_jsonc_comments(&content);
        let root: serde_json::Value =
            serde_json::from_str(&clean).context("Failed to parse config file")?;

        let mut current = &root;
        for key in servers_key {
            current = current
                .get(key)
                .ok_or_else(|| anyhow::anyhow!("Key '{}' not found in config", key))?;
        }

        let servers = extract_servers_from_object(current);

        Ok(PlatformMcpConfig {
            platform: platform_name.to_string(),
            config_path: config_path.to_string_lossy().to_string(),
            servers,
        })
    }

    /// Extract all MCP configs from detected platforms.
    #[allow(dead_code)]
    pub fn extract_all(platforms: &[&crate::setup::Platform]) -> Result<Self> {
        let mut registry = Self::new();
        let home = dirs::home_dir().context("No home directory")?;

        for platform in platforms {
            let config_path = home.join(&platform.config_path);
            if !config_path.exists() {
                continue;
            }

            match Self::extract_from_platform(
                &platform.name,
                &home.join(&platform.config_path),
                &platform.mcp_servers_key,
            ) {
                Ok(platform_config) => {
                    registry.platforms.push(platform_config);
                }
                Err(e) => {
                    tracing::warn!("Failed to extract MCPs from {}: {}", platform.name, e);
                }
            }
        }

        Ok(registry)
    }

    /// Get all unique MCP server names across all platforms.
    #[allow(dead_code)]
    pub fn unique_server_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .platforms
            .iter()
            .flat_map(|p| p.servers.iter().map(|s| s.name.as_str()))
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Get servers that exist in one platform but not another.
    #[allow(dead_code)]
    pub fn server_diff(&self, from: &str, to: &str) -> Vec<&McpServerEntry> {
        let from_servers: Vec<&McpServerEntry> = self
            .platforms
            .iter()
            .find(|p| p.platform == from)
            .map(|p| p.servers.iter().collect::<Vec<_>>())
            .unwrap_or_default();

        let to_server_names: Vec<&str> = self
            .platforms
            .iter()
            .find(|p| p.platform == to)
            .map(|p| {
                p.servers
                    .iter()
                    .map(|s| s.name.as_str())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        from_servers
            .into_iter()
            .filter(|s| !to_server_names.contains(&s.name.as_str()))
            .collect()
    }

    /// Sync selected servers to target platforms.
    #[allow(dead_code)]
    pub fn sync_servers(
        &self,
        server_names: &[&str],
        target_platforms: &[&str],
    ) -> Result<Vec<String>> {
        let mut synced = Vec::new();

        for platform_name in target_platforms {
            let platform = self
                .platforms
                .iter()
                .find(|p| p.platform == *platform_name)
                .ok_or_else(|| {
                    anyhow::anyhow!("Platform '{}' not found in registry", platform_name)
                })?;

            for server_name in server_names {
                if platform.servers.iter().any(|s| s.name == *server_name) {
                    synced.push(format!("{}.{}", platform_name, server_name));
                }
            }
        }

        Ok(synced)
    }
}

impl Default for McpConfigRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn extract_servers_from_object(servers_object: &serde_json::Value) -> Vec<McpServerEntry> {
    let mut servers = Vec::new();

    if let Some(obj) = servers_object.as_object() {
        for (name, config) in obj {
            let enabled = config
                .get("disabled")
                .and_then(|v| v.as_bool())
                .map(|d| !d)
                .or_else(|| config.get("enabled").and_then(|v| v.as_bool()))
                .unwrap_or(true);

            servers.push(McpServerEntry {
                name: name.clone(),
                config: config.clone(),
                enabled,
            });
        }
    }

    servers
}

/// Get the `mcp_servers_key` path for a platform from the registry.
#[allow(dead_code)]
pub fn get_mcp_servers_key_for_platform(platform: &crate::setup::Platform) -> &[String] {
    &platform.mcp_servers_key
}
