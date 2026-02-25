//! SKILL.md parser — compatible with OpenClaw's SKILL.md format.
//!
//! A SKILL.md file consists of:
//! 1. YAML frontmatter delimited by `---` lines.
//! 2. Markdown body containing instructions for the LLM.
//!
//! ```text
//! ---
//! name: my-skill
//! description: Does something useful.
//! version: 1.0.0
//! metadata:
//!   openclaw:
//!     requires:
//!       env:
//!         - MY_API_KEY
//!       bins:
//!         - curl
//!     primaryEnv: MY_API_KEY
//! ---
//!
//! # My Skill
//!
//! Instructions for the LLM go here...
//! ```

use std::path::Path;

use crate::error::{Result, SkillError};
use crate::types::{SkillDefinition, SkillMetadata, SkillRequirements, SkillSource};

/// Raw YAML frontmatter structure — mirrors OpenClaw's format.
#[derive(Debug, serde::Deserialize)]
struct RawFrontmatter {
    name: Option<String>,
    description: Option<String>,
    version: Option<String>,
    #[serde(default)]
    metadata: Option<RawMetadataWrapper>,
    // Direct fields (non-OpenClaw format, simpler).
    author: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    emoji: Option<String>,
    homepage: Option<String>,
    // Direct requires (simplified format).
    requires: Option<RawRequirements>,
    #[serde(rename = "primaryEnv")]
    primary_env: Option<String>,
}

/// Wrapper for the nested `metadata.openclaw` structure.
#[derive(Debug, serde::Deserialize)]
struct RawMetadataWrapper {
    /// OpenClaw-style nested metadata.
    openclaw: Option<RawOpenClawMetadata>,
    /// Also accept `clawdbot` (legacy name).
    clawdbot: Option<RawOpenClawMetadata>,
}

#[derive(Debug, serde::Deserialize)]
struct RawOpenClawMetadata {
    requires: Option<RawRequirements>,
    #[serde(rename = "primaryEnv")]
    primary_env: Option<String>,
    emoji: Option<String>,
    homepage: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct RawRequirements {
    #[serde(default)]
    env: Vec<String>,
    #[serde(default)]
    bins: Vec<String>,
    #[serde(default, rename = "anyBins")]
    any_bins: Vec<String>,
    #[serde(default)]
    config: Vec<String>,
}

/// Split a SKILL.md file into YAML frontmatter and markdown body.
///
/// Returns `(yaml_str, markdown_body)`.
fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let content = content.trim_start();

    // Must start with `---`.
    if !content.starts_with("---") {
        return None;
    }

    let after_first = &content[3..];
    // Find the closing `---`.
    let end = after_first.find("\n---")?;
    let yaml = after_first[..end].trim();
    let body = after_first[end + 4..].trim_start_matches(['\n', '\r']);

    Some((yaml, body))
}

/// Parse a SKILL.md file from its text content.
///
/// Accepts both OpenClaw format (`metadata.openclaw.requires`) and a
/// simplified flat format (`requires` at top level).
pub fn parse_skill_md(content: &str, source_path: &Path) -> Result<SkillDefinition> {
    let (yaml_str, body) = split_frontmatter(content).ok_or_else(|| SkillError::InvalidFormat {
        path: source_path.to_path_buf(),
        reason: "missing YAML frontmatter (must start with ---)".into(),
    })?;

    // Parse YAML using serde_json as intermediary (no serde_yaml dependency).
    let frontmatter: RawFrontmatter =
        parse_yaml_via_json(yaml_str).map_err(|e| SkillError::InvalidFormat {
            path: source_path.to_path_buf(),
            reason: format!("YAML parse error: {e}"),
        })?;

    let name = frontmatter
        .name
        .clone()
        .ok_or_else(|| SkillError::MissingField {
            path: source_path.to_path_buf(),
            field: "name".into(),
        })?;

    let description = frontmatter
        .description
        .clone()
        .unwrap_or_else(|| format!("Skill: {name}"));

    // Resolve metadata — prefer OpenClaw nested format, fall back to flat.
    let (requires, primary_env, emoji, homepage) =
        if let Some(ref meta_wrapper) = frontmatter.metadata {
            let oc = meta_wrapper
                .openclaw
                .as_ref()
                .or(meta_wrapper.clawdbot.as_ref());

            if let Some(oc) = oc {
                let req = oc
                    .requires
                    .as_ref()
                    .map_or_else(SkillRequirements::default, |r| SkillRequirements {
                        env: r.env.clone(),
                        bins: r.bins.clone(),
                        any_bins: r.any_bins.clone(),
                        config: r.config.clone(),
                    });
                (
                    req,
                    oc.primary_env.clone().or(frontmatter.primary_env),
                    oc.emoji.clone().or(frontmatter.emoji),
                    oc.homepage.clone().or(frontmatter.homepage),
                )
            } else {
                resolve_flat_metadata(&frontmatter)
            }
        } else {
            resolve_flat_metadata(&frontmatter)
        };

    let metadata = SkillMetadata {
        requires,
        primary_env,
        emoji,
        homepage,
        author: frontmatter.author,
        tags: frontmatter.tags.unwrap_or_default(),
    };

    Ok(SkillDefinition {
        name,
        description,
        version: frontmatter.version,
        metadata,
        instructions: body.to_owned(),
        source: SkillSource::Local(source_path.to_path_buf()),
        scripts: Vec::new(),
    })
}

fn resolve_flat_metadata(
    fm: &RawFrontmatter,
) -> (
    SkillRequirements,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    let requires = fm
        .requires
        .as_ref()
        .map_or_else(SkillRequirements::default, |r| SkillRequirements {
            env: r.env.clone(),
            bins: r.bins.clone(),
            any_bins: r.any_bins.clone(),
            config: r.config.clone(),
        });
    (
        requires,
        fm.primary_env.clone(),
        fm.emoji.clone(),
        fm.homepage.clone(),
    )
}

// ---------------------------------------------------------------------------
// Minimal YAML parser (avoids serde_yaml dependency)
// ---------------------------------------------------------------------------

/// Parse a simple YAML string by converting it to JSON first.
///
/// This handles the subset of YAML used in SKILL.md frontmatter:
/// - Simple key-value pairs
/// - Nested objects
/// - String lists (`- item`)
///
/// For full YAML compatibility we could add `serde_yaml`, but this covers
/// all real-world SKILL.md files from ClawHub.
fn parse_yaml_via_json<T: serde::de::DeserializeOwned>(
    yaml: &str,
) -> std::result::Result<T, String> {
    let json = yaml_to_json(yaml)?;
    serde_json::from_str(&json).map_err(|e| e.to_string())
}

fn yaml_to_json(yaml: &str) -> std::result::Result<String, String> {
    let mut root = serde_json::Map::new();
    parse_yaml_block(yaml, &mut root, 0)?;
    Ok(serde_json::Value::Object(root).to_string())
}

fn parse_yaml_block(
    yaml: &str,
    map: &mut serde_json::Map<String, serde_json::Value>,
    base_indent: usize,
) -> std::result::Result<(), String> {
    let lines: Vec<&str> = yaml.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        // Skip empty lines and comments.
        if trimmed.is_empty() || trimmed.starts_with('#') {
            i += 1;
            continue;
        }

        let indent = line.len() - line.trim_start().len();
        if indent < base_indent {
            break;
        }

        // List item at this level.
        if trimmed.starts_with("- ") {
            // This shouldn't happen at top level — lists are handled as values.
            i += 1;
            continue;
        }

        // Key-value pair.
        if let Some(colon_pos) = trimmed.find(':') {
            let key = trimmed[..colon_pos].trim().to_owned();
            let value_part = trimmed[colon_pos + 1..].trim();

            if value_part.is_empty() {
                // Nested object or list — look at next lines.
                i += 1;
                let child_indent = if i < lines.len() {
                    let next = lines[i];
                    next.len() - next.trim_start().len()
                } else {
                    indent + 2
                };

                // Check if it's a list (next line starts with `- `).
                if i < lines.len() && lines[i].trim_start().starts_with("- ") {
                    let mut list = Vec::new();
                    while i < lines.len() {
                        let l = lines[i];
                        let li = l.len() - l.trim_start().len();
                        if li < child_indent && !l.trim().is_empty() {
                            break;
                        }
                        let lt = l.trim();
                        if let Some(item) = lt.strip_prefix("- ") {
                            let val = item.trim();
                            // Remove quotes if present.
                            let val = val.trim_matches('"').trim_matches('\'');
                            list.push(serde_json::Value::String(val.to_owned()));
                        } else if lt.is_empty() {
                            // Skip blank lines inside list.
                        } else {
                            break;
                        }
                        i += 1;
                    }
                    map.insert(key, serde_json::Value::Array(list));
                } else {
                    // Nested object.
                    let mut child_map = serde_json::Map::new();
                    let block_end = find_block_end(&lines, i, child_indent);
                    let block = lines[i..block_end].join("\n");
                    parse_yaml_block(&block, &mut child_map, child_indent)?;
                    map.insert(key, serde_json::Value::Object(child_map));
                    i = block_end;
                }
            } else {
                // Inline value.
                let val = parse_yaml_value(value_part);
                map.insert(key, val);
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    Ok(())
}

fn find_block_end(lines: &[&str], start: usize, min_indent: usize) -> usize {
    let mut end = start;
    while end < lines.len() {
        let line = lines[end];
        let trimmed = line.trim();
        if trimmed.is_empty() {
            end += 1;
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent < min_indent {
            break;
        }
        end += 1;
    }
    end
}

fn parse_yaml_value(s: &str) -> serde_json::Value {
    // Remove surrounding quotes.
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        return serde_json::Value::String(s[1..s.len() - 1].to_owned());
    }

    // Inline YAML flow sequence: `[item1, item2, ...]`
    // Supports both JSON-quoted `["a", "b"]` and unquoted YAML `[a, b]`.
    if s.starts_with('[') && s.ends_with(']') {
        // Try direct JSON parse first (handles quoted strings).
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
            return v;
        }
        // Fall back: parse as comma-separated unquoted items.
        let inner = s[1..s.len() - 1].trim();
        if inner.is_empty() {
            return serde_json::Value::Array(Vec::new());
        }
        let items: Vec<serde_json::Value> = inner
            .split(',')
            .map(|item| {
                let item = item.trim().trim_matches('"').trim_matches('\'');
                serde_json::Value::String(item.to_owned())
            })
            .collect();
        return serde_json::Value::Array(items);
    }

    // Inline JSON object: `{key: val, ...}`
    if s.starts_with('{') && s.ends_with('}') {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
            return v;
        }
    }

    // Boolean.
    match s {
        "true" | "yes" | "on" => return serde_json::Value::Bool(true),
        "false" | "no" | "off" => return serde_json::Value::Bool(false),
        "null" | "~" => return serde_json::Value::Null,
        _ => {}
    }

    // Number.
    if let Ok(n) = s.parse::<i64>() {
        return serde_json::Value::Number(n.into());
    }
    if let Ok(n) = s.parse::<f64>()
        && let Some(n) = serde_json::Number::from_f64(n)
    {
        return serde_json::Value::Number(n);
    }

    serde_json::Value::String(s.to_owned())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parse_openclaw_format() {
        let content = r#"---
name: todoist-cli
description: Manage Todoist tasks from the command line.
version: 1.2.0
metadata:
  openclaw:
    requires:
      env:
        - TODOIST_API_KEY
      bins:
        - curl
    primaryEnv: TODOIST_API_KEY
    emoji: "check"
    homepage: https://github.com/example/todoist-cli
---

# Todoist CLI

You can manage Todoist tasks using the HTTP API.

## Usage

Use `http_request` tool to call the Todoist API.
"#;

        let skill = parse_skill_md(content, Path::new("test/SKILL.md")).unwrap();
        assert_eq!(skill.name, "todoist-cli");
        assert_eq!(
            skill.description,
            "Manage Todoist tasks from the command line."
        );
        assert_eq!(skill.version, Some("1.2.0".into()));
        assert_eq!(skill.metadata.requires.env, vec!["TODOIST_API_KEY"]);
        assert_eq!(skill.metadata.requires.bins, vec!["curl"]);
        assert_eq!(skill.metadata.primary_env, Some("TODOIST_API_KEY".into()));
        assert!(skill.instructions.contains("# Todoist CLI"));
        assert!(skill.instructions.contains("http_request"));
    }

    #[test]
    fn parse_flat_format() {
        let content = r#"---
name: simple-skill
description: A simple skill.
tags:
  - utility
  - demo
---

Just do the thing.
"#;

        let skill = parse_skill_md(content, Path::new("test/SKILL.md")).unwrap();
        assert_eq!(skill.name, "simple-skill");
        assert_eq!(skill.description, "A simple skill.");
        assert_eq!(skill.metadata.tags, vec!["utility", "demo"]);
        assert!(skill.instructions.trim() == "Just do the thing.");
    }

    #[test]
    fn missing_name_fails() {
        let content = "---\ndescription: no name\n---\nbody\n";
        let result = parse_skill_md(content, Path::new("test/SKILL.md"));
        assert!(result.is_err());
    }

    #[test]
    fn missing_frontmatter_fails() {
        let content = "# No frontmatter\nJust markdown.";
        let result = parse_skill_md(content, Path::new("test/SKILL.md"));
        assert!(result.is_err());
    }

    #[test]
    fn split_frontmatter_works() {
        let content = "---\nfoo: bar\n---\nbody here";
        let (yaml, body) = split_frontmatter(content).unwrap();
        assert_eq!(yaml, "foo: bar");
        assert_eq!(body, "body here");
    }

    #[test]
    fn yaml_to_json_simple() {
        let yaml = "name: hello\nversion: 1.0.0";
        let json = yaml_to_json(yaml).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["name"], "hello");
        assert_eq!(v["version"], "1.0.0");
    }

    #[test]
    fn yaml_to_json_nested() {
        let yaml = "metadata:\n  openclaw:\n    primaryEnv: MY_KEY";
        let json = yaml_to_json(yaml).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["metadata"]["openclaw"]["primaryEnv"], "MY_KEY");
    }

    #[test]
    fn yaml_to_json_list() {
        let yaml = "items:\n  - one\n  - two\n  - three";
        let json = yaml_to_json(yaml).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let items = v["items"].as_array().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], "one");
    }

    #[test]
    fn yaml_inline_array_quoted() {
        let yaml = r#"tags: ["oauth", "email", "auth"]"#;
        let json = yaml_to_json(yaml).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let tags = v["tags"].as_array().unwrap();
        assert_eq!(tags.len(), 3);
        assert_eq!(tags[0], "oauth");
        assert_eq!(tags[2], "auth");
    }

    #[test]
    fn yaml_inline_array_unquoted() {
        let yaml = "tags: [email, automation, productivity]";
        let json = yaml_to_json(yaml).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let tags = v["tags"].as_array().unwrap();
        assert_eq!(tags.len(), 3);
        assert_eq!(tags[0], "email");
    }

    #[test]
    fn yaml_inline_empty_array() {
        let yaml = "env: []";
        let json = yaml_to_json(yaml).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v["env"].as_array().unwrap().is_empty());
    }

    #[test]
    fn infer_path() {
        let skill = parse_skill_md(
            "---\nname: test\n---\nbody",
            Path::new("/skills/test/SKILL.md"),
        )
        .unwrap();
        match skill.source {
            SkillSource::Local(p) => assert_eq!(p, PathBuf::from("/skills/test/SKILL.md")),
            _ => panic!("expected Local source"),
        }
    }
}
