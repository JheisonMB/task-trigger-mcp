//! Internal cron scheduler — runs inside the daemon process.
//!
//! Instead of polling on a fixed interval, the scheduler computes the
//! nearest `next_fire_time` across all active background_agents and sleeps exactly
//! until that instant.  A `Notify` handle lets the daemon wake the
//! scheduler early when background_agents are added, updated, or re-enabled.

use std::str::FromStr;
use std::sync::Arc;

use chrono::Utc;
use cron::Schedule;
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;

use crate::application::ports::BackgroundAgentRepository;
use crate::db::Database;
use crate::domain::models::TriggerType;
use crate::executor::Executor;

/// The internal cron scheduler that runs as a tokio background_agent.
pub struct CronScheduler {
    db: Arc<Database>,
    executor: Arc<Executor>,
    cancel: CancellationToken,
    /// Wakes the scheduler to recalculate the next fire time.
    notify: Arc<Notify>,
    /// Track last execution time per background_agent to avoid double-firing.
    last_fired: Arc<Mutex<std::collections::HashMap<String, chrono::DateTime<Utc>>>>,
}

impl CronScheduler {
    pub fn new(db: Arc<Database>, executor: Arc<Executor>) -> Self {
        Self {
            db,
            executor,
            cancel: CancellationToken::new(),
            notify: Arc::new(Notify::new()),
            last_fired: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Get a handle to wake the scheduler when background_agents change.
    pub fn notifier(&self) -> Arc<Notify> {
        Arc::clone(&self.notify)
    }

    /// Start the scheduler loop as a background tokio background_agent.
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

    /// The main scheduler loop. Sleeps until the next background_agent is due,
    /// or wakes early on cancel/notify.
    async fn run_loop(&self) {
        loop {
            let sleep_dur = self.next_sleep_duration();

            tokio::select! {
                _ = self.cancel.cancelled() => break,
                _ = self.notify.notified() => {
                    // Tasks changed — recalculate immediately
                    continue;
                }
                _ = tokio::time::sleep(sleep_dur) => {
                    if let Err(e) = self.fire_due_tasks().await {
                        tracing::error!("Scheduler fire failed: {}", e);
                    }
                }
            }
        }
    }

    /// Compute how long to sleep until the nearest background_agent fires.
    /// Falls back to 60 s if there are no active background_agents or on parse errors.
    fn next_sleep_duration(&self) -> std::time::Duration {
        const FALLBACK: std::time::Duration = std::time::Duration::from_secs(60);

        let Ok(background_agents) = self.db.list_background_agents() else {
            return FALLBACK;
        };

        let now = Utc::now();
        let mut earliest: Option<chrono::DateTime<Utc>> = None;

        for background_agent in &background_agents {
            if !background_agent.enabled || background_agent.is_expired() {
                continue;
            }

            let cron_7field = to_7field_cron(&background_agent.schedule_expr);
            let Ok(schedule) = Schedule::from_str(&cron_7field) else {
                continue;
            };

            if let Some(next) = schedule.after(&now).next() {
                earliest = Some(match earliest {
                    Some(e) if next < e => next,
                    Some(e) => e,
                    None => next,
                });
            }
        }

        match earliest {
            Some(t) => {
                let delta = t.signed_duration_since(now);
                if delta.num_milliseconds() <= 0 {
                    // Already due — fire immediately
                    std::time::Duration::ZERO
                } else {
                    std::time::Duration::from_millis(delta.num_milliseconds() as u64)
                }
            }
            None => FALLBACK,
        }
    }

    /// Fire all background_agents whose next cron time is now (within a 1-second tolerance).
    async fn fire_due_tasks(&self) -> anyhow::Result<()> {
        let background_agents = self.db.list_background_agents()?;
        let now = Utc::now();

        for background_agent in &background_agents {
            if !background_agent.enabled {
                continue;
            }

            if background_agent.is_expired() {
                tracing::info!(
                    "BackgroundAgent '{}' has expired, disabling",
                    background_agent.id
                );
                self.db
                    .update_background_agent_enabled(&background_agent.id, false)?;
                continue;
            }

            let cron_7field = to_7field_cron(&background_agent.schedule_expr);
            let schedule = match Schedule::from_str(&cron_7field) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        "BackgroundAgent '{}' has invalid cron expression '{}': {}",
                        background_agent.id,
                        background_agent.schedule_expr,
                        e
                    );
                    continue;
                }
            };

            // Check if the background_agent should have fired between now-60s and now.
            // The 60 s window covers minor scheduling jitter.
            let window_start = now - chrono::Duration::seconds(60);
            let mut upcoming = schedule.after(&window_start);

            if let Some(next_fire) = upcoming.next() {
                if next_fire <= now {
                    let mut last_fired = self.last_fired.lock().await;
                    if let Some(last) = last_fired.get(&background_agent.id) {
                        if *last >= window_start {
                            continue;
                        }
                    }

                    last_fired.insert(background_agent.id.clone(), now);
                    drop(last_fired);

                    let executor = Arc::clone(&self.executor);
                    let background_agent = background_agent.clone();
                    tokio::spawn(async move {
                        match executor
                            .execute_task(&background_agent, TriggerType::Scheduled, false)
                            .await
                        {
                            Ok(code) => {
                                tracing::info!(
                                    "Scheduled background_agent '{}' completed (exit code: {})",
                                    background_agent.id,
                                    code
                                );
                            }
                            Err(e) => {
                                tracing::error!(
                                    "Scheduled background_agent '{}' failed: {}",
                                    background_agent.id,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_7field_cron() {
        assert_eq!(to_7field_cron("*/5 * * * *"), "0 */5 * * * * *");
        assert_eq!(to_7field_cron("0 9 * * *"), "0 0 9 * * * *");
        assert_eq!(to_7field_cron("0 9 * * 1-5"), "0 0 9 * * 1-5 *");
    }

    #[test]
    fn test_cron_parse_after_conversion() {
        let cases = vec![
            "*/5 * * * *",    // every 5 min
            "0 9 * * *",      // daily at 9am
            "0 9 * * 1-5",    // weekdays at 9am
            "30 14 1,15 * *", // 1st and 15th at 2:30pm
        ];

        for expr in cases {
            let converted = to_7field_cron(expr);
            let result = Schedule::from_str(&converted);
            assert!(
                result.is_ok(),
                "Failed to parse '{}' -> '{}': {:?}",
                expr,
                converted,
                result.err()
            );
        }
    }
}
