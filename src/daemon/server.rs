use anyhow::Result;
use std::sync::Arc;

use rmcp::ServiceExt;

use crate::application::notification_service::{DefaultNotificationService, NotificationService};
use crate::application::ports::StateRepository;
use crate::daemon::process::{kill_port_occupant, remove_pid_file, write_pid_file};
use crate::daemon::TaskTriggerHandler;
use crate::db::Database;
use crate::executor::Executor;
use crate::scheduler::cron_scheduler::CronScheduler;
use crate::watchers::WatcherEngine;

pub(crate) async fn run_http_server(port_override: Option<u16>) -> Result<()> {
    crate::domain::notification::clear_stale_notifications();
    init_tracing();

    let port = crate::resolve_port(port_override);
    let data_dir = crate::ensure_data_dir()?;
    let db = Arc::new(Database::new(&data_dir.join("background_agents.db"))?);
    let notification_service: Arc<dyn NotificationService> = Arc::new(DefaultNotificationService);
    let executor = Arc::new(Executor::new(
        Arc::clone(&db),
        Arc::clone(&notification_service),
    ));
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
                Arc::clone(&notification_service),
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

pub(crate) async fn run_stdio_server() -> Result<()> {
    init_tracing();
    tracing::info!("Starting in stdio MCP transport mode");

    let data_dir = crate::ensure_data_dir()?;
    let db = Arc::new(Database::new(&data_dir.join("background_agents.db"))?);
    let notification_service: Arc<dyn NotificationService> = Arc::new(DefaultNotificationService);
    let executor = Arc::new(Executor::new(
        Arc::clone(&db),
        Arc::clone(&notification_service),
    ));
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
        Arc::clone(&notification_service),
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

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing_subscriber::filter::LevelFilter::INFO.into()),
        )
        .with_target(false)
        .init();
}
