//! Workflow engine — define and execute multi-step workflows.
//!
//! A workflow is an ordered sequence of steps, each of which invokes a tool
//! on a specific adapter.  The engine handles execution, error propagation,
//! and result chaining between steps.

use std::sync::Arc;

use openintent_agent::runtime::ToolAdapter;
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
    /// Registered tool adapters used to resolve and dispatch step calls.
    adapters: Vec<Arc<dyn ToolAdapter>>,
    /// When true, continue executing remaining steps after a failure instead
    /// of aborting immediately.
    continue_on_error: bool,
}

impl WorkflowEngine {
    /// Create a new workflow engine with the given adapters.
    pub fn new(adapters: Vec<Arc<dyn ToolAdapter>>) -> Self {
        Self {
            adapters,
            continue_on_error: false,
        }
    }

    /// Builder method to set the adapters on an existing engine.
    pub fn with_adapters(mut self, adapters: Vec<Arc<dyn ToolAdapter>>) -> Self {
        self.adapters = adapters;
        self
    }

    /// Builder method to enable or disable continue-on-error behaviour.
    ///
    /// When enabled, the engine will keep executing remaining steps even if an
    /// earlier step fails.  The final `WorkflowResult.success` will be `false`
    /// if any step failed.
    pub fn with_continue_on_error(mut self, continue_on_error: bool) -> Self {
        self.continue_on_error = continue_on_error;
        self
    }

    /// Find the adapter whose `adapter_id()` matches `id`.
    fn find_adapter(&self, id: &str) -> Option<&Arc<dyn ToolAdapter>> {
        self.adapters.iter().find(|a| a.adapter_id() == id)
    }

    /// Execute a workflow, running each step in sequence.
    ///
    /// Returns a [`WorkflowResult`] summarising what happened.  If a step
    /// fails and `continue_on_error` is false (the default), the workflow is
    /// aborted and remaining steps are skipped.
    pub async fn execute(&self, workflow: &mut Workflow) -> Result<WorkflowResult> {
        if workflow.steps.is_empty() {
            return Err(IntentError::InvalidWorkflowState {
                reason: "workflow has no steps".into(),
            });
        }

        if self.adapters.is_empty() {
            return Err(IntentError::InvalidWorkflowState {
                reason: "no adapters configured".into(),
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
        let mut had_failure = false;

        for (index, step) in workflow.steps.iter().enumerate() {
            debug!(
                step = index,
                adapter = %step.adapter,
                tool = %step.tool,
                "executing workflow step"
            );

            // Resolve the adapter by its ID.
            let adapter = match self.find_adapter(&step.adapter) {
                Some(a) => a,
                None => {
                    warn!(
                        step = index,
                        adapter = %step.adapter,
                        "adapter not found for workflow step"
                    );

                    let result = StepResult {
                        step_index: index,
                        tool: step.tool.clone(),
                        success: false,
                        output: serde_json::json!({
                            "error": format!("adapter `{}` not found", step.adapter),
                        }),
                    };
                    step_results.push(result);
                    had_failure = true;

                    if self.continue_on_error {
                        continue;
                    }
                    break;
                }
            };

            // Call the adapter with the step's tool and params.
            match adapter.execute(&step.tool, step.params.clone()).await {
                Ok(output_str) => {
                    // Try to parse the output as JSON; fall back to a string wrapper.
                    let output = serde_json::from_str::<serde_json::Value>(&output_str)
                        .unwrap_or_else(|_| serde_json::json!({ "result": output_str }));

                    let result = StepResult {
                        step_index: index,
                        tool: step.tool.clone(),
                        success: true,
                        output,
                    };
                    step_results.push(result);
                }
                Err(e) => {
                    warn!(
                        step = index,
                        tool = %step.tool,
                        error = %e,
                        "workflow step failed"
                    );

                    let result = StepResult {
                        step_index: index,
                        tool: step.tool.clone(),
                        success: false,
                        output: serde_json::json!({
                            "error": e.to_string(),
                        }),
                    };
                    step_results.push(result);
                    had_failure = true;

                    if !self.continue_on_error {
                        break;
                    }
                }
            }
        }

        let all_success = !had_failure;
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
                reason: format!("cannot cancel workflow in {:?} state", workflow.status),
            });
        }
        warn!(workflow_id = %workflow.id, "cancelling workflow");
        workflow.status = WorkflowStatus::Cancelled;
        Ok(())
    }
}

impl Default for WorkflowEngine {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use openintent_agent::ToolDefinition;
    use serde_json::Value;

    // -- Mock adapter --------------------------------------------------------

    /// A mock adapter for testing the workflow engine.
    struct MockAdapter {
        id: String,
    }

    #[async_trait]
    impl ToolAdapter for MockAdapter {
        fn adapter_id(&self) -> &str {
            &self.id
        }

        fn tool_definitions(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                name: format!("{}_tool", self.id),
                description: format!("Mock tool for {}", self.id),
                input_schema: serde_json::json!({"type": "object"}),
            }]
        }

        async fn execute(
            &self,
            tool_name: &str,
            arguments: Value,
        ) -> openintent_agent::Result<String> {
            Ok(serde_json::json!({
                "adapter": self.id,
                "tool": tool_name,
                "args": arguments,
            })
            .to_string())
        }
    }

    /// A mock adapter that always fails execution.
    struct FailingAdapter {
        id: String,
    }

    #[async_trait]
    impl ToolAdapter for FailingAdapter {
        fn adapter_id(&self) -> &str {
            &self.id
        }

        fn tool_definitions(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                name: format!("{}_tool", self.id),
                description: format!("Failing tool for {}", self.id),
                input_schema: serde_json::json!({"type": "object"}),
            }]
        }

        async fn execute(
            &self,
            tool_name: &str,
            _arguments: Value,
        ) -> openintent_agent::Result<String> {
            Err(openintent_agent::AgentError::ToolExecutionFailed {
                tool_name: tool_name.to_owned(),
                reason: "simulated failure".into(),
            })
        }
    }

    // -- Helper --------------------------------------------------------------

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

    // -- Tests ---------------------------------------------------------------

    #[tokio::test]
    async fn execute_workflow_with_adapters() {
        let adapters: Vec<Arc<dyn ToolAdapter>> = vec![
            Arc::new(MockAdapter {
                id: "filesystem".into(),
            }),
            Arc::new(MockAdapter { id: "shell".into() }),
        ];

        let engine = WorkflowEngine::new(adapters);
        let mut wf = sample_workflow();
        let result = engine.execute(&mut wf).await.unwrap();

        assert!(result.success);
        assert_eq!(result.step_results.len(), 2);
        assert!(result.step_results[0].success);
        assert!(result.step_results[1].success);
        assert_eq!(wf.status, WorkflowStatus::Completed);

        // Verify the output contains what the mock returned.
        let first_output = &result.step_results[0].output;
        assert_eq!(first_output["adapter"], "filesystem");
        assert_eq!(first_output["tool"], "fs_list_directory");
    }

    #[tokio::test]
    async fn execute_workflow_no_adapters_returns_error() {
        let engine = WorkflowEngine::new(Vec::new());
        let mut wf = sample_workflow();
        let result = engine.execute(&mut wf).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("no adapters configured"));
    }

    #[tokio::test]
    async fn execute_workflow_missing_adapter_aborts() {
        // Only provide the filesystem adapter, not shell.
        let adapters: Vec<Arc<dyn ToolAdapter>> = vec![Arc::new(MockAdapter {
            id: "filesystem".into(),
        })];

        let engine = WorkflowEngine::new(adapters);
        let mut wf = sample_workflow();
        let result = engine.execute(&mut wf).await.unwrap();

        assert!(!result.success);
        // First step succeeds, second aborts because "shell" adapter is missing.
        assert_eq!(result.step_results.len(), 2);
        assert!(result.step_results[0].success);
        assert!(!result.step_results[1].success);
        assert_eq!(wf.status, WorkflowStatus::Failed);
    }

    #[tokio::test]
    async fn execute_workflow_continue_on_error() {
        let adapters: Vec<Arc<dyn ToolAdapter>> = vec![
            Arc::new(FailingAdapter {
                id: "filesystem".into(),
            }),
            Arc::new(MockAdapter { id: "shell".into() }),
        ];

        let engine = WorkflowEngine::new(adapters).with_continue_on_error(true);
        let mut wf = sample_workflow();
        let result = engine.execute(&mut wf).await.unwrap();

        assert!(!result.success);
        // Both steps attempted even though first failed.
        assert_eq!(result.step_results.len(), 2);
        assert!(!result.step_results[0].success);
        assert!(result.step_results[1].success);
        assert_eq!(wf.status, WorkflowStatus::Failed);
    }

    #[tokio::test]
    async fn execute_workflow_abort_on_error() {
        let adapters: Vec<Arc<dyn ToolAdapter>> = vec![
            Arc::new(FailingAdapter {
                id: "filesystem".into(),
            }),
            Arc::new(MockAdapter { id: "shell".into() }),
        ];

        let engine = WorkflowEngine::new(adapters);
        let mut wf = sample_workflow();
        let result = engine.execute(&mut wf).await.unwrap();

        assert!(!result.success);
        // Only first step attempted; second was skipped due to abort.
        assert_eq!(result.step_results.len(), 1);
        assert!(!result.step_results[0].success);
        assert_eq!(wf.status, WorkflowStatus::Failed);
    }

    #[tokio::test]
    async fn empty_workflow_fails() {
        let engine = WorkflowEngine::new(vec![Arc::new(MockAdapter { id: "test".into() })]);
        let mut wf = Workflow::new("empty", vec![]);
        let result = engine.execute(&mut wf).await;
        assert!(result.is_err());
    }

    #[test]
    fn cancel_idle_workflow_fails() {
        let engine = WorkflowEngine::default();
        let mut wf = sample_workflow();
        let result = engine.cancel(&mut wf);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn with_adapters_builder() {
        let adapters: Vec<Arc<dyn ToolAdapter>> = vec![
            Arc::new(MockAdapter {
                id: "filesystem".into(),
            }),
            Arc::new(MockAdapter { id: "shell".into() }),
        ];

        let engine = WorkflowEngine::default().with_adapters(adapters);
        let mut wf = sample_workflow();
        let result = engine.execute(&mut wf).await.unwrap();
        assert!(result.success);
    }
}
