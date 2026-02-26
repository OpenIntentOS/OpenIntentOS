//! Multi-agent orchestrator implementing a master-worker pattern.
//!
//! The orchestrator decomposes high-level goals into subtasks and dispatches
//! them to a pool of specialised worker agents.  Workers run concurrently and
//! the orchestrator collects their results into a consolidated output.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{Mutex, mpsc};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::error::AgentError;

// ---------------------------------------------------------------------------
// Worker types
// ---------------------------------------------------------------------------

/// The kind of work a worker agent specialises in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerSpecialization {
    /// General-purpose agent — handles any task.
    General,
    /// Optimised for web research and information synthesis.
    Research,
    /// Executes code and interprets results.
    CodeExecution,
    /// Reads and classifies emails.
    EmailProcessing,
    /// Analyses structured data and produces summaries.
    DataAnalysis,
}

impl std::fmt::Display for WorkerSpecialization {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::General => "general",
            Self::Research => "research",
            Self::CodeExecution => "code-execution",
            Self::EmailProcessing => "email-processing",
            Self::DataAnalysis => "data-analysis",
        };
        write!(f, "{s}")
    }
}

/// Current worker lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerStatus {
    /// Worker is idle and ready to accept tasks.
    Idle,
    /// Worker is actively processing a task.
    Busy,
    /// Worker has exited or encountered a fatal error.
    Stopped,
}

/// Message sent to a worker over its channel.
#[derive(Debug)]
pub enum WorkerMessage {
    /// Assign a task to this worker.
    Execute(OrchestratedTask),
    /// Ask the worker to shut down cleanly.
    Shutdown,
}

/// A handle the orchestrator holds for each worker.
pub struct WorkerHandle {
    /// Unique identifier for this worker.
    pub id: Uuid,
    /// Human-readable name (e.g. "worker-0").
    pub name: String,
    /// What this worker is specialised for.
    pub specialization: WorkerSpecialization,
    /// Current lifecycle status.
    pub status: WorkerStatus,
    /// Channel for sending messages to the worker task.
    pub(crate) tx: mpsc::Sender<WorkerMessage>,
}

impl std::fmt::Debug for WorkerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerHandle")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("specialization", &self.specialization)
            .field("status", &self.status)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Task types
// ---------------------------------------------------------------------------

/// A single unit of work managed by the orchestrator.
#[derive(Debug, Clone)]
pub struct OrchestratedTask {
    /// Unique task identifier.
    pub id: Uuid,
    /// Human-readable description of what must be done.
    pub description: String,
    /// If `Some`, only this worker should process the task.
    pub assigned_to: Option<Uuid>,
    /// Task IDs that must complete before this one can start.
    pub dependencies: Vec<Uuid>,
    /// Scheduling priority: higher value = higher priority (0–255).
    pub priority: u8,
    /// When the task was created.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl OrchestratedTask {
    /// Create a new task with auto-generated ID and current timestamp.
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            id: Uuid::now_v7(),
            description: description.into(),
            assigned_to: None,
            dependencies: Vec::new(),
            priority: 128,
            created_at: chrono::Utc::now(),
        }
    }

    /// Builder: set priority.
    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority;
        self
    }

    /// Builder: add a dependency.
    pub fn depends_on(mut self, task_id: Uuid) -> Self {
        self.dependencies.push(task_id);
        self
    }
}

/// Result produced by a worker after completing a task.
#[derive(Debug, Clone)]
pub struct TaskResult {
    /// ID of the task that was executed.
    pub task_id: Uuid,
    /// ID of the worker that ran the task.
    pub worker_id: Uuid,
    /// Text output from the task.
    pub output: String,
    /// Whether the task completed without errors.
    pub success: bool,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
}

// ---------------------------------------------------------------------------
// Orchestrator status
// ---------------------------------------------------------------------------

/// A snapshot of the orchestrator's current state.
#[derive(Debug)]
pub struct OrchestratorStatus {
    /// Number of workers in each status.
    pub idle: usize,
    pub busy: usize,
    pub stopped: usize,
    /// Tasks currently queued (not yet dispatched).
    pub queued: usize,
    /// Tasks that have completed.
    pub completed: usize,
}

// ---------------------------------------------------------------------------
// Orchestrator
// ---------------------------------------------------------------------------

/// Manages a pool of worker agents and a queue of tasks.
///
/// The master/orchestrator brain decomposes high-level goals, submits tasks
/// to the queue, and collects results.  Workers run as independent tokio tasks
/// and communicate via channels.
pub struct Orchestrator {
    /// Worker pool (mutable state protected by `Mutex`).
    workers: Arc<Mutex<Vec<WorkerHandle>>>,
    /// Tasks waiting to be dispatched.
    task_queue: Arc<Mutex<VecDeque<OrchestratedTask>>>,
    /// Completed task results keyed by task ID.
    results: Arc<Mutex<HashMap<Uuid, TaskResult>>>,
    /// Sender side of the result channel (workers publish here).
    result_tx: mpsc::Sender<TaskResult>,
    /// Receiver side (orchestrator collects here).
    result_rx: Arc<Mutex<mpsc::Receiver<TaskResult>>>,
}

impl Orchestrator {
    /// Create a new orchestrator with `max_workers` general-purpose workers.
    pub fn new(max_workers: usize) -> Self {
        let (result_tx, result_rx) = mpsc::channel(256);

        let mut workers = Vec::with_capacity(max_workers);
        for i in 0..max_workers {
            let (tx, mut rx) = mpsc::channel::<WorkerMessage>(32);
            let worker_id = Uuid::now_v7();
            let result_tx_clone = result_tx.clone();
            let worker_name = format!("worker-{i}");
            let wname = worker_name.clone();

            // Spawn the worker tokio task.
            tokio::spawn(async move {
                debug!(worker = %wname, "worker started");
                while let Some(msg) = rx.recv().await {
                    match msg {
                        WorkerMessage::Execute(task) => {
                            let start = Instant::now();
                            info!(
                                worker = %wname,
                                task_id = %task.id,
                                desc = %task.description,
                                "executing task"
                            );

                            // Stub execution: in production this calls the LLM.
                            let output = format!(
                                "[{}] Task completed: {}",
                                wname, task.description
                            );
                            let duration_ms = start.elapsed().as_millis() as u64;

                            let result = TaskResult {
                                task_id: task.id,
                                worker_id,
                                output,
                                success: true,
                                duration_ms,
                            };

                            if result_tx_clone.send(result).await.is_err() {
                                warn!(worker = %wname, "result channel closed");
                                break;
                            }
                        }
                        WorkerMessage::Shutdown => {
                            debug!(worker = %wname, "worker shutting down");
                            break;
                        }
                    }
                }
            });

            workers.push(WorkerHandle {
                id: worker_id,
                name: worker_name,
                specialization: WorkerSpecialization::General,
                status: WorkerStatus::Idle,
                tx,
            });
        }

        Self {
            workers: Arc::new(Mutex::new(workers)),
            task_queue: Arc::new(Mutex::new(VecDeque::new())),
            results: Arc::new(Mutex::new(HashMap::new())),
            result_tx,
            result_rx: Arc::new(Mutex::new(result_rx)),
        }
    }

    /// Decompose a high-level goal into a list of subtasks.
    ///
    /// Currently returns a stub decomposition.  In production this would call
    /// the LLM with a structured decomposition prompt.
    pub async fn decompose_goal(
        &self,
        goal: &str,
        _llm: &crate::llm::LlmClient,
    ) -> Result<Vec<OrchestratedTask>, AgentError> {
        // Stub: split goal into two tasks for demonstration.
        info!(goal = %goal, "decomposing goal into subtasks (stub)");

        let research = OrchestratedTask::new(format!("Research: {goal}")).with_priority(200);
        let synthesize = OrchestratedTask::new(format!("Synthesize findings for: {goal}"))
            .with_priority(150)
            .depends_on(research.id);

        Ok(vec![research, synthesize])
    }

    /// Submit a task to the queue and return its ID.
    pub async fn submit(&self, task: OrchestratedTask) -> Uuid {
        let id = task.id;
        info!(task_id = %id, desc = %task.description, "task submitted");
        self.task_queue.lock().await.push_back(task);
        id
    }

    /// Dispatch all queued tasks to available workers and wait for completion.
    ///
    /// Handles dependency ordering: tasks are only dispatched when all their
    /// declared dependency task IDs are present in the completed-results map.
    /// The algorithm interleaves dispatch and collection so that completing
    /// a task can unblock dependent tasks in the same call.
    ///
    /// Returns all `TaskResult` values collected during this call.
    pub async fn await_completion(&self) -> Vec<TaskResult> {
        let mut all_collected: Vec<TaskResult> = Vec::new();

        loop {
            // ── dispatch phase ───────────────────────────────────────────────
            // Scan the queue and dispatch every task whose deps are already
            // in the results map and which has an idle worker available.
            let dispatched = self.dispatch_ready_tasks().await;

            // ── termination check ────────────────────────────────────────────
            let queue_len = self.task_queue.lock().await.len();

            if dispatched == 0 && queue_len == 0 {
                // Nothing dispatched, nothing queued — we're done.
                break;
            }

            if dispatched == 0 && queue_len > 0 {
                // Tasks remain but none are dispatchable yet (deps unmet or no
                // idle workers).  Collect one result to make progress.
                if let Some(result) = self.collect_one_result().await {
                    all_collected.push(result);
                } else {
                    // Channel closed — nothing more to collect.
                    break;
                }
                continue;
            }

            // ── collection phase ─────────────────────────────────────────────
            // Collect exactly as many results as we just dispatched.
            for _ in 0..dispatched {
                if let Some(result) = self.collect_one_result().await {
                    all_collected.push(result);
                } else {
                    break;
                }
            }
        }

        all_collected
    }

    /// Scan the task queue and dispatch all tasks whose dependencies are met
    /// to available idle workers.  Returns the number of tasks dispatched.
    async fn dispatch_ready_tasks(&self) -> usize {
        let mut dispatched = 0usize;
        let mut queue = self.task_queue.lock().await;
        let mut workers = self.workers.lock().await;
        let results = self.results.lock().await;

        let mut i = 0;
        while i < queue.len() {
            // Check if all dependencies are satisfied.
            let deps_met = queue[i]
                .dependencies
                .iter()
                .all(|dep| results.contains_key(dep));

            if !deps_met {
                i += 1;
                continue;
            }

            // Find an idle worker.
            if let Some(worker) = workers.iter_mut().find(|w| w.status == WorkerStatus::Idle) {
                let task = queue.remove(i).expect("index in bounds");
                worker.status = WorkerStatus::Busy;
                if worker.tx.send(WorkerMessage::Execute(task)).await.is_ok() {
                    dispatched += 1;
                }
                // Do not increment i — an element was removed.
            } else {
                // No idle workers — stop scanning.
                break;
            }
        }

        dispatched
    }

    /// Wait for exactly one result from the result channel, record it in the
    /// results map, mark the worker idle, and return the result.
    async fn collect_one_result(&self) -> Option<TaskResult> {
        let result = self.result_rx.lock().await.recv().await?;

        // Mark the worker that produced this result as idle.
        {
            let mut workers = self.workers.lock().await;
            if let Some(w) = workers.iter_mut().find(|w| w.id == result.worker_id) {
                w.status = WorkerStatus::Idle;
            }
        }

        // Record in the completed-results map so dependents can be unblocked.
        self.results
            .lock()
            .await
            .insert(result.task_id, result.clone());

        Some(result)
    }

    /// Return a snapshot of the current orchestrator status.
    pub fn status(&self) -> OrchestratorStatus {
        // Use `try_lock` to avoid blocking in sync context.
        let (idle, busy, stopped) = if let Ok(workers) = self.workers.try_lock() {
            let idle = workers.iter().filter(|w| w.status == WorkerStatus::Idle).count();
            let busy = workers.iter().filter(|w| w.status == WorkerStatus::Busy).count();
            let stopped = workers.iter().filter(|w| w.status == WorkerStatus::Stopped).count();
            (idle, busy, stopped)
        } else {
            (0, 0, 0)
        };

        let queued = self
            .task_queue
            .try_lock()
            .map(|q| q.len())
            .unwrap_or(0);

        let completed = self
            .results
            .try_lock()
            .map(|r| r.len())
            .unwrap_or(0);

        OrchestratorStatus { idle, busy, stopped, queued, completed }
    }

    /// Send shutdown signal to all workers.
    pub async fn shutdown(&self) {
        let workers = self.workers.lock().await;
        for worker in workers.iter() {
            let _ = worker.tx.send(WorkerMessage::Shutdown).await;
        }
        info!("orchestrator shutdown signal sent to all workers");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn orchestrator_creates_workers() {
        let orch = Orchestrator::new(3);
        let status = orch.status();
        assert_eq!(status.idle, 3);
        assert_eq!(status.busy, 0);
    }

    #[tokio::test]
    async fn submit_and_complete_task() {
        let orch = Orchestrator::new(2);
        let task = OrchestratedTask::new("Test task");
        let task_id = orch.submit(task).await;

        let results = orch.await_completion().await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].task_id, task_id);
        assert!(results[0].success);
    }

    #[test]
    fn orchestrated_task_builder() {
        let dep_id = Uuid::now_v7();
        let task = OrchestratedTask::new("Build report")
            .with_priority(200)
            .depends_on(dep_id);
        assert_eq!(task.priority, 200);
        assert!(task.dependencies.contains(&dep_id));
    }
}
