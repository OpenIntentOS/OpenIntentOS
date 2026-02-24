//! ClawHub registry client — search, fetch, and download skills from the
//! OpenClaw skill registry.
//!
//! The ClawHub API provides a catalog of community-built skills that can be
//! installed locally and used within OpenIntentOS.

use crate::error::{Result, SkillError};
use crate::types::SkillSummary;

/// Default ClawHub registry URL.
const DEFAULT_REGISTRY_URL: &str = "https://registry.clawhub.ai";

/// HTTP client for the ClawHub skill registry.
pub struct RegistryClient {
    base_url: String,
    http: reqwest::Client,
}

impl RegistryClient {
    /// Create a new registry client.
    ///
    /// Uses `$CLAWHUB_REGISTRY` if set, otherwise falls back to the default
    /// registry URL.
    pub fn new() -> Self {
        let base_url =
            std::env::var("CLAWHUB_REGISTRY").unwrap_or_else(|_| DEFAULT_REGISTRY_URL.to_owned());
        Self {
            base_url,
            http: reqwest::Client::builder()
                .user_agent("OpenIntentOS/0.1.0")
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Create a client with a custom registry URL.
    pub fn with_url(url: impl Into<String>) -> Self {
        Self {
            base_url: url.into(),
            http: reqwest::Client::builder()
                .user_agent("OpenIntentOS/0.1.0")
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Search the registry for skills matching a query.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SkillSummary>> {
        let url = format!("{}/api/skills/search", self.base_url);

        let response = self
            .http
            .get(&url)
            .query(&[("q", query), ("limit", &limit.to_string())])
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(SkillError::Registry(format!(
                "search failed: HTTP {}",
                response.status()
            )));
        }

        let body: RegistrySearchResponse = response.json().await?;
        Ok(body.skills)
    }

    /// Fetch the full SKILL.md content for a skill by slug.
    pub async fn fetch_skill_md(&self, slug: &str) -> Result<String> {
        let url = format!("{}/api/skills/{}/skill.md", self.base_url, slug);

        let response = self.http.get(&url).send().await?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SkillError::NotFound(slug.to_owned()));
        }

        if !response.status().is_success() {
            return Err(SkillError::Registry(format!(
                "fetch failed: HTTP {}",
                response.status()
            )));
        }

        Ok(response.text().await?)
    }

    /// Fetch the file listing for a skill (to discover scripts).
    pub async fn fetch_skill_files(&self, slug: &str) -> Result<Vec<String>> {
        let url = format!("{}/api/skills/{}/files", self.base_url, slug);

        let response = self.http.get(&url).send().await?;

        if !response.status().is_success() {
            // If file listing is not supported, return empty list.
            return Ok(Vec::new());
        }

        let body: RegistryFilesResponse = response.json().await?;
        Ok(body.files)
    }

    /// Download a specific file from a skill.
    pub async fn download_file(&self, slug: &str, filename: &str) -> Result<Vec<u8>> {
        let url = format!("{}/api/skills/{}/files/{}", self.base_url, slug, filename);

        let response = self.http.get(&url).send().await?;

        if !response.status().is_success() {
            return Err(SkillError::Registry(format!(
                "download failed: HTTP {}",
                response.status()
            )));
        }

        Ok(response.bytes().await?.to_vec())
    }

    /// Fetch skill info / metadata by slug.
    pub async fn info(&self, slug: &str) -> Result<SkillSummary> {
        let url = format!("{}/api/skills/{}", self.base_url, slug);

        let response = self.http.get(&url).send().await?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SkillError::NotFound(slug.to_owned()));
        }

        if !response.status().is_success() {
            return Err(SkillError::Registry(format!(
                "info fetch failed: HTTP {}",
                response.status()
            )));
        }

        Ok(response.json().await?)
    }

    /// Fetch the URL to install a skill from a GitHub repo directly.
    /// Format: `github:<owner>/<repo>` or full URL.
    pub async fn fetch_from_url(&self, url: &str) -> Result<String> {
        // Handle github: shorthand.
        let resolved_url = if let Some(repo) = url.strip_prefix("github:") {
            format!("https://raw.githubusercontent.com/{}/main/SKILL.md", repo)
        } else {
            url.to_owned()
        };

        let response = self.http.get(&resolved_url).send().await?;

        if !response.status().is_success() {
            return Err(SkillError::Registry(format!(
                "URL fetch failed: HTTP {} for {}",
                response.status(),
                resolved_url
            )));
        }

        Ok(response.text().await?)
    }
}

impl Default for RegistryClient {
    fn default() -> Self {
        Self::new()
    }
}

// --- API response types ---

#[derive(Debug, serde::Deserialize)]
struct RegistrySearchResponse {
    #[serde(default)]
    skills: Vec<SkillSummary>,
}

#[derive(Debug, serde::Deserialize)]
struct RegistryFilesResponse {
    #[serde(default)]
    files: Vec<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_registry_url() {
        unsafe { std::env::remove_var("CLAWHUB_REGISTRY") };
        let client = RegistryClient::new();
        assert_eq!(client.base_url, DEFAULT_REGISTRY_URL);
    }

    #[test]
    fn custom_registry_url() {
        let client = RegistryClient::with_url("https://custom.registry.example.com");
        assert_eq!(client.base_url, "https://custom.registry.example.com");
    }
}
