//! Independent Station MCP service: public protocol workers, private consent and owner-thread tools.

mod auth;
mod bridge;
mod capture;
mod http;
mod knowledge;
mod tools;

use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use moon_core::config::AppConfig;
use moon_core::session::SessionManager;
use moon_core::station_api::mcp::{AdminRequest, AdminState};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::{Semaphore, oneshot};
use tokio_util::sync::CancellationToken;

/// Public origin installed by the SSH helper; binding remains hardcoded to loopback.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    pub public_url: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

/// Dedicated loopback port, independent of the Telegram Mini App.
fn default_port() -> u16 {
    19791
}

impl Config {
    /// Only a canonical HTTPS origin is accepted; paths, credentials and nonstandard ports are not.
    fn origin(&self) -> Result<String> {
        let url = url::Url::parse(&self.public_url)?;
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
        Ok(url.origin().ascii_serialization())
    }
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
    origin: String,
    auth: Arc<Mutex<auth::Auth>>,
    calls: mpsc::SyncSender<Call>,
    owner: std::thread::Thread,
    knowledge: knowledge::SharedKnowledge,
    http_slots: Arc<Semaphore>,
    report_slots: Arc<Semaphore>,
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
    stop: CancellationToken,
    thread: Option<std::thread::JoinHandle<()>>,
    _knowledge_worker: knowledge::Worker,
}

impl Mcp {
    /// Bind before returning, so setup cannot report a URL for an HTTP listener that failed.
    pub fn start(config: &Config) -> Result<Self> {
        let origin = config.origin()?;
        let mut auth = auth::Auth::open(moon_core::config::paths::station_mcp_auth())?;
        auth.bind_resource(&format!("{origin}/mcp"))?;
        let auth = Arc::new(Mutex::new(auth));
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, config.port))
            .context("bind MCP loopback listener")?;
        listener.set_nonblocking(true)?;
        let (worker, knowledge) = knowledge::Worker::start()?;
        let (send, calls) = mpsc::sync_channel(64);
        let shared = Arc::new(Shared {
            origin,
            auth,
            calls: send,
            owner: std::thread::current(),
            knowledge,
            http_slots: Arc::new(Semaphore::new(32)),
            report_slots: Arc::new(Semaphore::new(2)),
        });
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
                    let result = async {
                        let listener = tokio::net::TcpListener::from_std(listener)?;
                        axum::serve(listener, http::router(serving, stopping.clone()))
                            .with_graceful_shutdown(stopping.cancelled_owned())
                            .await
                    }
                    .await;
                    if let Err(e) = result {
                        log::error!("MCP listener stopped: {e}");
                    }
                });
                runtime.shutdown_timeout(Duration::from_secs(2));
            })?;
        Ok(Self {
            shared,
            calls,
            bridge: bridge::Owner::default(),
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
        state.endpoint = Some(self.shared.resource());
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
