pub mod email_oauth;

pub use email_oauth::execute_email_oauth_setup;

pub type SkillResult = Result<String, Box<dyn std::error::Error + Send + Sync>>;