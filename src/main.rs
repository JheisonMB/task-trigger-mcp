//! task-trigger-mcp — MCP server for AI agent task scheduling and file watching.
//!
//! Binary modes:
//! - `daemon start` — start the MCP server as a persistent background process
//! - `daemon stop` — stop the running daemon
//! - `daemon status` — check daemon health
//! - `stdio` — run in stdio MCP transport mode (legacy/fallback)
//! - (no args) — start in foreground with SSE transport

mod daemon;
mod db;
mod error;
mod executor;
mod scheduler;
mod state;
mod watchers;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::sync::Arc;

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

    /// Port for SSE/HTTP server (overrides `TASK_TRIGGER_PORT` env var).
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Daemon { action }) => handle_daemon_action(action, cli.port).await,
        Some(Commands::Stdio) => handle_stdio().await,
        None => {
            // Default: start SSE server in foreground
            handle_sse_server(cli.port).await
        }
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

/// Start the SSE/HTTP MCP server in foreground.
async fn handle_sse_server(port_override: Option<u16>) -> Result<()> {
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

    // Write PID file
    write_pid_file(&data_dir)?;

    // Store daemon state
    db.set_state("port", &port.to_string())?;
    db.set_state("version", env!("CARGO_PKG_VERSION"))?;
    db.set_state("last_start", &chrono::Utc::now().to_rfc3339())?;

    // Reload watchers from database
    if let Err(e) = watcher_engine.reload_from_db().await {
        tracing::error!("Failed to reload watchers: {}", e);
    }

    // Start internal cron scheduler
    let cron_scheduler = Arc::new(CronScheduler::new(
        Arc::clone(&db),
        Arc::clone(&executor),
    ));
    let scheduler_cancel = Arc::clone(&cron_scheduler).start();

    // Create the MCP handler
    let handler = TaskTriggerHandler::new(
        Arc::clone(&db),
        Arc::clone(&executor),
        Arc::clone(&watcher_engine),
        port,
    );

    tracing::info!(
        "Starting SSE MCP server on http://127.0.0.1:{}/sse",
        port
    );

    // Use rmcp's SSE server transport.
    let bind_addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse()?;
    let ct = rmcp::transport::sse_server::SseServer::serve(bind_addr)
        .await?
        .with_service(move || handler.clone());

    tracing::info!(
        "SSE server listening on http://127.0.0.1:{}/sse",
        port
    );

    // Wait for shutdown signal (SIGTERM or Ctrl+C)
    shutdown_signal().await;
    tracing::info!("Shutdown signal received");
    ct.cancel();

    // Cleanup
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
            // Check if already running
            if let Some(pid) = read_pid(&data_dir) {
                if is_process_running(pid) {
                    println!("Daemon is already running (PID: {pid})");
                    return Ok(());
                }
                // Stale PID file — clean it up
                remove_pid_file(&data_dir);
            }

            // Start the server in a background process
            let exe = std::env::current_exe()?;
            let mut cmd = std::process::Command::new(&exe);
            if let Some(port) = port_override {
                cmd.arg("--port").arg(port.to_string());
            }

            // Redirect stdout/stderr to daemon log
            let log_path = data_dir.join("daemon.log");
            let log_file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)?;
            let log_file_err = log_file.try_clone()?;

            cmd.stdout(log_file)
                .stderr(log_file_err)
                .stdin(std::process::Stdio::null());

            // Detach the process on unix
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

            // Wait briefly to verify the child didn't exit immediately
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
                    // Wait for the process to actually stop
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
                // Try to read daemon state from DB
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

    // Reload watchers (these will stop when the process exits)
    if let Err(e) = watcher_engine.reload_from_db().await {
        tracing::error!("Failed to reload watchers: {}", e);
    }

    // Start internal cron scheduler (also stops when process exits)
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

    // Create stdio transport
    let transport = rmcp::transport::io::stdio();

    // Serve via stdio
    let server = rmcp::serve_server(handler, transport).await?;
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
