//! task-trigger-mcp — MCP server for AI agent task scheduling and file watching.
//!
//! Binary modes:
//! - `daemon start` — start the MCP server as a persistent background process
//! - `daemon stop` — stop the running daemon
//! - `daemon status` — check daemon health
//! - `stdio` — run in stdio MCP transport mode (legacy/fallback)
//! - (no args) — start in foreground with Streamable HTTP transport

mod application;
mod daemon;
mod db;
mod domain;
mod error;
mod executor;
mod scheduler;
mod service_install;
mod watchers;

use anyhow::Result;
use clap::{Parser, Subcommand};
use rmcp::ServiceExt;
use std::sync::Arc;

use application::ports::{StateRepository, TaskRepository, WatcherRepository};
use daemon::TaskTriggerHandler;
use db::Database;
use executor::Executor;
use scheduler::cron_scheduler::CronScheduler;
use watchers::WatcherEngine;

/// task-trigger-mcp: A self-contained MCP server for AI agent task scheduling.
#[derive(Parser)]
#[command(name = "task-trigger-mcp", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Port for Streamable HTTP server (overrides `TASK_TRIGGER_PORT` env var).
    #[arg(long, short)]
    port: Option<u16>,
}

#[derive(Subcommand)]
enum Commands {
    /// Daemon management (start, stop, status, restart).
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
    /// Run in stdio MCP transport mode (legacy/fallback for clients without SSE).
    Stdio,
}

#[derive(Subcommand)]
enum DaemonAction {
    /// Start daemon in background.
    Start,
    /// Stop the running daemon.
    Stop,
    /// Check daemon status.
    Status,
    /// Restart the daemon.
    Restart,
    /// Tail daemon logs.
    Logs,
    /// Install as a system service (systemd on Linux, launchd on macOS).
    InstallService,
    /// Uninstall the system service.
    UninstallService,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Daemon { action }) => handle_daemon_action(action, cli.port).await,
        Some(Commands::Stdio) => handle_stdio().await,
        None => handle_http_server(cli.port).await,
    }
}

/// Wait for either SIGTERM or Ctrl+C (SIGINT).
async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate())
            .expect("failed to install SIGTERM handler");

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

/// Start the Streamable HTTP MCP server in foreground.
async fn handle_http_server(port_override: Option<u16>) -> Result<()> {
    init_tracing();

    let port = resolve_port(port_override);
    let data_dir = ensure_data_dir()?;
    let db = Arc::new(Database::new(&data_dir.join("tasks.db"))?);
    let executor = Arc::new(Executor::new(Arc::clone(&db)));
    let watcher_engine = Arc::new(WatcherEngine::new(Arc::clone(&db), Arc::clone(&executor)));

    tracing::info!(
        "task-trigger-mcp v{} starting on port {}",
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

    let cron_scheduler = Arc::new(CronScheduler::new(
        Arc::clone(&db),
        Arc::clone(&executor),
    ));
    let scheduler_cancel = Arc::clone(&cron_scheduler).start();

    let handler_db = Arc::clone(&db);
    let handler_executor = Arc::clone(&executor);
    let handler_watcher_engine = Arc::clone(&watcher_engine);

    let ct = tokio_util::sync::CancellationToken::new();

    let service = rmcp::transport::streamable_http_server::StreamableHttpService::new(
        move || {
            Ok(TaskTriggerHandler::new(
                Arc::clone(&handler_db),
                Arc::clone(&handler_executor),
                Arc::clone(&handler_watcher_engine),
                port,
            ))
        },
        rmcp::transport::streamable_http_server::session::local::LocalSessionManager::default()
            .into(),
        rmcp::transport::streamable_http_server::StreamableHttpServerConfig {
            cancellation_token: ct.child_token(),
            ..Default::default()
        },
    );

    let router = axum::Router::new().nest_service("/mcp", service);
    let bind_addr = format!("127.0.0.1:{port}");
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

    // Cleanup
    scheduler_cancel.cancel();    watcher_engine.stop_all().await;
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
                remove_pid_file(&data_dir);
            }

            let exe = std::env::current_exe()?;
            let mut cmd = std::process::Command::new(&exe);
            if let Some(port) = port_override {
                cmd.arg("--port").arg(port.to_string());
            }

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
                // SAFETY: setsid() is async-signal-safe and only affects
                // the child process's session group.
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
                eprintln!("Daemon failed to start — check logs at {}", log_path.display());
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
            let running = pid_info
                .map(is_process_running)
                .unwrap_or(false);

            if running {
                let pid = pid_info.expect("pid checked above");
                if let Ok(db) = Database::new(&data_dir.join("tasks.db")) {
                    let port = db
                        .get_state("port")?
                        .unwrap_or_else(|| "7755".to_string());
                    let version = db
                        .get_state("version")?
                        .unwrap_or_else(|| "unknown".to_string());
                    let last_start = db
                        .get_state("last_start")?
                        .unwrap_or_else(|| "unknown".to_string());
                    let tasks = db.list_tasks()?.len();
                    let watchers = db.list_watchers()?.len();
                    println!("Daemon: RUNNING (PID: {pid})");
                    println!("Version: {version}");
                    println!("Port: {port}");
                    println!("Started: {last_start}");
                    println!("Tasks: {tasks}");
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
            Box::pin(handle_daemon_action(DaemonAction::Stop, port_override)).await?;
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
            service_install::install_service(&exe, port)?;
        }

        DaemonAction::UninstallService => {
            service_install::uninstall_service()?;
        }
    }

    Ok(())
}

/// Handle stdio MCP transport mode.
async fn handle_stdio() -> Result<()> {
    init_tracing();
    tracing::info!("Starting in stdio MCP transport mode");

    let data_dir = ensure_data_dir()?;
    let db = Arc::new(Database::new(&data_dir.join("tasks.db"))?);
    let executor = Arc::new(Executor::new(Arc::clone(&db)));
    let watcher_engine = Arc::new(WatcherEngine::new(Arc::clone(&db), Arc::clone(&executor)));

    if let Err(e) = watcher_engine.reload_from_db().await {
        tracing::error!("Failed to reload watchers: {}", e);
    }

    let cron_scheduler = Arc::new(CronScheduler::new(
        Arc::clone(&db),
        Arc::clone(&executor),
    ));
    let _scheduler_cancel = Arc::clone(&cron_scheduler).start();

    let handler = TaskTriggerHandler::new(
        Arc::clone(&db),
        Arc::clone(&executor),
        Arc::clone(&watcher_engine),
        0, // No port in stdio mode
    );

    let transport = rmcp::transport::stdio();
    let server = handler.serve(transport).await?;
    tracing::info!("MCP stdio server started");

    server.waiting().await?;

    // Cleanup
    cron_scheduler.stop();
    watcher_engine.stop_all().await;
    tracing::info!("Stdio server stopped");

    Ok(())
}

// -- Utility functions --------------------------------------------------------

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
            std::env::var("TASK_TRIGGER_PORT")
                .ok()
                .and_then(|p| p.parse::<u16>().ok())
        })
        .unwrap_or(7755)
}

fn ensure_data_dir() -> Result<std::path::PathBuf> {
    let home =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;
    let data_dir = home.join(".task-trigger");
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
        // SAFETY: kill(pid, 0) checks if the process exists without
        // sending a signal. It only reads process state.
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

fn send_signal(pid: u32) {
    #[cfg(unix)]
    {
        // SAFETY: Sends SIGTERM to the specified PID. This is the
        // standard graceful shutdown signal on Unix systems.
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
