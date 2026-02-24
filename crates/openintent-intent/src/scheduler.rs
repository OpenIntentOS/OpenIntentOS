//! Background cron scheduler for OpenIntentOS.
//!
//! Provides a [`CronScheduler`] that manages recurring jobs and fires
//! [`CronEvent`]s through a tokio channel when a job is due.  Cron
//! expressions are parsed via the `cron` crate which supports standard
//! 6-field (with seconds) and 7-field formats.  Typical 5-field user
//! input is automatically normalized by prepending a `0` seconds field.

use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, error, info, warn};

use crate::error::{IntentError, Result};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A scheduled recurring job managed by [`CronScheduler`].
#[derive(Debug, Clone)]
pub struct ScheduledJob {
    /// Unique job identifier.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Parsed cron schedule.
    pub schedule: cron::Schedule,
    /// The command or intent to run when the job fires.
    pub command: String,
    /// Whether the job is currently active.
    pub enabled: bool,
    /// Timestamp of the most recent execution, if any.
    pub last_run: Option<DateTime<Utc>>,
    /// Timestamp of the next planned execution, if known.
    pub next_run: Option<DateTime<Utc>>,
}

/// Event emitted when a scheduled job fires.
#[derive(Debug, Clone)]
pub struct CronEvent {
    /// The ID of the job that fired.
    pub job_id: String,
    /// Human-readable name of the job.
    pub job_name: String,
    /// The command associated with the job.
    pub command: String,
    /// UTC timestamp when the job was fired.
    pub fired_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Normalize a cron expression to the 6/7-field format expected by the
/// `cron` crate.  If the user provides a standard 5-field expression we
/// prepend `0` as the seconds field.
fn normalize_cron_expr(expr: &str) -> String {
    let field_count = expr.split_whitespace().count();
    if field_count == 5 {
        format!("0 {expr}")
    } else {
        expr.to_string()
    }
}

/// Parse a cron expression string into a [`cron::Schedule`].
fn parse_schedule(expr: &str) -> Result<cron::Schedule> {
    let normalized = normalize_cron_expr(expr);
    cron::Schedule::from_str(&normalized).map_err(|e| IntentError::InvalidCronExpression {
        expression: expr.to_string(),
        reason: format!("invalid cron expression: {e}"),
    })
}

/// Compute the next upcoming run time from `after` for the given schedule.
fn next_run_after(schedule: &cron::Schedule, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
    schedule.after(&after).next()
}

// ---------------------------------------------------------------------------
// CronScheduler
// ---------------------------------------------------------------------------

/// Background cron scheduler that checks jobs every second and emits
/// [`CronEvent`]s through a channel when a job is due.
pub struct CronScheduler {
    /// Active scheduled jobs.
    jobs: Arc<RwLock<Vec<ScheduledJob>>>,
    /// Flag to signal the background loop to stop.
    running: Arc<AtomicBool>,
    /// Handle to the background tokio task.
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl CronScheduler {
    /// Create a new scheduler with no jobs.
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(RwLock::new(Vec::new())),
            running: Arc::new(AtomicBool::new(false)),
            handle: None,
        }
    }

    /// Add a new scheduled job.
    ///
    /// The cron expression is parsed immediately.  If it is invalid the
    /// job is rejected.  `next_run` is computed from the current time.
    pub async fn add_job(
        &self,
        id: impl Into<String>,
        name: impl Into<String>,
        cron_expr: &str,
        command: impl Into<String>,
    ) -> Result<()> {
        let id = id.into();
        let name = name.into();
        let command = command.into();
        let schedule = parse_schedule(cron_expr)?;
        let now = Utc::now();
        let next = next_run_after(&schedule, now);

        info!(job_id = %id, job_name = %name, cron = %cron_expr, "adding cron job");

        let job = ScheduledJob {
            id,
            name,
            schedule,
            command,
            enabled: true,
            last_run: None,
            next_run: next,
        };

        let mut jobs = self.jobs.write().await;
        jobs.push(job);
        Ok(())
    }

    /// Remove a job by its ID.
    pub async fn remove_job(&self, id: &str) -> Result<()> {
        let mut jobs = self.jobs.write().await;
        let before = jobs.len();
        jobs.retain(|j| j.id != id);
        if jobs.len() == before {
            return Err(IntentError::TriggerRegistrationFailed {
                reason: format!("cron job `{id}` not found"),
            });
        }
        info!(job_id = %id, "cron job removed");
        Ok(())
    }

    /// Enable a previously disabled job.
    pub async fn enable_job(&self, id: &str) -> Result<()> {
        let mut jobs = self.jobs.write().await;
        let job = jobs.iter_mut().find(|j| j.id == id).ok_or_else(|| {
            IntentError::TriggerRegistrationFailed {
                reason: format!("cron job `{id}` not found"),
            }
        })?;
        job.enabled = true;
        // Recompute next_run from now so the job does not immediately fire
        // for any missed windows while it was disabled.
        job.next_run = next_run_after(&job.schedule, Utc::now());
        debug!(job_id = %id, "cron job enabled");
        Ok(())
    }

    /// Disable a job so it will not fire until re-enabled.
    pub async fn disable_job(&self, id: &str) -> Result<()> {
        let mut jobs = self.jobs.write().await;
        let job = jobs.iter_mut().find(|j| j.id == id).ok_or_else(|| {
            IntentError::TriggerRegistrationFailed {
                reason: format!("cron job `{id}` not found"),
            }
        })?;
        job.enabled = false;
        debug!(job_id = %id, "cron job disabled");
        Ok(())
    }

    /// Return a snapshot of all registered jobs.
    pub async fn list_jobs(&self) -> Vec<ScheduledJob> {
        self.jobs.read().await.clone()
    }

    /// Start the background scheduler loop.
    ///
    /// Every second, each enabled job whose `next_run` is at or before the
    /// current time will fire: a [`CronEvent`] is sent through `event_tx`,
    /// `last_run` is updated, and the next occurrence is computed.
    pub async fn start(&mut self, event_tx: mpsc::UnboundedSender<CronEvent>) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            return Err(IntentError::Internal(
                "scheduler is already running".to_string(),
            ));
        }

        self.running.store(true, Ordering::SeqCst);
        let running = Arc::clone(&self.running);
        let jobs = Arc::clone(&self.jobs);

        let handle = tokio::spawn(async move {
            info!("cron scheduler started");

            while running.load(Ordering::SeqCst) {
                let now = Utc::now();
                {
                    let mut job_list = jobs.write().await;
                    for job in job_list.iter_mut() {
                        if !job.enabled {
                            continue;
                        }

                        let should_fire = match job.next_run {
                            Some(next) => next <= now,
                            None => false,
                        };

                        if should_fire {
                            let event = CronEvent {
                                job_id: job.id.clone(),
                                job_name: job.name.clone(),
                                command: job.command.clone(),
                                fired_at: now,
                            };

                            debug!(
                                job_id = %job.id,
                                job_name = %job.name,
                                "cron job fired"
                            );

                            if let Err(e) = event_tx.send(event) {
                                error!(
                                    job_id = %job.id,
                                    error = %e,
                                    "failed to send cron event"
                                );
                            }

                            job.last_run = Some(now);
                            job.next_run = next_run_after(&job.schedule, now);
                        }
                    }
                }

                tokio::time::sleep(Duration::from_secs(1)).await;
            }

            info!("cron scheduler stopped");
        });

        self.handle = Some(handle);
        Ok(())
    }

    /// Stop the background scheduler and wait for it to finish.
    pub async fn stop(&mut self) {
        if !self.running.load(Ordering::SeqCst) {
            warn!("stop called but scheduler is not running");
            return;
        }

        self.running.store(false, Ordering::SeqCst);

        if let Some(handle) = self.handle.take()
            && let Err(e) = handle.await
        {
            error!(error = %e, "scheduler task panicked during shutdown");
        }

        info!("cron scheduler shutdown complete");
    }

    /// Check whether the background scheduler loop is currently running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

impl Default for CronScheduler {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_6_field_cron() {
        let schedule = parse_schedule("0 30 9 * * 1-5");
        assert!(schedule.is_ok(), "6-field cron should parse successfully");
    }

    #[test]
    fn parse_valid_5_field_cron_normalized() {
        // Standard user input (5 fields) should be auto-normalized.
        let schedule = parse_schedule("30 9 * * 1-5");
        assert!(
            schedule.is_ok(),
            "5-field cron should be normalized and parse successfully"
        );
    }

    #[test]
    fn reject_invalid_cron() {
        let result = parse_schedule("not a cron");
        assert!(result.is_err(), "garbage input should be rejected");
    }

    #[tokio::test]
    async fn add_and_list_jobs() {
        let scheduler = CronScheduler::new();
        scheduler
            .add_job("j1", "job one", "* * * * *", "echo one")
            .await
            .unwrap();
        scheduler
            .add_job("j2", "job two", "0 12 * * *", "echo two")
            .await
            .unwrap();

        let jobs = scheduler.list_jobs().await;
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].id, "j1");
        assert_eq!(jobs[1].id, "j2");
    }

    #[tokio::test]
    async fn remove_job() {
        let scheduler = CronScheduler::new();
        scheduler
            .add_job("j1", "job one", "* * * * *", "echo one")
            .await
            .unwrap();

        scheduler.remove_job("j1").await.unwrap();
        assert!(scheduler.list_jobs().await.is_empty());
    }

    #[tokio::test]
    async fn remove_nonexistent_job_fails() {
        let scheduler = CronScheduler::new();
        let result = scheduler.remove_job("nope").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn five_field_normalization_produces_valid_schedule() {
        let scheduler = CronScheduler::new();
        // "*/5 * * * *" = every 5 minutes, 5-field format
        let result = scheduler
            .add_job("j1", "every 5m", "*/5 * * * *", "tick")
            .await;
        assert!(result.is_ok());

        let jobs = scheduler.list_jobs().await;
        assert_eq!(jobs.len(), 1);
        // next_run must have been computed
        assert!(jobs[0].next_run.is_some());
    }

    #[tokio::test]
    async fn enable_disable_job() {
        let scheduler = CronScheduler::new();
        scheduler
            .add_job("j1", "job one", "* * * * *", "cmd")
            .await
            .unwrap();

        scheduler.disable_job("j1").await.unwrap();
        let jobs = scheduler.list_jobs().await;
        assert!(!jobs[0].enabled);

        scheduler.enable_job("j1").await.unwrap();
        let jobs = scheduler.list_jobs().await;
        assert!(jobs[0].enabled);
    }

    #[tokio::test]
    async fn start_stop_lifecycle() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut scheduler = CronScheduler::new();

        assert!(!scheduler.is_running());

        scheduler.start(tx).await.unwrap();
        assert!(scheduler.is_running());

        scheduler.stop().await;
        assert!(!scheduler.is_running());
    }

    #[tokio::test]
    async fn next_run_is_computed_on_add() {
        let scheduler = CronScheduler::new();
        scheduler
            .add_job("j1", "minutely", "* * * * *", "tick")
            .await
            .unwrap();

        let jobs = scheduler.list_jobs().await;
        assert_eq!(jobs.len(), 1);
        let next = jobs[0].next_run;
        assert!(next.is_some(), "next_run should be set after add");
        // The next run should be in the future (or essentially now).
        assert!(next.unwrap() >= Utc::now() - chrono::Duration::seconds(2));
    }

    #[tokio::test]
    async fn scheduler_fires_event() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut scheduler = CronScheduler::new();

        // Use every-second schedule so it fires quickly.
        scheduler
            .add_job("fast", "fast job", "* * * * * *", "boom")
            .await
            .unwrap();

        scheduler.start(tx).await.unwrap();

        // Wait for at least one event (up to 3 seconds).
        let event = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await;

        scheduler.stop().await;

        let event = event
            .expect("timed out waiting for cron event")
            .expect("channel closed unexpectedly");
        assert_eq!(event.job_id, "fast");
        assert_eq!(event.job_name, "fast job");
        assert_eq!(event.command, "boom");
    }
}
