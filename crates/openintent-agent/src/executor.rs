//! Step executor.
//!
//! Takes a single [`Step`] from a [`Plan`] and executes it by invoking the
//! appropriate adapter tool.  Handles errors and retries with exponential
//! backoff.
//!
//! Supports DAG-based parallel execution: steps whose dependencies have all
//! completed are spawned concurrently in waves.

use std::collections::{HashMap, HashSet};
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

        // Check for built-in skills first
        if step.tool_name == "skill_email_oauth_setup_setup" {
            if let Some(email) = step.arguments.get("email").and_then(|v| v.as_str()) {
                // Execute the email OAuth setup script
                let script_path = "/Users/cw/development/OpenIntentOS/skills/email-oauth-setup/setup.sh";
                let mut cmd = tokio::process::Command::new("bash");
                cmd.arg(script_path)
                   .arg("--email")
                   .arg(email);
                
                // Add provider if specified
                if let Some(provider) = step.arguments.get("provider").and_then(|v| v.as_str()) {
                    cmd.arg("--provider").arg(provider);
                }
                
                match cmd.output().await {
                    Ok(output) => {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        
                        if output.status.success() {
                            return StepResult {
                                step_index: step.index,
                                status: StepStatus::Completed,
                                output: Some(format!("OAuth setup completed:\n{}", stdout)),
                                error: None,
                                attempts: 1,
                            };
                        } else {
                            return StepResult {
                                step_index: step.index,
                                status: StepStatus::Failed,
                                output: None,
                                error: Some(format!("OAuth setup failed:\n{}\n{}", stdout, stderr)),
                                attempts: 1,
                            };
                        }
                    }
                    Err(e) => {
                        return StepResult {
                            step_index: step.index,
                            status: StepStatus::Failed,
                            output: None,
                            error: Some(format!("Failed to execute OAuth setup script: {}", e)),
                            attempts: 1,
                        };
                    }
                }
            } else {
                return StepResult {
                    step_index: step.index,
                    status: StepStatus::Failed,
                    output: None,
                    error: Some("skill_email_oauth_setup_setup requires 'email' parameter".to_string()),
                    attempts: 0,
                };
            }
        }

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

    /// Execute a plan with DAG-based parallel step execution.
    ///
    /// Steps that have no unmet dependencies are executed concurrently in
    /// waves.  When a step completes, its dependents become eligible for
    /// execution in the next wave.
    ///
    /// If a step fails, all steps that transitively depend on it are
    /// automatically skipped.  Non-dependent steps continue executing.
    ///
    /// When all steps form a linear chain (A -> B -> C), this degrades
    /// gracefully to sequential execution (one step per wave).
    pub async fn execute_plan(&self, steps: &[Step]) -> Vec<StepResult> {
        if steps.is_empty() {
            return Vec::new();
        }

        let mut outputs: HashMap<u32, String> = HashMap::new();
        let mut result_map: HashMap<u32, StepResult> = HashMap::new();
        let mut completed: HashSet<u32> = HashSet::new();
        let mut failed: HashSet<u32> = HashSet::new();
        let mut executed: HashSet<u32> = HashSet::new();

        loop {
            let wave = next_wave(steps, &completed, &failed, &executed);

            if wave.is_empty() {
                // No more steps can be scheduled. Either all are done, or
                // remaining steps are blocked by failed dependencies.
                break;
            }

            tracing::info!(
                wave_size = wave.len(),
                step_indices = ?wave.iter().map(|&i| steps[i].index).collect::<Vec<_>>(),
                "launching execution wave"
            );

            // Clone data needed by spawned tasks.
            let mut handles = Vec::with_capacity(wave.len());

            for &step_idx in &wave {
                let step = steps[step_idx].clone();
                let step_index = step.index;
                executed.insert(step_index);

                // Check if any dependency failed -- if so, skip this step.
                let dep_failed = step.depends_on.iter().any(|dep| failed.contains(dep));

                if dep_failed {
                    tracing::info!(
                        step_index = step_index,
                        "skipping step due to failed dependency"
                    );
                    let skip_result = StepResult {
                        step_index,
                        status: StepStatus::Skipped,
                        output: None,
                        error: Some("skipped due to failed dependency".into()),
                        attempts: 0,
                    };
                    failed.insert(step_index);
                    result_map.insert(step_index, skip_result);
                    continue;
                }

                // Snapshot the outputs needed by this step.
                let prior_outputs = outputs.clone();
                let adapters = self.adapters.clone();
                let config = self.config.clone();

                handles.push(tokio::spawn(async move {
                    let executor = Executor::new(adapters, config);
                    let result = executor.execute_step(&step, &prior_outputs).await;
                    (step_index, result)
                }));
            }

            // Await all spawned tasks in this wave.
            for handle in handles {
                match handle.await {
                    Ok((step_index, result)) => {
                        if result.status == StepStatus::Completed {
                            if let Some(ref output) = result.output {
                                outputs.insert(step_index, output.clone());
                            }
                            completed.insert(step_index);
                        } else if result.status == StepStatus::Failed
                            || result.status == StepStatus::Skipped
                        {
                            tracing::warn!(
                                step_index = step_index,
                                status = ?result.status,
                                "step did not complete; dependents will be skipped"
                            );
                            failed.insert(step_index);
                        }
                        result_map.insert(step_index, result);
                    }
                    Err(join_err) => {
                        // The spawned task panicked. Record as failed.
                        tracing::error!(
                            error = %join_err,
                            "step execution task panicked"
                        );
                        let step_index = steps
                            .iter()
                            .map(|s| s.index)
                            .find(|idx| !result_map.contains_key(idx) && executed.contains(idx))
                            .unwrap_or(0);
                        failed.insert(step_index);
                        result_map.insert(
                            step_index,
                            StepResult {
                                step_index,
                                status: StepStatus::Failed,
                                output: None,
                                error: Some(format!("task panicked: {join_err}")),
                                attempts: 0,
                            },
                        );
                    }
                }
            }
        }

        // Mark any remaining unexecuted steps as skipped (blocked by failed deps).
        for step in steps {
            result_map.entry(step.index).or_insert_with(|| {
                tracing::info!(
                    step_index = step.index,
                    "step unreachable due to failed dependencies"
                );
                StepResult {
                    step_index: step.index,
                    status: StepStatus::Skipped,
                    output: None,
                    error: Some("unreachable due to failed dependency".into()),
                    attempts: 0,
                }
            });
        }

        // Return results ordered by step index to maintain deterministic output.
        let mut results: Vec<StepResult> = result_map.into_values().collect();
        results.sort_by_key(|r| r.step_index);
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
// DAG wave scheduling
// ---------------------------------------------------------------------------

/// Identify the next wave of executable steps.
///
/// A step is executable if:
/// - It hasn't been executed yet
/// - All its dependencies have completed successfully
/// - None of its dependencies have failed (steps with failed deps are still
///   "eligible" for the wave but will be skipped by the executor)
///
/// Returns the indices into the `steps` slice (not step.index values).
fn next_wave(
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
    use std::sync::atomic::{AtomicU32, Ordering};

    // -----------------------------------------------------------------------
    // Test adapters
    // -----------------------------------------------------------------------

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
        fail_count: AtomicU32,
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
            let count = self.fail_count.fetch_add(1, Ordering::SeqCst);
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

    /// Always-fail adapter for testing failure propagation.
    struct AlwaysFailAdapter;

    #[async_trait]
    impl ToolAdapter for AlwaysFailAdapter {
        fn adapter_id(&self) -> &str {
            "always_fail"
        }

        fn tool_definitions(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                name: "always_fail".into(),
                description: "Always fails".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }]
        }

        async fn execute(&self, _tool_name: &str, _arguments: Value) -> Result<String> {
            Err(AgentError::ToolExecutionFailed {
                tool_name: "always_fail".into(),
                reason: "always fails".into(),
            })
        }
    }

    /// Adapter that records the order of execution via a shared counter.
    struct OrderTrackingAdapter {
        call_counter: Arc<AtomicU32>,
    }

    #[async_trait]
    impl ToolAdapter for OrderTrackingAdapter {
        fn adapter_id(&self) -> &str {
            "order_tracker"
        }

        fn tool_definitions(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                name: "track".into(),
                description: "Tracks execution order".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }]
        }

        async fn execute(&self, _tool_name: &str, _arguments: Value) -> Result<String> {
            let order = self.call_counter.fetch_add(1, Ordering::SeqCst);
            Ok(format!("{order}"))
        }
    }

    // -----------------------------------------------------------------------
    // Helper to build a Step concisely in tests
    // -----------------------------------------------------------------------

    fn make_step(index: u32, tool: &str, depends_on: Vec<u32>) -> Step {
        Step {
            index,
            description: format!("Step {index}"),
            tool_name: tool.into(),
            arguments: serde_json::json!({"step": index}),
            depends_on,
            expected_outcome: String::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Placeholder resolution tests (unchanged)
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // Single-step executor tests (unchanged)
    // -----------------------------------------------------------------------

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
            fail_count: AtomicU32::new(0),
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

    // -----------------------------------------------------------------------
    // DAG parallel execution tests
    // -----------------------------------------------------------------------

    /// Sequential plan (A -> B -> C) executes in order, one step per wave.
    #[tokio::test]
    async fn dag_sequential_chain() {
        let adapter: Arc<dyn ToolAdapter> = Arc::new(EchoAdapter);
        let executor = Executor::new(vec![adapter], ExecutorConfig::default());

        let steps = vec![
            make_step(0, "echo", vec![]),
            make_step(1, "echo", vec![0]),
            make_step(2, "echo", vec![1]),
        ];

        let results = executor.execute_plan(&steps).await;
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].step_index, 0);
        assert_eq!(results[1].step_index, 1);
        assert_eq!(results[2].step_index, 2);
        for r in &results {
            assert_eq!(r.status, StepStatus::Completed);
        }
    }

    /// Parallel plan (A, B, C with no deps) -- all run in the first wave.
    #[tokio::test]
    async fn dag_fully_parallel() {
        let counter = Arc::new(AtomicU32::new(0));
        let adapter: Arc<dyn ToolAdapter> = Arc::new(OrderTrackingAdapter {
            call_counter: counter.clone(),
        });
        let executor = Executor::new(vec![adapter], ExecutorConfig::default());

        let steps = vec![
            make_step(0, "track", vec![]),
            make_step(1, "track", vec![]),
            make_step(2, "track", vec![]),
        ];

        let results = executor.execute_plan(&steps).await;
        assert_eq!(results.len(), 3);
        for r in &results {
            assert_eq!(r.status, StepStatus::Completed);
        }
        // All three should have been called (counter reaches 3).
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    /// Diamond pattern: A -> B, A -> C, B+C -> D.
    /// Wave 1: A. Wave 2: B, C (parallel). Wave 3: D.
    #[tokio::test]
    async fn dag_diamond_pattern() {
        let counter = Arc::new(AtomicU32::new(0));
        let adapter: Arc<dyn ToolAdapter> = Arc::new(OrderTrackingAdapter {
            call_counter: counter.clone(),
        });
        let executor = Executor::new(vec![adapter], ExecutorConfig::default());

        //   0
        //  / \
        // 1   2
        //  \ /
        //   3
        let steps = vec![
            make_step(0, "track", vec![]),
            make_step(1, "track", vec![0]),
            make_step(2, "track", vec![0]),
            make_step(3, "track", vec![1, 2]),
        ];

        let results = executor.execute_plan(&steps).await;
        assert_eq!(results.len(), 4);
        for r in &results {
            assert_eq!(r.status, StepStatus::Completed);
        }
        assert_eq!(counter.load(Ordering::SeqCst), 4);

        // Step 0 must have executed before steps 1 and 2.
        // Step 3 must have executed after steps 1 and 2.
        // Verify via the order values stored in output.
        let order_of = |idx: u32| -> u32 {
            results
                .iter()
                .find(|r| r.step_index == idx)
                .and_then(|r| r.output.as_deref())
                .and_then(|s| s.parse::<u32>().ok())
                .expect("expected numeric output")
        };

        let a_order = order_of(0);
        let b_order = order_of(1);
        let c_order = order_of(2);
        let d_order = order_of(3);

        assert!(a_order < b_order, "A must run before B");
        assert!(a_order < c_order, "A must run before C");
        assert!(d_order > b_order, "D must run after B");
        assert!(d_order > c_order, "D must run after C");
    }

    /// Failed step causes all dependents to be skipped.
    #[tokio::test]
    async fn dag_failed_step_skips_dependents() {
        let echo: Arc<dyn ToolAdapter> = Arc::new(EchoAdapter);
        let fail: Arc<dyn ToolAdapter> = Arc::new(AlwaysFailAdapter);

        let config = ExecutorConfig {
            max_retries: 0,
            initial_retry_delay: Duration::from_millis(1),
            ..ExecutorConfig::default()
        };
        let executor = Executor::new(vec![echo, fail], config);

        // Step 0 succeeds, Step 1 fails, Step 2 depends on 1 (skipped),
        // Step 3 depends on 0 only (succeeds).
        let steps = vec![
            make_step(0, "echo", vec![]),
            make_step(1, "always_fail", vec![]),
            make_step(2, "echo", vec![1]),
            make_step(3, "echo", vec![0]),
        ];

        let results = executor.execute_plan(&steps).await;
        assert_eq!(results.len(), 4);

        let status_of = |idx: u32| -> StepStatus {
            results
                .iter()
                .find(|r| r.step_index == idx)
                .map(|r| r.status)
                .expect("expected result for step")
        };

        assert_eq!(status_of(0), StepStatus::Completed);
        assert_eq!(status_of(1), StepStatus::Failed);
        assert_eq!(status_of(2), StepStatus::Skipped);
        assert_eq!(status_of(3), StepStatus::Completed);
    }

    /// Empty plan produces empty results.
    #[tokio::test]
    async fn dag_empty_plan() {
        let adapter: Arc<dyn ToolAdapter> = Arc::new(EchoAdapter);
        let executor = Executor::new(vec![adapter], ExecutorConfig::default());

        let results = executor.execute_plan(&[]).await;
        assert!(results.is_empty());
    }

    /// Single step plan executes correctly.
    #[tokio::test]
    async fn dag_single_step() {
        let adapter: Arc<dyn ToolAdapter> = Arc::new(EchoAdapter);
        let executor = Executor::new(vec![adapter], ExecutorConfig::default());

        let steps = vec![make_step(0, "echo", vec![])];

        let results = executor.execute_plan(&steps).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, StepStatus::Completed);
        assert_eq!(results[0].step_index, 0);
    }

    /// next_wave returns the correct indices for a mixed dependency graph.
    #[test]
    fn next_wave_returns_correct_indices() {
        //   0 (no deps)
        //   1 (depends on 0)
        //   2 (no deps)
        //   3 (depends on 1 and 2)
        let steps = vec![
            make_step(0, "echo", vec![]),
            make_step(1, "echo", vec![0]),
            make_step(2, "echo", vec![]),
            make_step(3, "echo", vec![1, 2]),
        ];

        let completed = HashSet::new();
        let failed = HashSet::new();
        let executed = HashSet::new();

        // Wave 1: steps 0 and 2 are ready (no deps).
        let wave1 = next_wave(&steps, &completed, &failed, &executed);
        assert_eq!(wave1, vec![0, 2]); // slice indices, matching step indices here

        // After 0 and 2 complete:
        let completed = HashSet::from([0, 2]);
        let executed = HashSet::from([0, 2]);

        // Wave 2: step 1 is ready (depends on 0, which completed).
        // step 3 needs 1 and 2 -- 2 is done but 1 is not yet.
        let wave2 = next_wave(&steps, &completed, &failed, &executed);
        assert_eq!(wave2, vec![1]); // slice index 1 = step index 1

        // After 1 completes:
        let completed = HashSet::from([0, 1, 2]);
        let executed = HashSet::from([0, 1, 2]);

        // Wave 3: step 3 is ready.
        let wave3 = next_wave(&steps, &completed, &failed, &executed);
        assert_eq!(wave3, vec![3]); // slice index 3 = step index 3

        // After 3 completes:
        let executed = HashSet::from([0, 1, 2, 3]);
        let completed = HashSet::from([0, 1, 2, 3]);

        // Wave 4: nothing left.
        let wave4 = next_wave(&steps, &completed, &failed, &executed);
        assert!(wave4.is_empty());
    }

    /// next_wave includes steps whose dependencies have failed (so they can
    /// be skipped), rather than blocking forever.
    #[test]
    fn next_wave_includes_steps_with_failed_deps() {
        let steps = vec![make_step(0, "echo", vec![]), make_step(1, "echo", vec![0])];

        let completed = HashSet::new();
        let failed = HashSet::from([0]);
        let executed = HashSet::from([0]);

        // Step 1 depends on 0 which failed -- it should still appear in the
        // wave so the executor can mark it as skipped.
        let wave = next_wave(&steps, &completed, &failed, &executed);
        assert_eq!(wave, vec![1]);
    }

    /// Mixed dependencies: some steps parallel, some sequential.
    ///   0 (no deps)
    ///   1 (no deps)
    ///   2 (depends on 0)
    ///   3 (depends on 1)
    ///   4 (depends on 2 and 3)
    #[tokio::test]
    async fn dag_mixed_dependencies() {
        let counter = Arc::new(AtomicU32::new(0));
        let adapter: Arc<dyn ToolAdapter> = Arc::new(OrderTrackingAdapter {
            call_counter: counter.clone(),
        });
        let executor = Executor::new(vec![adapter], ExecutorConfig::default());

        let steps = vec![
            make_step(0, "track", vec![]),
            make_step(1, "track", vec![]),
            make_step(2, "track", vec![0]),
            make_step(3, "track", vec![1]),
            make_step(4, "track", vec![2, 3]),
        ];

        let results = executor.execute_plan(&steps).await;
        assert_eq!(results.len(), 5);
        for r in &results {
            assert_eq!(r.status, StepStatus::Completed);
        }
        assert_eq!(counter.load(Ordering::SeqCst), 5);

        let order_of = |idx: u32| -> u32 {
            results
                .iter()
                .find(|r| r.step_index == idx)
                .and_then(|r| r.output.as_deref())
                .and_then(|s| s.parse::<u32>().ok())
                .expect("expected numeric output")
        };

        // Wave 1: 0, 1  |  Wave 2: 2, 3  |  Wave 3: 4
        let o0 = order_of(0);
        let o1 = order_of(1);
        let o2 = order_of(2);
        let o3 = order_of(3);
        let o4 = order_of(4);

        assert!(o0 < o2, "0 must run before 2");
        assert!(o1 < o3, "1 must run before 3");
        assert!(o4 > o2, "4 must run after 2");
        assert!(o4 > o3, "4 must run after 3");
    }

    /// Transitive failure: A fails -> B skipped -> C (depends on B) skipped.
    #[tokio::test]
    async fn dag_transitive_failure_skips_chain() {
        let echo: Arc<dyn ToolAdapter> = Arc::new(EchoAdapter);
        let fail: Arc<dyn ToolAdapter> = Arc::new(AlwaysFailAdapter);

        let config = ExecutorConfig {
            max_retries: 0,
            initial_retry_delay: Duration::from_millis(1),
            ..ExecutorConfig::default()
        };
        let executor = Executor::new(vec![echo, fail], config);

        // 0 (fails) -> 1 (skipped) -> 2 (skipped)
        let steps = vec![
            make_step(0, "always_fail", vec![]),
            make_step(1, "echo", vec![0]),
            make_step(2, "echo", vec![1]),
        ];

        let results = executor.execute_plan(&steps).await;
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].status, StepStatus::Failed);
        assert_eq!(results[1].status, StepStatus::Skipped);
        assert_eq!(results[2].status, StepStatus::Skipped);
    }

    /// Backward-compatible: the old sequential test still passes.
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
