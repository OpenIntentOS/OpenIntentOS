//! Skill system for OpenIntentOS — compatible with OpenClaw SKILL.md format.
//!
//! This crate provides:
//!
//! - **SKILL.md parser** — parses OpenClaw-compatible skill definitions with
//!   YAML frontmatter and markdown instructions.
//!
//! - **Skill loader** — discovers and loads skills from the local filesystem.
//!
//! - **ClawHub registry client** — searches and installs skills from the
//!   OpenClaw community skill registry (5,700+ skills).
//!
//! - **Skill manager** — install, remove, list, and update skills.
//!
//! - **Skill adapter** — bridges skills into the [`openintent_adapters::Adapter`]
//!   trait so script-based skills become tools the agent can invoke.
//!
//! # Integration
//!
//! Skills integrate with the agent runtime in two ways:
//!
//! 1. **Prompt injection** — skill instructions are appended to the system
//!    prompt, telling the LLM how to use existing tools to accomplish the
//!    skill's purpose.
//!
//! 2. **Script tools** — skills with executable scripts (`.sh`, `.py`, `.js`,
//!    `.ts`) are exposed as additional tools via [`SkillAdapter`].
//!
//! # Example
//!
//! ```rust,no_run
//! use openintent_skills::{SkillManager, SkillAdapter};
//! use std::path::PathBuf;
//!
//! // Load all installed skills.
//! let mut manager = SkillManager::new(PathBuf::from("skills"));
//! manager.load_all().unwrap();
//!
//! // Build system prompt extension from skill instructions.
//! let prompt_ext = manager.build_prompt_extension();
//!
//! // Create adapter for script-based tools.
//! let adapter = SkillAdapter::new("skills", manager.skills());
//! ```

pub mod adapter;
pub mod error;
pub mod loader;
pub mod manager;
pub mod parser;
pub mod registry;
pub mod types;

pub use adapter::SkillAdapter;
pub use error::{Result, SkillError};
pub use loader::{check_requirements, default_skills_dir, load_skills_from_dir};
pub use manager::SkillManager;
pub use parser::parse_skill_md;
pub use registry::RegistryClient;
pub use types::{
    ScriptInterpreter, SkillDefinition, SkillMetadata, SkillRequirements, SkillScript, SkillSource,
    SkillStatus, SkillSummary,
};
