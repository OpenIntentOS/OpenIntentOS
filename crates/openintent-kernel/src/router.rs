//! 3-level intent router.
//!
//! The router resolves natural-language intent strings to concrete handler
//! identifiers using a tiered matching strategy:
//!
//! | Level | Technique | Typical Latency |
//! |-------|-----------|-----------------|
//! | 1 | Exact match via [`aho_corasick`] (SIMD-accelerated) | < 0.01 ms |
//! | 2 | Pattern match via compiled [`regex`] with named captures | < 0.1 ms |
//! | 3 | Fallback marker indicating the intent should be sent to an LLM | N/A |
//!
//! The router is designed to be built incrementally: exact phrases and
//! patterns can be added at runtime, and the internal automaton is rebuilt
//! lazily on the next routing call.
//!
//! # Example
//!
//! ```rust
//! # use openintent_kernel::router::{IntentRouter, RouteResult};
//! let mut router = IntentRouter::new();
//!
//! router.add_exact("open feishu", "adapter:feishu:open");
//! router.add_exact("check email", "adapter:email:check");
//! router.add_pattern(
//!     r"send (?:a )?message to (?P<recipient>\S+)",
//!     "adapter:messaging:send",
//! ).unwrap();
//!
//! let result = router.route("open feishu");
//! assert!(matches!(result, RouteResult::ExactMatch { .. }));
//! ```

use std::collections::HashMap;

use aho_corasick::AhoCorasick;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::{KernelError, Result};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// The outcome of routing an intent string.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RouteResult {
    /// Level 1: the intent text matched one of the exact phrases registered
    /// with the router.
    ExactMatch {
        /// The handler identifier associated with the matched phrase.
        handler: String,
        /// The original phrase that matched.
        matched_phrase: String,
    },

    /// Level 2: the intent text matched a regex pattern.  Named captures are
    /// provided as key/value pairs.
    PatternMatch {
        /// The handler identifier associated with the matched pattern.
        handler: String,
        /// Named captures extracted from the regex.
        captures: HashMap<String, String>,
    },

    /// Level 3: no deterministic route was found.  The caller should forward
    /// the intent to an LLM for classification.
    LlmFallback {
        /// The original intent text that could not be routed.
        intent: String,
    },
}

/// A regex-based route with named captures.
#[derive(Debug, Clone)]
pub struct PatternRoute {
    /// Human-readable description of what this pattern matches.
    pub description: Option<String>,
    /// The handler identifier to invoke on match.
    pub handler: String,
    /// The compiled regex (stored alongside the raw pattern for diagnostics).
    compiled: Regex,
    /// The original pattern string.
    pub pattern: String,
}

// ---------------------------------------------------------------------------
// IntentRouter
// ---------------------------------------------------------------------------

/// Tiered intent router that resolves natural-language text to handler
/// identifiers.
///
/// The router is **not** `Clone` because it holds compiled automata that are
/// expensive to duplicate.  Wrap in `Arc` if shared access is needed.
pub struct IntentRouter {
    /// Exact phrases and their handler identifiers (lowercased keys).
    exact_phrases: Vec<(String, String)>,

    /// The compiled Aho-Corasick automaton (rebuilt lazily).
    automaton: Option<AhoCorasick>,

    /// Whether new exact phrases have been added since the last automaton
    /// build.
    automaton_dirty: bool,

    /// Regex-based pattern routes, evaluated in registration order.
    patterns: Vec<PatternRoute>,
}

impl IntentRouter {
    /// Create an empty router with no routes registered.
    #[must_use]
    pub fn new() -> Self {
        Self {
            exact_phrases: Vec::new(),
            automaton: None,
            automaton_dirty: false,
            patterns: Vec::new(),
        }
    }

    /// Register an exact phrase that maps to a handler.
    ///
    /// Matching is case-insensitive.  The internal automaton is rebuilt lazily
    /// on the next call to [`IntentRouter::route`].
    pub fn add_exact(&mut self, phrase: impl Into<String>, handler: impl Into<String>) {
        let phrase = phrase.into().to_lowercase();
        let handler = handler.into();
        tracing::debug!(phrase = %phrase, handler = %handler, "exact route added");
        self.exact_phrases.push((phrase, handler));
        self.automaton_dirty = true;
    }

    /// Register a regex pattern route.
    ///
    /// The pattern may contain named captures (e.g. `(?P<name>...)`) which
    /// will be extracted and returned in [`RouteResult::PatternMatch`].
    ///
    /// Returns an error if the regex fails to compile.
    pub fn add_pattern(
        &mut self,
        pattern: impl Into<String>,
        handler: impl Into<String>,
    ) -> Result<()> {
        self.add_pattern_with_description(pattern, handler, None)
    }

    /// Register a regex pattern route with an optional description.
    pub fn add_pattern_with_description(
        &mut self,
        pattern: impl Into<String>,
        handler: impl Into<String>,
        description: Option<String>,
    ) -> Result<()> {
        let pattern = pattern.into();
        let handler = handler.into();

        let compiled = Regex::new(&pattern).map_err(|e| KernelError::InvalidPattern {
            pattern: pattern.clone(),
            reason: e.to_string(),
        })?;

        tracing::debug!(
            pattern = %pattern,
            handler = %handler,
            "pattern route added"
        );

        self.patterns.push(PatternRoute {
            description,
            handler,
            compiled,
            pattern,
        });

        Ok(())
    }

    /// Route an intent string through the 3-level cascade.
    ///
    /// 1. Exact match (Aho-Corasick SIMD)
    /// 2. Pattern match (regex)
    /// 3. LLM fallback marker
    pub fn route(&mut self, intent: &str) -> RouteResult {
        let lowered = intent.to_lowercase();

        // Level 1: Exact match.
        if let Some(result) = self.try_exact_match(&lowered) {
            tracing::debug!(intent = %intent, handler = %result.handler(), "L1 exact match");
            return result;
        }

        // Level 2: Pattern match.
        if let Some(result) = self.try_pattern_match(&lowered) {
            tracing::debug!(intent = %intent, handler = %result.handler(), "L2 pattern match");
            return result;
        }

        // Level 3: Fallback.
        tracing::debug!(intent = %intent, "L3 LLM fallback");
        RouteResult::LlmFallback {
            intent: intent.to_string(),
        }
    }

    /// Return the number of registered exact phrases.
    pub fn exact_count(&self) -> usize {
        self.exact_phrases.len()
    }

    /// Return the number of registered pattern routes.
    pub fn pattern_count(&self) -> usize {
        self.patterns.len()
    }

    // -- Private helpers ----------------------------------------------------

    /// Rebuild the Aho-Corasick automaton if it is stale.
    fn ensure_automaton(&mut self) {
        if !self.automaton_dirty && self.automaton.is_some() {
            return;
        }

        if self.exact_phrases.is_empty() {
            self.automaton = None;
            self.automaton_dirty = false;
            return;
        }

        let phrases: Vec<&str> = self.exact_phrases.iter().map(|(p, _)| p.as_str()).collect();

        match AhoCorasick::new(&phrases) {
            Ok(ac) => {
                self.automaton = Some(ac);
                self.automaton_dirty = false;
                tracing::trace!(count = phrases.len(), "aho-corasick automaton rebuilt");
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to build aho-corasick automaton");
                self.automaton = None;
                self.automaton_dirty = false;
            }
        }
    }

    /// Attempt an exact match using the Aho-Corasick automaton.
    fn try_exact_match(&mut self, lowered: &str) -> Option<RouteResult> {
        self.ensure_automaton();

        let ac = self.automaton.as_ref()?;

        // Find the *longest* match so that "open feishu app" does not
        // incorrectly match a shorter "open feishu" if a longer phrase is
        // registered.  Aho-Corasick with `MatchKind::Standard` returns the
        // first match in the automaton order; we iterate all overlapping
        // matches and pick the longest.
        let mut best: Option<(usize, usize)> = None; // (pattern_index, match_len)

        for mat in ac.find_overlapping_iter(lowered) {
            let len = mat.end() - mat.start();
            if best.is_none_or(|(_, best_len)| len > best_len) {
                best = Some((mat.pattern().as_usize(), len));
            }
        }

        let (idx, _) = best?;
        let (phrase, handler) = &self.exact_phrases[idx];

        Some(RouteResult::ExactMatch {
            handler: handler.clone(),
            matched_phrase: phrase.clone(),
        })
    }

    /// Attempt a pattern match against all registered regex routes.
    fn try_pattern_match(&self, lowered: &str) -> Option<RouteResult> {
        for route in &self.patterns {
            if let Some(caps) = route.compiled.captures(lowered) {
                let mut captures = HashMap::new();
                for name in route.compiled.capture_names().flatten() {
                    if let Some(m) = caps.name(name) {
                        captures.insert(name.to_string(), m.as_str().to_string());
                    }
                }

                return Some(RouteResult::PatternMatch {
                    handler: route.handler.clone(),
                    captures,
                });
            }
        }
        None
    }
}

impl Default for IntentRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl RouteResult {
    /// Return the handler string for any variant (convenience accessor).
    pub fn handler(&self) -> &str {
        match self {
            Self::ExactMatch { handler, .. } | Self::PatternMatch { handler, .. } => handler,
            Self::LlmFallback { .. } => "",
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match_case_insensitive() {
        let mut router = IntentRouter::new();
        router.add_exact("Open Feishu", "adapter:feishu:open");

        let result = router.route("open feishu");
        match result {
            RouteResult::ExactMatch { handler, .. } => {
                assert_eq!(handler, "adapter:feishu:open");
            }
            other => panic!("expected ExactMatch, got {other:?}"),
        }
    }

    #[test]
    fn pattern_match_with_captures() {
        let mut router = IntentRouter::new();
        router
            .add_pattern(
                r"send (?:a )?message to (?P<recipient>\S+)",
                "adapter:messaging:send",
            )
            .expect("valid pattern");

        let result = router.route("send a message to alice");
        match result {
            RouteResult::PatternMatch { handler, captures } => {
                assert_eq!(handler, "adapter:messaging:send");
                assert_eq!(captures.get("recipient").map(String::as_str), Some("alice"));
            }
            other => panic!("expected PatternMatch, got {other:?}"),
        }
    }

    #[test]
    fn llm_fallback_for_unknown_intent() {
        let mut router = IntentRouter::new();
        router.add_exact("open feishu", "adapter:feishu:open");

        let result = router.route("analyze my quarterly sales data");
        match result {
            RouteResult::LlmFallback { intent } => {
                assert_eq!(intent, "analyze my quarterly sales data");
            }
            other => panic!("expected LlmFallback, got {other:?}"),
        }
    }

    #[test]
    fn exact_match_takes_precedence_over_pattern() {
        let mut router = IntentRouter::new();
        router.add_exact("check email", "adapter:email:check");
        router
            .add_pattern(r"check (?P<what>\S+)", "generic:check")
            .unwrap();

        let result = router.route("check email");
        assert!(matches!(result, RouteResult::ExactMatch { .. }));
    }

    #[test]
    fn invalid_regex_is_rejected() {
        let mut router = IntentRouter::new();
        let result = router.add_pattern("[invalid(", "handler");
        assert!(result.is_err());
    }

    #[test]
    fn add_routes_at_runtime() {
        let mut router = IntentRouter::new();
        assert_eq!(router.exact_count(), 0);
        assert_eq!(router.pattern_count(), 0);

        router.add_exact("hello", "greet");
        assert_eq!(router.exact_count(), 1);

        router
            .add_pattern(r"bye (?P<name>\w+)", "farewell")
            .unwrap();
        assert_eq!(router.pattern_count(), 1);

        // Exact works.
        assert!(matches!(
            router.route("hello"),
            RouteResult::ExactMatch { .. }
        ));

        // Pattern works.
        let result = router.route("bye world");
        match result {
            RouteResult::PatternMatch { captures, .. } => {
                assert_eq!(captures.get("name").map(String::as_str), Some("world"));
            }
            other => panic!("expected PatternMatch, got {other:?}"),
        }

        // Add another exact and verify automaton is rebuilt.
        router.add_exact("goodbye", "farewell:exact");
        assert!(matches!(
            router.route("goodbye"),
            RouteResult::ExactMatch { .. }
        ));
    }
}
