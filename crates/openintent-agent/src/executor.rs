//! Step executor.
//!
//! Takes a single [`Step`] from a [`Plan`] and executes it by invoking the
//! appropriate adapter tool.  Handles errors and retries with exponential
//! backoff.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::planner::{Step, StepStatus};
use crate::runtime::ToolAdapter;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the step executor.
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    /// Maximum number of retry attempts per step (0 = no retries).
    pub max_retries: u32,

    /// Initial delay between retries.
    pub initial_retry_delay: Duration,

    /// Multiplier applied to the delay after each retry (exponential backoff).
    pub retry_backoff_factor: f64,

    /// Maximum delay between retries (caps the backoff).
    pub max_retry_delay: Duration,

    /// Timeout for a single tool execution.
    pub execution_timeout: Duration,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            max_retries: 2,
            initial_retry_delay: Duration::from_millis(500),
            retry_backoff_factor: 2.0,
            max_retry_delay: Duration::from_secs(10),
            execution_timeout: Duration::from_secs(60),
        }
    }
}

// ---------------------------------------------------------------------------
// Step result
// ---------------------------------------------------------------------------

/// The result of executing a single step.
#[derive(Debug, Clone)]
pub struct StepResult {
    /// The index of the step that was executed.
    pub step_index: u32,

    /// The final status of the step.
    pub status: StepStatus,

    /// The output from the tool (if successful).
    pub output: Option<String>,

    /// Error message (if failed).
    pub error: Option<String>,

    /// Number of attempts made (1 = first try succeeded).
    pub attempts: u32,
}

// ---------------------------------------------------------------------------
// Executor
// ---------------------------------------------------------------------------

/// Executes individual plan steps by delegating to tool adapters.
pub struct Executor {
    /// Registered tool adapters.
    adapters: Vec<Arc<dyn ToolAdapter>>,

    /// Executor configuration.
    config: ExecutorConfig,
}

impl Executor {
    /// Create a new executor with the given adapters and configuration.
    pub fn new(adapters: Vec<Arc<dyn ToolAdapter>>, config: ExecutorConfig) -> Self {
        Self { adapters, config }
    }

    /// Execute a single step.
    ///
    /// Resolves any placeholder references in the step's arguments using
    /// `prior_outputs`, then invokes the tool with retry logic.
    ///
    /// # Arguments
    ///
    /// * `step` -- The step to execute.
    /// * `prior_outputs` -- Map from step index to output string, for
    ///   resolving `{{step_N.output}}` placeholders.
    pub async fn execute_step(
        &self,
        step: &Step,
        prior_outputs: &HashMap<u32, String>,
    ) -> StepResult {
        tracing::info!(
            step_index = step.index,
            tool = %step.tool_name,
            description = %step.description,
            "executing step"
        );

        // Check that all dependencies have been satisfied.
        for dep in &step.depends_on {
            if !prior_outputs.contains_key(dep) {
                tracing::warn!(
                    step_index = step.index,
                    missing_dep = dep,
                    "step dependency not satisfied"
                );
                return StepResult {
                    step_index: step.index,
                    status: StepStatus::Skipped,
                    output: None,
                    error: Some(format!("dependency step {dep} has no output")),
                    attempts: 0,
                };
            }
        }

        // Resolve argument placeholders.
        let arguments = resolve_placeholders(&step.arguments, prior_outputs);

        // Find the adapter for this tool.
        let adapter = match self.find_adapter(&step.tool_name) {
            Some(a) => a,
            None => {
                return StepResult {
                    step_index: step.index,
                    status: StepStatus::Failed,
                    output: None,
                    error: Some(format!("no adapter found for tool `{}`", step.tool_name)),
                    attempts: 0,
                };
            }
        };

        // Execute with retries.
        let mut delay = self.config.initial_retry_delay;
        let max_attempts = self.config.max_retries + 1;

        for attempt in 1..=max_attempts {
            tracing::debug!(
                step_index = step.index,
                attempt,
                max_attempts,
                "tool execution attempt"
            );

            let result = tokio::time::timeout(
                self.config.execution_timeout,
                adapter.execute(&step.tool_name, arguments.clone()),
            )
            .await;

            match result {
                Ok(Ok(output)) => {
                    tracing::info!(
                        step_index = step.index,
                        attempt,
                        "step completed successfully"
                    );
                    return StepResult {
                        step_index: step.index,
                        status: StepStatus::Completed,
                        output: Some(output),
                        error: None,
                        attempts: attempt,
                    };
                }
                Ok(Err(e)) => {
                    tracing::warn!(
                        step_index = step.index,
                        attempt,
                        error = %e,
                        "tool execution failed"
                    );

                    if attempt < max_attempts {
                        tracing::debug!(delay = ?delay, "retrying after delay");
                        tokio::time::sleep(delay).await;
                        delay = Duration::from_secs_f64(
                            (delay.as_secs_f64() * self.config.retry_backoff_factor)
                                .min(self.config.max_retry_delay.as_secs_f64()),
                        );
                    } else {
                        return StepResult {
                            step_index: step.index,
                            status: StepStatus::Failed,
                            output: None,
                            error: Some(format!("{e}")),
                            attempts: attempt,
                        };
                    }
                }
                Err(_elapsed) => {
                    tracing::warn!(
                        step_index = step.index,
                        attempt,
                        timeout = ?self.config.execution_timeout,
                        "tool execution timed out"
                    );

                    if attempt < max_attempts {
                        tokio::time::sleep(delay).await;
                        delay = Duration::from_secs_f64(
                            (delay.as_secs_f64() * self.config.retry_backoff_factor)
                                .min(self.config.max_retry_delay.as_secs_f64()),
                        );
                    } else {
                        return StepResult {
                            step_index: step.index,
                            status: StepStatus::Failed,
                            output: None,
                            error: Some(format!(
                                "timed out after {:?}",
                                self.config.execution_timeout
                            )),
                            attempts: attempt,
                        };
                    }
                }
            }
        }

        // Should not be reached, but just in case:
        StepResult {
            step_index: step.index,
            status: StepStatus::Failed,
            output: None,
            error: Some("unexpected executor state".into()),
            attempts: max_attempts,
        }
    }

    /// Execute all steps in a plan sequentially, feeding outputs forward.
    pub async fn execute_plan(&self, steps: &[Step]) -> Vec<StepResult> {
        let mut outputs: HashMap<u32, String> = HashMap::new();
        let mut results = Vec::with_capacity(steps.len());

        for step in steps {
            let result = self.execute_step(step, &outputs).await;

            if let Some(ref output) = result.output {
                outputs.insert(step.index, output.clone());
            }

            let failed = result.status == StepStatus::Failed;
            results.push(result);

            // If a step fails and subsequent steps depend on it, they will
            // be skipped via the dependency check.  We continue execution
            // for independent steps.
            if failed {
                tracing::warn!(
                    step_index = step.index,
                    "step failed; dependent steps may be skipped"
                );
            }
        }

        results
    }

    /// Find the adapter that can execute a given tool.
    fn find_adapter(&self, tool_name: &str) -> Option<&Arc<dyn ToolAdapter>> {
        self.adapters
            .iter()
            .find(|a| a.tool_definitions().iter().any(|td| td.name == tool_name))
    }
}

// ---------------------------------------------------------------------------
// Placeholder resolution
// ---------------------------------------------------------------------------

/// Resolve `{{step_N.output}}` placeholders in a JSON value by substituting
/// the actual outputs from prior steps.
fn resolve_placeholders(value: &Value, outputs: &HashMap<u32, String>) -> Value {
    match value {
        Value::String(s) => {
            let mut resolved = s.clone();
            for (index, output) in outputs {
                let placeholder = format!("{{{{step_{index}.output}}}}");
                if resolved.contains(&placeholder) {
                    resolved = resolved.replace(&placeholder, output);
                }
            }
            Value::String(resolved)
        }
        Value::Object(map) => {
            let resolved_map = map
                .iter()
                .map(|(k, v)| (k.clone(), resolve_placeholders(v, outputs)))
                .collect();
            Value::Object(resolved_map)
        }
        Value::Array(arr) => {
            let resolved_arr = arr
                .iter()
                .map(|v| resolve_placeholders(v, outputs))
                .collect();
            Value::Array(resolved_arr)
        }
        // Numbers, booleans, null pass through unchanged.
        other => other.clone(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{AgentError, Result};
    use crate::llm::types::ToolDefinition;
    use async_trait::async_trait;

    struct EchoAdapter;

    #[async_trait]
    impl ToolAdapter for EchoAdapter {
        fn adapter_id(&self) -> &str {
            "echo"
        }

        fn tool_definitions(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                name: "echo".into(),
                description: "Echoes input".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }]
        }

        async fn execute(&self, _tool_name: &str, arguments: Value) -> Result<String> {
            Ok(arguments.to_string())
        }
    }

    struct FailAdapter {
        fail_count: std::sync::atomic::AtomicU32,
        fail_until: u32,
    }

    #[async_trait]
    impl ToolAdapter for FailAdapter {
        fn adapter_id(&self) -> &str {
            "fail"
        }

        fn tool_definitions(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                name: "flaky_tool".into(),
                description: "Fails then succeeds".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }]
        }

        async fn execute(&self, _tool_name: &str, _arguments: Value) -> Result<String> {
            let count = self
                .fail_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if count < self.fail_until {
                Err(AgentError::ToolExecutionFailed {
                    tool_name: "flaky_tool".into(),
                    reason: format!("simulated failure {count}"),
                })
            } else {
                Ok("success after retries".into())
            }
        }
    }

    #[test]
    fn resolve_single_placeholder() {
        let mut outputs = HashMap::new();
        outputs.insert(0, "file contents".into());

        let value = serde_json::json!({"text": "{{step_0.output}}"});
        let resolved = resolve_placeholders(&value, &outputs);
        assert_eq!(resolved["text"], "file contents");
    }

    #[test]
    fn resolve_multiple_placeholders() {
        let mut outputs = HashMap::new();
        outputs.insert(0, "first".into());
        outputs.insert(1, "second".into());

        let value = serde_json::json!({
            "a": "{{step_0.output}}",
            "b": "{{step_1.output}}",
            "c": "no placeholder"
        });
        let resolved = resolve_placeholders(&value, &outputs);
        assert_eq!(resolved["a"], "first");
        assert_eq!(resolved["b"], "second");
        assert_eq!(resolved["c"], "no placeholder");
    }

    #[test]
    fn resolve_nested_placeholder() {
        let mut outputs = HashMap::new();
        outputs.insert(0, "data".into());

        let value = serde_json::json!({
            "nested": {
                "inner": "prefix_{{step_0.output}}_suffix"
            }
        });
        let resolved = resolve_placeholders(&value, &outputs);
        assert_eq!(resolved["nested"]["inner"], "prefix_data_suffix");
    }

    #[test]
    fn resolve_no_matching_placeholder() {
        let outputs = HashMap::new();
        let value = serde_json::json!({"text": "{{step_99.output}}"});
        let resolved = resolve_placeholders(&value, &outputs);
        // Unresolved placeholder stays as-is.
        assert_eq!(resolved["text"], "{{step_99.output}}");
    }

    #[tokio::test]
    async fn execute_step_success() {
        let adapter: Arc<dyn ToolAdapter> = Arc::new(EchoAdapter);
        let executor = Executor::new(vec![adapter], ExecutorConfig::default());

        let step = Step {
            index: 0,
            description: "Echo test".into(),
            tool_name: "echo".into(),
            arguments: serde_json::json!({"message": "hello"}),
            depends_on: vec![],
            expected_outcome: String::new(),
        };

        let result = executor.execute_step(&step, &HashMap::new()).await;
        assert_eq!(result.status, StepStatus::Completed);
        assert!(result.output.is_some());
        assert_eq!(result.attempts, 1);
    }

    #[tokio::test]
    async fn execute_step_unknown_tool() {
        let adapter: Arc<dyn ToolAdapter> = Arc::new(EchoAdapter);
        let executor = Executor::new(vec![adapter], ExecutorConfig::default());

        let step = Step {
            index: 0,
            description: "Unknown tool".into(),
            tool_name: "nonexistent".into(),
            arguments: serde_json::json!({}),
            depends_on: vec![],
            expected_outcome: String::new(),
        };

        let result = executor.execute_step(&step, &HashMap::new()).await;
        assert_eq!(result.status, StepStatus::Failed);
        assert!(result.error.is_some());
    }

    #[tokio::test]
    async fn execute_step_missing_dependency() {
        let adapter: Arc<dyn ToolAdapter> = Arc::new(EchoAdapter);
        let executor = Executor::new(vec![adapter], ExecutorConfig::default());

        let step = Step {
            index: 1,
            description: "Depends on step 0".into(),
            tool_name: "echo".into(),
            arguments: serde_json::json!({}),
            depends_on: vec![0],
            expected_outcome: String::new(),
        };

        // No prior outputs provided.
        let result = executor.execute_step(&step, &HashMap::new()).await;
        assert_eq!(result.status, StepStatus::Skipped);
    }

    #[tokio::test]
    async fn execute_step_retries_on_failure() {
        let adapter: Arc<dyn ToolAdapter> = Arc::new(FailAdapter {
            fail_count: std::sync::atomic::AtomicU32::new(0),
            fail_until: 1, // Fail once, then succeed.
        });

        let config = ExecutorConfig {
            max_retries: 2,
            initial_retry_delay: Duration::from_millis(10),
            ..ExecutorConfig::default()
        };

        let executor = Executor::new(vec![adapter], config);

        let step = Step {
            index: 0,
            description: "Flaky tool".into(),
            tool_name: "flaky_tool".into(),
            arguments: serde_json::json!({}),
            depends_on: vec![],
            expected_outcome: String::new(),
        };

        let result = executor.execute_step(&step, &HashMap::new()).await;
        assert_eq!(result.status, StepStatus::Completed);
        assert_eq!(result.attempts, 2); // First attempt failed, second succeeded.
        assert_eq!(result.output.as_deref(), Some("success after retries"));
    }

    #[tokio::test]
    async fn execute_plan_sequential() {
        let adapter: Arc<dyn ToolAdapter> = Arc::new(EchoAdapter);
        let executor = Executor::new(vec![adapter], ExecutorConfig::default());

        let steps = vec![
            Step {
                index: 0,
                description: "Step 0".into(),
                tool_name: "echo".into(),
                arguments: serde_json::json!({"msg": "first"}),
                depends_on: vec![],
                expected_outcome: String::new(),
            },
            Step {
                index: 1,
                description: "Step 1".into(),
                tool_name: "echo".into(),
                arguments: serde_json::json!({"msg": "second"}),
                depends_on: vec![0],
                expected_outcome: String::new(),
            },
        ];

        let results = executor.execute_plan(&steps).await;
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].status, StepStatus::Completed);
        assert_eq!(results[1].status, StepStatus::Completed);
    }
}
