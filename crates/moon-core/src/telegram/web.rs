//! Loopback-only Mini App HTTP server.
//!
//! All `touche` types stay inside this module. The listener binds only `127.0.0.1` with an
//! OS-selected port. Static assets are embedded; the authenticated `POST /api/session` route
//! verifies Telegram `initData` and then authorizes the signed identity against paired chat ids.
//!
//! Every `/api/*` route, read or write, re-verifies initData HMAC, the paired chat, and
//! `auth_date` within `INIT_DATA_MAX_AGE_SECS` (3600 s) before anything reaches the consumer.
//! Routes: `POST /api/session`, `POST /api/report`, `POST /api/cores`, `POST /api/balances`,
//! `POST /api/orders`, `POST /api/order/cancel`, `POST /api/panic`, `POST /api/core/switch`,
//! `POST /api/cores/switch`, `POST /api/core/cancel_all`, `POST /api/trades`,
//! `POST /api/strategies`, `POST /api/strategy/toggle`, and `POST /api/core/reconnect`.

use std::io::{self, Read};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};
use touche::body::HttpBody;
use touche::{Body, HeaderMap, Method, Request, Response, Server, StatusCode};

use super::init_data::{
    InitDataError, SignedInitData, authorize_paired_identity, verify_init_data,
};
use crate::config::Secret;
use crate::util::time::now_unix_secs;

pub mod dto;

use dto::{
    BalancesDto, CommandResultDto, CoreSwitchDto, CoresDto, OrdersDto, ReportDto, ReportPeriodDto,
    ScopeResultDto, StrategiesDto, TradesDto,
};

const INDEX_HTML: &str = include_str!("web/index.html");
const APP_CSS: &str = include_str!("web/app.css");
const APP_JS: &str = include_str!("web/app.js");

/// Maximum JSON body accepted on Mini App API routes.
const MAX_BODY_BYTES: u64 = 16 * 1024;
/// Maximum number of request headers.
const MAX_HEADER_COUNT: usize = 32;
/// Maximum combined header name+value bytes.
const MAX_HEADER_BYTES: usize = 8 * 1024;
/// How long an idle connection may wait for request bytes.
const READ_TIMEOUT: Duration = Duration::from_secs(10);
/// How long an API handler waits for its consumer reply.
const API_REPLY_TIMEOUT: Duration = Duration::from_secs(5);
/// Header carrying raw Telegram `initData`.
const INIT_DATA_HEADER: &str = "x-telegram-init-data";

/// Owner of the loopback Mini App listener thread.
///
/// Stop and Drop wake the accept loop, join the listener thread, and leave no bound socket.
pub struct MiniAppServer {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    events_tx: SyncSender<MiniAppApiRequest>,
    events_rx: Option<mpsc::Receiver<MiniAppApiRequest>>,
}

/// Inputs required to bind the Mini App listener. The process is not started until
/// [`MiniAppServer::bind`].
pub struct MiniAppServerConfig {
    /// Bot token used only for HMAC verification; never logged or returned.
    pub token: Secret,
    /// Chat ids allowed to authenticate Mini App API requests.
    pub authorized_chat_ids: Vec<i64>,
}

/// Authenticated Mini App API event emitted after HMAC and pairing checks succeed.
pub enum MiniAppApiRequest {
    /// Data-free session check. The body is discarded after bounds checks.
    Session {
        /// Signed identity that passed pairing.
        identity: SignedInitData,
        /// Paired chat id.
        chat_id: i64,
        /// One-shot reply for the HTTP handler.
        reply: SyncSender<Result<(), MiniAppApiError>>,
    },
    /// Report for one named period.
    Report {
        /// Signed identity that passed pairing.
        identity: SignedInitData,
        /// Paired chat id.
        chat_id: i64,
        /// Window requested in the JSON body.
        period: ReportPeriodDto,
        /// One-shot typed reply for the HTTP handler.
        reply: SyncSender<Result<ReportDto, MiniAppApiError>>,
    },
    /// Live status of every visible core.
    Cores {
        /// Signed identity that passed pairing.
        identity: SignedInitData,
        /// Paired chat id.
        chat_id: i64,
        /// One-shot typed reply for the HTTP handler.
        reply: SyncSender<Result<CoresDto, MiniAppApiError>>,
    },
    /// Balances for every visible core.
    Balances {
        /// Signed identity that passed pairing.
        identity: SignedInitData,
        /// Paired chat id.
        chat_id: i64,
        /// One-shot typed reply for the HTTP handler.
        reply: SyncSender<Result<BalancesDto, MiniAppApiError>>,
    },
    /// Open orders for every visible core.
    Orders {
        /// Signed identity that passed pairing.
        identity: SignedInitData,
        /// Paired chat id.
        chat_id: i64,
        /// One-shot typed reply for the HTTP handler.
        reply: SyncSender<Result<OrdersDto, MiniAppApiError>>,
    },
    /// Cancel one open order. Nothing is sent when that uid is not on the core.
    CancelOrder {
        /// Signed identity that passed pairing.
        identity: SignedInitData,
        /// Paired chat id.
        chat_id: i64,
        /// Core that owns the order.
        core: u64,
        /// Order uid from the same list the orders route returns.
        uid: u64,
        /// One-shot typed reply for the HTTP handler.
        reply: SyncSender<Result<CommandResultDto, MiniAppApiError>>,
    },
    /// Arm or disarm Panic Sell for one market.
    PanicSell {
        /// Signed identity that passed pairing.
        identity: SignedInitData,
        /// Paired chat id.
        chat_id: i64,
        /// Core that owns the market.
        core: u64,
        /// Market key. Longer than 64 bytes is rejected before this event is built.
        market: String,
        /// Armed state the page asked for.
        on: bool,
        /// One-shot typed reply for the HTTP handler.
        reply: SyncSender<Result<CommandResultDto, MiniAppApiError>>,
    },
    /// Flip one core's trading or auto-detect switch. Nothing is sent for an unknown core.
    CoreSwitch {
        /// Signed identity that passed pairing.
        identity: SignedInitData,
        /// Paired chat id.
        chat_id: i64,
        /// Target core.
        core: u64,
        /// Which switch to flip.
        switch: CoreSwitchDto,
        /// State the page asked for.
        on: bool,
        /// One-shot typed reply for the HTTP handler.
        reply: SyncSender<Result<CommandResultDto, MiniAppApiError>>,
    },
    /// Flip one switch on several cores. Empty or over-cap lists are rejected before this
    /// event is built.
    CoresSwitch {
        /// Signed identity that passed pairing.
        identity: SignedInitData,
        /// Paired chat id.
        chat_id: i64,
        /// Target cores; unknown ids are never sent.
        cores: Vec<u64>,
        /// Which switch to flip.
        switch: CoreSwitchDto,
        /// State the page asked for.
        on: bool,
        /// One-shot typed reply for the HTTP handler.
        reply: SyncSender<Result<ScopeResultDto, MiniAppApiError>>,
    },
    /// Cancel every open order of one core. Nothing is sent for an unknown core.
    CancelAllOrders {
        /// Signed identity that passed pairing.
        identity: SignedInitData,
        /// Paired chat id.
        chat_id: i64,
        /// Target core.
        core: u64,
        /// One-shot typed reply for the HTTP handler.
        reply: SyncSender<Result<CommandResultDto, MiniAppApiError>>,
    },
    /// Latest closed trades for every visible core.
    Trades {
        /// Signed identity that passed pairing.
        identity: SignedInitData,
        /// Paired chat id.
        chat_id: i64,
        /// One-shot typed reply for the HTTP handler.
        reply: SyncSender<Result<TradesDto, MiniAppApiError>>,
    },
    /// Strategies for every visible core.
    Strategies {
        /// Signed identity that passed pairing.
        identity: SignedInitData,
        /// Paired chat id.
        chat_id: i64,
        /// One-shot typed reply for the HTTP handler.
        reply: SyncSender<Result<StrategiesDto, MiniAppApiError>>,
    },
    /// Turn one strategy on or off. Nothing is sent for an unknown core or strategy.
    StrategyToggle {
        /// Signed identity that passed pairing.
        identity: SignedInitData,
        /// Paired chat id.
        chat_id: i64,
        /// Core that owns the strategy.
        core: u64,
        /// Strategy id from the same list the strategies route returns.
        id: u64,
        /// State the page asked for.
        on: bool,
        /// One-shot typed reply for the HTTP handler.
        reply: SyncSender<Result<CommandResultDto, MiniAppApiError>>,
    },
    /// Reconnect one core. Nothing is sent for an unknown core.
    CoreReconnect {
        /// Signed identity that passed pairing.
        identity: SignedInitData,
        /// Paired chat id.
        chat_id: i64,
        /// Target core.
        core: u64,
        /// One-shot typed reply for the HTTP handler.
        reply: SyncSender<Result<CommandResultDto, MiniAppApiError>>,
    },
}

/// Handler-level API failure that does not carry secrets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MiniAppApiError {
    /// Consumer refused the request.
    Rejected,
    /// The consumer could not finish the read.
    ReadFailed,
    /// The consumer is already busy with another read.
    Busy,
    /// The signed chat is not allowed to see this data.
    Forbidden,
    /// The named target does not exist.
    NotFound,
}

impl MiniAppServer {
    /// Bind `127.0.0.1:0`, spawn the listener thread, and return a stopped-on-drop owner.
    ///
    /// Args:
    ///     config: Token and paired chat ids. The token is copied only into the HMAC verifier.
    ///
    /// Returns:
    ///     A running loopback server whose [`MiniAppServer::addr`] is the OS-assigned port.
    ///
    /// Errors:
    ///     Returns I/O errors from bind or thread spawn. The listener is not left running.
    pub fn bind(config: MiniAppServerConfig) -> io::Result<Self> {
        Self::bind_localized(config, std::collections::BTreeMap::new())
    }

    /// Bind with localized UI strings supplied by the application, including initial error states.
    pub fn bind_localized(
        config: MiniAppServerConfig,
        labels: std::collections::BTreeMap<String, String>,
    ) -> io::Result<Self> {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let listener = TcpListener::bind(addr)?;
        let bound = listener.local_addr()?;
        if !bound.ip().is_loopback() {
            return Err(io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "mini app listener refused a non-loopback bind",
            ));
        }
        listener.set_nonblocking(false)?;
        let stop = Arc::new(AtomicBool::new(false));
        let (events_tx, events_rx) = mpsc::sync_channel(64);
        let app = App {
            labels: Arc::new(labels),
            token: Arc::new(config.token),
            authorized_chat_ids: Arc::new(config.authorized_chat_ids),
            events_tx: events_tx.clone(),
            stop: Arc::clone(&stop),
        };
        let incoming = StoppableAccept {
            listener,
            stop: Arc::clone(&stop),
        };
        let join = thread::Builder::new()
            .name("telegram-miniapp".into())
            .spawn(move || {
                let _ = Server::builder()
                    .read_timeout(READ_TIMEOUT)
                    .from_connections(incoming)
                    .serve_single_thread(app);
            })?;
        Ok(Self {
            addr: bound,
            stop,
            join: Some(join),
            events_tx,
            events_rx: Some(events_rx),
        })
    }

    /// OS-assigned loopback address of the listener.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Loopback TCP port of the listener.
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Bounded sender of authenticated API events.
    pub fn events(&self) -> &SyncSender<MiniAppApiRequest> {
        &self.events_tx
    }

    /// Take the event receiver. The first caller owns draining authenticated API events.
    pub fn take_events(&mut self) -> Option<mpsc::Receiver<MiniAppApiRequest>> {
        self.events_rx.take()
    }

    /// Wake the accept loop and join the listener thread. Safe to call twice.
    pub fn stop(&mut self) {
        if self.stop.swap(true, Ordering::SeqCst) {
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
            return;
        }
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(200));
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for MiniAppServer {
    /// Stop and join the listener thread before releasing the server owner.
    fn drop(&mut self) {
        self.stop();
    }
}

/// Accept loop that ends when [`MiniAppServer::stop`] sets the flag and connects once.
struct StoppableAccept {
    listener: TcpListener,
    stop: Arc<AtomicBool>,
}

impl Iterator for StoppableAccept {
    type Item = TcpStream;

    /// Bound stalled writes as well as reads on the connection owned by the listener thread.
    fn next(&mut self) -> Option<Self::Item> {
        if self.stop.load(Ordering::SeqCst) {
            return None;
        }
        match self.listener.accept() {
            Ok((stream, _)) => {
                if self.stop.load(Ordering::SeqCst) {
                    None
                } else {
                    stream.set_write_timeout(Some(READ_TIMEOUT)).ok()?;
                    Some(stream)
                }
            }
            Err(_) => None,
        }
    }
}

/// Body of `POST /api/report`. Unknown fields are rejected.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportPeriodBody {
    period: ReportPeriodDto,
}

/// Body of `POST /api/order/cancel`. Unknown fields are rejected.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CancelOrderBody {
    core: u64,
    uid: u64,
}

/// Body of `POST /api/core/switch`. Unknown fields are rejected.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CoreSwitchBody {
    core: u64,
    switch: CoreSwitchDto,
    on: bool,
}

/// Most cores accepted by `POST /api/cores/switch`.
const MAX_SCOPE_CORES: usize = 256;

/// Body of `POST /api/cores/switch`. Unknown fields are rejected.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CoresSwitchBody {
    cores: Vec<u64>,
    switch: CoreSwitchDto,
    on: bool,
}

/// Body of `POST /api/core/cancel_all`. Unknown fields are rejected.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CancelAllBody {
    core: u64,
}

/// Body of `POST /api/strategy/toggle`. Unknown fields are rejected.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StrategyToggleBody {
    core: u64,
    id: u64,
    on: bool,
}

/// Body of `POST /api/core/reconnect`. Unknown fields are rejected.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CoreReconnectBody {
    core: u64,
}

/// Longest `market` accepted by `POST /api/panic`, in bytes.
const MAX_MARKET_BYTES: usize = 64;

/// Body of `POST /api/panic`. Unknown fields are rejected.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PanicSellBody {
    core: u64,
    market: String,
    on: bool,
}

/// Per-connection `touche` service. Clone is cheap: only Arcs and a channel sender.
#[derive(Clone)]
struct App {
    labels: Arc<std::collections::BTreeMap<String, String>>,
    token: Arc<Secret>,
    authorized_chat_ids: Arc<Vec<i64>>,
    events_tx: SyncSender<MiniAppApiRequest>,
    stop: Arc<AtomicBool>,
}

impl App {
    /// Dispatch one HTTP request onto the fixed Mini App route table.
    // Route closures return `Result<_, Response<Body>>`. Boxing that error would
    // change the shared helper the same way `authenticate` refuses to.
    #[allow(clippy::result_large_err)]
    fn handle(&self, request: Request<Body>) -> Response<Body> {
        if self.stop.load(Ordering::SeqCst) {
            return status_response(StatusCode::SERVICE_UNAVAILABLE, "stopped");
        }
        if let Err(status) = check_peer_and_headers(request.headers(), request.uri().host()) {
            return status_response(status, "rejected");
        }
        match (request.method(), request.uri().path()) {
            (&Method::GET, "/") => {
                let labels = serde_json::to_string(&*self.labels)
                    .unwrap_or_default()
                    .replace('<', "\\u003c");
                static_response(
                    "text/html; charset=utf-8",
                    &INDEX_HTML.replace("__TELEGRAM_LABELS__", &labels),
                )
            }
            (&Method::GET, "/app.css") => static_response("text/css; charset=utf-8", APP_CSS),
            (&Method::GET, "/app.js") => {
                static_response("application/javascript; charset=utf-8", APP_JS)
            }
            (&Method::POST, "/api/session") => self.handle_session(request),
            (&Method::POST, "/api/report") => {
                self.handle_api(request, |identity, chat_id, body, reply| {
                    let parsed = serde_json::from_value::<ReportPeriodBody>(body)
                        .map_err(|_| status_response(StatusCode::BAD_REQUEST, "json"))?;
                    Ok(MiniAppApiRequest::Report {
                        identity,
                        chat_id,
                        period: parsed.period,
                        reply,
                    })
                })
            }
            (&Method::POST, "/api/cores") => {
                self.handle_api(request, |identity, chat_id, _body, reply| {
                    Ok(MiniAppApiRequest::Cores {
                        identity,
                        chat_id,
                        reply,
                    })
                })
            }
            (&Method::POST, "/api/balances") => {
                self.handle_api(request, |identity, chat_id, _body, reply| {
                    Ok(MiniAppApiRequest::Balances {
                        identity,
                        chat_id,
                        reply,
                    })
                })
            }
            (&Method::POST, "/api/orders") => {
                self.handle_api(request, |identity, chat_id, _body, reply| {
                    Ok(MiniAppApiRequest::Orders {
                        identity,
                        chat_id,
                        reply,
                    })
                })
            }
            (&Method::POST, "/api/order/cancel") => {
                self.handle_api(request, |identity, chat_id, body, reply| {
                    let parsed = serde_json::from_value::<CancelOrderBody>(body)
                        .map_err(|_| status_response(StatusCode::BAD_REQUEST, "json"))?;
                    Ok(MiniAppApiRequest::CancelOrder {
                        identity,
                        chat_id,
                        core: parsed.core,
                        uid: parsed.uid,
                        reply,
                    })
                })
            }
            (&Method::POST, "/api/panic") => {
                self.handle_api(request, |identity, chat_id, body, reply| {
                    let parsed = serde_json::from_value::<PanicSellBody>(body)
                        .map_err(|_| status_response(StatusCode::BAD_REQUEST, "json"))?;
                    if parsed.market.len() > MAX_MARKET_BYTES {
                        return Err(status_response(StatusCode::BAD_REQUEST, "json"));
                    }
                    Ok(MiniAppApiRequest::PanicSell {
                        identity,
                        chat_id,
                        core: parsed.core,
                        market: parsed.market,
                        on: parsed.on,
                        reply,
                    })
                })
            }
            (&Method::POST, "/api/core/switch") => {
                self.handle_api(request, |identity, chat_id, body, reply| {
                    let parsed = serde_json::from_value::<CoreSwitchBody>(body)
                        .map_err(|_| status_response(StatusCode::BAD_REQUEST, "json"))?;
                    Ok(MiniAppApiRequest::CoreSwitch {
                        identity,
                        chat_id,
                        core: parsed.core,
                        switch: parsed.switch,
                        on: parsed.on,
                        reply,
                    })
                })
            }
            (&Method::POST, "/api/cores/switch") => {
                self.handle_api(request, |identity, chat_id, body, reply| {
                    let parsed = serde_json::from_value::<CoresSwitchBody>(body)
                        .map_err(|_| status_response(StatusCode::BAD_REQUEST, "json"))?;
                    if parsed.cores.is_empty() || parsed.cores.len() > MAX_SCOPE_CORES {
                        return Err(status_response(StatusCode::BAD_REQUEST, "json"));
                    }
                    Ok(MiniAppApiRequest::CoresSwitch {
                        identity,
                        chat_id,
                        cores: parsed.cores,
                        switch: parsed.switch,
                        on: parsed.on,
                        reply,
                    })
                })
            }
            (&Method::POST, "/api/core/cancel_all") => {
                self.handle_api(request, |identity, chat_id, body, reply| {
                    let parsed = serde_json::from_value::<CancelAllBody>(body)
                        .map_err(|_| status_response(StatusCode::BAD_REQUEST, "json"))?;
                    Ok(MiniAppApiRequest::CancelAllOrders {
                        identity,
                        chat_id,
                        core: parsed.core,
                        reply,
                    })
                })
            }
            (&Method::POST, "/api/trades") => {
                self.handle_api(request, |identity, chat_id, _body, reply| {
                    Ok(MiniAppApiRequest::Trades {
                        identity,
                        chat_id,
                        reply,
                    })
                })
            }
            (&Method::POST, "/api/strategies") => {
                self.handle_api(request, |identity, chat_id, _body, reply| {
                    Ok(MiniAppApiRequest::Strategies {
                        identity,
                        chat_id,
                        reply,
                    })
                })
            }
            (&Method::POST, "/api/strategy/toggle") => {
                self.handle_api(request, |identity, chat_id, body, reply| {
                    let parsed = serde_json::from_value::<StrategyToggleBody>(body)
                        .map_err(|_| status_response(StatusCode::BAD_REQUEST, "json"))?;
                    Ok(MiniAppApiRequest::StrategyToggle {
                        identity,
                        chat_id,
                        core: parsed.core,
                        id: parsed.id,
                        on: parsed.on,
                        reply,
                    })
                })
            }
            (&Method::POST, "/api/core/reconnect") => {
                self.handle_api(request, |identity, chat_id, body, reply| {
                    let parsed = serde_json::from_value::<CoreReconnectBody>(body)
                        .map_err(|_| status_response(StatusCode::BAD_REQUEST, "json"))?;
                    Ok(MiniAppApiRequest::CoreReconnect {
                        identity,
                        chat_id,
                        core: parsed.core,
                        reply,
                    })
                })
            }
            (&Method::GET, "/api/session")
            | (
                &Method::GET,
                "/api/report"
                | "/api/cores"
                | "/api/balances"
                | "/api/orders"
                | "/api/order/cancel"
                | "/api/panic"
                | "/api/core/switch"
                | "/api/cores/switch"
                | "/api/core/cancel_all"
                | "/api/trades"
                | "/api/strategies"
                | "/api/strategy/toggle"
                | "/api/core/reconnect",
            )
            | (&Method::HEAD, "/")
            | (&Method::OPTIONS, _) => status_response(StatusCode::METHOD_NOT_ALLOWED, "method"),
            _ => {
                if matches!(
                    request.method(),
                    &Method::POST | &Method::PUT | &Method::PATCH | &Method::DELETE
                ) && request.uri().path().starts_with("/api/")
                {
                    status_response(StatusCode::NOT_FOUND, "not_found")
                } else if request.method() != Method::GET {
                    status_response(StatusCode::METHOD_NOT_ALLOWED, "method")
                } else {
                    status_response(StatusCode::NOT_FOUND, "not_found")
                }
            }
        }
    }

    /// Authenticate, authorize, and emit a data-free session check.
    fn handle_session(&self, request: Request<Body>) -> Response<Body> {
        let (identity, chat_id) = match self.authenticate(&request) {
            Ok(parts) => parts,
            Err(response) => return response,
        };
        if let Err(response) = require_json_content_type(request.headers()) {
            return response;
        }
        if let Err(response) = read_limited_json_object(request) {
            return response;
        }
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        match self.events_tx.try_send(MiniAppApiRequest::Session {
            identity,
            chat_id,
            reply: reply_tx,
        }) {
            Ok(()) => {}
            Err(_) => return status_response(StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
        }
        match reply_rx.recv_timeout(API_REPLY_TIMEOUT) {
            Ok(Ok(())) => json_response(StatusCode::OK, json!({"ok": true})),
            Ok(Err(_)) => status_response(StatusCode::FORBIDDEN, "rejected"),
            Err(RecvTimeoutError::Timeout) => {
                status_response(StatusCode::GATEWAY_TIMEOUT, "timeout")
            }
            Err(RecvTimeoutError::Disconnected) => {
                status_response(StatusCode::SERVICE_UNAVAILABLE, "unavailable")
            }
        }
    }

    /// Authenticate one JSON API route and wait for a typed consumer reply.
    ///
    /// Args:
    ///     request: POST whose body is a JSON object and whose header carries initData.
    ///     build: Turns the signed identity and that object into one [`MiniAppApiRequest`].
    ///         An `Err` skips the consumer and becomes the HTTP response.
    ///
    /// Returns:
    ///     `200` and the typed JSON on success. `Rejected` and `Forbidden` are both
    ///     `403 rejected`. `ReadFailed` is `500 read_failed`, `Busy` is `503 busy`,
    ///     `NotFound` is `404 not_found`, a late reply is `504 timeout`, and a dropped
    ///     or full channel is `503 unavailable`.
    ///
    /// [`handle_session`] stays on its own path so its `403` source line is unchanged.
    fn handle_api<T: Serialize>(
        &self,
        request: Request<Body>,
        build: impl FnOnce(
            SignedInitData,
            i64,
            Value,
            SyncSender<Result<T, MiniAppApiError>>,
        ) -> Result<MiniAppApiRequest, Response<Body>>,
    ) -> Response<Body> {
        let (identity, chat_id) = match self.authenticate(&request) {
            Ok(parts) => parts,
            Err(response) => return response,
        };
        if let Err(response) = require_json_content_type(request.headers()) {
            return response;
        }
        let body = match read_limited_json_object(request) {
            Ok(value) => value,
            Err(response) => return response,
        };
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let event = match build(identity, chat_id, body, reply_tx) {
            Ok(event) => event,
            Err(response) => return response,
        };
        match self.events_tx.try_send(event) {
            Ok(()) => {}
            Err(_) => return status_response(StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
        }
        match reply_rx.recv_timeout(API_REPLY_TIMEOUT) {
            Ok(Ok(value)) => json_response(StatusCode::OK, value),
            Ok(Err(MiniAppApiError::Rejected | MiniAppApiError::Forbidden)) => {
                status_response(StatusCode::FORBIDDEN, "rejected")
            }
            Ok(Err(MiniAppApiError::ReadFailed)) => {
                status_response(StatusCode::INTERNAL_SERVER_ERROR, "read_failed")
            }
            Ok(Err(MiniAppApiError::Busy)) => {
                status_response(StatusCode::SERVICE_UNAVAILABLE, "busy")
            }
            Ok(Err(MiniAppApiError::NotFound)) => {
                status_response(StatusCode::NOT_FOUND, "not_found")
            }
            Err(RecvTimeoutError::Timeout) => {
                status_response(StatusCode::GATEWAY_TIMEOUT, "timeout")
            }
            Err(RecvTimeoutError::Disconnected) => {
                status_response(StatusCode::SERVICE_UNAVAILABLE, "unavailable")
            }
        }
    }

    /// Validate initData HMAC and paired identity. Body is not consumed.
    // Failures are HTTP responses. Boxing `Response<Body>` would change every `?` on this path.
    #[allow(clippy::result_large_err)]
    fn authenticate(
        &self,
        request: &Request<Body>,
    ) -> Result<(SignedInitData, i64), Response<Body>> {
        let init_data = request
            .headers()
            .get(INIT_DATA_HEADER)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| status_response(StatusCode::UNAUTHORIZED, "missing_init_data"))?;
        let signed = verify_init_data(init_data, self.token.expose(), now_unix_secs())
            .map_err(|error| status_response(auth_status(error), auth_reason(error)))?;
        let chat_id = authorize_paired_identity(&signed, &self.authorized_chat_ids)
            .map_err(|error| status_response(auth_status(error), auth_reason(error)))?;
        Ok((signed, chat_id))
    }
}

impl touche::server::Service for App {
    type Body = Body;
    type Error = io::Error;

    /// Close every response so keep-alive cannot outlive the owned listener's shutdown.
    fn call(&mut self, request: Request<Body>) -> Result<Response<Self::Body>, Self::Error> {
        let mut response = self.handle(request);
        response.headers_mut().insert(
            touche::header::CONNECTION,
            touche::header::HeaderValue::from_static("close"),
        );
        Ok(response)
    }
}

/// Reject non-loopback Host headers and oversized header maps.
fn check_peer_and_headers(headers: &HeaderMap, host: Option<&str>) -> Result<(), StatusCode> {
    if headers.len() > MAX_HEADER_COUNT {
        return Err(StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE);
    }
    let mut total = 0usize;
    for (name, value) in headers.iter() {
        total = total
            .saturating_add(name.as_str().len())
            .saturating_add(value.as_bytes().len());
        if total > MAX_HEADER_BYTES {
            return Err(StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE);
        }
    }
    if let Some(host_header) = headers
        .get("host")
        .and_then(|value| value.to_str().ok())
        .or(host)
    {
        let host_only = host_header.split(':').next().unwrap_or(host_header);
        if host_only != "127.0.0.1" && host_only != "localhost" {
            return Err(StatusCode::FORBIDDEN);
        }
    }
    Ok(())
}

/// Require `application/json` (optional charset) on API POSTs.
// The error is the HTTP response itself. Boxing it changes the helper's `Result`.
#[allow(clippy::result_large_err)]
fn require_json_content_type(headers: &HeaderMap) -> Result<(), Response<Body>> {
    let value = headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| status_response(StatusCode::UNSUPPORTED_MEDIA_TYPE, "content_type"))?;
    let media = value.split(';').next().unwrap_or(value).trim();
    if !media.eq_ignore_ascii_case("application/json") {
        return Err(status_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "content_type",
        ));
    }
    Ok(())
}

/// Read a JSON object body capped at [`MAX_BODY_BYTES`].
// The error is the HTTP response itself. Boxing it changes the helper's `Result`.
#[allow(clippy::result_large_err)]
fn read_limited_json_object(request: Request<Body>) -> Result<Value, Response<Body>> {
    if let Some(len) = request.body().len() {
        if len > MAX_BODY_BYTES {
            return Err(status_response(StatusCode::PAYLOAD_TOO_LARGE, "body"));
        }
    }
    let mut reader = request.into_body().into_reader();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let read = reader
            .read(&mut chunk)
            .map_err(|_| status_response(StatusCode::BAD_REQUEST, "body"))?;
        if read == 0 {
            break;
        }
        if buf.len().saturating_add(read) as u64 > MAX_BODY_BYTES {
            return Err(status_response(StatusCode::PAYLOAD_TOO_LARGE, "body"));
        }
        buf.extend_from_slice(&chunk[..read]);
    }
    if buf.is_empty() {
        return Ok(json!({}));
    }
    let value: Value = serde_json::from_slice(&buf)
        .map_err(|_| status_response(StatusCode::BAD_REQUEST, "json"))?;
    if !value.is_object() {
        return Err(status_response(StatusCode::BAD_REQUEST, "json"));
    }
    Ok(value)
}

/// Map initData errors to HTTP statuses without leaking HMAC material.
fn auth_status(error: InitDataError) -> StatusCode {
    match error {
        InitDataError::Unpaired => StatusCode::FORBIDDEN,
        InitDataError::Stale | InitDataError::Future => StatusCode::UNAUTHORIZED,
        InitDataError::InvalidHash
        | InitDataError::MissingHash
        | InitDataError::DuplicateKey
        | InitDataError::Malformed
        | InitDataError::MissingUser
        | InitDataError::InvalidUser
        | InitDataError::MissingAuthDate
        | InitDataError::InvalidAuthDate => StatusCode::UNAUTHORIZED,
    }
}

/// Stable machine reason for an initData failure.
fn auth_reason(error: InitDataError) -> &'static str {
    match error {
        InitDataError::Malformed => "malformed",
        InitDataError::DuplicateKey => "duplicate",
        InitDataError::MissingHash | InitDataError::InvalidHash => "hash",
        InitDataError::MissingAuthDate | InitDataError::InvalidAuthDate => "auth_date",
        InitDataError::MissingUser | InitDataError::InvalidUser => "user",
        InitDataError::Stale => "stale",
        InitDataError::Future => "future",
        InitDataError::Unpaired => "unpaired",
    }
}

/// Build a small JSON error body.
fn status_response(status: StatusCode, error: &'static str) -> Response<Body> {
    json_response(status, json!({ "error": error }))
}

/// Serialize `value` as a JSON response.
fn json_response(status: StatusCode, value: impl Serialize) -> Response<Body> {
    let body = serde_json::to_vec(&value).unwrap_or_else(|_| b"{\"error\":\"encode\"}".to_vec());
    Response::builder()
        .status(status)
        .header("content-type", "application/json; charset=utf-8")
        .header("cache-control", "no-store")
        .body(Body::from(body))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

/// Serve an embedded static asset.
fn static_response(content_type: &'static str, body: &str) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", content_type)
        .header("cache-control", "no-store")
        .body(Body::from(body.as_bytes().to_vec()))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

#[cfg(test)]
mod tests;
