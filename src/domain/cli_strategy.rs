//! CLI execution strategies.
//!
//! Each CLI has its own strategy for building command arguments.

use tokio::process::Command;

/// Strategy for building CLI commands.
pub trait CliStrategy {
    /// Build the command with appropriate arguments.
    fn build_command(
        &self,
        cmd: &mut Command,
        prompt: &str,
        model: Option<&str>,
        working_dir: Option<&str>,
    );
}

/// `OpenCode` CLI strategy.
pub struct OpenCodeStrategy;

impl CliStrategy for OpenCodeStrategy {
    fn build_command(
        &self,
        cmd: &mut Command,
        prompt: &str,
        model: Option<&str>,
        working_dir: Option<&str>,
    ) {
        cmd.arg("run").arg(prompt);
        if let Some(m) = model {
            cmd.arg("-m").arg(m);
        }
        if let Some(dir) = working_dir {
            cmd.arg("--dir").arg(dir);
        }
    }
}

/// Kiro CLI strategy.
pub struct KiroStrategy;

impl CliStrategy for KiroStrategy {
    fn build_command(
        &self,
        cmd: &mut Command,
        prompt: &str,
        model: Option<&str>,
        _working_dir: Option<&str>,
    ) {
        cmd.arg("chat")
            .arg("--no-interactive")
            .arg("--trust-all-tools")
            .arg(prompt);
        if let Some(m) = model {
            cmd.arg("--model").arg(m);
        }
    }
}

/// GitHub `Copilot` CLI strategy.
pub struct CopilotStrategy;

impl CliStrategy for CopilotStrategy {
    fn build_command(
        &self,
        cmd: &mut Command,
        prompt: &str,
        model: Option<&str>,
        _working_dir: Option<&str>,
    ) {
        cmd.arg("-p").arg(prompt).arg("--allow-all-tools");
        if let Some(m) = model {
            cmd.arg("--model").arg(m);
        }
    }
}
