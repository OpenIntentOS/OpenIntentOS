//! Task planner.
//!
//! Takes a high-level user intent and decomposes it into an ordered sequence
//! of executable steps using the LLM.  Each step identifies the tool/adapter
//! to invoke and the expected outcome.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::error::{AgentError, Result};
use crate::llm::LlmClient;
use crate::llm::types::{ChatRequest, LlmResponse, Message, ToolDefinition};

// ---------------------------------------------------------------------------
// Plan types
// ---------------------------------------------------------------------------

/// A plan produced by decomposing a high-level intent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    /// Unique identifier for this plan.
    pub id: Uuid,

    /// The original intent that was decomposed.
    pub intent: String,

    /// Ordered list of steps to execute.
    pub steps: Vec<Step>,

    /// Overall rationale for the decomposition.
    pub rationale: String,
}

/// A single step within a plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    /// Zero-based index of this step in the plan.
    pub index: u32,

    /// Human-readable description of what this step does.
    pub description: String,

    /// The tool to invoke for this step.
    pub tool_name: String,

    /// Arguments to pass to the tool (may contain placeholders referencing
    /// outputs of prior steps, e.g. `"{{step_0.output}}"`).
    pub arguments: Value,

    /// Which prior step outputs this step depends on.
    #[serde(default)]
    pub depends_on: Vec<u32>,

    /// What the expected outcome looks like (for the reflector to validate).
    #[serde(default)]
    pub expected_outcome: String,
}

/// Current execution state of a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    /// Not yet started.
    Pending,
    /// Currently executing.
    Running,
    /// Completed successfully.
    Completed,
    /// Failed (may be retried).
    Failed,
    /// Skipped (e.g. dependency failed and was unrecoverable).
    Skipped,
}

// ---------------------------------------------------------------------------
// Planner
// ---------------------------------------------------------------------------

/// Configuration for the task planner.
#[derive(Debug, Clone)]
pub struct PlannerConfig {
    /// Model to use for planning (should be a capable model).
    pub model: String,

    /// Maximum tokens for the planning response.
    pub max_tokens: u32,
}

impl Default for PlannerConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            max_tokens: 4096,
        }
    }
}

/// Decomposes high-level intents into executable plans using an LLM.
pub struct Planner {
    llm: Arc<LlmClient>,
    config: PlannerConfig,
}

impl Planner {
    /// Create a new planner.
    pub fn new(llm: Arc<LlmClient>, config: PlannerConfig) -> Self {
        Self { llm, config }
    }

    /// Decompose a user intent into a plan.
    ///
    /// The planner sends the intent along with available tool definitions to
    /// the LLM and asks it to produce a structured plan.
    ///
    /// # Arguments
    ///
    /// * `intent` -- The natural language intent from the user.
    /// * `available_tools` -- Tool definitions the plan may reference.
    /// * `context` -- Optional additional context (e.g. prior conversation).
    pub async fn plan(
        &self,
        intent: &str,
        available_tools: &[ToolDefinition],
        context: Option<&str>,
    ) -> Result<Plan> {
        let system_prompt = self.build_system_prompt(available_tools);
        let user_prompt = self.build_user_prompt(intent, context);

        let request = ChatRequest {
            model: self.config.model.clone(),
            messages: vec![Message::system(system_prompt), Message::user(user_prompt)],
            tools: Vec::new(),
            temperature: Some(0.0),
            max_tokens: Some(self.config.max_tokens),
            stream: false,
        };

        let response = self.llm.chat(&request).await?;

        match response {
            LlmResponse::Text(text) => self.parse_plan(intent, &text),
            LlmResponse::ToolCalls(_) => Err(AgentError::PlanningFailed {
                reason: "LLM unexpectedly returned tool calls during planning".into(),
            }),
        }
    }

    /// Build the system prompt for the planning LLM call.
    fn build_system_prompt(&self, available_tools: &[ToolDefinition]) -> String {
        let tool_list: String = available_tools
            .iter()
            .map(|t| format!("- `{}`: {}", t.name, t.description))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            r#"You are a task planner for OpenIntentOS. Your job is to decompose a user's intent into an ordered sequence of concrete steps.

## Available Tools
{tool_list}

## Output Format
Respond with valid JSON (no markdown fencing) in this exact structure:
{{
  "rationale": "Brief explanation of your approach",
  "steps": [
    {{
      "index": 0,
      "description": "What this step does",
      "tool_name": "name_of_tool",
      "arguments": {{}},
      "depends_on": [],
      "expected_outcome": "What success looks like"
    }}
  ]
}}

## Rules
- Use only the tools listed above.
- Keep the plan minimal — fewest steps necessary.
- Steps execute sequentially by default; use depends_on only for explicit data dependencies.
- Arguments may reference prior step outputs with {{{{step_N.output}}}}.
- If the intent can be fulfilled in a single step, use a single step."#,
        )
    }

    /// Build the user prompt for the planning LLM call.
    fn build_user_prompt(&self, intent: &str, context: Option<&str>) -> String {
        let mut prompt = format!("Decompose this intent into an executable plan:\n\n{intent}");
        if let Some(ctx) = context {
            prompt.push_str(&format!("\n\nAdditional context:\n{ctx}"));
        }
        prompt
    }

    /// Parse the LLM's JSON response into a [`Plan`].
    fn parse_plan(&self, intent: &str, text: &str) -> Result<Plan> {
        // Try to extract JSON from the response (the LLM might wrap it in
        // markdown code fences despite instructions).
        let json_str = extract_json_block(text);

        let v: Value = serde_json::from_str(json_str).map_err(|e| AgentError::PlanningFailed {
            reason: format!("failed to parse plan JSON: {e}\nRaw response:\n{text}"),
        })?;

        let rationale = v["rationale"]
            .as_str()
            .unwrap_or("No rationale provided")
            .to_owned();

        let steps_value = v["steps"]
            .as_array()
            .ok_or_else(|| AgentError::PlanningFailed {
                reason: "plan JSON missing `steps` array".into(),
            })?;

        let steps: Vec<Step> = steps_value
            .iter()
            .enumerate()
            .map(|(i, sv)| {
                Ok(Step {
                    index: sv["index"].as_u64().unwrap_or(i as u64) as u32,
                    description: sv["description"]
                        .as_str()
                        .unwrap_or("Unnamed step")
                        .to_owned(),
                    tool_name: sv["tool_name"].as_str().unwrap_or_default().to_owned(),
                    arguments: sv["arguments"].clone(),
                    depends_on: sv["depends_on"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_u64().map(|n| n as u32))
                                .collect()
                        })
                        .unwrap_or_default(),
                    expected_outcome: sv["expected_outcome"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                })
            })
            .collect::<Result<Vec<_>>>()?;

        if steps.is_empty() {
            return Err(AgentError::PlanningFailed {
                reason: "plan contains zero steps".into(),
            });
        }

        tracing::info!(
            intent = %intent,
            step_count = steps.len(),
            "plan generated"
        );

        Ok(Plan {
            id: Uuid::now_v7(),
            intent: intent.to_owned(),
            steps,
            rationale,
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Try to extract a JSON block from text that might be wrapped in markdown
/// code fences.
fn extract_json_block(text: &str) -> &str {
    let trimmed = text.trim();

    // Check for ```json ... ``` fences.
    if let Some(start) = trimmed.find("```json") {
        let json_start = start + 7; // len("```json")
        if let Some(end) = trimmed[json_start..].find("```") {
            return trimmed[json_start..json_start + end].trim();
        }
    }

    // Check for ``` ... ``` fences (without language tag).
    if let Some(start) = trimmed.find("```") {
        let json_start = start + 3;
        if let Some(end) = trimmed[json_start..].find("```") {
            return trimmed[json_start..json_start + end].trim();
        }
    }

    // Try the raw text as JSON.
    trimmed
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_json_from_fenced_block() {
        let text = r#"Here is the plan:
```json
{"rationale": "test", "steps": []}
```"#;
        let json = extract_json_block(text);
        assert_eq!(json, r#"{"rationale": "test", "steps": []}"#);
    }

    #[test]
    fn extract_json_from_bare_fences() {
        let text = r#"```
{"rationale": "test", "steps": []}
```"#;
        let json = extract_json_block(text);
        assert_eq!(json, r#"{"rationale": "test", "steps": []}"#);
    }

    #[test]
    fn extract_json_plain() {
        let text = r#"{"rationale": "test", "steps": []}"#;
        let json = extract_json_block(text);
        assert_eq!(json, text);
    }

    #[test]
    fn parse_plan_valid() {
        let config = crate::llm::LlmClientConfig::anthropic("test-key", "claude-sonnet-4-20250514");
        let llm = Arc::new(LlmClient::new(config).unwrap());
        let planner = Planner::new(llm, PlannerConfig::default());

        let json = r#"{
            "rationale": "Read the file then summarize it",
            "steps": [
                {
                    "index": 0,
                    "description": "Read the target file",
                    "tool_name": "read_file",
                    "arguments": {"path": "/tmp/test.txt"},
                    "depends_on": [],
                    "expected_outcome": "File contents returned"
                },
                {
                    "index": 1,
                    "description": "Summarize the content",
                    "tool_name": "summarize",
                    "arguments": {"text": "{{step_0.output}}"},
                    "depends_on": [0],
                    "expected_outcome": "A concise summary"
                }
            ]
        }"#;

        let plan = planner.parse_plan("summarize test.txt", json).unwrap();
        assert_eq!(plan.intent, "summarize test.txt");
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[0].tool_name, "read_file");
        assert_eq!(plan.steps[1].depends_on, vec![0]);
        assert_eq!(plan.rationale, "Read the file then summarize it");
    }

    #[test]
    fn parse_plan_empty_steps_fails() {
        let config = crate::llm::LlmClientConfig::anthropic("test-key", "claude-sonnet-4-20250514");
        let llm = Arc::new(LlmClient::new(config).unwrap());
        let planner = Planner::new(llm, PlannerConfig::default());

        let json = r#"{"rationale": "nothing to do", "steps": []}"#;
        let result = planner.parse_plan("do nothing", json);
        assert!(result.is_err());
    }

    #[test]
    fn parse_plan_invalid_json_fails() {
        let config = crate::llm::LlmClientConfig::anthropic("test-key", "claude-sonnet-4-20250514");
        let llm = Arc::new(LlmClient::new(config).unwrap());
        let planner = Planner::new(llm, PlannerConfig::default());

        let result = planner.parse_plan("test", "not json at all");
        assert!(result.is_err());
    }

    #[test]
    fn step_status_serialization() {
        let status = StepStatus::Completed;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"completed\"");

        let parsed: StepStatus = serde_json::from_str("\"failed\"").unwrap();
        assert_eq!(parsed, StepStatus::Failed);
    }
}
