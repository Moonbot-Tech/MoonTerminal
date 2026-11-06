//! Local OAuth authorization store. The private control socket is the only consent authority.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;

use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use moon_core::station_api::mcp::{
    AdminRequest, AdminState, AuditEntry, GrantInfo, PendingConsent, Scope,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

/// Short access tokens limit stolen-bearer exposure; refresh tokens rotate at every exchange.
const ACCESS_MS: i64 = 10 * 60 * 1000;
const REFRESH_MS: i64 = 30 * 24 * 60 * 60 * 1000;
const CONSENT_MS: i64 = 10 * 60 * 1000;

/// Cryptographically random opaque identifier; raw credentials never appear in stored tokens.
pub(super) fn random() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|e| anyhow::anyhow!("OS random: {e}"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// Domain-independent one-way representation for high-entropy opaque credentials.
pub(super) fn hash(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_bytes()))
}

/// Authenticate a refresh lineage without retaining every consumed token for thirty days.
fn refresh_mac(key: &str, payload: &str) -> Result<Hmac<Sha256>> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes())?;
    mac.update(payload.as_bytes());
    Ok(mac)
}

/// Wall-clock UTC milliseconds; supplied explicitly to state transitions for deterministic tests.
pub(super) fn now() -> i64 {
    moon_core::util::now_unix_ms_i64()
}

/// A registered client. Registration grants no authority to read or command cores.
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Client {
    pub id: String,
    pub name: String,
    pub redirects: Vec<String>,
    pub method: String,
    pub secret_hash: Option<String>,
    pub expires_ms: i64,
}

/// A grant retains its client binding through both code exchange and token refresh.
#[derive(Clone, Serialize, Deserialize)]
struct Grant {
    info: GrantInfo,
    client_id: String,
    refresh_key: String,
    refresh_hash: Option<String>,
}

/// Only short-lived access hashes are retained; refresh lineage is authenticated by its grant key.
#[derive(Clone, Serialize, Deserialize)]
struct Token {
    grant: String,
    expires_ms: i64,
}

/// Only durable authorization state is saved; in-progress browser flows die on restart.
#[derive(Clone, Default, Serialize, Deserialize)]
struct Disk {
    #[serde(default)]
    resource: String,
    enabled: bool,
    clients: BTreeMap<String, Client>,
    grants: BTreeMap<String, Grant>,
    tokens: BTreeMap<String, Token>,
    audit: VecDeque<AuditEntry>,
}

/// Browser flow bound to a cookie, PKCE challenge, exact registered redirect and owner-entered code.
struct Pending {
    info: PendingConsent,
    client_id: String,
    owner_code: String,
    browser_hash: String,
    challenge: String,
    state: String,
    approved: Option<(String, String)>,
}

/// A single-use code is only redeemable by the client and PKCE verifier that began the flow.
struct Code {
    grant: String,
    redirect: String,
    challenge: String,
    expires_ms: i64,
}

/// Identity carried into the owner queue; it is revalidated there, not trusted as a cached grant.
#[derive(Clone)]
pub(super) struct Principal {
    pub token_hash: String,
    pub grant_id: String,
}

/// Successful OAuth token response, used only as a no-store HTTP body.
#[derive(Serialize)]
pub(super) struct Tokens {
    access_token: String,
    token_type: &'static str,
    expires_in: i64,
    refresh_token: String,
    scope: String,
}

/// One station's bounded authorization state, shared by HTTP workers and owner administration.
pub(super) struct Auth {
    disk: Disk,
    path: PathBuf,
    pending: BTreeMap<String, Pending>,
    codes: BTreeMap<String, Code>,
}

impl Auth {
    /// Moving the public endpoint changes the token audience and requires fresh owner consent.
    pub fn bind_resource(&mut self, resource: &str) -> Result<()> {
        if self.disk.resource == resource {
            return Ok(());
        }
        let mut next = self.disk.clone();
        next.resource = resource.into();
        next.grants.clear();
        next.tokens.clear();
        self.save(next)?;
        self.pending.clear();
        self.codes.clear();
        Ok(())
    }

    /// An unreadable existing store fails closed instead of silently replacing its permissions.
    pub fn open(path: PathBuf) -> Result<Self> {
        let disk = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("read MCP authorization store")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Disk::default(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self {
            disk,
            path,
            pending: BTreeMap::new(),
            codes: BTreeMap::new(),
        })
    }

    /// Replace durable state before publishing a transition to other requests.
    fn save(&mut self, next: Disk) -> Result<()> {
        moon_core::config::write_file_atomic(
            &self.path,
            &serde_json::to_vec(&next)?,
            "MCP authorization",
        )?;
        self.disk = next;
        Ok(())
    }

    /// Remove expired ephemeral state and credentials; retain registered clients with live grants.
    fn prune(&mut self, at: i64) {
        self.pending.retain(|_, p| p.info.expires_ms > at);
        self.codes.retain(|_, c| c.expires_ms > at);
        self.disk.tokens.retain(|_, t| t.expires_ms > at);
        let grants = &self.disk.grants;
        self.disk.clients.retain(|id, c| {
            c.expires_ms > at
                || grants.values().any(|g| g.client_id == *id)
                || self
                    .pending
                    .values()
                    .any(|pending| pending.client_id == *id)
        });
    }

    /// Register a bounded public/confidential OAuth client without granting bot access.
    pub fn register(
        &mut self,
        name: String,
        redirects: Vec<String>,
        method: String,
        at: i64,
    ) -> Result<(Client, Option<String>)> {
        self.prune(at);
        ensure!(self.disk.enabled, "MCP is disabled");
        ensure!(
            self.disk.clients.len() < 128,
            "too many registered clients; retry later"
        );
        ensure!(
            !name.is_empty() && name.len() <= 128 && !name.chars().any(char::is_control),
            "invalid client name"
        );
        ensure!(
            !redirects.is_empty() && redirects.len() <= 8,
            "invalid redirect count"
        );
        for redirect in &redirects {
            validate_redirect(redirect)?;
        }
        ensure!(
            ["none", "client_secret_post", "client_secret_basic"].contains(&method.as_str()),
            "unsupported token authentication method"
        );
        let secret = if method == "none" {
            None
        } else {
            Some(random()?)
        };
        let client = Client {
            id: random()?,
            name,
            redirects,
            method,
            secret_hash: secret.as_deref().map(hash),
            expires_ms: at + CONSENT_MS,
        };
        let mut next = self.disk.clone();
        next.clients.insert(client.id.clone(), client.clone());
        self.save(next)?;
        Ok((client, secret))
    }

    /// Match token-endpoint authentication to the method selected at registration.
    pub fn authenticate_client(&self, id: &str, secret: Option<&str>, method: &str) -> Result<()> {
        let client = self.disk.clients.get(id).context("unknown client")?;
        ensure!(
            client.method == method && client.secret_hash == secret.map(hash),
            "invalid client authentication"
        );
        Ok(())
    }

    /// Start consent only for an exact registered redirect with mandatory S256 PKCE.
    pub fn begin(
        &mut self,
        client_id: String,
        redirect: String,
        challenge: String,
        state: String,
        scopes: Vec<Scope>,
        browser: &str,
        at: i64,
    ) -> Result<(String, String)> {
        self.prune(at);
        ensure!(self.disk.enabled, "MCP is disabled");
        ensure!(
            self.pending.len() < 32,
            "too many pending requests; retry later"
        );
        let client = self
            .disk
            .clients
            .get(&client_id)
            .context("unknown client")?;
        ensure!(
            client.redirects.contains(&redirect),
            "unregistered redirect"
        );
        ensure!(
            challenge.len() == 43
                && URL_SAFE_NO_PAD
                    .decode(&challenge)
                    .is_ok_and(|v| v.len() == 32),
            "invalid S256 challenge"
        );
        ensure!(state.len() <= 2048, "state is too large");
        ensure!(!scopes.is_empty(), "at least one scope is required");
        let id = random()?;
        // Hex avoids ambiguous characters when the owner copies the code into the terminal.
        let mut bytes = [0u8; 5];
        getrandom::getrandom(&mut bytes).map_err(|e| anyhow::anyhow!("OS random: {e}"))?;
        let owner_code = bytes.iter().map(|b| format!("{b:02X}")).collect::<String>();
        self.pending.insert(
            id.clone(),
            Pending {
                info: PendingConsent {
                    request_id: id.clone(),
                    client_name: client.name.clone(),
                    redirect_uri: redirect,
                    scopes,
                    expires_ms: at + CONSENT_MS,
                },
                client_id,
                owner_code: owner_code.clone(),
                browser_hash: hash(browser),
                challenge,
                state,
                approved: None,
            },
        );
        Ok((id, owner_code))
    }

    /// The browser can only collect its own approved code; the redirect is never supplied here.
    pub fn continue_browser(&mut self, id: &str, browser: &str, at: i64) -> Result<Option<String>> {
        self.prune(at);
        ensure!(self.disk.enabled, "MCP is disabled");
        let pending = self
            .pending
            .get(id)
            .context("authorization expired or denied")?;
        ensure!(
            pending.browser_hash == hash(browser),
            "wrong authorization browser"
        );
        let Some((code, grant)) = &pending.approved else {
            return Ok(None);
        };
        ensure!(
            self.disk.grants.contains_key(grant),
            "authorization revoked"
        );
        let mut redirect = Url::parse(&pending.info.redirect_uri)?;
        redirect
            .query_pairs_mut()
            .append_pair("code", code)
            .append_pair("state", &pending.state);
        self.pending.remove(id);
        Ok(Some(redirect.to_string()))
    }

    /// Keep the approval code visible during polling, only to the browser that started this flow.
    pub fn browser_code(&self, id: &str, browser: &str) -> Option<&str> {
        self.pending
            .get(id)
            .filter(|p| p.browser_hash == hash(browser))
            .map(|p| p.owner_code.as_str())
    }

    /// Consume a code only after validating every binding; no failed exchange issues credentials.
    pub fn exchange(
        &mut self,
        client: &str,
        code: &str,
        redirect: &str,
        verifier: &str,
        at: i64,
    ) -> Result<Tokens> {
        self.prune(at);
        ensure!(self.disk.enabled, "MCP is disabled");
        ensure!(
            (43..=128).contains(&verifier.len())
                && verifier
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-._~".contains(&c)),
            "invalid verifier"
        );
        let key = hash(code);
        let saved = self.codes.get(&key).context("invalid or expired code")?;
        let grant = self
            .disk
            .grants
            .get(&saved.grant)
            .context("authorization revoked")?;
        ensure!(
            grant.client_id == client
                && saved.redirect == redirect
                && saved.challenge == hash(verifier),
            "invalid code binding"
        );
        let grant_id = saved.grant.clone();
        let result = self.issue(&grant_id, at)?;
        self.codes.remove(&key);
        Ok(result)
    }

    /// Rotate a refresh credential; reuse revokes its entire grant, including derived access tokens.
    pub fn refresh(&mut self, client: &str, refresh: &str, at: i64) -> Result<Tokens> {
        self.prune(at);
        ensure!(self.disk.enabled, "MCP is disabled");
        ensure!(refresh.len() <= 256, "invalid refresh token");
        let (payload, signature) = refresh.rsplit_once('.').context("invalid refresh token")?;
        let fields: Vec<_> = payload.split('.').collect();
        ensure!(
            fields.len() == 4 && fields[0] == "r1",
            "invalid refresh token"
        );
        let grant_id = fields[1];
        let grant = self
            .disk
            .grants
            .get(grant_id)
            .context("authorization revoked")?;
        ensure!(grant.client_id == client, "wrong refresh client");
        refresh_mac(&grant.refresh_key, payload)?
            .verify_slice(&URL_SAFE_NO_PAD.decode(signature)?)
            .map_err(|_| anyhow::anyhow!("invalid refresh token"))?;
        ensure!(fields[2].parse::<i64>()? > at, "expired refresh token");
        if grant.refresh_hash.as_deref() != Some(&hash(refresh)) {
            self.revoke(grant_id)?;
            bail!("refresh replay; authorization revoked");
        }
        self.issue(grant_id, at)
    }

    /// Keep one signed refresh lineage per grant; persistence precedes handing out either token.
    fn issue(&mut self, grant: &str, at: i64) -> Result<Tokens> {
        let saved = &self
            .disk
            .grants
            .get(grant)
            .context("authorization revoked")?;
        let scope = saved
            .info
            .scopes
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let access_token = random()?;
        let payload = format!("r1.{grant}.{}.{}", at + REFRESH_MS, random()?);
        let signature = refresh_mac(&saved.refresh_key, &payload)?
            .finalize()
            .into_bytes();
        let refresh_token = format!("{payload}.{}", URL_SAFE_NO_PAD.encode(signature));
        let mut next = self.disk.clone();
        // A runaway refresh loop can exhaust only its own short-lived access quota.
        ensure!(
            next.tokens
                .values()
                .filter(|token| token.grant == grant)
                .count()
                < 64,
            "too many refreshes; wait for this client's older access tokens to expire"
        );
        next.grants
            .get_mut(grant)
            .context("authorization revoked")?
            .refresh_hash = Some(hash(&refresh_token));
        next.tokens.insert(
            hash(&access_token),
            Token {
                grant: grant.into(),
                expires_ms: at + ACCESS_MS,
            },
        );
        self.save(next)?;
        Ok(Tokens {
            access_token,
            token_type: "Bearer",
            expires_in: ACCESS_MS / 1000,
            refresh_token,
            scope,
        })
    }

    /// Validate each HTTP call and produce only the identity needed for subsequent rechecks.
    pub fn bearer(&mut self, raw: &str, at: i64) -> Result<Principal> {
        let key = hash(raw);
        let token = self.disk.tokens.get(&key).context("invalid bearer")?;
        ensure!(
            self.disk.enabled && token.expires_ms > at,
            "expired or disabled bearer"
        );
        // Contact time is in-memory until the next durable transition, avoiding a write per read.
        self.disk
            .grants
            .get_mut(&token.grant)
            .context("authorization revoked")?
            .info
            .last_access_ms = Some(at);
        Ok(Principal {
            token_hash: key,
            grant_id: token.grant.clone(),
        })
    }

    /// Recheck identity, permission and exact core scope at the point where work executes.
    pub fn check(
        &self,
        who: &Principal,
        scope: Scope,
        cores: &[u64],
        at: i64,
    ) -> Result<GrantInfo> {
        let token = self
            .disk
            .tokens
            .get(&who.token_hash)
            .context("authorization revoked")?;
        ensure!(
            self.disk.enabled && token.expires_ms > at && token.grant == who.grant_id,
            "authorization expired or revoked"
        );
        let grant = &self
            .disk
            .grants
            .get(&who.grant_id)
            .context("authorization revoked")?
            .info;
        ensure!(
            grant.scopes.contains(&scope),
            "permission denied: {}",
            scope.as_str()
        );
        ensure!(
            cores.iter().all(|c| grant.cores.contains(c)),
            "core is outside the authorization"
        );
        Ok(grant.clone())
    }

    /// Remove both the grant and all derived credentials in one committed transition.
    fn revoke(&mut self, id: &str) -> Result<()> {
        let mut next = self.disk.clone();
        next.grants.remove(id);
        next.tokens.retain(|_, token| token.grant != id);
        self.save(next)
    }

    /// Apply an owner action received through the protected Station control socket.
    pub fn admin(
        &mut self,
        request: AdminRequest,
        known_cores: &[u64],
        at: i64,
    ) -> Result<AdminState> {
        self.prune(at);
        match request {
            AdminRequest::Status => {}
            AdminRequest::SetEnabled { enabled } => {
                let mut next = self.disk.clone();
                next.enabled = enabled;
                if !enabled {
                    next.grants.clear();
                    next.tokens.clear();
                }
                self.save(next)?;
                if !enabled {
                    self.pending.clear();
                    self.codes.clear();
                }
            }
            AdminRequest::Deny { request_id } => {
                self.pending.remove(&request_id);
            }
            AdminRequest::Revoke { grant_id } => self.revoke(&grant_id)?,
            AdminRequest::Approve {
                code,
                mut cores,
                mut scopes,
            } => {
                ensure!(self.disk.enabled, "MCP is disabled");
                ensure!(
                    self.disk.grants.len() < 32,
                    "too many authorizations; revoke an unused client"
                );
                cores.sort_unstable();
                cores.dedup();
                scopes.sort_unstable();
                scopes.dedup();
                ensure!(
                    !cores.is_empty() && cores.iter().all(|c| known_cores.contains(c)),
                    "select configured cores"
                );
                ensure!(scopes.contains(&Scope::Read), "read permission is required");
                let normalized = code.trim().to_ascii_uppercase();
                let (id, pending) = self
                    .pending
                    .iter()
                    .find(|(_, p)| p.owner_code == normalized && p.approved.is_none())
                    .context("unknown or already used consent code")?;
                ensure!(
                    scopes.iter().all(|s| pending.info.scopes.contains(s)),
                    "grant exceeds requested scopes"
                );
                let id = id.clone();
                let grant_id = random()?;
                let raw_code = random()?;
                let grant = Grant {
                    client_id: pending.client_id.clone(),
                    refresh_key: random()?,
                    refresh_hash: None,
                    info: GrantInfo {
                        id: grant_id.clone(),
                        client_name: pending.info.client_name.clone(),
                        cores,
                        scopes,
                        created_ms: at,
                        last_access_ms: None,
                    },
                };
                let authorization = Code {
                    grant: grant_id.clone(),
                    redirect: pending.info.redirect_uri.clone(),
                    challenge: pending.challenge.clone(),
                    expires_ms: at + 90_000,
                };
                let mut next = self.disk.clone();
                next.grants.insert(grant_id.clone(), grant);
                self.save(next)?;
                self.codes.insert(hash(&raw_code), authorization);
                self.pending
                    .get_mut(&id)
                    .context("missing consent")?
                    .approved = Some((raw_code, grant_id));
            }
        }
        Ok(self.status())
    }

    /// Produce owner-visible state without credentials or approval codes.
    pub fn status(&self) -> AdminState {
        AdminState {
            enabled: self.disk.enabled,
            pending: self
                .pending
                .values()
                .filter(|p| p.approved.is_none())
                .map(|p| p.info.clone())
                .collect(),
            grants: self.disk.grants.values().map(|g| g.info.clone()).collect(),
            audit: self.disk.audit.iter().rev().cloned().collect(),
            ..AdminState::default()
        }
    }

    /// Record command admission/result without leaking financial data into the owner audit view.
    pub fn audit(&mut self, entry: AuditEntry) -> Result<()> {
        let mut next = self.disk.clone();
        next.audit.push_back(entry);
        while next.audit.len() > 256 {
            next.audit.pop_front();
        }
        self.save(next)
    }
}

/// Redirects must be exact HTTPS URLs, with the OAuth native-app loopback exception only.
fn validate_redirect(value: &str) -> Result<()> {
    ensure!(value.len() <= 2048, "redirect too long");
    let url = Url::parse(value)?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"));
    ensure!(
        url.scheme() == "https" || (url.scheme() == "http" && loopback),
        "redirect requires HTTPS"
    );
    ensure!(
        url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none(),
        "invalid redirect"
    );
    Ok(())
}

#[cfg(test)]
mod tests;
