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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_strategy() -> CliStrategy {
        let mut env_vars = HashMap::new();
        env_vars.insert("FOO".to_string(), "bar".to_string());

        CliStrategy {
            binary: "test-cli".to_string(),
            headless_mode: "--headless --quiet".to_string(),
            model_flag: Some("--model".to_string()),
            supports_working_dir: true,
            working_dir_flag: Some("--workdir".to_string()),
            env_vars,
        }
    }

    #[test]
    fn test_build_command_basic() {
        let strategy = sample_strategy();
        let cmd = strategy.build_command("test prompt", None, None);

        let cmd_str = format!("{:?}", cmd);
        assert!(cmd_str.contains("test-cli"));
    }

    #[test]
    fn test_build_command_with_model() {
        let strategy = sample_strategy();
        let cmd = strategy.build_command("test prompt", Some("gpt-4"), None);

        let cmd_str = format!("{:?}", cmd);
        assert!(cmd_str.contains("--model"));
        assert!(cmd_str.contains("gpt-4"));
    }

    #[test]
    fn test_build_command_with_working_dir() {
        let strategy = sample_strategy();
        let cmd = strategy.build_command("test prompt", None, Some("/tmp/project"));

        let cmd_str = format!("{:?}", cmd);
        assert!(cmd_str.contains("--workdir"));
        assert!(cmd_str.contains("/tmp/project"));
    }

    #[test]
    fn test_build_command_no_working_dir_when_not_supported() {
        let mut strategy = sample_strategy();
        strategy.supports_working_dir = false;

        let cmd = strategy.build_command("test prompt", None, Some("/tmp/project"));

        let cmd_str = format!("{:?}", cmd);
        assert!(!cmd_str.contains("--workdir"));
    }

    #[test]
    fn test_build_command_no_model_flag() {
        let mut strategy = sample_strategy();
        strategy.model_flag = None;

        let cmd = strategy.build_command("test prompt", Some("gpt-4"), None);

        let cmd_str = format!("{:?}", cmd);
        assert!(!cmd_str.contains("--model"));
    }

    #[test]
    fn test_build_command_empty_headless_mode() {
        let mut strategy = sample_strategy();
        strategy.headless_mode = String::new();

        let cmd = strategy.build_command("test prompt", None, None);

        let cmd_str = format!("{:?}", cmd);
        assert!(cmd_str.contains("test-cli"));
    }

    #[test]
    fn test_build_command_all_options() {
        let strategy = sample_strategy();
        let cmd = strategy.build_command("my prompt", Some("claude-3"), Some("/home/project"));

        let cmd_str = format!("{:?}", cmd);
        assert!(cmd_str.contains("my prompt"));
        assert!(cmd_str.contains("--model"));
        assert!(cmd_str.contains("claude-3"));
        assert!(cmd_str.contains("--workdir"));
        assert!(cmd_str.contains("/home/project"));
    }
}
