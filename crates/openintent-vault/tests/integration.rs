//! Integration tests for the openintent-vault crate.
//!
//! These tests exercise the full vault lifecycle including credential storage,
//! retrieval, update, deletion, and policy evaluation.

use openintent_vault::crypto;
use openintent_vault::store::{CredentialType, Vault};
use openintent_vault::{PolicyDecision, PolicyEngine};

/// Create a test vault with a random master key.
fn test_vault() -> Vault {
    let key = crypto::random_bytes(crypto::KEY_LEN).unwrap();
    Vault::open_in_memory(&key).unwrap()
}

// ═══════════════════════════════════════════════════════════════════════
//  Credential lifecycle
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn credential_store_retrieve_delete() {
    let vault = test_vault();
    let data = serde_json::json!({"api_key": "sk-test-12345"});

    // Store.
    vault
        .store_credential(
            "anthropic",
            CredentialType::ApiKey,
            &data,
            None,
            Some("work"),
            None,
        )
        .unwrap();

    // Retrieve.
    let cred = vault.get_credential("anthropic").unwrap();
    assert_eq!(cred.provider, "anthropic");
    assert_eq!(cred.credential_type, CredentialType::ApiKey);
    assert_eq!(cred.data["api_key"], "sk-test-12345");
    assert_eq!(cred.user_label.as_deref(), Some("work"));

    // Delete.
    vault.delete_credential("anthropic").unwrap();

    // Verify gone.
    let result = vault.get_credential("anthropic");
    assert!(result.is_err());
}

#[test]
fn credential_duplicate_rejected() {
    let vault = test_vault();
    let data = serde_json::json!({"api_key": "key1"});

    vault
        .store_credential("github", CredentialType::ApiKey, &data, None, None, None)
        .unwrap();

    let result = vault.store_credential("github", CredentialType::ApiKey, &data, None, None, None);
    assert!(result.is_err());
}

#[test]
fn credential_update() {
    let vault = test_vault();
    let data1 = serde_json::json!({"api_key": "old-key"});
    let data2 = serde_json::json!({"api_key": "new-key"});

    vault
        .store_credential("slack", CredentialType::ApiKey, &data1, None, None, None)
        .unwrap();

    vault.update_credential("slack", &data2, None).unwrap();

    let cred = vault.get_credential("slack").unwrap();
    assert_eq!(cred.data["api_key"], "new-key");
}

#[test]
fn credential_list_summaries() {
    let vault = test_vault();

    vault
        .store_credential(
            "github",
            CredentialType::OAuth,
            &serde_json::json!({"access_token": "gho_xxx"}),
            Some(&["repo".to_string(), "user".to_string()]),
            Some("personal"),
            None,
        )
        .unwrap();

    vault
        .store_credential(
            "anthropic",
            CredentialType::ApiKey,
            &serde_json::json!({"api_key": "sk-ant-xxx"}),
            None,
            Some("work"),
            None,
        )
        .unwrap();

    let list = vault.list_credentials().unwrap();
    assert_eq!(list.len(), 2);

    // Sorted by provider name.
    assert_eq!(list[0].provider, "anthropic");
    assert_eq!(list[1].provider, "github");
    assert_eq!(list[1].credential_type, CredentialType::OAuth);
    assert_eq!(list[1].scopes.as_ref().unwrap().len(), 2);
}

#[test]
fn credential_oauth_with_scopes_and_expiry() {
    let vault = test_vault();
    let expires = chrono::Utc::now() + chrono::Duration::hours(1);
    let data = serde_json::json!({
        "access_token": "gho_xxx",
        "refresh_token": "ghr_yyy",
        "token_type": "Bearer"
    });

    vault
        .store_credential(
            "github",
            CredentialType::OAuth,
            &data,
            Some(&["repo".to_string(), "user:email".to_string()]),
            Some("work"),
            Some(expires),
        )
        .unwrap();

    let cred = vault.get_credential("github").unwrap();
    assert_eq!(cred.credential_type, CredentialType::OAuth);
    assert_eq!(cred.data["access_token"], "gho_xxx");
    assert_eq!(cred.data["refresh_token"], "ghr_yyy");
    assert_eq!(cred.scopes.as_ref().unwrap().len(), 2);
    assert!(cred.expires_at.is_some());
}

#[test]
fn credential_not_found() {
    let vault = test_vault();
    let result = vault.get_credential("nonexistent");
    assert!(result.is_err());
}

#[test]
fn credential_delete_not_found() {
    let vault = test_vault();
    let result = vault.delete_credential("nonexistent");
    assert!(result.is_err());
}

#[test]
fn credential_update_not_found() {
    let vault = test_vault();
    let result = vault.update_credential("nonexistent", &serde_json::json!({"key": "val"}), None);
    assert!(result.is_err());
}

// ═══════════════════════════════════════════════════════════════════════
//  Vault on disk
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn vault_open_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("vault.db");
    let key = crypto::random_bytes(crypto::KEY_LEN).unwrap();

    let vault = Vault::open(&db_path, &key).unwrap();
    vault
        .store_credential(
            "test",
            CredentialType::ApiKey,
            &serde_json::json!({"api_key": "xxx"}),
            None,
            None,
            None,
        )
        .unwrap();

    let cred = vault.get_credential("test").unwrap();
    assert_eq!(cred.data["api_key"], "xxx");

    assert!(db_path.exists());
}

// ═══════════════════════════════════════════════════════════════════════
//  Crypto helpers
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn encrypt_decrypt_roundtrip() {
    let key = crypto::random_bytes(crypto::KEY_LEN).unwrap();
    let plaintext = b"secret data that must survive encryption";

    let (nonce, ciphertext) = crypto::encrypt(plaintext, &key).unwrap();
    let decrypted = crypto::decrypt(&nonce, &ciphertext, &key).unwrap();

    assert_eq!(decrypted, plaintext);
}

#[test]
fn key_derivation_from_password() {
    let (salt, key) = crypto::derive_key_from_password(b"my-strong-password").unwrap();
    assert_eq!(key.len(), crypto::KEY_LEN);
    assert_eq!(salt.len(), crypto::SALT_LEN);

    // Deriving again with the same salt and password should produce the same key.
    let mut key2 = [0u8; crypto::KEY_LEN];
    crypto::derive_key_with_salt(b"my-strong-password", &salt, &mut key2);
    assert_eq!(key, key2);
}

#[test]
fn password_verification() {
    let password = b"correct horse battery staple";
    let (salt, key) = crypto::derive_key_from_password(password).unwrap();

    assert!(crypto::verify_password(password, &salt, &key));
    assert!(!crypto::verify_password(b"wrong password", &salt, &key));
}

// ═══════════════════════════════════════════════════════════════════════
//  Policy engine
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn policy_engine_default_is_confirm() {
    let vault = test_vault();
    let engine = PolicyEngine::new(&vault);

    // No policies -- default should be Confirm.
    let decision = engine.evaluate("github", "push", "repo:main").unwrap();
    assert_eq!(decision, PolicyDecision::Confirm);
}

#[test]
fn policy_engine_allow_policy() {
    let vault = test_vault();

    vault
        .store_credential(
            "anthropic",
            CredentialType::ApiKey,
            &serde_json::json!({"api_key": "sk-xxx"}),
            None,
            None,
            None,
        )
        .unwrap();

    let engine = PolicyEngine::new(&vault);

    engine
        .add_policy("anthropic", "chat", "*", PolicyDecision::Allow, None)
        .unwrap();

    let decision = engine.evaluate("anthropic", "chat", "model:opus").unwrap();
    assert_eq!(decision, PolicyDecision::Allow);
}

#[test]
fn policy_engine_specific_overrides_wildcard() {
    let vault = test_vault();
    let engine = PolicyEngine::new(&vault);

    // Wildcard: allow everything.
    engine
        .add_policy("github", "*", "*", PolicyDecision::Allow, None)
        .unwrap();

    // Specific: deny pushes to main.
    engine
        .add_policy("github", "push", "branch:main", PolicyDecision::Deny, None)
        .unwrap();

    let decision = engine.evaluate("github", "push", "branch:main").unwrap();
    assert_eq!(decision, PolicyDecision::Deny);

    // Other actions still use the wildcard allow.
    let decision = engine.evaluate("github", "read", "repo:any").unwrap();
    assert_eq!(decision, PolicyDecision::Allow);
}

#[test]
fn policy_engine_audit_log() {
    let vault = test_vault();
    let engine = PolicyEngine::new(&vault);

    engine
        .add_policy("github", "read", "*", PolicyDecision::Allow, None)
        .unwrap();

    engine.evaluate("github", "read", "repo:foo").unwrap();
    engine.evaluate("github", "push", "repo:bar").unwrap();

    let log = engine.query_audit_log(None, None, 100).unwrap();
    assert_eq!(log.len(), 2);

    // Most recent first.
    assert_eq!(log[0].action, "push");
    assert_eq!(log[1].action, "read");
}

#[test]
fn policy_engine_remove_policy() {
    let vault = test_vault();
    let engine = PolicyEngine::new(&vault);

    let id = engine
        .add_policy("github", "push", "*", PolicyDecision::Deny, None)
        .unwrap();

    engine.remove_policy(id).unwrap();

    // Should fall back to default Confirm.
    let decision = engine.evaluate("github", "push", "repo:main").unwrap();
    assert_eq!(decision, PolicyDecision::Confirm);
}
