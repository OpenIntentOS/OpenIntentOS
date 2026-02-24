//! Integration tests for the openintent-web crate.
//!
//! These tests verify the web server configuration and state setup.
//! Full HTTP endpoint testing requires a running server with an LLM client,
//! so these tests focus on configuration and state construction.

use openintent_web::WebConfig;

#[test]
fn web_config_defaults() {
    let config = WebConfig::default();
    assert_eq!(config.bind_addr, "127.0.0.1");
    assert_eq!(config.port, 3000);
}

#[test]
fn web_config_custom() {
    let config = WebConfig {
        bind_addr: "0.0.0.0".into(),
        port: 8080,
    };
    assert_eq!(config.bind_addr, "0.0.0.0");
    assert_eq!(config.port, 8080);
}
