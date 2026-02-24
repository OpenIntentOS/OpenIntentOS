//! Trigger system — define when and how workflows are activated.
//!
//! Triggers determine how a workflow starts: manually, on a cron schedule,
//! or in response to a system event.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tracing::{debug, info};
use uuid::Uuid;

use crate::error::{IntentError, Result};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// How a workflow is triggered.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum TriggerType {
    /// Triggered manually by the user.
    #[default]
    Manual,

    /// Triggered on a cron-like schedule.
    ///
    /// The `expression` field uses a simplified cron format:
    /// `minute hour day_of_month month day_of_week`
    ///
    /// Full cron parsing will be integrated via the `cron` crate in a
    /// future version.
    Cron {
        /// Cron expression string.
        expression: String,
    },

    /// Triggered in response to a named system event.
    Event {
        /// The event name to listen for (e.g. "file_changed", "task_completed").
        event_name: String,
    },
}

impl std::fmt::Display for TriggerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Manual => write!(f, "manual"),
            Self::Cron { expression } => write!(f, "cron({expression})"),
            Self::Event { event_name } => write!(f, "event({event_name})"),
        }
    }
}

/// A registered trigger with metadata.
#[derive(Debug, Clone)]
struct RegisteredTrigger {
    /// The trigger configuration.
    trigger: TriggerType,
    /// The workflow ID this trigger activates.
    workflow_id: Uuid,
    /// Whether this trigger is currently active.
    active: bool,
}

// ---------------------------------------------------------------------------
// Trigger manager
// ---------------------------------------------------------------------------

/// Manages trigger registration and firing.
///
/// The trigger manager maintains a registry of triggers and their associated
/// workflow IDs.  When a trigger fires (either via cron tick or event
/// dispatch), the associated workflow is queued for execution.
pub struct TriggerManager {
    /// Registered triggers, keyed by a unique trigger ID.
    triggers: HashMap<Uuid, RegisteredTrigger>,
}

impl TriggerManager {
    /// Create a new, empty trigger manager.
    pub fn new() -> Self {
        Self {
            triggers: HashMap::new(),
        }
    }

    /// Register a new trigger for the given workflow.
    ///
    /// Returns the unique trigger ID.
    pub fn register(&mut self, workflow_id: Uuid, trigger: TriggerType) -> Result<Uuid> {
        // Validate cron expressions (basic check for now).
        if let TriggerType::Cron { ref expression } = trigger {
            Self::validate_cron(expression)?;
        }

        let trigger_id = Uuid::now_v7();
        info!(
            trigger_id = %trigger_id,
            workflow_id = %workflow_id,
            trigger_type = %trigger,
            "registering trigger"
        );

        self.triggers.insert(
            trigger_id,
            RegisteredTrigger {
                trigger,
                workflow_id,
                active: true,
            },
        );

        Ok(trigger_id)
    }

    /// Unregister a trigger by its ID.
    pub fn unregister(&mut self, trigger_id: &Uuid) -> Result<()> {
        if self.triggers.remove(trigger_id).is_none() {
            return Err(IntentError::TriggerRegistrationFailed {
                reason: format!("trigger {trigger_id} not found"),
            });
        }
        info!(trigger_id = %trigger_id, "trigger unregistered");
        Ok(())
    }

    /// Deactivate a trigger without removing it.
    pub fn deactivate(&mut self, trigger_id: &Uuid) -> Result<()> {
        let trigger = self.triggers.get_mut(trigger_id).ok_or_else(|| {
            IntentError::TriggerRegistrationFailed {
                reason: format!("trigger {trigger_id} not found"),
            }
        })?;
        trigger.active = false;
        debug!(trigger_id = %trigger_id, "trigger deactivated");
        Ok(())
    }

    /// Reactivate a previously deactivated trigger.
    pub fn activate(&mut self, trigger_id: &Uuid) -> Result<()> {
        let trigger = self.triggers.get_mut(trigger_id).ok_or_else(|| {
            IntentError::TriggerRegistrationFailed {
                reason: format!("trigger {trigger_id} not found"),
            }
        })?;
        trigger.active = true;
        debug!(trigger_id = %trigger_id, "trigger activated");
        Ok(())
    }

    /// Fire all triggers that match a given event name.
    ///
    /// Returns the list of workflow IDs that should be executed.
    pub fn fire_event(&self, event_name: &str) -> Vec<Uuid> {
        let mut workflow_ids = Vec::new();

        for (trigger_id, registered) in &self.triggers {
            if !registered.active {
                continue;
            }
            if let TriggerType::Event {
                event_name: ref name,
            } = registered.trigger
                && name == event_name
            {
                debug!(
                    trigger_id = %trigger_id,
                    workflow_id = %registered.workflow_id,
                    event = event_name,
                    "event trigger fired"
                );
                workflow_ids.push(registered.workflow_id);
            }
        }

        if workflow_ids.is_empty() {
            debug!(event = event_name, "no triggers matched event");
        }

        workflow_ids
    }

    /// Return the number of registered triggers.
    pub fn count(&self) -> usize {
        self.triggers.len()
    }

    /// Return the number of active triggers.
    pub fn active_count(&self) -> usize {
        self.triggers.values().filter(|t| t.active).count()
    }

    // -- Internals -----------------------------------------------------------

    /// Validate a cron expression using the `cron` crate.
    ///
    /// Accepts standard 5-field expressions (which are normalized by
    /// prepending a `0` seconds field) as well as 6- and 7-field formats
    /// supported by the `cron` crate directly.
    fn validate_cron(expression: &str) -> Result<()> {
        use std::str::FromStr;

        let schedule_str = if expression.split_whitespace().count() == 5 {
            format!("0 {expression}")
        } else {
            expression.to_string()
        };

        cron::Schedule::from_str(&schedule_str).map_err(|e| {
            IntentError::InvalidCronExpression {
                expression: expression.to_string(),
                reason: format!("invalid cron expression: {e}"),
            }
        })?;

        Ok(())
    }
}

impl Default for TriggerManager {
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
    fn register_manual_trigger() {
        let mut mgr = TriggerManager::new();
        let wf_id = Uuid::now_v7();
        let trigger_id = mgr.register(wf_id, TriggerType::Manual).unwrap();
        assert_eq!(mgr.count(), 1);
        assert_eq!(mgr.active_count(), 1);

        mgr.unregister(&trigger_id).unwrap();
        assert_eq!(mgr.count(), 0);
    }

    #[test]
    fn register_cron_trigger() {
        let mut mgr = TriggerManager::new();
        let wf_id = Uuid::now_v7();
        let result = mgr.register(
            wf_id,
            TriggerType::Cron {
                expression: "0 9 * * 1-5".into(),
            },
        );
        assert!(result.is_ok());
    }

    #[test]
    fn invalid_cron_expression_rejected() {
        let mut mgr = TriggerManager::new();
        let wf_id = Uuid::now_v7();
        let result = mgr.register(
            wf_id,
            TriggerType::Cron {
                expression: "bad".into(),
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn fire_event_trigger() {
        let mut mgr = TriggerManager::new();
        let wf_id = Uuid::now_v7();
        mgr.register(
            wf_id,
            TriggerType::Event {
                event_name: "file_changed".into(),
            },
        )
        .unwrap();

        let fired = mgr.fire_event("file_changed");
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0], wf_id);

        let not_fired = mgr.fire_event("other_event");
        assert!(not_fired.is_empty());
    }

    #[test]
    fn deactivated_trigger_does_not_fire() {
        let mut mgr = TriggerManager::new();
        let wf_id = Uuid::now_v7();
        let trigger_id = mgr
            .register(
                wf_id,
                TriggerType::Event {
                    event_name: "test".into(),
                },
            )
            .unwrap();

        mgr.deactivate(&trigger_id).unwrap();
        let fired = mgr.fire_event("test");
        assert!(fired.is_empty());

        mgr.activate(&trigger_id).unwrap();
        let fired = mgr.fire_event("test");
        assert_eq!(fired.len(), 1);
    }
}
