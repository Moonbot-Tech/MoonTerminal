//! Loopback-only Mini App HTTP server.
//!
//! All `touche` types stay inside this module. The listener binds only `127.0.0.1` with an
//! OS-selected port. Static assets are embedded; the authenticated `POST /api/session` route
//! verifies Telegram `initData` and then authorizes the signed identity against paired chat ids.

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
/// How long the handler waits for a session-check reply.
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
    /// Chat ids allowed to pass the Mini App session check.
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
}

/// Handler-level API failure that does not carry secrets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MiniAppApiError {
    /// Consumer refused the request.
    Rejected,
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
            (&Method::GET, "/api/session") | (&Method::HEAD, "/") | (&Method::OPTIONS, _) => {
                status_response(StatusCode::METHOD_NOT_ALLOWED, "method")
            }
            _ => {
                if matches!(
                    request.method(),
                    &Method::POST | &Method::PUT | &Method::PATCH | &Method::DELETE
                ) && request.uri().path().starts_with("/api/")
                {
                    status_response(StatusCode::NOT_FOUND, "not_found")
                } else if request.method() != &Method::GET {
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

    /// Validate initData HMAC and paired identity. Body is not consumed.
    fn authenticate(
        &self,
        request: &Request<Body>,
    ) -> Result<(SignedInitData, i64), Response<Body>> {
        let init_data = request
            .headers()
            .get(INIT_DATA_HEADER)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| status_response(StatusCode::UNAUTHORIZED, "missing_init_data"))?;
        if init_data.len() > MAX_HEADER_BYTES {
            return Err(status_response(StatusCode::PAYLOAD_TOO_LARGE, "init_data"));
        }
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
        if host_only != "127.0.0.1" && host_only != "localhost" && host_only != "[::1]" {
            return Err(StatusCode::FORBIDDEN);
        }
    }
    Ok(())
}

/// Require `application/json` (optional charset) on API POSTs.
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
