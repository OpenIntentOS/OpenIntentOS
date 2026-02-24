//! Permission policy engine and audit logging.
//!
//! The policy engine evaluates whether an action (e.g. "read email", "send
//! message", "delete file") is allowed, needs user confirmation, or is denied
//! for a given provider.
//!
//! # Policy Evaluation
//!
//! Policies are evaluated in specificity order:
//!
//! 1. Exact match on `(provider, action, resource)`.
//! 2. Wildcard resource match on `(provider, action, *)`.
//! 3. Wildcard action match on `(provider, *, *)`.
//! 4. Default: [`PolicyDecision::Confirm`] (ask the user).
//!
//! If multiple policies match at the same specificity level, the most
//! restrictive decision wins: `Deny > Confirm > Allow`.
//!
//! # Audit Log
//!
//! Every policy evaluation is recorded in the `audit_log` table with the
//! provider, action, resource, decision, and a detail string. This provides
//! a complete history of what the AI agent did and why.

use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::error::{Result, VaultError};
use crate::store::Vault;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// The outcome of a policy evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDecision {
    /// The action is allowed without user interaction.
    Allow = 0,

    /// The action requires explicit user confirmation before proceeding.
    Confirm = 1,

    /// The action is blocked unconditionally.
    Deny = 2,
}

impl PolicyDecision {
    /// Convert to the string stored in SQLite.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Confirm => "confirm",
            Self::Deny => "deny",
        }
    }

    /// Parse from the string stored in SQLite.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "allow" => Some(Self::Allow),
            "confirm" => Some(Self::Confirm),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }
}

impl std::fmt::Display for PolicyDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A permission policy rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    /// Database row ID (populated after storage).
    pub id: Option<i64>,

    /// The provider this policy applies to (e.g. "github", "slack").
    pub provider: String,

    /// The action this policy governs (e.g. "send_message", "read", "*").
    pub action: String,

    /// The resource this policy applies to (e.g. "channel:general", "*").
    pub resource: String,

    /// The decision for matching requests.
    pub decision: PolicyDecision,

    /// Optional rate limit: maximum number of allowed actions per hour.
    /// Only meaningful when `decision` is [`PolicyDecision::Allow`].
    pub rate_limit: Option<i64>,

    /// When this policy was created.
    pub created_at: DateTime<Utc>,
}

/// A single entry in the audit log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Database row ID.
    pub id: i64,

    /// The provider involved.
    pub provider: String,

    /// The action attempted.
    pub action: String,

    /// The resource targeted (if any).
    pub resource: Option<String>,

    /// The policy decision that was applied.
    pub decision: PolicyDecision,

    /// Additional detail or context.
    pub detail: Option<String>,

    /// When this event occurred.
    pub timestamp: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Policy Engine
// ---------------------------------------------------------------------------

/// Evaluates actions against stored policies and records audit entries.
///
/// The engine operates on the same SQLite database as the [`Vault`], sharing
/// the `policies` and `audit_log` tables.
pub struct PolicyEngine<'a> {
    vault: &'a Vault,
}

impl<'a> PolicyEngine<'a> {
    /// Create a policy engine operating on the given vault.
    pub fn new(vault: &'a Vault) -> Self {
        Self { vault }
    }

    /// Evaluate whether an action is allowed for a given provider and resource.
    ///
    /// The evaluation result is automatically recorded in the audit log.
    ///
    /// # Policy Resolution Order
    ///
    /// 1. Exact match `(provider, action, resource)`.
    /// 2. Wildcard resource `(provider, action, *)`.
    /// 3. Wildcard action `(provider, *, *)`.
    /// 4. Default: `Confirm`.
    ///
    /// If multiple policies match at the same level, the most restrictive
    /// decision wins (`Deny > Confirm > Allow`).
    pub fn evaluate(&self, provider: &str, action: &str, resource: &str) -> Result<PolicyDecision> {
        let conn = self.vault.connection();

        // Query all potentially matching policies, ordered by specificity.
        let mut stmt = conn.prepare(
            "SELECT id, provider, action, resource, decision, rate_limit, created_at
             FROM policies
             WHERE provider = ?1
               AND (action = ?2 OR action = '*')
               AND (resource = ?3 OR resource = '*')
             ORDER BY
               CASE WHEN action = '*' THEN 1 ELSE 0 END,
               CASE WHEN resource = '*' THEN 1 ELSE 0 END",
        )?;

        let policies: Vec<(String, String, String)> = stmt
            .query_map(params![provider, action, resource], |row| {
                Ok((
                    row.get::<_, String>(2)?, // action
                    row.get::<_, String>(3)?, // resource
                    row.get::<_, String>(4)?, // decision
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let decision = if policies.is_empty() {
            // No matching policy — default to requiring confirmation.
            tracing::debug!(
                provider = provider,
                action = action,
                resource = resource,
                "no matching policy, defaulting to Confirm"
            );
            PolicyDecision::Confirm
        } else {
            // Group by specificity tier and take the most restrictive in the
            // most specific tier.
            let mut best_decision = PolicyDecision::Allow;
            let mut best_specificity = u8::MAX; // lower = more specific

            for (pol_action, pol_resource, pol_decision) in &policies {
                let specificity = match (pol_action.as_str(), pol_resource.as_str()) {
                    (a, r) if a != "*" && r != "*" => 0, // exact match
                    (a, _) if a != "*" => 1,             // wildcard resource
                    _ => 2,                              // wildcard action
                };

                let decision =
                    PolicyDecision::parse(pol_decision).unwrap_or(PolicyDecision::Confirm);

                if specificity < best_specificity
                    || (specificity == best_specificity && decision > best_decision)
                {
                    best_specificity = specificity;
                    best_decision = decision;
                }
            }

            tracing::debug!(
                provider = provider,
                action = action,
                resource = resource,
                decision = %best_decision,
                matched_policies = policies.len(),
                "policy evaluated"
            );

            best_decision
        };

        // Record the evaluation in the audit log.
        self.record_audit(provider, action, Some(resource), decision, None)?;

        Ok(decision)
    }

    /// Add a new policy rule.
    ///
    /// Returns the ID of the newly created policy.
    pub fn add_policy(
        &self,
        provider: &str,
        action: &str,
        resource: &str,
        decision: PolicyDecision,
        rate_limit: Option<i64>,
    ) -> Result<i64> {
        let conn = self.vault.connection();
        let now = Utc::now().timestamp();

        conn.execute(
            "INSERT INTO policies (provider, action, resource, decision, rate_limit, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                provider,
                action,
                resource,
                decision.as_str(),
                rate_limit,
                now
            ],
        )?;

        let id = conn.last_insert_rowid();

        tracing::info!(
            id = id,
            provider = provider,
            action = action,
            resource = resource,
            decision = %decision,
            "added policy"
        );

        Ok(id)
    }

    /// Remove a policy by its ID.
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::PolicyNotFound`] if no policy with the given ID
    /// exists.
    pub fn remove_policy(&self, policy_id: i64) -> Result<()> {
        let conn = self.vault.connection();
        let rows = conn.execute("DELETE FROM policies WHERE id = ?1", params![policy_id])?;

        if rows == 0 {
            return Err(VaultError::PolicyNotFound { policy_id });
        }

        tracing::info!(policy_id = policy_id, "removed policy");
        Ok(())
    }

    /// List all policies, optionally filtered by provider.
    pub fn list_policies(&self, provider: Option<&str>) -> Result<Vec<Policy>> {
        let conn = self.vault.connection();

        let (sql, provider_param): (&str, Option<&str>) = match provider {
            Some(p) => (
                "SELECT id, provider, action, resource, decision, rate_limit, created_at
                 FROM policies WHERE provider = ?1 ORDER BY provider, action, resource",
                Some(p),
            ),
            None => (
                "SELECT id, provider, action, resource, decision, rate_limit, created_at
                 FROM policies ORDER BY provider, action, resource",
                None,
            ),
        };

        let mut stmt = conn.prepare(sql)?;

        let rows = if let Some(p) = provider_param {
            stmt.query_map(params![p], map_policy_row)?
        } else {
            stmt.query_map([], map_policy_row)?
        };

        let mut policies = Vec::new();
        for row in rows {
            policies.push(row?);
        }

        Ok(policies)
    }

    // -- Audit Log ----------------------------------------------------------

    /// Record an action in the audit log.
    pub fn record_audit(
        &self,
        provider: &str,
        action: &str,
        resource: Option<&str>,
        decision: PolicyDecision,
        detail: Option<&str>,
    ) -> Result<()> {
        let conn = self.vault.connection();
        let now = Utc::now().timestamp();

        conn.execute(
            "INSERT INTO audit_log (provider, action, resource, decision, detail, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![provider, action, resource, decision.as_str(), detail, now],
        )?;

        tracing::trace!(
            provider = provider,
            action = action,
            decision = %decision,
            "audit entry recorded"
        );

        Ok(())
    }

    /// Query the audit log with optional filters.
    ///
    /// - `provider`: filter by provider name.
    /// - `since`: only entries after this timestamp.
    /// - `limit`: maximum number of entries to return (most recent first).
    pub fn query_audit_log(
        &self,
        provider: Option<&str>,
        since: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<Vec<AuditEntry>> {
        let conn = self.vault.connection();
        let since_ts = since.map(|dt| dt.timestamp()).unwrap_or(0);

        let sql = match provider {
            Some(_) => {
                "SELECT id, provider, action, resource, decision, detail, timestamp
                 FROM audit_log
                 WHERE provider = ?1 AND timestamp >= ?2
                 ORDER BY timestamp DESC
                 LIMIT ?3"
            }
            None => {
                "SELECT id, provider, action, resource, decision, detail, timestamp
                 FROM audit_log
                 WHERE timestamp >= ?1
                 ORDER BY timestamp DESC
                 LIMIT ?2"
            }
        };

        let mut stmt = conn.prepare(sql)?;

        let entries: Vec<AuditEntry> = if let Some(p) = provider {
            stmt.query_map(params![p, since_ts, limit as i64], map_audit_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![since_ts, limit as i64], map_audit_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };

        Ok(entries)
    }
}

// ---------------------------------------------------------------------------
// Row mapping helpers
// ---------------------------------------------------------------------------

fn map_policy_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Policy> {
    Ok(Policy {
        id: Some(row.get(0)?),
        provider: row.get(1)?,
        action: row.get(2)?,
        resource: row.get(3)?,
        decision: PolicyDecision::parse(&row.get::<_, String>(4)?)
            .unwrap_or(PolicyDecision::Confirm),
        rate_limit: row.get(5)?,
        created_at: DateTime::from_timestamp(row.get::<_, i64>(6)?, 0).unwrap_or_default(),
    })
}

fn map_audit_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AuditEntry> {
    Ok(AuditEntry {
        id: row.get(0)?,
        provider: row.get(1)?,
        action: row.get(2)?,
        resource: row.get(3)?,
        decision: PolicyDecision::parse(&row.get::<_, String>(4)?)
            .unwrap_or(PolicyDecision::Confirm),
        detail: row.get(5)?,
        timestamp: DateTime::from_timestamp(row.get::<_, i64>(6)?, 0).unwrap_or_default(),
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto;
    use crate::store::Vault;

    fn test_vault() -> Vault {
        let key = crypto::random_bytes(crypto::KEY_LEN).unwrap();
        Vault::open_in_memory(&key).unwrap()
    }

    #[test]
    fn default_decision_is_confirm() {
        let vault = test_vault();
        let engine = PolicyEngine::new(&vault);

        let decision = engine.evaluate("github", "push", "repo:main").unwrap();
        assert_eq!(decision, PolicyDecision::Confirm);
    }

    #[test]
    fn exact_match_policy() {
        let vault = test_vault();
        let engine = PolicyEngine::new(&vault);

        engine
            .add_policy(
                "slack",
                "send_message",
                "channel:general",
                PolicyDecision::Allow,
                None,
            )
            .unwrap();

        let decision = engine
            .evaluate("slack", "send_message", "channel:general")
            .unwrap();
        assert_eq!(decision, PolicyDecision::Allow);
    }

    #[test]
    fn wildcard_resource_policy() {
        let vault = test_vault();
        let engine = PolicyEngine::new(&vault);

        engine
            .add_policy("github", "read", "*", PolicyDecision::Allow, None)
            .unwrap();

        let decision = engine
            .evaluate("github", "read", "repo:openintentos")
            .unwrap();
        assert_eq!(decision, PolicyDecision::Allow);
    }

    #[test]
    fn wildcard_action_policy() {
        let vault = test_vault();
        let engine = PolicyEngine::new(&vault);

        engine
            .add_policy("notion", "*", "*", PolicyDecision::Deny, None)
            .unwrap();

        let decision = engine
            .evaluate("notion", "create_page", "workspace:main")
            .unwrap();
        assert_eq!(decision, PolicyDecision::Deny);
    }

    #[test]
    fn specific_policy_overrides_wildcard() {
        let vault = test_vault();
        let engine = PolicyEngine::new(&vault);

        // Wildcard: allow everything for github.
        engine
            .add_policy("github", "*", "*", PolicyDecision::Allow, None)
            .unwrap();

        // Specific: deny pushes to main.
        engine
            .add_policy("github", "push", "branch:main", PolicyDecision::Deny, None)
            .unwrap();

        // The specific deny should win.
        let decision = engine.evaluate("github", "push", "branch:main").unwrap();
        assert_eq!(decision, PolicyDecision::Deny);

        // Other actions still use the wildcard allow.
        let decision = engine.evaluate("github", "read", "repo:any").unwrap();
        assert_eq!(decision, PolicyDecision::Allow);
    }

    #[test]
    fn most_restrictive_wins_at_same_level() {
        let vault = test_vault();
        let engine = PolicyEngine::new(&vault);

        // Two policies at the same specificity level.
        engine
            .add_policy("slack", "send_message", "*", PolicyDecision::Allow, None)
            .unwrap();
        engine
            .add_policy("slack", "send_message", "*", PolicyDecision::Deny, None)
            .unwrap();

        let decision = engine
            .evaluate("slack", "send_message", "channel:x")
            .unwrap();
        assert_eq!(decision, PolicyDecision::Deny);
    }

    #[test]
    fn remove_policy() {
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

    #[test]
    fn remove_nonexistent_policy_errors() {
        let vault = test_vault();
        let engine = PolicyEngine::new(&vault);

        let result = engine.remove_policy(9999);
        assert!(matches!(result, Err(VaultError::PolicyNotFound { .. })));
    }

    #[test]
    fn list_policies() {
        let vault = test_vault();
        let engine = PolicyEngine::new(&vault);

        engine
            .add_policy("github", "read", "*", PolicyDecision::Allow, None)
            .unwrap();
        engine
            .add_policy("github", "push", "branch:main", PolicyDecision::Deny, None)
            .unwrap();
        engine
            .add_policy("slack", "send_message", "*", PolicyDecision::Confirm, None)
            .unwrap();

        let all = engine.list_policies(None).unwrap();
        assert_eq!(all.len(), 3);

        let github_only = engine.list_policies(Some("github")).unwrap();
        assert_eq!(github_only.len(), 2);
    }

    #[test]
    fn audit_log_records_evaluations() {
        let vault = test_vault();
        let engine = PolicyEngine::new(&vault);

        engine
            .add_policy("github", "read", "*", PolicyDecision::Allow, None)
            .unwrap();

        // Trigger evaluations.
        engine.evaluate("github", "read", "repo:foo").unwrap();
        engine.evaluate("github", "push", "repo:bar").unwrap();

        let log = engine.query_audit_log(None, None, 100).unwrap();
        assert_eq!(log.len(), 2);

        // Most recent first.
        assert_eq!(log[0].action, "push");
        assert_eq!(log[0].decision, PolicyDecision::Confirm); // default
        assert_eq!(log[1].action, "read");
        assert_eq!(log[1].decision, PolicyDecision::Allow);
    }

    #[test]
    fn audit_log_filtered_by_provider() {
        let vault = test_vault();
        let engine = PolicyEngine::new(&vault);

        engine.evaluate("github", "read", "repo:a").unwrap();
        engine.evaluate("slack", "send", "channel:b").unwrap();
        engine.evaluate("github", "push", "repo:c").unwrap();

        let github_log = engine.query_audit_log(Some("github"), None, 100).unwrap();
        assert_eq!(github_log.len(), 2);
        assert!(github_log.iter().all(|e| e.provider == "github"));
    }

    #[test]
    fn policy_with_rate_limit() {
        let vault = test_vault();
        let engine = PolicyEngine::new(&vault);

        let id = engine
            .add_policy("email", "send", "*", PolicyDecision::Allow, Some(10))
            .unwrap();

        let policies = engine.list_policies(Some("email")).unwrap();
        assert_eq!(policies.len(), 1);
        assert_eq!(policies[0].rate_limit, Some(10));
        assert!(policies[0].id == Some(id));
    }

    #[test]
    fn policy_decision_ordering() {
        // Ensure Deny > Confirm > Allow for the "most restrictive wins" logic.
        assert!(PolicyDecision::Deny > PolicyDecision::Confirm);
        assert!(PolicyDecision::Confirm > PolicyDecision::Allow);
    }
}
