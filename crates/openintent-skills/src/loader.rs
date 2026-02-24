//! Skill loader — discovers and loads skills from the filesystem.
//!
//! Skills are stored in directories, each containing a `SKILL.md` file and
//! optional script files.  The loader walks the skills directory and produces
//! [`SkillDefinition`] values.

use std::path::{Path, PathBuf};

use crate::error::{Result, SkillError};
use crate::parser::parse_skill_md;
use crate::types::{ScriptInterpreter, SkillDefinition, SkillScript, SkillStatus};

/// Load all skills from the given directory.
///
/// Each subdirectory is expected to contain a `SKILL.md` file.
/// Directories without `SKILL.md` are silently skipped.
pub fn load_skills_from_dir(dir: &Path) -> Result<Vec<SkillDefinition>> {
    if !dir.exists() {
        tracing::debug!(path = %dir.display(), "skills directory does not exist");
        return Ok(Vec::new());
    }

    let mut skills = Vec::new();

    let entries = std::fs::read_dir(dir).map_err(SkillError::Io)?;

    for entry in entries {
        let entry = entry.map_err(SkillError::Io)?;
        let path = entry.path();

        if !path.is_dir() {
            // Also check for standalone SKILL.md files at the top level.
            if path.file_name().is_some_and(|n| n == "SKILL.md") {
                match load_skill_from_file(&path) {
                    Ok(skill) => skills.push(skill),
                    Err(e) => {
                        tracing::warn!(
                            path = %path.display(),
                            error = %e,
                            "failed to load skill"
                        );
                    }
                }
            }
            continue;
        }

        let skill_md = path.join("SKILL.md");
        if !skill_md.exists() {
            tracing::trace!(path = %path.display(), "no SKILL.md, skipping");
            continue;
        }

        match load_skill_from_dir(&path) {
            Ok(skill) => {
                tracing::info!(
                    name = %skill.name,
                    scripts = skill.scripts.len(),
                    "loaded skill"
                );
                skills.push(skill);
            }
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "failed to load skill"
                );
            }
        }
    }

    tracing::info!(count = skills.len(), dir = %dir.display(), "skills loaded");
    Ok(skills)
}

/// Load a single skill from a directory.
///
/// The directory must contain a `SKILL.md` file.  Any script files
/// (`.sh`, `.py`, `.js`, `.ts`) are detected and attached.
pub fn load_skill_from_dir(dir: &Path) -> Result<SkillDefinition> {
    let skill_md = dir.join("SKILL.md");
    if !skill_md.exists() {
        return Err(SkillError::NotFound(dir.display().to_string()));
    }

    let mut skill = load_skill_from_file(&skill_md)?;

    // Discover script files in the same directory.
    skill.scripts = discover_scripts(dir)?;

    Ok(skill)
}

/// Load a skill from a `SKILL.md` file path.
fn load_skill_from_file(path: &Path) -> Result<SkillDefinition> {
    let content = std::fs::read_to_string(path)?;
    parse_skill_md(&content, path)
}

/// Discover executable scripts in a skill directory.
fn discover_scripts(dir: &Path) -> Result<Vec<SkillScript>> {
    let mut scripts = Vec::new();

    let entries = std::fs::read_dir(dir)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if !path.is_file() {
            continue;
        }

        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        if let Some(interpreter) = ScriptInterpreter::from_extension(ext) {
            let filename = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();

            scripts.push(SkillScript {
                filename,
                path: path.clone(),
                interpreter,
            });
        }
    }

    Ok(scripts)
}

/// Check whether a skill's runtime requirements are satisfied.
pub fn check_requirements(skill: &SkillDefinition) -> SkillStatus {
    let req = &skill.metadata.requires;

    // Check required environment variables.
    for var in &req.env {
        if std::env::var(var).is_err() {
            tracing::debug!(
                skill = %skill.name,
                var = %var,
                "missing required env var"
            );
            return SkillStatus::Degraded;
        }
    }

    // Check required binaries.
    for bin in &req.bins {
        if !binary_exists(bin) {
            tracing::debug!(
                skill = %skill.name,
                bin = %bin,
                "missing required binary"
            );
            return SkillStatus::Unavailable;
        }
    }

    // Check any-of binaries.
    if !req.any_bins.is_empty() && !req.any_bins.iter().any(|b| binary_exists(b)) {
        tracing::debug!(
            skill = %skill.name,
            bins = ?req.any_bins,
            "none of the anyBins found"
        );
        return SkillStatus::Unavailable;
    }

    SkillStatus::Ready
}

/// Check if a binary is available on PATH.
fn binary_exists(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Return the default skills directory path.
///
/// Priority:
/// 1. `$OPENINTENT_SKILLS_DIR` environment variable
/// 2. `./skills/` relative to current working directory
pub fn default_skills_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("OPENINTENT_SKILLS_DIR") {
        return PathBuf::from(dir);
    }
    PathBuf::from("skills")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_from_nonexistent_dir() {
        let skills = load_skills_from_dir(Path::new("/nonexistent/path")).unwrap();
        assert!(skills.is_empty());
    }

    #[test]
    fn check_requirements_no_reqs() {
        let skill = SkillDefinition {
            name: "test".into(),
            description: "test".into(),
            version: None,
            metadata: Default::default(),
            instructions: String::new(),
            source: Default::default(),
            scripts: Vec::new(),
        };
        assert_eq!(check_requirements(&skill), SkillStatus::Ready);
    }

    #[test]
    fn check_requirements_missing_bin() {
        let mut skill = SkillDefinition {
            name: "test".into(),
            description: "test".into(),
            version: None,
            metadata: Default::default(),
            instructions: String::new(),
            source: Default::default(),
            scripts: Vec::new(),
        };
        skill.metadata.requires.bins = vec!["nonexistent_binary_xyz_123".into()];
        assert_eq!(check_requirements(&skill), SkillStatus::Unavailable);
    }

    #[test]
    fn default_skills_dir_fallback() {
        // Without env var set, should return ./skills
        unsafe { std::env::remove_var("OPENINTENT_SKILLS_DIR") };
        assert_eq!(default_skills_dir(), PathBuf::from("skills"));
    }

    #[test]
    fn load_from_temp_dir() {
        let tmp = tempfile::tempdir().unwrap();

        // Create a skill directory.
        let skill_dir = tmp.path().join("my-skill");
        std::fs::create_dir(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: my-skill\ndescription: test skill\n---\nDo something.",
        )
        .unwrap();

        // Create a script.
        std::fs::write(skill_dir.join("run.sh"), "#!/bin/bash\necho hello").unwrap();

        let skills = load_skills_from_dir(tmp.path()).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "my-skill");
        assert_eq!(skills[0].scripts.len(), 1);
        assert_eq!(skills[0].scripts[0].filename, "run.sh");
    }
}
