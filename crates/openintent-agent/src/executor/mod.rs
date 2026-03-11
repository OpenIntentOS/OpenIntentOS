//! Step executor.
//!
//! Takes a single [`Step`] from a [`Plan`] and executes it by invoking the
//! appropriate adapter tool.  Handles errors and retries with exponential
//! backoff.
//!
//! Supports DAG-based parallel execution: steps whose dependencies have all
//! completed are spawned concurrently in waves.

pub mod react;
pub mod tool_router;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::planner::{Step, StepStatus};
use crate::runtime::ToolAdapter;

use react::resolve_placeholders;
use tool_router::next_wave;

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

#[cfg(test)]
mod tests;
