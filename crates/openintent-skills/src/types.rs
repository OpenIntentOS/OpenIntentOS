//! Skill type definitions — compatible with OpenClaw's SKILL.md format.
//!
//! A skill is a self-contained unit of capability described by a `SKILL.md`
//! file with YAML frontmatter (metadata) and a markdown body (instructions).
//! Skills can be prompt-only (instructions for the LLM) or include executable
//! scripts that are exposed as tools.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A fully parsed and loaded skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDefinition {
    /// Unique skill name / slug (e.g. `todoist-cli`, `git-commit-helper`).
    pub name: String,

    /// Short human-readable description of what the skill does.
    pub description: String,

    /// Semantic version string (e.g. `1.2.0`).
    pub version: Option<String>,

    /// Structured metadata parsed from YAML frontmatter.
    pub metadata: SkillMetadata,

    /// The raw markdown body — instructions for the LLM.
    pub instructions: String,

    /// Where this skill was loaded from.
    #[serde(skip)]
    pub source: SkillSource,

    /// Executable scripts bundled with this skill, if any.
    #[serde(skip)]
    pub scripts: Vec<SkillScript>,
}

/// Metadata extracted from the YAML frontmatter of a SKILL.md file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillMetadata {
    /// Runtime requirements.
    #[serde(default)]
    pub requires: SkillRequirements,

    /// The main credential environment variable for this skill.
    #[serde(rename = "primaryEnv")]
    pub primary_env: Option<String>,

    /// Optional emoji for display.
    pub emoji: Option<String>,

    /// Homepage or repository URL.
    pub homepage: Option<String>,

    /// Author name or handle.
    pub author: Option<String>,

    /// Tags for categorization and search.
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Runtime requirements declared by a skill.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillRequirements {
    /// Environment variables the skill expects.
    #[serde(default)]
    pub env: Vec<String>,

    /// CLI binaries that must all be installed.
    #[serde(default)]
    pub bins: Vec<String>,

    /// CLI binaries where at least one must exist.
    #[serde(default, rename = "anyBins")]
    pub any_bins: Vec<String>,

    /// Config file paths the skill reads.
    #[serde(default)]
    pub config: Vec<String>,
}

/// Where a skill was loaded from.
#[derive(Debug, Clone, Default)]
pub enum SkillSource {
    /// Loaded from a local directory.
    Local(PathBuf),

    /// Installed from the ClawHub registry.
    ClawHub { slug: String },

    /// Installed from a direct URL.
    Url(String),

    /// Built-in / bundled skill.
    #[default]
    Builtin,
}

/// An executable script bundled with a skill.
#[derive(Debug, Clone)]
pub struct SkillScript {
    /// The script filename (e.g. `run.sh`, `main.py`, `index.js`).
    pub filename: String,

    /// Absolute path to the script file.
    pub path: PathBuf,

    /// The interpreter to use (inferred from extension).
    pub interpreter: ScriptInterpreter,
}

/// Supported script interpreters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptInterpreter {
    /// Shell script (`.sh`, `.bash`).
    Shell,
    /// Python script (`.py`).
    Python,
    /// JavaScript (`.js`, `.mjs`).
    JavaScript,
    /// TypeScript (`.ts`, `.mts`).
    TypeScript,
}

impl ScriptInterpreter {
    /// Detect interpreter from file extension.
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "sh" | "bash" => Some(Self::Shell),
            "py" => Some(Self::Python),
            "js" | "mjs" => Some(Self::JavaScript),
            "ts" | "mts" => Some(Self::TypeScript),
            _ => None,
        }
    }

    /// Return the command used to execute scripts with this interpreter.
    pub fn command(&self) -> &str {
        match self {
            Self::Shell => "bash",
            Self::Python => "python3",
            Self::JavaScript => "node",
            Self::TypeScript => "deno",
        }
    }

    /// Return the arguments needed before the script path.
    pub fn args(&self) -> &[&str] {
        match self {
            Self::Shell | Self::Python | Self::JavaScript => &[],
            Self::TypeScript => &["run", "--allow-all"],
        }
    }
}

/// Summary information about a skill from the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSummary {
    /// The skill slug / identifier.
    pub slug: String,

    /// Short description.
    pub description: String,

    /// Author name or handle.
    pub author: Option<String>,

    /// Number of installs.
    pub installs: Option<u64>,

    /// Tags for categorization.
    #[serde(default)]
    pub tags: Vec<String>,

    /// Latest version.
    pub version: Option<String>,
}

/// Status of a loaded skill after requirement checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillStatus {
    /// All requirements satisfied, skill is ready to use.
    Ready,
    /// Some requirements not met, skill may not function correctly.
    Degraded,
    /// Critical requirements missing, skill cannot function.
    Unavailable,
}
