//! Error types for the skills subsystem.

use std::path::PathBuf;

/// Skill-specific errors.
#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("skill not found: `{0}`")]
    NotFound(String),

    #[error("invalid SKILL.md format in `{path}`: {reason}")]
    InvalidFormat { path: PathBuf, reason: String },

    #[error("missing required field `{field}` in SKILL.md at `{path}`")]
    MissingField { path: PathBuf, field: String },

    #[error("skill `{name}` is already installed")]
    AlreadyInstalled { name: String },

    #[error("unsatisfied requirement for skill `{skill}`: {requirement}")]
    UnsatisfiedRequirement { skill: String, requirement: String },

    #[error("registry error: {0}")]
    Registry(String),

    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("script execution failed for skill `{skill}`: {reason}")]
    ScriptFailed { skill: String, reason: String },

    #[error("internal error: {0}")]
    Internal(String),
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, SkillError>;
