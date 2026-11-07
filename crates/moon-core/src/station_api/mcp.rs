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
        access: Vec<CoreAccess>,
    },
    Deny {
        request_id: String,
    },
    Revoke {
        grant_id: String,
    },
    /// Replace a connected client's permissions. Its tokens stay valid and carry the new ones from
    /// the next call on, because every call reads them from the grant. `expected` is what the owner
    /// was looking at: a change made elsewhere since is refused rather than silently overwritten.
    SetAccess {
        grant_id: String,
        expected: Vec<CoreAccess>,
        access: Vec<CoreAccess>,
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

/// What a client may do on one core. A list of these rather than a map keyed by core: requests
/// travel in internally tagged enums, which buffer map keys as strings and cannot read them back
/// as integers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CoreAccess {
    pub core: u64,
    pub scopes: Vec<Scope>,
}

/// One revocable authorization: its permissions per stable configured core UID.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "StoredGrantInfo")]
pub struct GrantInfo {
    pub id: String,
    pub client_name: String,
    pub access: Vec<CoreAccess>,
    pub created_ms: i64,
    pub last_access_ms: Option<i64>,
    /// What the client asked for when it connected: the ceiling of any later change to its
    /// permissions. `None` on a grant approved before this was kept, whose request is unknown,
    /// which leaves only the owner's own choice to go by.
    pub requested: Option<Vec<Scope>>,
}

impl GrantInfo {
    /// The grant's permissions on a core, or `None` for a core outside it.
    pub fn scopes_on(&self, core: u64) -> Option<&[Scope]> {
        self.access
            .iter()
            .find(|entry| entry.core == core)
            .map(|entry| entry.scopes.as_slice())
    }

    /// Every permission the grant holds on any core, for OAuth's one flat `scope` value. It says
    /// what a token may ask for, never what it may do on a given core: that is [`Self::scopes_on`].
    pub fn all_scopes(&self) -> Vec<Scope> {
        let mut scopes: Vec<Scope> = self
            .access
            .iter()
            .flat_map(|entry| entry.scopes.iter().copied())
            .collect();
        scopes.sort_unstable();
        scopes.dedup();
        scopes
    }
}

/// A grant as stored, including the shape from before permissions were per core: one list of cores
/// and one list of permissions, which meant those permissions on every one of those cores.
#[derive(Deserialize)]
struct StoredGrantInfo {
    id: String,
    client_name: String,
    #[serde(default)]
    access: Vec<CoreAccess>,
    #[serde(default)]
    cores: Vec<u64>,
    #[serde(default)]
    scopes: Vec<Scope>,
    created_ms: i64,
    last_access_ms: Option<i64>,
    #[serde(default)]
    requested: Option<Vec<Scope>>,
}

impl From<StoredGrantInfo> for GrantInfo {
    /// An older grant keeps exactly the access its owner approved.
    fn from(stored: StoredGrantInfo) -> Self {
        let access = if stored.access.is_empty() {
            let scopes = stored.scopes;
            stored
                .cores
                .into_iter()
                .map(|core| CoreAccess {
                    core,
                    scopes: scopes.clone(),
                })
                .collect()
        } else {
            stored.access
        };
        Self {
            id: stored.id,
            client_name: stored.client_name,
            access,
            created_ms: stored.created_ms,
            last_access_ms: stored.last_access_ms,
            requested: stored.requested,
        }
    }
}

/// One entry of the owner's change journal, one per applied action. It states what the action
/// changed and leaves out unchanged parameters, tool arguments, logs and tokens. Its later fields
/// carry a default, so a store written before them still reads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub at_ms: i64,
    pub grant_id: String,
    pub action: String,
    pub cores: Vec<u64>,
    /// The latest known state of the action; updated in place as its result arrives.
    pub outcome: String,
    /// The operation this entry follows. Empty in entries written before it existed.
    #[serde(default)]
    pub operation: String,
    /// The client's self-declared name at the time, kept so a revoked client's entries still read.
    #[serde(default)]
    pub client_name: String,
    /// What the action targeted: a strategy's name, a market, an order.
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub changes: Vec<AuditChange>,
    /// Changed fields beyond the ones kept in `changes`.
    #[serde(default)]
    pub changes_omitted: u32,
}

/// One value an action changed. Either side is absent when there was no earlier value (a new
/// order, a new strategy's field), or the source has none to state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuditChange {
    pub field: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

#[cfg(test)]
mod tests;
