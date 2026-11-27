//! Independent Station MCP service: public protocol workers, private consent and owner-thread tools.
//!
//! Two ways in, either or both: the public HTTPS listener behind Caddy (`public_url`, Claude and
//! other clients that open a URL), and the station's own OpenAI Secure MCP Tunnel client
//! (`tunnel_id`, ChatGPT), which needs no inbound port at all.

mod auth;
mod bridge;
mod capture;
mod edit_view;
mod http;
mod knowledge;
mod site;
mod tools;
mod tunnel;

use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use moon_core::config::AppConfig;
use moon_core::session::SessionManager;
use moon_core::station_api::mcp::{
    AdminRequest, AdminState, TUNNEL_KEY_MISSING, TunnelState, valid_tunnel_id,
};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::{Semaphore, oneshot};
use tokio_util::sync::CancellationToken;

/// `[mcp]` of `station.toml`: the public origin installed by the SSH helper, the OpenAI tunnel
/// the terminal set up, or both. Binding remains hardcoded to loopback.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    #[serde(default)]
    pub public_url: Option<String>,
    #[serde(default = "default_port")]
    pub port: u16,
    /// The OpenAI tunnel ChatGPT reaches the station through; its key is the credential
    /// [`crate::cores::TUNNEL_KEY_CREDENTIAL`].
    #[serde(default)]
    pub tunnel_id: Option<String>,
}

/// Dedicated loopback port, independent of the Telegram Mini App.
fn default_port() -> u16 {
    19791
}

impl Config {
    /// The public origin, when there is one. Only a canonical HTTPS origin is accepted; paths,
    /// credentials and nonstandard ports are not.
    fn origin(&self) -> Result<Option<String>> {
        let Some(public_url) = &self.public_url else {
            return Ok(None);
        };
        let url = url::Url::parse(public_url)?;
        ensure!(
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.port().is_none()
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none()
                && url.username().is_empty()
                && url.password().is_none(),
            "MCP public_url must be an HTTPS origin"
        );
        ensure!(self.port >= 1024, "MCP loopback port must be unprivileged");
        Ok(Some(url.origin().ascii_serialization()))
    }

    /// The tunnel ID, when there is one, in the form OpenAI issues: `tunnel_` and 32 lowercase
    /// letters or digits, optionally after a four-character namespace and `_`. It goes into a
    /// URL path, so nothing else is let through.
    fn tunnel(&self) -> Result<Option<String>> {
        let Some(id) = &self.tunnel_id else {
            return Ok(None);
        };
        ensure!(
            valid_tunnel_id(id),
            "MCP tunnel_id is not an OpenAI tunnel ID"
        );
        Ok(Some(id.clone()))
    }
}

/// The mark the tunnel puts on a request from a ChatGPT account no grant names yet. It may list
/// the tools; any call is answered with the code that pairs it, and nothing runs.
#[derive(Clone)]
pub(super) struct TunnelGuest {
    /// The account's `X-Openai-Subject`; `None` when the request carried none.
    pub subject: Option<String>,
}

/// One tool invocation, bounded in time and queue depth before the Station owner sees it.
struct Call {
    who: auth::Principal,
    name: String,
    arguments: Value,
    expires: Instant,
    reply: oneshot::Sender<std::result::Result<Value, String>>,
}

/// Shared protocol state; the network worker never owns or locks SessionManager.
struct Shared {
    /// The public HTTPS origin; empty on a station reached only through the tunnel, which starts
    /// no HTTPS listener and so never reads it.
    origin: String,
    auth: Arc<Mutex<auth::Auth>>,
    calls: mpsc::SyncSender<Call>,
    owner: std::thread::Thread,
    knowledge: knowledge::SharedKnowledge,
    http_slots: Arc<Semaphore>,
    report_slots: Arc<Semaphore>,
    /// The MoonTerminal site's search API, asked off the owner thread.
    site: Arc<site::Client>,
    /// Site requests in flight at once: each holds a blocking worker for up to two timeouts.
    site_slots: Arc<Semaphore>,
}

impl Shared {
    /// Exact token audience and URL the user copies into their AI client.
    fn resource(&self) -> String {
        format!("{}/mcp", self.origin)
    }
}

/// Owns service lifetime and the only mutable tool state; Telegram is not involved.
pub(crate) struct Mcp {
    shared: Arc<Shared>,
    calls: mpsc::Receiver<Call>,
    bridge: bridge::Owner,
    /// The tunnel as the owner's page shows it; `None` without `tunnel_id`.
    tunnel: Option<Arc<Mutex<TunnelState>>>,
    stop: CancellationToken,
    thread: Option<std::thread::JoinHandle<()>>,
    _knowledge_worker: knowledge::Worker,
}

impl Mcp {
    /// Bind before returning, so setup cannot report a URL for an HTTP listener that failed. A
    /// tunnel key that cannot be read leaves the tunnel off and says so on the owner's page; it
    /// does not take the HTTPS listener down with it.
    ///
    /// Args:
    ///     config: `[mcp]`.
    ///     tunnel_key: The credential holding the tunnel key, read only when `config` names a
    ///         tunnel.
    pub fn start(
        config: &Config,
        tunnel_key: impl FnOnce() -> Result<moon_core::config::Secret>,
    ) -> Result<Self> {
        let origin = config.origin()?;
        let tunnel_id = config.tunnel()?;
        ensure!(
            origin.is_some() || tunnel_id.is_some(),
            "[mcp] needs public_url, tunnel_id or both"
        );
        let tunnel = tunnel_id.map(|id| {
            let key = tunnel_key()
                .inspect_err(|e| log::error!("mcp tunnel: no key, not polling: {e:#}"))
                .ok();
            (id, key)
        });
        let mut auth = auth::Auth::open(moon_core::config::paths::station_mcp_auth())?;
        // Only the public endpoint is a token audience, and moving it keeps tunnel grants
        // (`Auth::bind_resource`): neither way in revokes the other's clients.
        if let Some(origin) = &origin {
            auth.bind_resource(&format!("{origin}/mcp"))?;
        }
        let auth = Arc::new(Mutex::new(auth));
        let listener = origin
            .as_ref()
            .map(|_| -> Result<std::net::TcpListener> {
                let listener =
                    std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, config.port))
                        .context("bind MCP loopback listener")?;
                listener.set_nonblocking(true)?;
                Ok(listener)
            })
            .transpose()?;
        let (worker, knowledge) = knowledge::Worker::start()?;
        let (send, calls) = mpsc::sync_channel(64);
        let shared = Arc::new(Shared {
            origin: origin.unwrap_or_default(),
            auth,
            calls: send,
            owner: std::thread::current(),
            knowledge,
            http_slots: Arc::new(Semaphore::new(32)),
            report_slots: Arc::new(Semaphore::new(2)),
            site: Arc::new(site::Client::new()),
            site_slots: Arc::new(Semaphore::new(4)),
        });
        let (tunnel_state, polling) = match tunnel {
            Some((id, key)) => {
                let state = Arc::new(Mutex::new(TunnelState {
                    tunnel_id: id.clone(),
                    problem: key.is_none().then(|| TUNNEL_KEY_MISSING.to_owned()),
                    ..TunnelState::default()
                }));
                let polling = key.map(|key| (shared.clone(), id, key, state.clone()));
                (Some(state), polling)
            }
            None => (None, None),
        };
        let serving = shared.clone();
        let stop = CancellationToken::new();
        let stopping = stop.clone();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("mcp-http")
            .build()?;
        let thread = std::thread::Builder::new()
            .name("mcp-service".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let listening = async {
                        let Some(listener) = listener else {
                            return;
                        };
                        let result = async {
                            let listener = tokio::net::TcpListener::from_std(listener)?;
                            axum::serve(listener, http::router(serving, stopping.clone()))
                                .with_graceful_shutdown(stopping.clone().cancelled_owned())
                                .await
                        }
                        .await;
                        if let Err(e) = result {
                            log::error!("MCP listener stopped: {e}");
                        }
                    };
                    let tunneling = async {
                        if let Some((shared, id, key, state)) = polling {
                            tunnel::run(shared, id, key, state, stopping.clone()).await;
                        }
                    };
                    tokio::join!(listening, tunneling);
                });
                runtime.shutdown_timeout(Duration::from_secs(2));
            })?;
        Ok(Self {
            shared,
            calls,
            bridge: bridge::Owner::default(),
            tunnel: tunnel_state,
            stop,
            thread: Some(thread),
            _knowledge_worker: worker,
        })
    }

    /// Run only on the existing station owner: ingest diagnostic events and drain a bounded batch.
    pub fn tick(&mut self, session: &SessionManager, cfg: &AppConfig) {
        self.bridge.tick(session, cfg, &self.shared);
        for _ in 0..16 {
            let Ok(call) = self.calls.try_recv() else {
                break;
            };
            if call.reply.is_closed() || Instant::now() >= call.expires {
                continue;
            }
            let result = self
                .bridge
                .call(
                    session,
                    cfg,
                    &self.shared,
                    &call.who,
                    &call.name,
                    call.arguments,
                )
                .map_err(|e| e.to_string());
            let _ = call.reply.send(result);
        }
    }

    /// Owner controls arrive through the pre-existing authenticated SSH/Unix-socket path.
    pub fn admin(
        &self,
        request: AdminRequest,
        cores: Vec<moon_core::station_api::mcp::CoreInfo>,
    ) -> Result<AdminState> {
        let ids: Vec<_> = cores.iter().map(|core| core.uid).collect();
        let mut state = self
            .shared
            .auth
            .lock()
            .map_err(|_| anyhow::anyhow!("MCP authorization unavailable"))?
            .admin(request, &ids, auth::now())?;
        state.cores = cores;
        state.configured = true;
        state.endpoint = (!self.shared.origin.is_empty()).then(|| self.shared.resource());
        state.tunnel = self
            .tunnel
            .as_ref()
            .and_then(|tunnel| tunnel.lock().ok().map(|state| Box::new(state.clone())));
        if let Ok(knowledge) = self.shared.knowledge.read() {
            state.knowledge_revision =
                (!knowledge.revision.is_empty()).then(|| knowledge.revision.clone());
            state.knowledge_checked_ms = (knowledge.checked_ms > 0).then_some(knowledge.checked_ms);
        }
        Ok(state)
    }
}

impl Drop for Mcp {
    /// Cancel transport before joining; a lost AI connection never prolongs a log subscription.
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests;
