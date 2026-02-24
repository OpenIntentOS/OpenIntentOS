//! Model router.
//!
//! Routes LLM requests to different models based on estimated task complexity.
//! Simple tasks get routed to smaller/cheaper models (e.g. Haiku) while complex
//! tasks escalate to more capable models (e.g. Opus).

use serde::{Deserialize, Serialize};

use crate::error::{AgentError, Result};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for a single model endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    /// The LLM provider (e.g. `"anthropic"`, `"openai"`, `"ollama"`).
    pub provider: String,

    /// The model identifier (e.g. `"claude-sonnet-4-20250514"`, `"claude-opus-4-20250514"`).
    pub model: String,

    /// API key for this provider.  May be empty for local providers.
    #[serde(default)]
    pub api_key: String,

    /// Base URL for the API endpoint.
    pub base_url: String,

    /// Maximum tokens this model supports per response.
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,

    /// Estimated cost tier (lower = cheaper).  Used for routing decisions.
    #[serde(default = "default_cost_tier")]
    pub cost_tier: u8,
}

fn default_max_tokens() -> u32 {
    4096
}

fn default_cost_tier() -> u8 {
    1
}

impl ModelConfig {
    /// Create a config for Anthropic Claude Haiku (fast, cheap).
    pub fn anthropic_haiku(api_key: impl Into<String>) -> Self {
        Self {
            provider: "anthropic".into(),
            model: "claude-haiku-3-20241022".into(),
            api_key: api_key.into(),
            base_url: "https://api.anthropic.com".into(),
            max_tokens: 4096,
            cost_tier: 1,
        }
    }

    /// Create a config for Anthropic Claude Sonnet (balanced).
    pub fn anthropic_sonnet(api_key: impl Into<String>) -> Self {
        Self {
            provider: "anthropic".into(),
            model: "claude-sonnet-4-20250514".into(),
            api_key: api_key.into(),
            base_url: "https://api.anthropic.com".into(),
            max_tokens: 8192,
            cost_tier: 2,
        }
    }

    /// Create a config for Anthropic Claude Opus (most capable).
    pub fn anthropic_opus(api_key: impl Into<String>) -> Self {
        Self {
            provider: "anthropic".into(),
            model: "claude-opus-4-20250514".into(),
            api_key: api_key.into(),
            base_url: "https://api.anthropic.com".into(),
            max_tokens: 8192,
            cost_tier: 3,
        }
    }
}

// ---------------------------------------------------------------------------
// Complexity estimation
// ---------------------------------------------------------------------------

/// Estimated complexity of a task, used to select a model tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Complexity {
    /// Simple, factual, or short tasks.
    Simple,
    /// Moderate tasks requiring some reasoning.
    Medium,
    /// Complex tasks requiring deep reasoning, multi-step planning, or code
    /// generation.
    Complex,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Routes requests to the appropriate model based on task complexity.
///
/// The router holds a tiered set of model configurations: one for each
/// complexity level.  When a request arrives, the router estimates complexity
/// from heuristics on the input and selects the matching model tier.
#[derive(Debug, Clone)]
pub struct ModelRouter {
    /// Model for simple tasks (cheapest / fastest).
    simple: Option<ModelConfig>,
    /// Model for medium tasks.
    medium: Option<ModelConfig>,
    /// Model for complex tasks (most capable).
    complex: Option<ModelConfig>,
}

impl ModelRouter {
    /// Create a new router with explicit model configurations for each tier.
    pub fn new(
        simple: Option<ModelConfig>,
        medium: Option<ModelConfig>,
        complex: Option<ModelConfig>,
    ) -> Self {
        Self {
            simple,
            medium,
            complex,
        }
    }

    /// Create a router that uses a single model for all complexity levels.
    pub fn single(config: ModelConfig) -> Self {
        Self {
            simple: Some(config.clone()),
            medium: Some(config.clone()),
            complex: Some(config),
        }
    }

    /// Select the best model for the given complexity level.
    ///
    /// Falls back to the nearest available tier if the exact tier is not
    /// configured.
    pub fn select(&self, complexity: Complexity) -> Result<&ModelConfig> {
        let primary = match complexity {
            Complexity::Simple => &self.simple,
            Complexity::Medium => &self.medium,
            Complexity::Complex => &self.complex,
        };

        // Try the requested tier first, then fall back through the hierarchy.
        primary
            .as_ref()
            .or(self.medium.as_ref())
            .or(self.complex.as_ref())
            .or(self.simple.as_ref())
            .ok_or_else(|| AgentError::NoModelConfigured {
                provider: "any".into(),
            })
    }

    /// Estimate the complexity of a task from its input text.
    ///
    /// This uses simple heuristics.  In a production system this would be
    /// replaced with a local classifier model (ONNX).
    pub fn estimate_complexity(input: &str) -> Complexity {
        let word_count = input.split_whitespace().count();
        let has_code_markers = input.contains("```")
            || input.contains("fn ")
            || input.contains("class ")
            || input.contains("def ");
        let has_multi_step = input.contains(" and then ")
            || input.contains(" after that ")
            || input.contains(" step ")
            || input.contains(" steps ");
        let has_analysis_keywords = input.contains("analyze")
            || input.contains("compare")
            || input.contains("evaluate")
            || input.contains("synthesize")
            || input.contains("design")
            || input.contains("architect");

        if has_code_markers || has_analysis_keywords || (has_multi_step && word_count > 50) {
            Complexity::Complex
        } else if word_count > 30 || has_multi_step {
            Complexity::Medium
        } else {
            Complexity::Simple
        }
    }

    /// Convenience: estimate complexity and select the model in one step.
    pub fn route(&self, input: &str) -> Result<&ModelConfig> {
        let complexity = Self::estimate_complexity(input);
        tracing::debug!(?complexity, "routed request to model tier");
        self.select(complexity)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_router() -> ModelRouter {
        ModelRouter::new(
            Some(ModelConfig::anthropic_haiku("key")),
            Some(ModelConfig::anthropic_sonnet("key")),
            Some(ModelConfig::anthropic_opus("key")),
        )
    }

    #[test]
    fn simple_input_routes_to_haiku() {
        let router = test_router();
        let config = router.route("What time is it?").unwrap();
        assert!(config.model.contains("haiku"));
    }

    #[test]
    fn medium_input_routes_to_sonnet() {
        let router = test_router();
        let input = "Please summarize the following document and then extract the key action items from the meeting notes that were discussed at length";
        let config = router.route(input).unwrap();
        assert!(config.model.contains("sonnet"));
    }

    #[test]
    fn complex_input_routes_to_opus() {
        let router = test_router();
        let input = "Analyze the codebase architecture and design a new module that handles authentication with OAuth2 providers";
        let config = router.route(input).unwrap();
        assert!(config.model.contains("opus"));
    }

    #[test]
    fn code_markers_trigger_complex() {
        assert_eq!(
            ModelRouter::estimate_complexity("Write a function:\n```rust\nfn main() {}\n```"),
            Complexity::Complex
        );
    }

    #[test]
    fn single_router_always_returns_same() {
        let config = ModelConfig::anthropic_sonnet("key");
        let router = ModelRouter::single(config);
        let simple = router.select(Complexity::Simple).unwrap();
        let complex = router.select(Complexity::Complex).unwrap();
        assert_eq!(simple.model, complex.model);
    }

    #[test]
    fn fallback_when_tier_missing() {
        let router = ModelRouter::new(None, Some(ModelConfig::anthropic_sonnet("key")), None);
        // Simple tier is missing, should fall back to medium.
        let config = router.select(Complexity::Simple).unwrap();
        assert!(config.model.contains("sonnet"));
    }

    #[test]
    fn empty_router_returns_error() {
        let router = ModelRouter::new(None, None, None);
        assert!(router.select(Complexity::Simple).is_err());
    }
}
