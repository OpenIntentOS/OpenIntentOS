//! Tests for the step executor.

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
    // Placeholder resolution tests
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
    // Single-step executor tests
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
        let steps = vec![
            make_step(0, "echo", vec![]),
            make_step(1, "echo", vec![0]),
            make_step(2, "echo", vec![]),
            make_step(3, "echo", vec![1, 2]),
        ];

        let completed = HashSet::new();
        let failed = HashSet::new();
        let executed = HashSet::new();

        let wave1 = next_wave(&steps, &completed, &failed, &executed);
        assert_eq!(wave1, vec![0, 2]);

        let completed = HashSet::from([0, 2]);
        let executed = HashSet::from([0, 2]);

        let wave2 = next_wave(&steps, &completed, &failed, &executed);
        assert_eq!(wave2, vec![1]);

        let completed = HashSet::from([0, 1, 2]);
        let executed = HashSet::from([0, 1, 2]);

        let wave3 = next_wave(&steps, &completed, &failed, &executed);
        assert_eq!(wave3, vec![3]);

        let executed = HashSet::from([0, 1, 2, 3]);
        let completed = HashSet::from([0, 1, 2, 3]);

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

        let wave = next_wave(&steps, &completed, &failed, &executed);
        assert_eq!(wave, vec![1]);
    }

    /// Mixed dependencies: some steps parallel, some sequential.
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