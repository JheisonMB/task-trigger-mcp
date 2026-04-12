//! Dynamic CLI execution strategy.
//!
//! All CLI definitions come from the registry (platforms.json).
//! Commands are built dynamically based on the saved configuration.

use std::collections::HashMap;
use tokio::process::Command;

/// Strategy for building CLI commands from registry config.
pub struct CliStrategy {
    pub binary: String,
    pub headless_mode: String,
    pub model_flag: Option<String>,
    pub supports_working_dir: bool,
    pub working_dir_flag: Option<String>,
    pub env_vars: HashMap<String, String>,
}

impl CliStrategy {
    /// Build a command using the registry-defined configuration.
    pub fn build_command(
        &self,
        prompt: &str,
        model: Option<&str>,
        working_dir: Option<&str>,
    ) -> Command {
        let mut cmd = Command::new(&self.binary);

        // Set environment variables
        for (key, value) in &self.env_vars {
            cmd.env(key, value);
        }

        // Add headless mode flags (before prompt)
        for arg in shell_words::split(&self.headless_mode).unwrap_or_default() {
            cmd.arg(arg);
        }

        // Add prompt
        cmd.arg(prompt);

        // Add model if specified
        if let Some(m) = model {
            if let Some(ref flag) = self.model_flag {
                cmd.arg(flag).arg(m);
            }
        }

        // Add working directory if supported
        if self.supports_working_dir {
            if let Some(dir) = working_dir {
                if let Some(ref flag) = self.working_dir_flag {
                    cmd.arg(flag).arg(dir);
                }
            }
        }

        cmd
    }
}
