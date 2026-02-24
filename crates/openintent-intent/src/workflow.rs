//! Workflow engine — define and execute multi-step workflows.
//!
//! A workflow is an ordered sequence of steps, each of which invokes a tool
//! on a specific adapter.  The engine handles execution, error propagation,
//! and result chaining between steps.

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::error::{IntentError, Result};
use crate::trigger::TriggerType;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// The current execution status of a workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStatus {
    /// The workflow is defined but has not been started.
    Idle,
    /// The workflow is currently executing.
    Running,
    /// The workflow completed successfully.
    Completed,
    /// The workflow failed during execution.
    Failed,
    /// The workflow was cancelled by the user.
    Cancelled,
}

/// A single step within a workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStep {
    /// Human-readable description of what this step does.
    pub action: String,
    /// The adapter that owns the tool (e.g. "filesystem", "shell").
    pub adapter: String,
    /// The tool to invoke on the adapter.
    pub tool: String,
    /// JSON parameters to pass to the tool.
    pub params: serde_json::Value,
}

/// A complete workflow definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    /// Unique identifier.
    pub id: Uuid,
    /// Human-readable name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// The ordered sequence of steps to execute.
    pub steps: Vec<WorkflowStep>,
    /// How this workflow is triggered.
    pub trigger: TriggerType,
    /// Whether this workflow is enabled.
    pub enabled: bool,
    /// Current execution status.
    pub status: WorkflowStatus,
}

impl Workflow {
    /// Create a new workflow with the given name and steps.
    pub fn new(name: impl Into<String>, steps: Vec<WorkflowStep>) -> Self {
        Self {
            id: Uuid::now_v7(),
            name: name.into(),
            description: None,
            steps,
            trigger: TriggerType::Manual,
            enabled: true,
            status: WorkflowStatus::Idle,
        }
    }

    /// Set the description for this workflow.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the trigger for this workflow.
    pub fn with_trigger(mut self, trigger: TriggerType) -> Self {
        self.trigger = trigger;
        self
    }
}

// ---------------------------------------------------------------------------
// Workflow result
// ---------------------------------------------------------------------------

/// The result of executing a single workflow step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    /// The index of this step in the workflow.
    pub step_index: usize,
    /// The tool that was executed.
    pub tool: String,
    /// Whether the step succeeded.
    pub success: bool,
    /// The output returned by the tool.
    pub output: serde_json::Value,
}

/// The result of executing an entire workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowResult {
    /// The workflow that was executed.
    pub workflow_id: Uuid,
    /// Whether all steps completed successfully.
    pub success: bool,
    /// Per-step results in execution order.
    pub step_results: Vec<StepResult>,
}

// ---------------------------------------------------------------------------
// Workflow engine
// ---------------------------------------------------------------------------

/// The workflow execution engine.
///
/// Executes workflows step-by-step, invoking adapter tools and collecting
/// results.  In this initial version, execution is sequential — parallel
/// step execution is a planned enhancement.
pub struct WorkflowEngine {
    // TODO: Hold references to the adapter registry and agent runtime
    // so we can resolve adapter IDs to live Adapter instances and dispatch
    // tool calls.
}

impl WorkflowEngine {
    /// Create a new workflow engine.
    pub fn new() -> Self {
        Self {}
    }

    /// Execute a workflow, running each step in sequence.
    ///
    /// Returns a [`WorkflowResult`] summarising what happened.  If a step
    /// fails, the workflow is aborted and remaining steps are skipped.
    pub async fn execute(&self, workflow: &mut Workflow) -> Result<WorkflowResult> {
        if workflow.steps.is_empty() {
            return Err(IntentError::InvalidWorkflowState {
                reason: "workflow has no steps".into(),
            });
        }

        info!(
            workflow_id = %workflow.id,
            name = %workflow.name,
            steps = workflow.steps.len(),
            "starting workflow execution"
        );

        workflow.status = WorkflowStatus::Running;
        let mut step_results = Vec::with_capacity(workflow.steps.len());

        for (index, step) in workflow.steps.iter().enumerate() {
            debug!(
                step = index,
                adapter = %step.adapter,
                tool = %step.tool,
                "executing workflow step"
            );

            // TODO: Resolve the adapter from the registry and call execute_tool.
            // For now, we produce a placeholder result to keep the pipeline
            // compiling and testable.
            let output = serde_json::json!({
                "status": "not_implemented",
                "message": format!(
                    "adapter `{}` tool `{}` execution is not yet wired up",
                    step.adapter, step.tool
                ),
            });

            let result = StepResult {
                step_index: index,
                tool: step.tool.clone(),
                success: true,
                output,
            };

            step_results.push(result);
        }

        let all_success = step_results.iter().all(|r| r.success);
        workflow.status = if all_success {
            WorkflowStatus::Completed
        } else {
            WorkflowStatus::Failed
        };

        info!(
            workflow_id = %workflow.id,
            success = all_success,
            "workflow execution complete"
        );

        Ok(WorkflowResult {
            workflow_id: workflow.id,
            success: all_success,
            step_results,
        })
    }

    /// Cancel a running workflow.
    pub fn cancel(&self, workflow: &mut Workflow) -> Result<()> {
        if workflow.status != WorkflowStatus::Running {
            return Err(IntentError::InvalidWorkflowState {
                reason: format!(
                    "cannot cancel workflow in {:?} state",
                    workflow.status
                ),
            });
        }
        warn!(workflow_id = %workflow.id, "cancelling workflow");
        workflow.status = WorkflowStatus::Cancelled;
        Ok(())
    }
}

impl Default for WorkflowEngine {
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

    fn sample_workflow() -> Workflow {
        Workflow::new(
            "test-workflow",
            vec![
                WorkflowStep {
                    action: "List home directory".into(),
                    adapter: "filesystem".into(),
                    tool: "fs_list_directory".into(),
                    params: serde_json::json!({"path": "/tmp"}),
                },
                WorkflowStep {
                    action: "Show date".into(),
                    adapter: "shell".into(),
                    tool: "shell_execute".into(),
                    params: serde_json::json!({"command": "date"}),
                },
            ],
        )
    }

    #[tokio::test]
    async fn execute_workflow() {
        let engine = WorkflowEngine::new();
        let mut wf = sample_workflow();
        let result = engine.execute(&mut wf).await.unwrap();
        assert!(result.success);
        assert_eq!(result.step_results.len(), 2);
        assert_eq!(wf.status, WorkflowStatus::Completed);
    }

    #[tokio::test]
    async fn empty_workflow_fails() {
        let engine = WorkflowEngine::new();
        let mut wf = Workflow::new("empty", vec![]);
        let result = engine.execute(&mut wf).await;
        assert!(result.is_err());
    }

    #[test]
    fn cancel_idle_workflow_fails() {
        let engine = WorkflowEngine::new();
        let mut wf = sample_workflow();
        let result = engine.cancel(&mut wf);
        assert!(result.is_err());
    }
}
