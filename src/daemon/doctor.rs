use anyhow::{Context, Result};

use crate::application::ports::AgentRepository;
use crate::db::Database;
use crate::daemon::process::is_process_running;

pub(crate) async fn run_doctor() -> Result<()> {
    const DOCTOR_BANNER: &str = r#"
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

    println!("\x1b[32m{DOCTOR_BANNER}\x1b[0m");
    println!("  \x1b[1mcanopy doctor\x1b[0m");
    println!("  ─────────────────────────────────────────────\n");

    let home = dirs::home_dir().context("No home directory")?;
    let canopy_dir = home.join(".canopy");
    let db_path = canopy_dir.join("background_agents.db");
    let cli_config_path = canopy_dir.join("cli_config.json");
    let configured_marker = canopy_dir.join(".configured");

    let mut issues = Vec::new();

    if canopy_dir.exists() {
        println!(
            "  \x1b[32m✓\x1b[0m Data directory: {}",
            canopy_dir.display()
        );
    } else {
        println!(
            "  \x1b[31m✗\x1b[0m Data directory not found: {}",
            canopy_dir.display()
        );
        issues.push("Run 'canopy setup' to initialize");
    }

    if db_path.exists() {
        println!("  \x1b[32m✓\x1b[0m Database: {}", db_path.display());
        if let Ok(db) = Database::new(&db_path) {
            if let Ok(agents) = db.list_agents() {
                let cron_count = agents.iter().filter(|a| a.is_cron()).count();
                let watch_count = agents.iter().filter(|a| a.is_watch()).count();
                println!("    Agents: {} (cron: {}, watch: {})", agents.len(), cron_count, watch_count);
            }
        }
    } else {
        println!("  \x1b[33m⚠\x1b[0m  Database not found (will be created on setup)");
    }

    if cli_config_path.exists() {
        println!(
            "  \x1b[32m✓\x1b[0m CLI config: {}",
            cli_config_path.display()
        );
        if let Some(registry) = crate::domain::cli_config::CliRegistry::load(&cli_config_path) {
            println!("    Available CLIs: {}", registry.names().join(", "));
        }
    } else {
        println!("  \x1b[33m⚠\x1b[0m  CLI config not found (run setup)");
    }

    let pid_path = canopy_dir.join("daemon.pid");
    if let Ok(pid_str) = std::fs::read_to_string(&pid_path) {
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            if is_process_running(pid) {
                println!("  \x1b[32m✓\x1b[0m Daemon running (PID: {})", pid);
            } else {
                println!("  \x1b[31m✗\x1b[0m Daemon not running (stale PID: {})", pid);
                issues.push("Stale PID file — run 'canopy daemon start'");
            }
        }
    } else {
        println!("  \x1b[33m⚠\x1b[0m  Daemon not running");
    }

    if configured_marker.exists() {
        println!("  \x1b[32m✓\x1b[0m Setup completed");
    } else {
        println!("  \x1b[33m⚠\x1b[0m  Setup not completed");
        issues.push("Run 'canopy setup'");
    }

    let available_clis = crate::domain::models::Cli::detect_available();
    if !available_clis.is_empty() {
        println!(
            "  \x1b[32m✓\x1b[0m CLIs in PATH: {}",
            available_clis
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    } else {
        println!("  \x1b[31m✗\x1b[0m No supported CLIs found in PATH");
        issues.push("Install at least one: opencode, kiro-cli, copilot, or qwen");
    }

    if !issues.is_empty() {
        println!("\n  \x1b[1;33m⚠ Suggestions:\x1b[0m");
        for issue in &issues {
            println!("    • {}", issue);
        }
    } else {
        println!("\n  \x1b[32m✅ All checks passed!\x1b[0m");
    }
    println!();

    Ok(())
}