//! Lock-free task scheduler.
//!
//! The scheduler accepts [`Task`] submissions, enqueues them into a set of
//! priority-partitioned [`crossbeam::queue::SegQueue`]s, and drives execution
//! via a background tokio task that continuously polls the queues.
//!
//! # Priority model
//!
//! Four priority lanes are maintained.  The background worker drains
//! **Critical** before **High**, **High** before **Normal**, and so on,
//! ensuring that high-priority work is never starved by bulk low-priority
//! submissions.
//!
//! # Task lifecycle
//!
//! ```text
//! Pending  -->  Queued  -->  Running  -->  Completed
//!                                     \->  Failed
//!                                     \->  Cancelled
//! ```
//!
//! Tasks may be cancelled at any point before they enter the `Running` state.
//! Once running, cancellation is cooperative via the [`tokio::sync::CancellationToken`]
//! mechanism exposed on each task's context (not yet wired -- reserved for v2).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use crossbeam::queue::SegQueue;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::error::{KernelError, Result};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Unique, time-ordered task identifier (UUID v7).
pub type TaskId = Uuid;

/// Priority level that determines the scheduling lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TaskPriority {
    /// Must execute before anything else.
    Critical = 0,
    /// Important but not safety-critical.
    High = 1,
    /// Default priority for most work.
    Normal = 2,
    /// Background / best-effort.
    Low = 3,
}

/// Lifecycle state of a scheduled task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskStatus {
    /// Created but not yet enqueued (e.g. waiting for a delay).
    Pending,
    /// Sitting in the priority queue, waiting for the worker to pick it up.
    Queued,
    /// Currently executing.
    Running,
    /// Finished successfully.
    Completed,
    /// Finished with an error.
    Failed,
    /// Cancelled before or during execution.
    Cancelled,
}

/// When the task should first become eligible for execution.
#[derive(Debug, Clone)]
pub enum SchedulePolicy {
    /// Execute as soon as the worker picks it up.
    Immediate,
    /// Execute after `delay` has elapsed.
    Delayed { delay: Duration },
    /// Execute at a specific wall-clock time.
    At { when: DateTime<Utc> },
    /// Cron-style recurring schedule (expression stored as a string for now;
    /// a dedicated cron parser will be wired in a later iteration).
    Cron { expression: String },
}

/// The async closure that the scheduler will execute.
///
/// We box the future so that callers can supply arbitrary async work without
/// leaking concrete types into the scheduler.
pub type TaskFn = Box<
    dyn FnOnce() -> Pin<Box<dyn Future<Output = std::result::Result<(), String>> + Send>>
        + Send
        + Sync,
>;

/// Metadata snapshot of a task visible to external callers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInfo {
    pub id: TaskId,
    pub name: String,
    pub priority: TaskPriority,
    pub status: TaskStatus,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

/// Internal representation of a task that lives on the queue.
struct QueuedTask {
    id: TaskId,
    name: String,
    priority: TaskPriority,
    work: TaskFn,
}

// ---------------------------------------------------------------------------
// Scheduler
// ---------------------------------------------------------------------------

/// Lock-free, priority-aware task scheduler.
///
/// The scheduler is cheaply cloneable (`Arc`-backed) and safe to share across
/// threads and async tasks.
#[derive(Clone)]
pub struct Scheduler {
    inner: Arc<SchedulerInner>,
}

struct SchedulerInner {
    /// One lock-free queue per priority lane.
    queues: [SegQueue<QueuedTask>; 4],

    /// Authoritative task metadata.  Updated atomically via `DashMap`.
    tasks: DashMap<TaskId, TaskInfo>,

    /// Wakes the background worker when new work arrives.
    notify: Notify,

    /// When `true` the scheduler will not accept new work.
    shutdown: std::sync::atomic::AtomicBool,
}

impl Scheduler {
    /// Create a new scheduler **without** starting the background worker.
    ///
    /// Call [`Scheduler::start`] to spawn the worker onto the tokio runtime.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(SchedulerInner {
                queues: [
                    SegQueue::new(),
                    SegQueue::new(),
                    SegQueue::new(),
                    SegQueue::new(),
                ],
                tasks: DashMap::new(),
                notify: Notify::new(),
                shutdown: std::sync::atomic::AtomicBool::new(false),
            }),
        }
    }

    /// Spawn the background worker that polls the queues and executes tasks.
    ///
    /// Returns a [`JoinHandle`] that resolves when the scheduler is shut down.
    pub fn start(&self) -> JoinHandle<()> {
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            tracing::info!("scheduler worker started");
            Self::worker_loop(&inner).await;
            tracing::info!("scheduler worker stopped");
        })
    }

    /// Submit a task for immediate execution.
    pub fn submit(
        &self,
        name: impl Into<String>,
        priority: TaskPriority,
        work: TaskFn,
    ) -> Result<TaskId> {
        self.submit_with_policy(name, priority, SchedulePolicy::Immediate, work)
    }

    /// Submit a task with a specific [`SchedulePolicy`].
    pub fn submit_with_policy(
        &self,
        name: impl Into<String>,
        priority: TaskPriority,
        policy: SchedulePolicy,
        work: TaskFn,
    ) -> Result<TaskId> {
        if self
            .inner
            .shutdown
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(KernelError::SchedulerShutdown);
        }

        let id = Uuid::now_v7();
        let name = name.into();

        let info = TaskInfo {
            id,
            name: name.clone(),
            priority,
            status: TaskStatus::Pending,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            error: None,
        };
        self.inner.tasks.insert(id, info);

        tracing::debug!(task_id = %id, task_name = %name, ?priority, "task submitted");

        match policy {
            SchedulePolicy::Immediate => {
                self.enqueue(id, name, priority, work);
            }
            SchedulePolicy::Delayed { delay } => {
                let scheduler = self.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(delay).await;
                    scheduler.enqueue(id, name, priority, work);
                });
            }
            SchedulePolicy::At { when } => {
                let scheduler = self.clone();
                tokio::spawn(async move {
                    let now = Utc::now();
                    if when > now {
                        let delta = (when - now).to_std().unwrap_or(Duration::from_millis(0));
                        tokio::time::sleep(delta).await;
                    }
                    scheduler.enqueue(id, name, priority, work);
                });
            }
            SchedulePolicy::Cron { expression } => {
                // Store the cron expression in task metadata for future use.
                // Full cron scheduling will be implemented in a later iteration
                // when we integrate a dedicated cron parser crate.
                tracing::warn!(
                    task_id = %id,
                    cron = %expression,
                    "cron scheduling is not yet fully implemented; running once immediately"
                );
                self.enqueue(id, name, priority, work);
            }
        }

        Ok(id)
    }

    /// Cancel a task that has not yet started running.
    ///
    /// Tasks that are already `Running`, `Completed`, `Failed`, or `Cancelled`
    /// cannot be cancelled through this method.
    pub fn cancel(&self, task_id: TaskId) -> Result<()> {
        let mut entry = self
            .inner
            .tasks
            .get_mut(&task_id)
            .ok_or(KernelError::TaskNotFound { task_id })?;

        match entry.status {
            TaskStatus::Pending | TaskStatus::Queued => {
                entry.status = TaskStatus::Cancelled;
                entry.completed_at = Some(Utc::now());
                tracing::info!(task_id = %task_id, "task cancelled");
                Ok(())
            }
            other => Err(KernelError::InvalidTaskState {
                task_id,
                reason: format!("cannot cancel task in state {other:?}"),
            }),
        }
    }

    /// Query the current status of a task.
    pub fn status(&self, task_id: TaskId) -> Result<TaskInfo> {
        self.inner
            .tasks
            .get(&task_id)
            .map(|entry| entry.clone())
            .ok_or(KernelError::TaskNotFound { task_id })
    }

    /// Return a snapshot of all known tasks keyed by their ID.
    pub fn all_tasks(&self) -> HashMap<TaskId, TaskInfo> {
        self.inner
            .tasks
            .iter()
            .map(|entry| (*entry.key(), entry.value().clone()))
            .collect()
    }

    /// Signal the scheduler to stop accepting new work and drain remaining
    /// tasks.  The background worker will exit after the current task (if any)
    /// finishes.
    pub fn shutdown(&self) {
        tracing::info!("scheduler shutdown requested");
        self.inner
            .shutdown
            .store(true, std::sync::atomic::Ordering::Release);
        self.inner.notify.notify_one();
    }

    // -- Private helpers ----------------------------------------------------

    /// Move a task from `Pending` to `Queued` and push it onto the
    /// appropriate priority lane.
    fn enqueue(&self, id: TaskId, name: String, priority: TaskPriority, work: TaskFn) {
        // Update status to Queued.
        if let Some(mut entry) = self.inner.tasks.get_mut(&id) {
            // If the task was cancelled while waiting for its delay, skip.
            if entry.status == TaskStatus::Cancelled {
                tracing::debug!(task_id = %id, "skipping enqueue for cancelled task");
                return;
            }
            entry.status = TaskStatus::Queued;
        }

        let lane = priority as usize;
        self.inner.queues[lane].push(QueuedTask {
            id,
            name,
            priority,
            work,
        });
        self.inner.notify.notify_one();
    }

    /// Background worker loop.
    async fn worker_loop(inner: &SchedulerInner) {
        loop {
            // Try to dequeue work in priority order.
            let task = inner.queues[0]
                .pop()
                .or_else(|| inner.queues[1].pop())
                .or_else(|| inner.queues[2].pop())
                .or_else(|| inner.queues[3].pop());

            match task {
                Some(queued) => {
                    // Check if cancelled while queued.
                    let should_run = inner
                        .tasks
                        .get(&queued.id)
                        .map(|e| e.status == TaskStatus::Queued)
                        .unwrap_or(false);

                    if !should_run {
                        tracing::debug!(task_id = %queued.id, "skipping cancelled/removed task");
                        continue;
                    }

                    // Transition to Running.
                    if let Some(mut entry) = inner.tasks.get_mut(&queued.id) {
                        entry.status = TaskStatus::Running;
                        entry.started_at = Some(Utc::now());
                    }

                    tracing::info!(
                        task_id = %queued.id,
                        task_name = %queued.name,
                        priority = ?queued.priority,
                        "task running"
                    );

                    let future = (queued.work)();
                    let result = future.await;

                    if let Some(mut entry) = inner.tasks.get_mut(&queued.id) {
                        entry.completed_at = Some(Utc::now());
                        match result {
                            Ok(()) => {
                                entry.status = TaskStatus::Completed;
                                tracing::info!(task_id = %queued.id, "task completed");
                            }
                            Err(err) => {
                                entry.status = TaskStatus::Failed;
                                entry.error = Some(err.clone());
                                tracing::error!(
                                    task_id = %queued.id,
                                    error = %err,
                                    "task failed"
                                );
                            }
                        }
                    }
                }
                None => {
                    // Nothing to do.  Check for shutdown before sleeping.
                    if inner.shutdown.load(std::sync::atomic::Ordering::Acquire) {
                        break;
                    }
                    // Park until notified of new work or shutdown.
                    inner.notify.notified().await;

                    if inner.shutdown.load(std::sync::atomic::Ordering::Acquire) {
                        break;
                    }
                }
            }
        }
    }
}

impl Default for Scheduler {
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
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn submit_and_complete() {
        let scheduler = Scheduler::new();
        let handle = scheduler.start();

        let counter = Arc::new(AtomicU32::new(0));
        let c = Arc::clone(&counter);

        let id = scheduler
            .submit(
                "test-task",
                TaskPriority::Normal,
                Box::new(move || {
                    let c = Arc::clone(&c);
                    Box::pin(async move {
                        c.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    })
                }),
            )
            .expect("submit should succeed");

        // Give the worker time to process.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let info = scheduler.status(id).expect("task should exist");
        assert_eq!(info.status, TaskStatus::Completed);
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        scheduler.shutdown();
        handle.await.expect("worker should exit cleanly");
    }

    #[tokio::test]
    async fn priority_ordering() {
        let scheduler = Scheduler::new();

        let order = Arc::new(std::sync::Mutex::new(Vec::new()));

        // Submit low first, then critical -- critical should run first.
        let o1 = Arc::clone(&order);
        scheduler
            .submit(
                "low-task",
                TaskPriority::Low,
                Box::new(move || {
                    let o = Arc::clone(&o1);
                    Box::pin(async move {
                        o.lock().unwrap().push("low");
                        Ok(())
                    })
                }),
            )
            .expect("submit low");

        let o2 = Arc::clone(&order);
        scheduler
            .submit(
                "critical-task",
                TaskPriority::Critical,
                Box::new(move || {
                    let o = Arc::clone(&o2);
                    Box::pin(async move {
                        o.lock().unwrap().push("critical");
                        Ok(())
                    })
                }),
            )
            .expect("submit critical");

        // Start the worker *after* both tasks are queued so ordering is
        // deterministic.
        let handle = scheduler.start();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let result = order.lock().unwrap().clone();
        assert_eq!(result, vec!["critical", "low"]);

        scheduler.shutdown();
        handle.await.expect("worker exit");
    }

    #[tokio::test]
    async fn cancel_pending_task() {
        let scheduler = Scheduler::new();

        // Submit a delayed task and cancel it before it fires.
        let id = scheduler
            .submit_with_policy(
                "cancel-me",
                TaskPriority::Normal,
                SchedulePolicy::Delayed {
                    delay: Duration::from_secs(60),
                },
                Box::new(|| Box::pin(async { Ok(()) })),
            )
            .expect("submit");

        scheduler.cancel(id).expect("cancel should succeed");
        let info = scheduler.status(id).expect("task should exist");
        assert_eq!(info.status, TaskStatus::Cancelled);
    }

    #[tokio::test]
    async fn task_failure_is_recorded() {
        let scheduler = Scheduler::new();
        let handle = scheduler.start();

        let id = scheduler
            .submit(
                "fail-task",
                TaskPriority::Normal,
                Box::new(|| Box::pin(async { Err("boom".to_string()) })),
            )
            .expect("submit");

        tokio::time::sleep(Duration::from_millis(50)).await;

        let info = scheduler.status(id).expect("task should exist");
        assert_eq!(info.status, TaskStatus::Failed);
        assert_eq!(info.error.as_deref(), Some("boom"));

        scheduler.shutdown();
        handle.await.expect("worker exit");
    }

    #[tokio::test]
    async fn shutdown_rejects_new_work() {
        let scheduler = Scheduler::new();
        scheduler.shutdown();

        let result = scheduler.submit(
            "late-task",
            TaskPriority::Normal,
            Box::new(|| Box::pin(async { Ok(()) })),
        );
        assert!(matches!(result, Err(KernelError::SchedulerShutdown)));
    }
}
