//! Internal cron scheduler — runs inside the daemon process.
//!
//! Replaces the OS scheduler (crontab/launchd) approach. The daemon
//! owns a tokio task that wakes up every 30 seconds, checks which
//! tasks are due, and executes them via the Executor.

use std::str::FromStr;
use std::sync::Arc;

use chrono::Utc;
use cron::Schedule;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::db::Database;
use crate::executor::Executor;
use crate::state::TriggerType;

/// The internal cron scheduler that runs as a tokio task.
pub struct CronScheduler {
    db: Arc<Database>,
    executor: Arc<Executor>,
    cancel: CancellationToken,
    /// Track last execution time per task to avoid double-firing.
    last_fired: Arc<Mutex<std::collections::HashMap<String, chrono::DateTime<Utc>>>>,
}

impl CronScheduler {
    pub fn new(db: Arc<Database>, executor: Arc<Executor>) -> Self {
        Self {
            db,
            executor,
            cancel: CancellationToken::new(),
            last_fired: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Start the scheduler loop as a background tokio task.
    ///
    /// Returns a `CancellationToken` that can be used to stop the scheduler.
    pub fn start(self: Arc<Self>) -> CancellationToken {
        let cancel = self.cancel.clone();
        let scheduler = Arc::clone(&self);

        tokio::spawn(async move {
            tracing::info!("Internal cron scheduler started");
            scheduler.run_loop().await;
            tracing::info!("Internal cron scheduler stopped");
        });

        cancel
    }

    /// The main scheduler loop. Checks every 30 seconds for tasks that are due.
    async fn run_loop(&self) {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));

        loop {
            tokio::select! {
                _ = self.cancel.cancelled() => {
                    break;
                }
                _ = interval.tick() => {
                    if let Err(e) = self.check_and_execute().await {
                        tracing::error!("Scheduler check failed: {}", e);
                    }
                }
            }
        }
    }

    /// Check all enabled tasks and execute any that are due.
    async fn check_and_execute(&self) -> anyhow::Result<()> {
        let tasks = self.db.list_tasks()?;
        let now = Utc::now();

        for task in &tasks {
            if !task.enabled {
                continue;
            }

            // Check expiry
            if task.is_expired() {
                tracing::info!("Task '{}' has expired, disabling", task.id);
                self.db.update_task_enabled(&task.id, false)?;
                continue;
            }

            // Parse the cron expression (convert 5-field to 7-field for the cron crate)
            let cron_7field = to_7field_cron(&task.schedule_expr);
            let schedule = match Schedule::from_str(&cron_7field) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        "Task '{}' has invalid cron expression '{}': {}",
                        task.id,
                        task.schedule_expr,
                        e
                    );
                    continue;
                }
            };

            // Find the most recent time this schedule should have fired
            // We look for any fire time between now-30s and now
            let window_start = now - chrono::Duration::seconds(30);

            let mut upcoming = schedule.after(&window_start);
            if let Some(next_fire) = upcoming.next() {
                if next_fire <= now {
                    // This task is due — check we haven't already fired it
                    let mut last_fired = self.last_fired.lock().await;
                    if let Some(last) = last_fired.get(&task.id) {
                        if *last >= window_start {
                            // Already fired within this window
                            continue;
                        }
                    }

                    // Mark as fired
                    last_fired.insert(task.id.clone(), now);
                    drop(last_fired);

                    // Execute in a spawned task to not block the scheduler
                    let executor = Arc::clone(&self.executor);
                    let task = task.clone();
                    tokio::spawn(async move {
                        match executor
                            .execute_task(&task, TriggerType::Scheduled)
                            .await
                        {
                            Ok(code) => {
                                tracing::info!(
                                    "Scheduled task '{}' completed (exit code: {})",
                                    task.id,
                                    code
                                );
                            }
                            Err(e) => {
                                tracing::error!(
                                    "Scheduled task '{}' failed: {}",
                                    task.id,
                                    e
                                );
                            }
                        }
                    });
                }
            }
        }

        Ok(())
    }

    /// Stop the scheduler.
    pub fn stop(&self) {
        self.cancel.cancel();
    }
}

/// Convert a standard 5-field cron expression to the 7-field format
/// expected by the `cron` crate: `sec min hour day month dow year`.
///
/// Input:  `*/5 * * * *`       (min hour day month dow)
/// Output: `0 */5 * * * * *`   (sec min hour day month dow year)
fn to_7field_cron(expr: &str) -> String {
    format!("0 {} *", expr.trim())
}

