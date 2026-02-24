//! Zero-copy IPC bus.
//!
//! The IPC bus provides a lightweight publish/subscribe mechanism built on top
//! of [`tokio::sync::broadcast`].  All kernel subsystems communicate through
//! [`Event`]s published to the bus.
//!
//! Events are wrapped in [`Arc`] so that broadcasting to multiple subscribers
//! does not require cloning the payload.  For truly zero-copy scenarios across
//! crate boundaries, payloads can carry `rkyv`-serialized bytes that receivers
//! access without deserialization.
//!
//! # Usage
//!
//! ```rust,no_run
//! # use openintent_kernel::ipc::{IpcBus, Event};
//! # async fn example() {
//! let bus = IpcBus::new(256);
//! let mut rx = bus.subscribe();
//!
//! bus.publish(Event::SystemEvent {
//!     kind: "startup".into(),
//!     message: "kernel initialized".into(),
//! }).unwrap();
//!
//! let event = rx.recv().await.unwrap();
//! # }
//! ```

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::error::Result;

// ---------------------------------------------------------------------------
// Event types
// ---------------------------------------------------------------------------

/// An event that flows through the IPC bus.
///
/// Every variant carries enough context for subscribers to filter and dispatch
/// without needing to parse opaque blobs.  New variants should be added here
/// as the kernel grows; backward compatibility is maintained by treating
/// unknown variants as `SystemEvent` in older subscribers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    /// A new natural-language intent was received from the user or a trigger.
    IntentReceived {
        /// Unique identifier for this intent.
        intent_id: Uuid,
        /// Raw text of the intent.
        text: String,
        /// When the intent was received.
        timestamp: DateTime<Utc>,
    },

    /// A scheduler task changed state.
    TaskStatusChanged {
        /// The task whose status changed.
        task_id: Uuid,
        /// Human-readable task name.
        task_name: String,
        /// New status as a string (e.g. "Running", "Completed").
        new_status: String,
        /// When the transition occurred.
        timestamp: DateTime<Utc>,
    },

    /// An adapter emitted an event (e.g. incoming message, webhook payload).
    AdapterEvent {
        /// Which adapter produced this event.
        adapter_id: String,
        /// Adapter-specific event kind.
        kind: String,
        /// JSON-serialized payload.
        payload: String,
        timestamp: DateTime<Utc>,
    },

    /// An adapter or subsystem requires authentication before it can proceed.
    AuthRequired {
        /// The service that needs authentication.
        adapter_id: String,
        /// Human-readable reason.
        reason: String,
        timestamp: DateTime<Utc>,
    },

    /// Generic system-level event for anything that does not fit the above.
    SystemEvent {
        /// A short, machine-readable event kind (e.g. "startup", "shutdown").
        kind: String,
        /// Human-readable description.
        message: String,
    },
}

// ---------------------------------------------------------------------------
// IPC Bus
// ---------------------------------------------------------------------------

/// Publish/subscribe event bus backed by [`tokio::sync::broadcast`].
///
/// The bus is cheaply cloneable (`Arc`-backed) and `Send + Sync`.  Subscribers
/// receive [`Arc<Event>`] references, avoiding per-subscriber cloning of the
/// event payload.
#[derive(Clone)]
pub struct IpcBus {
    inner: Arc<IpcBusInner>,
}

struct IpcBusInner {
    sender: broadcast::Sender<Arc<Event>>,
}

impl IpcBus {
    /// Create a new bus with the given channel capacity.
    ///
    /// If a subscriber falls behind by more than `capacity` events, it will
    /// receive a [`broadcast::error::RecvError::Lagged`] error indicating how
    /// many events were missed.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            inner: Arc::new(IpcBusInner { sender }),
        }
    }

    /// Publish an event to all current subscribers.
    ///
    /// Returns the number of receivers that will observe this event.  If there
    /// are no active subscribers the event is silently dropped (this is not
    /// considered an error during early startup).
    pub fn publish(&self, event: Event) -> Result<usize> {
        let event = Arc::new(event);
        match self.inner.sender.send(event) {
            Ok(n) => {
                tracing::trace!(receivers = n, "event published to ipc bus");
                Ok(n)
            }
            Err(_) => {
                // No active receivers -- this is common during startup/shutdown.
                tracing::trace!("event published but no active receivers");
                Ok(0)
            }
        }
    }

    /// Create a new subscriber that will receive all future events.
    ///
    /// Events published *before* this call are **not** replayed.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Event>> {
        tracing::trace!("new ipc subscriber created");
        self.inner.sender.subscribe()
    }

    /// Return the current number of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.inner.sender.receiver_count()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publish_and_receive() {
        let bus = IpcBus::new(16);
        let mut rx = bus.subscribe();

        let event = Event::SystemEvent {
            kind: "test".into(),
            message: "hello".into(),
        };

        let receivers = bus.publish(event).expect("publish should succeed");
        assert_eq!(receivers, 1);

        let received = rx.recv().await.expect("should receive event");
        match received.as_ref() {
            Event::SystemEvent { kind, message } => {
                assert_eq!(kind, "test");
                assert_eq!(message, "hello");
            }
            other => panic!("unexpected event variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn multiple_subscribers() {
        let bus = IpcBus::new(16);
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        bus.publish(Event::SystemEvent {
            kind: "multi".into(),
            message: "broadcast".into(),
        })
        .expect("publish");

        let e1 = rx1.recv().await.expect("rx1");
        let e2 = rx2.recv().await.expect("rx2");

        // Both subscribers receive the same Arc (pointer equality).
        assert!(Arc::ptr_eq(&e1, &e2));
    }

    #[tokio::test]
    async fn publish_with_no_subscribers_is_ok() {
        let bus = IpcBus::new(16);
        let result = bus.publish(Event::SystemEvent {
            kind: "lonely".into(),
            message: "no one listening".into(),
        });
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    #[tokio::test]
    async fn subscriber_count() {
        let bus = IpcBus::new(16);
        assert_eq!(bus.subscriber_count(), 0);

        let _rx1 = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 1);

        let _rx2 = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 2);

        drop(_rx1);
        assert_eq!(bus.subscriber_count(), 1);
    }

    #[tokio::test]
    async fn intent_received_event() {
        let bus = IpcBus::new(16);
        let mut rx = bus.subscribe();

        let intent_id = Uuid::now_v7();
        bus.publish(Event::IntentReceived {
            intent_id,
            text: "send an email to Alice".into(),
            timestamp: Utc::now(),
        })
        .expect("publish");

        let received = rx.recv().await.expect("receive");
        match received.as_ref() {
            Event::IntentReceived {
                intent_id: id,
                text,
                ..
            } => {
                assert_eq!(*id, intent_id);
                assert_eq!(text, "send an email to Alice");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}
