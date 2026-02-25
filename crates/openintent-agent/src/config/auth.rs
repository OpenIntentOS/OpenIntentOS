//! Authentication profile validation.
//!
//! This module provides JSON schema validation for authentication profiles,
//! ensuring secure and consistent auth configuration across the system.

use std::collections::HashMap;
use std::path::Path;

use jsonschema::{Draft, JSONSchema};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{AgentError, Result};

/// Authentication provider type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AuthProvider {
    OAuth2,
    ApiKey,
    Bearer,
    Basic,
    Custom,
}

/// OAuth2 configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuth2Config {
    pub client_id: String,
    pub client_secret: Option<String>, // Optional for PKCE flows
    pub auth_url: String,
    pub token_url: String,
    pub scopes: Vec<String>,
    pub redirect_uri: String,
    #[serde(default)]
    pub use_pkce: bool,
}

/// Authentication profile configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthProfile {
    pub name: String,
    pub provider: AuthProvider,
    pub description: Option<String>,
    
    // OAuth2 specific
    pub oauth2: Option<OAuth2Config>,
    
    // API Key specific
    pub api_key_header: Option<String>,
    pub api_key_query_param: Option<String>,
    
    // Bearer token specific
    pub bearer_token: Option<String>,
    
    // Basic auth specific
    pub username: Option<String>,
    pub password: Option<String>,
    
    // Custom headers
    pub custom_headers: Option<HashMap<String, String>>,
    
    // Validation settings
    #[serde(default)]
    pub validate_ssl: bool,
    pub base_url: Option<String>,
    
    // Metadata
    #[serde(default)]
    pub enabled: bool,
    pub tags: Option<Vec<String>>,
}

impl Default for AuthProfile {
    fn default() -> Self {
        Self {
            name: String::new(),
            provider: AuthProvider::ApiKey,
            description: None,
            oauth2: None,
            api_key_header: None,
            api_key_query_param: None,
            bearer_token: None,
            username: None,
            password: None,
            custom_headers: None,
            validate_ssl: true,
            base_url: None,
            enabled: true,
            tags: None,
        }
    }
}

/// Authentication profile validator.
pub struct AuthProfileValidator {
    schema: JSONSchema,
}

impl AuthProfileValidator {
    /// Create a new validator with the built-in schema.
    pub fn new() -> Result<Self> {
        let schema_value = Self::get_auth_profile_schema();
        let schema = JSONSchema::options()
            .with_draft(Draft::Draft7)
            .compile(&schema_value)
            .map_err(|e| AgentError::ValidationError {
                reason: format!("Failed to compile auth profile schema: {}", e),
            })?;

        Ok(Self { schema })
    }

    /// Validate an authentication profile.
    pub fn validate(&self, profile: &AuthProfile) -> Result<()> {
        let profile_json = serde_json::to_value(profile).map_err(|e| AgentError::ValidationError {
            reason: format!("Failed to serialize auth profile: {}", e),
        })?;

        let validation_result = self.schema.validate(&profile_json);
        
        if let Err(errors) = validation_result {
            let error_messages: Vec<String> = errors
                .map(|e| format!("{}: {}", e.instance_path, e))
                .collect();
            
            return Err(AgentError::ValidationError {
                reason: format!("Auth profile validation failed: {}", error_messages.join(", ")),
            });
        }

        // Additional semantic validation
        self.validate_semantic(profile)?;

        Ok(())
    }

    /// Validate a collection of auth profiles from a file.
    pub fn validate_file<P: AsRef<Path>>(&self, path: P) -> Result<Vec<AuthProfile>> {
        let content = std::fs::read_to_string(path).map_err(|e| AgentError::ValidationError {
            reason: format!("Failed to read auth profiles file: {}", e),
        })?;

        let profiles: Vec<AuthProfile> = if content.trim_start().starts_with('[') {
            // JSON array format
            serde_json::from_str(&content).map_err(|e| AgentError::ValidationError {
                reason: format!("Failed to parse JSON auth profiles: {}", e),
            })?
        } else {
            // TOML format
            toml::from_str(&content).map_err(|e| AgentError::ValidationError {
                reason: format!("Failed to parse TOML auth profiles: {}", e),
            })?
        };

        // Validate each profile
        for profile in &profiles {
            self.validate(profile)?;
        }

        // Check for duplicate names
        let mut names = std::collections::HashSet::new();
        for profile in &profiles {
            if !names.insert(&profile.name) {
                return Err(AgentError::ValidationError {
                    reason: format!("Duplicate auth profile name: {}", profile.name),
                });
            }
        }

        Ok(profiles)
    }

    /// Perform semantic validation beyond JSON schema.
    fn validate_semantic(&self, profile: &AuthProfile) -> Result<()> {
        // Validate OAuth2 configuration
        if profile.provider == AuthProvider::OAuth2 {
            let oauth2 = profile.oauth2.as_ref().ok_or_else(|| AgentError::ValidationError {
                reason: "OAuth2 provider requires oauth2 configuration".into(),
            })?;

            if oauth2.scopes.is_empty() {
                return Err(AgentError::ValidationError {
                    reason: "OAuth2 configuration must specify at least one scope".into(),
                });
            }

            if oauth2.use_pkce && oauth2.client_secret.is_some() {
                return Err(AgentError::ValidationError {
                    reason: "PKCE flows should not include client_secret".into(),
                });
            }
        }

        // Validate API Key configuration
        if profile.provider == AuthProvider::ApiKey {
            if profile.api_key_header.is_none() && profile.api_key_query_param.is_none() {
                return Err(AgentError::ValidationError {
                    reason: "API Key provider requires either api_key_header or api_key_query_param".into(),
                });
            }
        }

        // Validate Bearer token configuration
        if profile.provider == AuthProvider::Bearer && profile.bearer_token.is_none() {
            return Err(AgentError::ValidationError {
                reason: "Bearer provider requires bearer_token".into(),
            });
        }

        // Validate Basic auth configuration
        if profile.provider == AuthProvider::Basic {
            if profile.username.is_none() || profile.password.is_none() {
                return Err(AgentError::ValidationError {
                    reason: "Basic auth provider requires both username and password".into(),
                });
            }
        }

        // Validate URLs
        if let Some(ref base_url) = profile.base_url {
            if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
                return Err(AgentError::ValidationError {
                    reason: "base_url must be a valid HTTP or HTTPS URL".into(),
                });
            }
        }

        if let Some(ref oauth2) = profile.oauth2 {
            for url in [&oauth2.auth_url, &oauth2.token_url, &oauth2.redirect_uri] {
                if !url.starts_with("http://") && !url.starts_with("https://") {
                    return Err(AgentError::ValidationError {
                        reason: format!("OAuth2 URL must be valid HTTP or HTTPS: {}", url),
                    });
                }
            }
        }

        Ok(())
    }

    /// Get the JSON schema for authentication profiles.
    fn get_auth_profile_schema() -> Value {
        json!({
            "$schema": "https://json-schema.org/draft/2019-09/schema",
            "title": "Authentication Profile",
            "type": "object",
            "required": ["name", "provider"],
            "properties": {
                "name": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 100,
                    "pattern": "^[a-zA-Z0-9_-]+$",
                    "description": "Unique identifier for the auth profile"
                },
                "provider": {
                    "type": "string",
                    "enum": ["oauth2", "apikey", "bearer", "basic", "custom"],
                    "description": "Authentication provider type"
                },
                "description": {
                    "type": "string",
                    "maxLength": 500,
                    "description": "Human-readable description of the auth profile"
                },
                "oauth2": {
                    "type": "object",
                    "required": ["client_id", "auth_url", "token_url", "scopes", "redirect_uri"],
                    "properties": {
                        "client_id": {
                            "type": "string",
                            "minLength": 1
                        },
                        "client_secret": {
                            "type": "string",
                            "minLength": 1
                        },
                        "auth_url": {
                            "type": "string",
                            "format": "uri"
                        },
                        "token_url": {
                            "type": "string",
                            "format": "uri"
                        },
                        "scopes": {
                            "type": "array",
                            "items": {
                                "type": "string",
                                "minLength": 1
                            },
                            "minItems": 1
                        },
                        "redirect_uri": {
                            "type": "string",
                            "format": "uri"
                        },
                        "use_pkce": {
                            "type": "boolean",
                            "default": false
                        }
                    }
                },
                "api_key_header": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Header name for API key authentication"
                },
                "api_key_query_param": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Query parameter name for API key authentication"
                },
                "bearer_token": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Bearer token for authentication"
                },
                "username": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Username for basic authentication"
                },
                "password": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Password for basic authentication"
                },
                "custom_headers": {
                    "type": "object",
                    "additionalProperties": {
                        "type": "string"
                    },
                    "description": "Custom headers to include in requests"
                },
                "validate_ssl": {
                    "type": "boolean",
                    "default": true,
                    "description": "Whether to validate SSL certificates"
                },
                "base_url": {
                    "type": "string",
                    "format": "uri",
                    "description": "Base URL for the API endpoint"
                },
                "enabled": {
                    "type": "boolean",
                    "default": true,
                    "description": "Whether this auth profile is enabled"
                },
                "tags": {
                    "type": "array",
                    "items": {
                        "type": "string",
                        "minLength": 1
                    },
                    "description": "Tags for categorizing auth profiles"
                }
            }
        })
    }
}

impl Default for AuthProfileValidator {
    fn default() -> Self {
        Self::new().expect("Failed to create default AuthProfileValidator")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_oauth2_profile() {
        let validator = AuthProfileValidator::new().unwrap();
        
        let profile = AuthProfile {
            name: "github_oauth".to_string(),
            provider: AuthProvider::OAuth2,
            oauth2: Some(OAuth2Config {
                client_id: "client123".to_string(),
                client_secret: Some("secret456".to_string()),
                auth_url: "https://github.com/login/oauth/authorize".to_string(),
                token_url: "https://github.com/login/oauth/access_token".to_string(),
                scopes: vec!["read:user".to_string(), "repo".to_string()],
                redirect_uri: "http://localhost:8080/callback".to_string(),
                use_pkce: false,
            }),
            ..Default::default()
        };

        assert!(validator.validate(&profile).is_ok());
    }

    #[test]
    fn valid_api_key_profile() {
        let validator = AuthProfileValidator::new().unwrap();
        
        let profile = AuthProfile {
            name: "api_key_auth".to_string(),
            provider: AuthProvider::ApiKey,
            api_key_header: Some("X-API-Key".to_string()),
            base_url: Some("https://api.example.com".to_string()),
            ..Default::default()
        };

        assert!(validator.validate(&profile).is_ok());
    }

    #[test]
    fn invalid_oauth2_without_config() {
        let validator = AuthProfileValidator::new().unwrap();
        
        let profile = AuthProfile {
            name: "broken_oauth".to_string(),
            provider: AuthProvider::OAuth2,
            oauth2: None,
            ..Default::default()
        };

        assert!(validator.validate(&profile).is_err());
    }

    #[test]
    fn invalid_api_key_without_header_or_param() {
        let validator = AuthProfileValidator::new().unwrap();
        
        let profile = AuthProfile {
            name: "broken_api_key".to_string(),
            provider: AuthProvider::ApiKey,
            api_key_header: None,
            api_key_query_param: None,
            ..Default::default()
        };

        assert!(validator.validate(&profile).is_err());
    }

    #[test]
    fn invalid_name_with_special_chars() {
        let validator = AuthProfileValidator::new().unwrap();
        
        let profile = AuthProfile {
            name: "invalid@name!".to_string(),
            provider: AuthProvider::ApiKey,
            api_key_header: Some("X-API-Key".to_string()),
            ..Default::default()
        };

        assert!(validator.validate(&profile).is_err());
    }

    #[test]
    fn pkce_without_client_secret() {
        let validator = AuthProfileValidator::new().unwrap();
        
        let profile = AuthProfile {
            name: "pkce_oauth".to_string(),
            provider: AuthProvider::OAuth2,
            oauth2: Some(OAuth2Config {
                client_id: "client123".to_string(),
                client_secret: None,
                auth_url: "https://example.com/auth".to_string(),
                token_url: "https://example.com/token".to_string(),
                scopes: vec!["openid".to_string()],
                redirect_uri: "http://localhost:8080/callback".to_string(),
                use_pkce: true,
            }),
            ..Default::default()
        };

        assert!(validator.validate(&profile).is_ok());
    }

    #[test]
    fn pkce_with_client_secret_should_fail() {
        let validator = AuthProfileValidator::new().unwrap();
        
        let profile = AuthProfile {
            name: "bad_pkce".to_string(),
            provider: AuthProvider::OAuth2,
            oauth2: Some(OAuth2Config {
                client_id: "client123".to_string(),
                client_secret: Some("secret".to_string()),
                auth_url: "https://example.com/auth".to_string(),
                token_url: "https://example.com/token".to_string(),
                scopes: vec!["openid".to_string()],
                redirect_uri: "http://localhost:8080/callback".to_string(),
                use_pkce: true,
            }),
            ..Default::default()
        };

        assert!(validator.validate(&profile).is_err());
    }
}