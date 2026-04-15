#![allow(clippy::doc_markdown)]
//! canopy — MCP server for AI agent background_agent scheduling and file watching.
//!
//! Binary modes:
//! - `daemon start` — start the MCP server as a persistent background process
//! - `daemon stop` — stop the running daemon
//! - `daemon status` — check daemon health
//! - `stdio` — run in stdio MCP transport mode (legacy/fallback)
//! - (no args) — start in foreground with Streamable HTTP transport

mod application;
mod config;
mod daemon;
mod db;
mod domain;
mod executor;
mod scheduler;
pub(crate) mod service_install;
mod setup;
mod tui;
mod watchers;

use anyhow::Result;
use clap::{Parser, Subcommand};
use rmcp::ServiceExt;
use std::sync::Arc;

use application::ports::{BackgroundAgentRepository, StateRepository, WatcherRepository};
use daemon::TaskTriggerHandler;
use db::Database;
use executor::Executor;
use scheduler::cron_scheduler::CronScheduler;
use watchers::WatcherEngine;

/// canopy: A self-contained MCP server for AI agent background_agent scheduling.
#[derive(Parser)]
#[command(name = "canopy", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Port for Streamable HTTP server (overrides `CANOPY_PORT` env var).
    #[arg(long, short, global = true)]
    port: Option<u16>,
}

#[derive(Subcommand)]
enum Commands {
    /// Daemon management (start, stop, status, restart).
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
    /// Sync MCP configurations across platforms (extract, compare, apply).
    Sync {
        #[command(subcommand)]
        action: SyncAction,
    },
    /// Diagnose environment and canopy health.
    Doctor,
    /// Run in stdio MCP transport mode (legacy/fallback for clients without SSE).
    Stdio,
    /// Launch the Agent Hub TUI.
    Tui,
    /// Run the setup wizard (configure MCP, start daemon, install service).
    Setup,
    /// Start the MCP server in foreground (used internally by daemon start).
    #[command(hide = true)]
    Serve,
}

#[derive(Subcommand)]
enum SyncAction {
    /// Extract MCP configurations from all platforms.
    Extract,
    /// Compare MCP configurations across platforms.
    Compare,
    /// Sync selected MCPs to target platforms.
    Apply,
}

#[derive(Subcommand)]
enum DaemonAction {
    /// Start daemon in background (auto-installs service for persistence).
    Start,
    /// Stop the running daemon.
    Stop,
    /// Check daemon status.
    Status,
    /// Restart the daemon (stop + start).
    Restart,
    /// Tail daemon logs.
    Logs,
    /// Install (or re-enable) the system service so the daemon starts on boot.
    InstallService,
    /// Remove the system service.
    UninstallService,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Daemon { action }) => handle_daemon_action(action, cli.port).await,
        Some(Commands::Sync { action }) => handle_sync_action(action).await,
        Some(Commands::Doctor) => handle_doctor().await,
        Some(Commands::Stdio) => handle_stdio().await,
        Some(Commands::Serve) => handle_http_server(cli.port).await,
        Some(Commands::Tui) => {
            // Auto-setup if not configured
            if setup::needs_setup() {
                setup::run_setup_silent()?;
            }
            // Background daily registry refresh
            setup::maybe_refresh_registry();
            tui::run_tui()?;
            Ok(())
        }
        Some(Commands::Setup) => {
            setup::run_setup()?;
            Ok(())
        }
        None => {
            // Auto-setup if not configured
            if setup::needs_setup() {
                setup::run_setup_silent()?;
            }
            // Background daily registry refresh
            setup::maybe_refresh_registry();
            tui::run_tui()?;
            Ok(())
        }
    }
}

/// Wait for either SIGTERM or Ctrl+C (SIGINT).
async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm =
            signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");

        tokio::select! {
            _ = ctrl_c => {},
            _ = sigterm.recv() => {},
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.ok();
    }
}

/// Handle MCP configuration actions.
async fn handle_sync_action(action: SyncAction) -> anyhow::Result<()> {
    use anyhow::Context;
    use config::McpConfigRegistry;
    use std::io;

    let _home = dirs::home_dir().context("No home directory")?;
    print!("  Fetching platform registry... ");
    io::Write::flush(&mut io::stdout())?;
    let registry = setup::fetch_registry_raw()?;
    println!("\x1b[32m✓\x1b[0m {} platform(s)", registry.platforms.len());
    println!();

    match action {
        SyncAction::Extract => {
            println!("  Extracting MCP configurations...\n");

            let platforms: Vec<_> = registry.platforms.iter().collect();
            let mcp_registry = McpConfigRegistry::extract_all(&platforms)?;

            for platform_config in &mcp_registry.platforms {
                println!(
                    "  \x1b[32m✓\x1b[0m {} ({} servers)",
                    platform_config.platform,
                    platform_config.servers.len()
                );
                for server in &platform_config.servers {
                    let status = if server.enabled { "🟢" } else { "⚫" };
                    println!("      {} {}", status, server.name);
                }
            }

            if mcp_registry.platforms.is_empty() {
                println!("  \x1b[33m⏭\x1b[0m  No platforms with config files found.");
            }
        }

        SyncAction::Compare => {
            println!("  Comparing MCP configurations across platforms...\n");

            let platforms: Vec<_> = registry.platforms.iter().collect();
            let mcp_registry = McpConfigRegistry::extract_all(&platforms)?;

            let all_configs = &mcp_registry.platforms;
            if all_configs.len() < 2 {
                println!("  Need at least 2 platforms with configs to compare.");
                return Ok(());
            }

            let all_servers: std::collections::HashSet<String> = all_configs
                .iter()
                .flat_map(|c| c.servers.iter().map(|s| s.name.clone()))
                .collect();

            let max_name_len = all_configs
                .iter()
                .map(|c| c.platform.len())
                .max()
                .unwrap_or(8);

            println!(
                "  {:<20} {}",
                "Server",
                all_configs
                    .iter()
                    .map(|c| format!("{:^width$}", c.platform, width = max_name_len))
                    .collect::<Vec<_>>()
                    .join("  ")
            );
            println!("  {:─<50}", "");

            for server_name in &all_servers {
                let mut row = format!("  {:<20}", server_name);
                for config in all_configs {
                    let has = config.servers.iter().any(|s| s.name == *server_name);
                    let icon = if has {
                        "\x1b[32m✓\x1b[0m"
                    } else {
                        "\x1b[31m✗\x1b[0m"
                    };
                    row.push_str(&format!("  {:^width$}", icon, width = max_name_len));
                }
                println!("{}", row);
            }
            println!();

            // Show diff summary
            for (i, config) in all_configs.iter().enumerate() {
                for other in &all_configs[i + 1..] {
                    let only_in_from = mcp_registry.server_diff(&config.platform, &other.platform);
                    let only_in_to = mcp_registry.server_diff(&other.platform, &config.platform);

                    if !only_in_from.is_empty() || !only_in_to.is_empty() {
                        println!("  \x1b[1m{} vs {}\x1b[0m", config.platform, other.platform);
                        if !only_in_from.is_empty() {
                            println!(
                                "    Only in {}: {}",
                                config.platform,
                                only_in_from
                                    .iter()
                                    .map(|s| s.name.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            );
                        }
                        if !only_in_to.is_empty() {
                            println!(
                                "    Only in {}: {}",
                                other.platform,
                                only_in_to
                                    .iter()
                                    .map(|s| s.name.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            );
                        }
                        println!();
                    }
                }
            }
        }

        SyncAction::Apply => {
            println!("  MCP configuration sync — interactive mode\n");
            println!("  This feature will be available in a future release.");
        }
    }

    Ok(())
}

/// Start the Streamable HTTP MCP server in foreground.
async fn handle_http_server(port_override: Option<u16>) -> Result<()> {
    init_tracing();

    let port = resolve_port(port_override);
    let data_dir = ensure_data_dir()?;
    let db = Arc::new(Database::new(&data_dir.join("background_agents.db"))?);
    let executor = Arc::new(Executor::new(Arc::clone(&db)));
    let watcher_engine = Arc::new(WatcherEngine::new(Arc::clone(&db), Arc::clone(&executor)));

    tracing::info!(
        "canopy v{} starting on port {}",
        env!("CARGO_PKG_VERSION"),
        port
    );

    write_pid_file(&data_dir)?;

    db.set_state("port", &port.to_string())?;
    db.set_state("version", env!("CARGO_PKG_VERSION"))?;
    db.set_state("last_start", &chrono::Utc::now().to_rfc3339())?;

    if let Err(e) = watcher_engine.reload_from_db().await {
        tracing::error!("Failed to reload watchers: {}", e);
    }

    let cron_scheduler = Arc::new(CronScheduler::new(Arc::clone(&db), Arc::clone(&executor)));
    let scheduler_notify = cron_scheduler.notifier();
    let scheduler_cancel = Arc::clone(&cron_scheduler).start();

    let handler_db = Arc::clone(&db);
    let handler_executor = Arc::clone(&executor);
    let handler_watcher_engine = Arc::clone(&watcher_engine);
    let handler_scheduler_notify = Arc::clone(&scheduler_notify);

    let ct = tokio_util::sync::CancellationToken::new();

    let service = rmcp::transport::streamable_http_server::StreamableHttpService::new(
        move || {
            Ok(TaskTriggerHandler::new(
                Arc::clone(&handler_db),
                Arc::clone(&handler_executor),
                Arc::clone(&handler_watcher_engine),
                Arc::clone(&handler_scheduler_notify),
                port,
            ))
        },
        rmcp::transport::streamable_http_server::session::local::LocalSessionManager::default()
            .into(),
        {
            #[allow(clippy::field_reassign_with_default)]
            {
                let mut cfg =
                    rmcp::transport::streamable_http_server::StreamableHttpServerConfig::default();
                cfg.cancellation_token = ct.child_token();
                cfg
            }
        },
    );

    let router = axum::Router::new().nest_service("/mcp", service);
    let bind_addr = format!("127.0.0.1:{port}");

    // Kill any stale process occupying the port before binding
    kill_port_occupant(port);

    let tcp_listener = tokio::net::TcpListener::bind(&bind_addr).await?;

    tracing::info!(
        "Streamable HTTP MCP server listening on http://{}/mcp",
        bind_addr
    );

    axum::serve(tcp_listener, router)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            tracing::info!("Shutdown signal received");
            ct.cancel();
        })
        .await?;

    scheduler_cancel.cancel();
    watcher_engine.stop_all().await;
    remove_pid_file(&data_dir);
    tracing::info!("Daemon stopped");

    Ok(())
}

/// Handle daemon management subcommands.
async fn handle_daemon_action(action: DaemonAction, port_override: Option<u16>) -> Result<()> {
    let data_dir = ensure_data_dir()?;

    match action {
        DaemonAction::Start => {
            if let Some(pid) = read_pid(&data_dir) {
                if is_process_running(pid) {
                    println!("Daemon is already running (PID: {pid})");
                    return Ok(());
                }
                // Stale PID — clean up
                remove_pid_file(&data_dir);
            }

            let exe = std::env::current_exe()?;
            let port = resolve_port(port_override);

            // Kill any stale process occupying the port before spawning
            kill_port_occupant(port);

            #[cfg(target_os = "linux")]
            {
                if is_systemd_available() {
                    let home = dirs::home_dir().expect("No home directory");
                    let service_path = home.join(".config/systemd/user/canopy.service");
                    // Install if missing, or re-enable if it exists but is disabled.
                    let needs_install = !service_path.exists() || !is_service_enabled();
                    if needs_install {
                        print!("  Installing system service... ");
                        match service_install::install_service(&exe, port) {
                            Ok(_) => println!("\x1b[32m✅\x1b[0m installed"),
                            Err(e) => println!("\x1b[33m⚠\x1b[0m  {}", e),
                        }
                    }
                }
            }

            #[cfg(target_os = "macos")]
            {
                let home = dirs::home_dir().expect("No home directory");
                let plist_path = home.join("Library/LaunchAgents/com.canopy.plist");
                if !plist_path.exists() {
                    print!("  Installing system service... ");
                    match service_install::install_service(&exe, port) {
                        Ok(_) => println!("\x1b[32m✅\x1b[0m installed"),
                        Err(e) => println!("\x1b[33m⚠\x1b[0m  {}", e),
                    }
                }
            }

            let mut cmd = std::process::Command::new(&exe);
            cmd.arg("serve");
            if let Some(port) = port_override {
                cmd.arg("--port").arg(port.to_string());
            }

            // Kill any stale process occupying the port before spawning
            kill_port_occupant(port);

            let log_path = data_dir.join("daemon.log");
            let log_file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)?;
            let log_file_err = log_file.try_clone()?;

            cmd.stdout(log_file)
                .stderr(log_file_err)
                .stdin(std::process::Stdio::null());

            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                unsafe {
                    cmd.pre_exec(|| {
                        libc::setsid();
                        Ok(())
                    });
                }
            }

            let child = cmd.spawn()?;
            let child_pid = child.id();

            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            if is_process_running(child_pid) {
                println!("Daemon started (PID: {child_pid})");
                println!("Logs: {}", log_path.display());
            } else {
                eprintln!(
                    "Daemon failed to start — check logs at {}",
                    log_path.display()
                );
                return Err(anyhow::anyhow!("Daemon process exited immediately"));
            }
        }

        DaemonAction::Stop => {
            if let Some(pid) = read_pid(&data_dir) {
                if is_process_running(pid) {
                    send_signal(pid);
                    println!("Sent stop signal to daemon (PID: {pid})");
                    for _ in 0..20 {
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                        if !is_process_running(pid) {
                            break;
                        }
                    }
                    remove_pid_file(&data_dir);
                    if is_process_running(pid) {
                        eprintln!("Warning: daemon (PID: {pid}) did not stop within 5 seconds");
                    } else {
                        println!("Daemon stopped");
                    }
                } else {
                    println!("Daemon is not running (stale PID file)");
                    remove_pid_file(&data_dir);
                }
            } else {
                println!("Daemon is not running (no PID file)");
            }
        }

        DaemonAction::Status => {
            let pid_info = read_pid(&data_dir);
            let running = pid_info.map(is_process_running).unwrap_or(false);

            if running {
                let pid = pid_info.expect("pid checked above");
                if let Ok(db) = Database::new(&data_dir.join("background_agents.db")) {
                    let port = db.get_state("port")?.unwrap_or_else(|| "7755".to_string());
                    let version = db
                        .get_state("version")?
                        .unwrap_or_else(|| "unknown".to_string());
                    let last_start = db
                        .get_state("last_start")?
                        .unwrap_or_else(|| "unknown".to_string());
                    let background_agents = db.list_background_agents()?.len();
                    let watchers = db.list_watchers()?.len();
                    println!("Daemon: RUNNING (PID: {pid})");
                    println!("Version: {version}");
                    println!("Port: {port}");
                    println!("Started: {last_start}");
                    println!("Tasks: {background_agents}");
                    println!("Watchers: {watchers}");
                } else {
                    println!("Daemon: RUNNING (PID: {pid})");
                }
            } else {
                println!("Daemon: STOPPED");
                if pid_info.is_some() {
                    remove_pid_file(&data_dir);
                }
            }
        }

        DaemonAction::Restart => {
            println!("  Restarting daemon...");
            let stop_result =
                Box::pin(handle_daemon_action(DaemonAction::Stop, port_override)).await;
            if let Err(e) = stop_result {
                eprintln!("Warning: stop failed: {}", e);
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            Box::pin(handle_daemon_action(DaemonAction::Start, port_override)).await?;
        }

        DaemonAction::Logs => {
            let log_path = data_dir.join("daemon.log");
            if log_path.exists() {
                print_last_n_lines(&log_path, 50)?;
            } else {
                println!("No daemon logs found at {}", log_path.display());
            }
        }

        DaemonAction::InstallService => {
            let exe = std::env::current_exe()?;
            let port = resolve_port(port_override);
            println!("Installing canopy system service...");
            match service_install::install_service(&exe, port) {
                Ok(_) => println!("\x1b[32m✅\x1b[0m Service installed and enabled"),
                Err(e) => {
                    eprintln!("\x1b[31m✗\x1b[0m  Failed: {e}");
                    return Err(e);
                }
            }
        }

        DaemonAction::UninstallService => {
            println!("Removing canopy system service...");
            match service_install::uninstall_service() {
                Ok(_) => println!("\x1b[32m✅\x1b[0m Service uninstalled"),
                Err(e) => {
                    eprintln!("\x1b[31m✗\x1b[0m  Failed: {e}");
                    return Err(e);
                }
            }
        }
    }

    Ok(())
}

async fn handle_doctor() -> anyhow::Result<()> {
    use anyhow::Context;

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
        if let Ok(db) = crate::db::Database::new(&db_path) {
            if let Ok(background_agents) = db.list_background_agents() {
                println!("    Tasks: {}", background_agents.len());
            }
            if let Ok(watchers) = db.list_watchers() {
                println!("    Watchers: {}", watchers.len());
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

/// Handle stdio MCP transport mode.
async fn handle_stdio() -> Result<()> {
    init_tracing();
    tracing::info!("Starting in stdio MCP transport mode");

    let data_dir = ensure_data_dir()?;
    let db = Arc::new(Database::new(&data_dir.join("background_agents.db"))?);
    let executor = Arc::new(Executor::new(Arc::clone(&db)));
    let watcher_engine = Arc::new(WatcherEngine::new(Arc::clone(&db), Arc::clone(&executor)));

    if let Err(e) = watcher_engine.reload_from_db().await {
        tracing::error!("Failed to reload watchers: {}", e);
    }

    let cron_scheduler = Arc::new(CronScheduler::new(Arc::clone(&db), Arc::clone(&executor)));
    let scheduler_notify = cron_scheduler.notifier();
    let _scheduler_cancel = Arc::clone(&cron_scheduler).start();

    let handler = TaskTriggerHandler::new(
        Arc::clone(&db),
        Arc::clone(&executor),
        Arc::clone(&watcher_engine),
        scheduler_notify,
        0,
    );

    let transport = rmcp::transport::stdio();
    let server = handler.serve(transport).await?;
    tracing::info!("MCP stdio server started");

    server.waiting().await?;

    cron_scheduler.stop();
    watcher_engine.stop_all().await;
    tracing::info!("Stdio server stopped");

    Ok(())
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing_subscriber::filter::LevelFilter::INFO.into()),
        )
        .with_target(false)
        .init();
}

fn resolve_port(port_override: Option<u16>) -> u16 {
    port_override
        .or_else(|| {
            std::env::var("CANOPY_PORT")
                .ok()
                .and_then(|p| p.parse::<u16>().ok())
        })
        .unwrap_or(7755)
}

pub(crate) fn ensure_data_dir() -> Result<std::path::PathBuf> {
    let home =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;
    let data_dir = home.join(".canopy");
    std::fs::create_dir_all(&data_dir)?;
    std::fs::create_dir_all(data_dir.join("logs"))?;
    Ok(data_dir)
}

fn write_pid_file(data_dir: &std::path::Path) -> Result<()> {
    let pid = std::process::id();
    std::fs::write(data_dir.join("daemon.pid"), pid.to_string())?;
    Ok(())
}

fn remove_pid_file(data_dir: &std::path::Path) {
    let _ = std::fs::remove_file(data_dir.join("daemon.pid"));
}

fn read_pid(data_dir: &std::path::Path) -> Option<u32> {
    std::fs::read_to_string(data_dir.join("daemon.pid"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

fn is_process_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// Check if systemd is available and running (important for WSL compatibility).
fn is_systemd_available() -> bool {
    #[cfg(target_os = "linux")]
    {
        // Check if systemctl binary is available and systemd is the init system
        std::process::Command::new("systemctl")
            .args(["--user", "is-system-running"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Returns true if the canopy systemd user service is enabled (starts on boot).
#[cfg(target_os = "linux")]
fn is_service_enabled() -> bool {
    std::process::Command::new("systemctl")
        .args(["--user", "is-enabled", "--quiet", "canopy.service"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Kill whatever process is currently listening on the given port.
/// This prevents "address already in use" errors when starting the daemon.
fn kill_port_occupant(port: u16) {
    #[cfg(unix)]
    {
        // Use `ss` or `lsof` to find the PID listening on the port
        let output = std::process::Command::new("ss")
            .args(["-tlnp", &format!("sport = :{port}")])
            .output();

        if let Ok(out) = output {
            let text = String::from_utf8_lossy(&out.stdout);
            // Parse PID from ss output — format: "pid=12345,"
            for line in text.lines() {
                if let Some(pid_start) = line.find("pid=") {
                    let rest = &line[pid_start + 4..];
                    if let Some(end) = rest.find(|c: char| !c.is_ascii_digit()) {
                        if let Ok(pid) = rest[..end].parse::<u32>() {
                            let self_pid = std::process::id();
                            if pid != self_pid && pid != 0 {
                                eprintln!("Port {port} occupied by PID {pid} — sending SIGTERM");
                                unsafe { libc::kill(pid as i32, libc::SIGTERM) };
                                // Brief wait for process to exit
                                std::thread::sleep(std::time::Duration::from_millis(500));
                                if unsafe { libc::kill(pid as i32, 0) } == 0 {
                                    eprintln!("PID {pid} did not exit — sending SIGKILL");
                                    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
                                    std::thread::sleep(std::time::Duration::from_millis(200));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = port;
    }
}

fn send_signal(pid: u32) {
    #[cfg(unix)]
    {
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        eprintln!("Cannot send signal on this platform");
    }
}

/// Efficiently read the last N lines of a file without loading the entire file.
fn print_last_n_lines(path: &std::path::Path, n: usize) -> Result<()> {
    use std::io::{BufRead, BufReader};

    let file = std::fs::File::open(path)?;
    let reader = BufReader::new(file);
    let lines: Vec<String> = reader.lines().collect::<std::io::Result<Vec<_>>>()?;

    let start = lines.len().saturating_sub(n);
    for line in &lines[start..] {
        println!("{line}");
    }
    Ok(())
}
