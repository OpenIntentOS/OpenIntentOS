//! Skill manager — install, remove, list, and update skills.
//!
//! The manager coordinates between the filesystem loader and the registry
//! client to provide a unified skill management interface.

use std::path::{Path, PathBuf};

use crate::error::{Result, SkillError};
use crate::loader::{check_requirements, load_skill_from_dir, load_skills_from_dir};
use crate::parser::parse_skill_md;
use crate::registry::RegistryClient;
use crate::types::{ScriptInterpreter, SkillDefinition, SkillStatus};

/// Manages the local skill inventory.
pub struct SkillManager {
    /// Base directory where skills are stored.
    skills_dir: PathBuf,

    /// Registry client for remote operations.
    registry: RegistryClient,

    /// Currently loaded skills (in-memory cache).
    skills: Vec<SkillDefinition>,
}

impl SkillManager {
    /// Create a new skill manager.
    ///
    /// If `skills_dir` does not exist, it will be created on first install.
    pub fn new(skills_dir: PathBuf) -> Self {
        Self {
            skills_dir,
            registry: RegistryClient::new(),
            skills: Vec::new(),
        }
    }

    /// Create a skill manager with a custom registry client.
    pub fn with_registry(skills_dir: PathBuf, registry: RegistryClient) -> Self {
        Self {
            skills_dir,
            registry,
            skills: Vec::new(),
        }
    }

    /// Load all skills from the skills directory.
    pub fn load_all(&mut self) -> Result<&[SkillDefinition]> {
        self.skills = load_skills_from_dir(&self.skills_dir)?;
        Ok(&self.skills)
    }

    /// Return the currently loaded skills.
    pub fn skills(&self) -> &[SkillDefinition] {
        &self.skills
    }

    /// Return the skills directory path.
    pub fn skills_dir(&self) -> &Path {
        &self.skills_dir
    }

    /// Get a loaded skill by name.
    pub fn get(&self, name: &str) -> Option<&SkillDefinition> {
        self.skills.iter().find(|s| s.name == name)
    }

    /// Install a skill from the ClawHub registry by slug.
    pub async fn install_from_registry(&mut self, slug: &str) -> Result<SkillDefinition> {
        // Check if already installed.
        if self.get(slug).is_some() {
            return Err(SkillError::AlreadyInstalled {
                name: slug.to_owned(),
            });
        }

        tracing::info!(slug = %slug, "installing skill from registry");

        // Fetch SKILL.md from registry.
        let skill_md_content = self.registry.fetch_skill_md(slug).await?;

        // Create skill directory.
        let skill_dir = self.skills_dir.join(slug);
        std::fs::create_dir_all(&skill_dir)?;

        // Write SKILL.md.
        std::fs::write(skill_dir.join("SKILL.md"), &skill_md_content)?;

        // Fetch and download any script files.
        let files = self.registry.fetch_skill_files(slug).await?;
        for filename in &files {
            if filename == "SKILL.md" {
                continue;
            }

            // Only download recognized script types.
            let ext = Path::new(filename)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            if ScriptInterpreter::from_extension(ext).is_some() {
                match self.registry.download_file(slug, filename).await {
                    Ok(data) => {
                        let file_path = skill_dir.join(filename);
                        std::fs::write(&file_path, &data)?;

                        // Make shell scripts executable.
                        #[cfg(unix)]
                        if ext == "sh" || ext == "bash" {
                            use std::os::unix::fs::PermissionsExt;
                            let perms = std::fs::Permissions::from_mode(0o755);
                            std::fs::set_permissions(&file_path, perms)?;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            slug = %slug,
                            file = %filename,
                            error = %e,
                            "failed to download skill file"
                        );
                    }
                }
            }
        }

        // Write source metadata.
        let source_meta = serde_json::json!({
            "source": "clawhub",
            "slug": slug,
            "installed_at": chrono::Utc::now().to_rfc3339(),
        });
        std::fs::write(
            skill_dir.join(".source.json"),
            serde_json::to_string_pretty(&source_meta)?,
        )?;

        // Load the installed skill.
        let skill = load_skill_from_dir(&skill_dir)?;
        self.skills.push(skill.clone());

        tracing::info!(name = %skill.name, "skill installed successfully");
        Ok(skill)
    }

    /// Install a skill from a URL (GitHub shorthand or full URL).
    pub async fn install_from_url(&mut self, url: &str) -> Result<SkillDefinition> {
        tracing::info!(url = %url, "installing skill from URL");

        let content = self.registry.fetch_from_url(url).await?;

        // Parse to get the name.
        let skill = parse_skill_md(&content, Path::new("remote/SKILL.md"))?;

        // Check if already installed.
        if self.get(&skill.name).is_some() {
            return Err(SkillError::AlreadyInstalled { name: skill.name });
        }

        // Create skill directory.
        let skill_dir = self.skills_dir.join(&skill.name);
        std::fs::create_dir_all(&skill_dir)?;

        // Write SKILL.md.
        std::fs::write(skill_dir.join("SKILL.md"), &content)?;

        // Write source metadata.
        let source_meta = serde_json::json!({
            "source": "url",
            "url": url,
            "installed_at": chrono::Utc::now().to_rfc3339(),
        });
        std::fs::write(
            skill_dir.join(".source.json"),
            serde_json::to_string_pretty(&source_meta)?,
        )?;

        // Reload from directory.
        let skill = load_skill_from_dir(&skill_dir)?;
        self.skills.push(skill.clone());

        tracing::info!(name = %skill.name, "skill installed from URL");
        Ok(skill)
    }

    /// Remove an installed skill.
    pub fn remove(&mut self, name: &str) -> Result<()> {
        let skill_dir = self.skills_dir.join(name);
        if !skill_dir.exists() {
            return Err(SkillError::NotFound(name.to_owned()));
        }

        std::fs::remove_dir_all(&skill_dir)?;
        self.skills.retain(|s| s.name != name);

        tracing::info!(name = %name, "skill removed");
        Ok(())
    }

    /// Search the ClawHub registry.
    pub async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<crate::types::SkillSummary>> {
        self.registry.search(query, limit).await
    }

    /// Fetch info about a skill from the registry.
    pub async fn info(&self, slug: &str) -> Result<crate::types::SkillSummary> {
        self.registry.info(slug).await
    }

    /// List all installed skills with their status.
    pub fn list_with_status(&self) -> Vec<(&SkillDefinition, SkillStatus)> {
        self.skills
            .iter()
            .map(|s| (s, check_requirements(s)))
            .collect()
    }

    /// Build the combined system prompt extension from all loaded skills.
    ///
    /// This concatenates the instructions from all ready skills into a single
    /// string that can be appended to the system prompt.
    pub fn build_prompt_extension(&self) -> String {
        let ready_skills: Vec<_> = self
            .skills
            .iter()
            .filter(|s| check_requirements(s) != SkillStatus::Unavailable)
            .collect();

        if ready_skills.is_empty() {
            return String::new();
        }

        let mut prompt = String::from("\n\n## Installed Skills\n\n");
        prompt.push_str("You have the following skills available. ");
        prompt.push_str("Use them when relevant to the user's request.\n\n");

        for skill in &ready_skills {
            prompt.push_str(&format!("### Skill: {}\n", skill.name));
            if !skill.description.is_empty() {
                prompt.push_str(&format!("_{}_\n\n", skill.description));
            }
            prompt.push_str(&skill.instructions);
            prompt.push_str("\n\n---\n\n");
        }

        prompt
    }

    /// Ensure the skills directory exists, creating it if needed.
    pub fn ensure_dir(&self) -> Result<()> {
        if !self.skills_dir.exists() {
            std::fs::create_dir_all(&self.skills_dir)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manager_load_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let mut mgr = SkillManager::new(tmp.path().to_path_buf());
        let skills = mgr.load_all().unwrap();
        assert!(skills.is_empty());
    }

    #[test]
    fn manager_load_and_remove() {
        let tmp = tempfile::tempdir().unwrap();

        // Create a skill.
        let skill_dir = tmp.path().join("test-skill");
        std::fs::create_dir(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: test-skill\ndescription: A test.\n---\nInstructions here.",
        )
        .unwrap();

        let mut mgr = SkillManager::new(tmp.path().to_path_buf());
        mgr.load_all().unwrap();
        assert_eq!(mgr.skills().len(), 1);
        assert!(mgr.get("test-skill").is_some());

        // Remove.
        mgr.remove("test-skill").unwrap();
        assert!(mgr.skills().is_empty());
        assert!(!skill_dir.exists());
    }

    #[test]
    fn manager_remove_nonexistent() {
        let tmp = tempfile::tempdir().unwrap();
        let mut mgr = SkillManager::new(tmp.path().to_path_buf());
        let result = mgr.remove("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn build_prompt_extension_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = SkillManager::new(tmp.path().to_path_buf());
        assert!(mgr.build_prompt_extension().is_empty());
    }

    #[test]
    fn build_prompt_extension_with_skills() {
        let tmp = tempfile::tempdir().unwrap();

        // Create two skills.
        for name in &["skill-a", "skill-b"] {
            let dir = tmp.path().join(name);
            std::fs::create_dir(&dir).unwrap();
            std::fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: Skill {name}\n---\nDo {name} things."),
            )
            .unwrap();
        }

        let mut mgr = SkillManager::new(tmp.path().to_path_buf());
        mgr.load_all().unwrap();

        let ext = mgr.build_prompt_extension();
        assert!(ext.contains("skill-a"));
        assert!(ext.contains("skill-b"));
        assert!(ext.contains("Do skill-a things."));
    }
}
