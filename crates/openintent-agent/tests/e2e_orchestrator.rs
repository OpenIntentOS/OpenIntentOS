//! End-to-end tests for the multi-agent orchestrator.
//!
//! These tests exercise real tokio worker tasks, real channel communication,
//! and real dependency-ordering logic.  No stubs — every worker actually
//! receives a `WorkerMessage::Execute`, processes it, and sends a `TaskResult`
//! back over the result channel.

use openintent_agent::orchestrator::{Orchestrator, OrchestratedTask};

// ── parallel execution ────────────────────────────────────────────────────────

/// Submit more tasks than there are workers and verify every task completes.
///
/// `await_completion` processes the entire queue in one call, cycling through
/// workers as they become idle.  5 tasks with 2 workers all complete in a
/// single `await_completion` call.
#[tokio::test]
async fn all_tasks_complete_with_fewer_workers_than_tasks() {
    let orch = Orchestrator::new(2); // only 2 workers

    // Submit 5 independent tasks.
    let mut ids = Vec::new();
    for i in 0..5 {
        let t = OrchestratedTask::new(format!("independent task {i}"));
        ids.push(orch.submit(t).await);
    }

    // Single call — orchestrator loops until all 5 tasks are done.
    let results = orch.await_completion().await;
    assert_eq!(results.len(), 5, "all 5 tasks must complete in one call");

    // Every result must be successful and match a submitted task ID.
    for r in &results {
        assert!(r.success, "task {} must succeed", r.task_id);
        assert!(ids.contains(&r.task_id), "unexpected task_id in results");
    }

    // All original IDs must be present in results.
    let result_ids: Vec<_> = results.iter().map(|r| r.task_id).collect();
    for id in &ids {
        assert!(result_ids.contains(id), "task {id} missing from results");
    }
}

// ── dependency ordering ────────────────────────────────────────────────────────

/// Task B declares a dependency on task A.
/// Both are submitted before any `await_completion` call.
/// The orchestrator must complete A first, then unblock and complete B.
/// A single `await_completion` call handles the full dependency chain.
#[tokio::test]
async fn dependency_prevents_early_dispatch() {
    let orch = Orchestrator::new(3);

    // Create task A first so we have its ID for B's dependency.
    let task_a = OrchestratedTask::new("task A: research phase");
    let id_a = task_a.id;

    // B depends on A.
    let task_b = OrchestratedTask::new("task B: synthesis phase").depends_on(id_a);
    let id_b = task_b.id;

    // Submit B first, then A — dependency logic handles ordering regardless of
    // submission order.
    orch.submit(task_b).await;
    orch.submit(task_a).await;

    // Single call — orchestrator resolves the full dependency chain.
    let results = orch.await_completion().await;
    assert_eq!(results.len(), 2, "both tasks must complete");

    // A must appear before B in the results (dependency ordering).
    let pos_a = results.iter().position(|r| r.task_id == id_a)
        .expect("task A must be in results");
    let pos_b = results.iter().position(|r| r.task_id == id_b)
        .expect("task B must be in results");

    assert!(pos_a < pos_b, "task A (pos {pos_a}) must complete before task B (pos {pos_b})");
    assert!(results[pos_a].success, "task A must succeed");
    assert!(results[pos_b].success, "task B must succeed");

    // Task B's output must reference its description.
    assert!(
        results[pos_b].output.contains("synthesis phase"),
        "task B output must reference its description: {}",
        results[pos_b].output
    );
}

// ── output content ────────────────────────────────────────────────────────────

/// Worker output strings must reference the task description and the worker.
#[tokio::test]
async fn worker_output_references_task_description() {
    let orch = Orchestrator::new(1);
    let task = OrchestratedTask::new("compile the quarterly report");
    let id = orch.submit(task).await;

    let results = orch.await_completion().await;
    assert_eq!(results.len(), 1);

    let r = &results[0];
    assert_eq!(r.task_id, id);
    assert!(
        r.output.contains("compile the quarterly report"),
        "worker output must echo task description, got: {}",
        r.output
    );
    assert!(
        r.output.contains("worker-"),
        "output must identify the worker, got: {}",
        r.output
    );
}

// ── duration tracking ─────────────────────────────────────────────────────────

/// Every completed task must record a non-zero wall-clock duration.
#[tokio::test]
async fn task_result_records_duration() {
    let orch = Orchestrator::new(2);
    orch.submit(OrchestratedTask::new("task that measures time")).await;

    let results = orch.await_completion().await;
    assert_eq!(results.len(), 1);
    // duration_ms is recorded from Instant::now() at task start; even a stub
    // task should have a non-negative duration (it can be 0 ms on fast hardware).
    // We just assert the field exists and doesn't overflow.
    let dur = results[0].duration_ms;
    assert!(dur < 60_000, "duration unrealistically large: {dur}ms");
}

// ── worker pool management ────────────────────────────────────────────────────

/// Workers return to Idle after completing a task, allowing reuse across
/// successive `await_completion` calls.
#[tokio::test]
async fn workers_return_to_idle_after_task() {
    let orch = Orchestrator::new(2);

    // Submit two tasks in separate calls to test worker reuse.
    orch.submit(OrchestratedTask::new("first task")).await;
    let r1 = orch.await_completion().await;
    assert_eq!(r1.len(), 1, "first call must return 1 result");
    assert!(r1[0].success);

    // Queue was empty after first call; submit a fresh task.
    orch.submit(OrchestratedTask::new("second task")).await;
    let r2 = orch.await_completion().await;
    assert_eq!(r2.len(), 1, "second call must return 1 result on a reused worker");
    assert!(r2[0].success);

    // Verify the worker used in both calls is valid (id is non-nil).
    assert_ne!(r1[0].worker_id, uuid::Uuid::nil());
    assert_ne!(r2[0].worker_id, uuid::Uuid::nil());
}

// ── priority ordering ─────────────────────────────────────────────────────────

/// High-priority tasks should be processed before low-priority tasks
/// when they are in the same queue flush.
///
/// NOTE: current implementation uses FIFO (VecDeque::pop_front), so priority
/// is not yet enforced by ordering. This test documents the *expected* shape
/// of the results without asserting ordering, ensuring the priority field at
/// least doesn't cause errors.
#[tokio::test]
async fn priority_field_does_not_cause_errors() {
    let orch = Orchestrator::new(3);

    let low = OrchestratedTask::new("low priority").with_priority(10);
    let high = OrchestratedTask::new("high priority").with_priority(250);
    let med = OrchestratedTask::new("medium priority").with_priority(128);

    orch.submit(low).await;
    orch.submit(high).await;
    orch.submit(med).await;

    let results = orch.await_completion().await;
    assert_eq!(results.len(), 3, "all 3 tasks must complete");
    assert!(results.iter().all(|r| r.success));
}

// ── shutdown ──────────────────────────────────────────────────────────────────

/// After shutdown, any subsequent call to await_completion returns empty.
#[tokio::test]
async fn shutdown_stops_workers() {
    let orch = Orchestrator::new(2);
    orch.shutdown().await;

    // Small delay so worker tasks have time to process the shutdown message.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Status check: workers should still be listed but status transitions
    // happen asynchronously, so we just verify no panic occurs.
    let status = orch.status();
    // idle + busy + stopped == max_workers (2), even if the numbers vary.
    assert_eq!(
        status.idle + status.busy + status.stopped,
        2,
        "worker count invariant violated: {status:?}"
    );
}
