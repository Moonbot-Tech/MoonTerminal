//! Private administrative contract for Station MCP. Only the SSH control channel accepts it.

use serde::{Deserialize, Serialize};

/// Permissions the station enforces independently of tool descriptions or model instructions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Read,
    Strategies,
    Trading,
    Settings,
}

impl Scope {
    /// OAuth scope spelling, shared by discovery, consent and token checks.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Strategies => "strategies",
            Self::Trading => "trading",
            Self::Settings => "settings",
        }
    }
}

/// Owner actions, never exposed as MCP tools or public HTTP routes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum AdminRequest {
    Status,
    /// A code copied from the authorization page binds consent to that browser's request.
    Approve {
        code: String,
        cores: Vec<u64>,
        scopes: Vec<Scope>,
    },
    Deny {
        request_id: String,
    },
    Revoke {
        grant_id: String,
    },
    /// Turning off access also revokes all grants; turning it back on requires fresh consent.
    SetEnabled {
        enabled: bool,
    },
}

/// Safe-to-display status: no authorization codes, tokens or bot credentials.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AdminState {
    pub configured: bool,
    pub enabled: bool,
    pub endpoint: Option<String>,
    pub cores: Vec<CoreInfo>,
    pub pending: Vec<PendingConsent>,
    pub grants: Vec<GrantInfo>,
    pub audit: Vec<AuditEntry>,
    pub knowledge_revision: Option<String>,
    pub knowledge_checked_ms: Option<i64>,
}

/// Configured core choices for owner consent, independent of the Telegram bot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CoreInfo {
    pub uid: u64,
    pub name: String,
}

/// A public client's request is untrusted display data until the owner supplies its page code.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingConsent {
    pub request_id: String,
    pub client_name: String,
    pub redirect_uri: String,
    pub scopes: Vec<Scope>,
    pub expires_ms: i64,
}

/// One revocable authorization, restricted to stable configured core UIDs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GrantInfo {
    pub id: String,
    pub client_name: String,
    pub cores: Vec<u64>,
    pub scopes: Vec<Scope>,
    pub created_ms: i64,
    pub last_access_ms: Option<i64>,
}

/// Bounded command history deliberately excludes tool arguments, logs and tokens.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub at_ms: i64,
    pub grant_id: String,
    pub action: String,
    pub cores: Vec<u64>,
    pub outcome: String,
}
