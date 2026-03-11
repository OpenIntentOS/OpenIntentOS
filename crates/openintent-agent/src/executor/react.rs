//! ReAct-loop helpers for the step executor.
//!
//! Provides placeholder resolution for step arguments, substituting
//! `{{step_N.output}}` tokens with actual outputs from prior steps.

use std::collections::HashMap;

use serde_json::Value;

/// Resolve `{{step_N.output}}` placeholders in a JSON value by substituting
/// the actual outputs from prior steps.
pub fn resolve_placeholders(value: &Value, outputs: &HashMap<u32, String>) -> Value {
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
