//! Integration tests for the openintent-kernel crate.
//!
//! These tests exercise the scheduler, IPC bus, intent router, and adapter
//! registry as integrated subsystems.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use openintent_kernel::{
    AdapterRegistry, AdapterStatus, Event, IntentRouter, IpcBus, RouteResult, Scheduler,
    TaskPriority, TaskStatus,
};

// ═══════════════════════════════════════════════════════════════════════
//  Scheduler integration
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn scheduler_submit_multiple_tasks() {
    let scheduler = Scheduler::new();
    let handle = scheduler.start();

    let counter = Arc::new(AtomicU32::new(0));

    // Submit 5 tasks.
    let mut ids = Vec::new();
    for i in 0..5u32 {
        let c = Arc::clone(&counter);
        let id = scheduler
            .submit(
                format!("task-{i}"),
                TaskPriority::Normal,
                Box::new(move || {
                    let c = Arc::clone(&c);
                    Box::pin(async move {
                        c.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    })
                }),
            )
            .unwrap();
        ids.push(id);
    }

    // Wait for all tasks to complete.
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(counter.load(Ordering::SeqCst), 5);

    for id in &ids {
        let info = scheduler.status(*id).unwrap();
        assert_eq!(info.status, TaskStatus::Completed);
        assert!(info.started_at.is_some());
        assert!(info.completed_at.is_some());
    }

    scheduler.shutdown();
    handle.await.unwrap();
}

#[tokio::test]
async fn scheduler_all_tasks_snapshot() {
    let scheduler = Scheduler::new();
    let handle = scheduler.start();

    let c = Arc::new(AtomicU32::new(0));
    let c2 = Arc::clone(&c);

    scheduler
        .submit(
            "task-a",
            TaskPriority::High,
            Box::new(move || {
                let c = Arc::clone(&c2);
                Box::pin(async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            }),
        )
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let all = scheduler.all_tasks();
    assert_eq!(all.len(), 1);
    let (_, info) = all.iter().next().unwrap();
    assert_eq!(info.name, "task-a");
    assert_eq!(info.priority, TaskPriority::High);

    scheduler.shutdown();
    handle.await.unwrap();
}

#[tokio::test]
async fn scheduler_failed_task_preserves_error() {
    let scheduler = Scheduler::new();
    let handle = scheduler.start();

    let id = scheduler
        .submit(
            "fail-task",
            TaskPriority::Normal,
            Box::new(|| Box::pin(async { Err("something went wrong".to_string()) })),
        )
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let info = scheduler.status(id).unwrap();
    assert_eq!(info.status, TaskStatus::Failed);
    assert_eq!(info.error.as_deref(), Some("something went wrong"));

    scheduler.shutdown();
    handle.await.unwrap();
}

#[tokio::test]
async fn scheduler_cancel_before_execution() {
    let scheduler = Scheduler::new();

    // Submit a delayed task so it does not execute immediately.
    let id = scheduler
        .submit_with_policy(
            "cancel-me",
            TaskPriority::Normal,
            openintent_kernel::SchedulePolicy::Delayed {
                delay: Duration::from_secs(300),
            },
            Box::new(|| Box::pin(async { Ok(()) })),
        )
        .unwrap();

    scheduler.cancel(id).unwrap();
    let info = scheduler.status(id).unwrap();
    assert_eq!(info.status, TaskStatus::Cancelled);
}

// ═══════════════════════════════════════════════════════════════════════
//  IPC bus integration
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn ipc_bus_publish_and_receive_multiple_events() {
    let bus = IpcBus::new(64);
    let mut rx = bus.subscribe();

    // Publish several events.
    bus.publish(Event::SystemEvent {
        kind: "start".into(),
        message: "kernel starting".into(),
    })
    .unwrap();

    bus.publish(Event::SystemEvent {
        kind: "ready".into(),
        message: "kernel ready".into(),
    })
    .unwrap();

    let e1 = rx.recv().await.unwrap();
    let e2 = rx.recv().await.unwrap();

    match e1.as_ref() {
        Event::SystemEvent { kind, .. } => assert_eq!(kind, "start"),
        other => panic!("unexpected event: {other:?}"),
    }

    match e2.as_ref() {
        Event::SystemEvent { kind, .. } => assert_eq!(kind, "ready"),
        other => panic!("unexpected event: {other:?}"),
    }
}

#[tokio::test]
async fn ipc_bus_multiple_subscribers_receive_same_event() {
    let bus = IpcBus::new(16);
    let mut rx1 = bus.subscribe();
    let mut rx2 = bus.subscribe();
    let mut rx3 = bus.subscribe();

    assert_eq!(bus.subscriber_count(), 3);

    bus.publish(Event::SystemEvent {
        kind: "broadcast".into(),
        message: "hello all".into(),
    })
    .unwrap();

    let e1 = rx1.recv().await.unwrap();
    let e2 = rx2.recv().await.unwrap();
    let e3 = rx3.recv().await.unwrap();

    // All subscribers receive the same Arc (pointer equality).
    assert!(Arc::ptr_eq(&e1, &e2));
    assert!(Arc::ptr_eq(&e2, &e3));
}

#[tokio::test]
async fn ipc_bus_task_status_changed_event() {
    let bus = IpcBus::new(16);
    let mut rx = bus.subscribe();

    let task_id = uuid::Uuid::now_v7();
    bus.publish(Event::TaskStatusChanged {
        task_id,
        task_name: "file-read".into(),
        new_status: "Completed".into(),
        timestamp: chrono::Utc::now(),
    })
    .unwrap();

    let received = rx.recv().await.unwrap();
    match received.as_ref() {
        Event::TaskStatusChanged {
            task_id: id,
            task_name,
            new_status,
            ..
        } => {
            assert_eq!(*id, task_id);
            assert_eq!(task_name, "file-read");
            assert_eq!(new_status, "Completed");
        }
        other => panic!("expected TaskStatusChanged, got {other:?}"),
    }
}

#[tokio::test]
async fn ipc_bus_subscriber_count_tracks_drops() {
    let bus = IpcBus::new(16);
    assert_eq!(bus.subscriber_count(), 0);

    let rx1 = bus.subscribe();
    assert_eq!(bus.subscriber_count(), 1);

    let rx2 = bus.subscribe();
    assert_eq!(bus.subscriber_count(), 2);

    drop(rx1);
    assert_eq!(bus.subscriber_count(), 1);

    drop(rx2);
    assert_eq!(bus.subscriber_count(), 0);
}

// ═══════════════════════════════════════════════════════════════════════
//  Intent router integration
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn intent_router_full_cascade() {
    let mut router = IntentRouter::new();

    // Level 1: Exact matches.
    router.add_exact("open feishu", "adapter:feishu:open");
    router.add_exact("check email", "adapter:email:check");

    // Level 2: Pattern matches.
    router
        .add_pattern(
            r"send (?:a )?message to (?P<recipient>\S+)",
            "adapter:messaging:send",
        )
        .unwrap();
    router
        .add_pattern(r"search (?:for )?(?P<query>.+)", "adapter:web:search")
        .unwrap();

    assert_eq!(router.exact_count(), 2);
    assert_eq!(router.pattern_count(), 2);

    // L1: Exact match (case-insensitive).
    let result = router.route("Open Feishu");
    match &result {
        RouteResult::ExactMatch { handler, .. } => {
            assert_eq!(handler, "adapter:feishu:open");
        }
        other => panic!("expected ExactMatch, got {other:?}"),
    }

    // L2: Pattern match with captures.
    let result = router.route("send a message to alice");
    match &result {
        RouteResult::PatternMatch { handler, captures } => {
            assert_eq!(handler, "adapter:messaging:send");
            assert_eq!(captures.get("recipient").map(String::as_str), Some("alice"));
        }
        other => panic!("expected PatternMatch, got {other:?}"),
    }

    // L2: Another pattern match.
    let result = router.route("search for rust programming");
    match &result {
        RouteResult::PatternMatch { handler, captures } => {
            assert_eq!(handler, "adapter:web:search");
            assert!(captures["query"].contains("rust programming"));
        }
        other => panic!("expected PatternMatch, got {other:?}"),
    }

    // L3: LLM fallback for unmatched intents.
    let result = router.route("analyze my quarterly sales data");
    match &result {
        RouteResult::LlmFallback { intent } => {
            assert_eq!(intent, "analyze my quarterly sales data");
        }
        other => panic!("expected LlmFallback, got {other:?}"),
    }
}

#[test]
fn intent_router_exact_takes_precedence() {
    let mut router = IntentRouter::new();
    router.add_exact("check email", "adapter:email:check");
    router
        .add_pattern(r"check (?P<what>\S+)", "generic:check")
        .unwrap();

    let result = router.route("check email");
    assert!(
        matches!(result, RouteResult::ExactMatch { .. }),
        "exact match should take precedence over pattern match"
    );
}

#[test]
fn intent_router_dynamic_route_addition() {
    let mut router = IntentRouter::new();

    // Initially no routes.
    let result = router.route("hello");
    assert!(matches!(result, RouteResult::LlmFallback { .. }));

    // Add a route at runtime.
    router.add_exact("hello", "greet:handler");

    // Now it matches.
    let result = router.route("hello");
    match result {
        RouteResult::ExactMatch { handler, .. } => assert_eq!(handler, "greet:handler"),
        other => panic!("expected ExactMatch, got {other:?}"),
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  Adapter registry integration
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn adapter_registry_full_lifecycle() {
    let registry = AdapterRegistry::new();

    // Register adapters.
    registry.register("email", "Email IMAP/SMTP adapter");
    registry.register("filesystem", "Local filesystem adapter");
    registry.register("shell", "Shell command adapter");

    assert_eq!(registry.count(), 3);

    // All start as Registered.
    let all = registry.list_all();
    assert_eq!(all.len(), 3);
    for info in &all {
        assert_eq!(info.status, AdapterStatus::Registered);
    }

    // Connect some adapters.
    registry
        .set_status("email", AdapterStatus::Connected)
        .unwrap();
    registry
        .set_status("filesystem", AdapterStatus::Connected)
        .unwrap();

    assert!(registry.is_available("email"));
    assert!(registry.is_available("filesystem"));
    assert!(!registry.is_available("shell"));

    // List by status.
    let connected = registry.list_by_status(AdapterStatus::Connected);
    assert_eq!(connected.len(), 2);

    // Error state.
    registry.set_error("email", "connection timeout").unwrap();
    let info = registry.get("email").unwrap();
    assert_eq!(info.status, AdapterStatus::Error);
    assert_eq!(info.last_error.as_deref(), Some("connection timeout"));
    assert!(!registry.is_available("email"));

    // Recovery clears error message.
    registry
        .set_status("email", AdapterStatus::Connected)
        .unwrap();
    let info = registry.get("email").unwrap();
    assert_eq!(info.status, AdapterStatus::Connected);
    assert!(info.last_error.is_none());

    // Health check recording.
    registry.record_health_check("email").unwrap();
    let info = registry.get("email").unwrap();
    assert!(info.last_health_check.is_some());

    // Unregister.
    let removed = registry.unregister("shell");
    assert!(removed.is_some());
    assert_eq!(registry.count(), 2);
    assert!(registry.get("shell").is_err());

    // List IDs.
    let ids = registry.list_ids();
    assert_eq!(ids.len(), 2);
}

#[test]
fn adapter_registry_not_found_error() {
    let registry = AdapterRegistry::new();
    let result = registry.get("nonexistent");
    assert!(result.is_err());

    let result = registry.set_status("nonexistent", AdapterStatus::Connected);
    assert!(result.is_err());
}
