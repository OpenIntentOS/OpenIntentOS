//! DAG-based wave scheduling for the step executor.
//!
//! Determines which steps are ready to execute in the current wave by
//! inspecting dependency completion status.

use std::collections::HashSet;

use crate::planner::Step;

/// Identify the next wave of executable steps.
///
/// A step is executable if:
/// - It hasn't been executed yet
/// - All its dependencies have completed successfully or failed
///   (steps with failed deps are still "eligible" for the wave but
///   will be skipped by the executor)
///
/// Returns the indices into the `steps` slice (not `step.index` values).
pub fn next_wave(
    steps: &[Step],
    completed: &HashSet<u32>,
    failed: &HashSet<u32>,
    executed: &HashSet<u32>,
) -> Vec<usize> {
    steps
        .iter()
        .enumerate()
        .filter(|(_, step)| {
            // Not yet executed.
            if executed.contains(&step.index) {
                return false;
            }

            // All dependencies must be resolved (completed or failed).
            // A step whose dependency failed will be picked up and skipped
            // by the executor, rather than being blocked forever.
            step.depends_on
                .iter()
                .all(|dep| completed.contains(dep) || failed.contains(dep))
        })
        .map(|(i, _)| i)
        .collect()
}
