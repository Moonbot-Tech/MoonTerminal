//! The station's own OpenAI Secure MCP Tunnel client: ChatGPT reaches the station's MCP with no
//! inbound port, public address or certificate.
//!
//! The station long-polls OpenAI for the requests ChatGPT addressed to its tunnel and answers each
//! one through the same MCP service the HTTPS listener serves, in-process, with no socket in
//! between. The wire contract is openai/tunnel-client `docs/protocol.md`: `GET /poll` returns 204
//! or a batch of commands; every command gets one `POST /response` echoing its `request_id`, with
//! its shard token in a header. The tunnel key authenticates the station to OpenAI and is never
//! sent anywhere else.
//!
//! Who is calling comes from the `X-Openai-Subject` header OpenAI's tunnel service puts on every
//! tool call: a pseudonymous ID of the ChatGPT account, stable across its chats and devices
//! (measured 2026-11-27). A paired account carries its grant's principal into the MCP request; an
//! unpaired one is marked a [`TunnelGuest`], which may list tools and is answered with a pairing
//! code on any call.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use axum::body::Body;
use axum::http::{HeaderName, HeaderValue, Request, header};
use moon_core::config::Secret;
use moon_core::station_api::mcp::{
    TUNNEL_KEY_REJECTED, TUNNEL_NOT_FOUND, TUNNEL_UNREACHABLE, TunnelState,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use super::{Shared, TunnelGuest, auth, http};

/// OpenAI's control plane for tunnels.
const API: &str = "https://api.openai.com";
/// The long-poll wait the station asks for; OpenAI bounds it itself.
const POLL_WAIT_MS: u64 = 15_000;
/// The most one poll may take, the long wait included.
const POLL_TIMEOUT: Duration = Duration::from_secs(30);
/// The most one response POST may take when the command set no deadline of its own.
const POST_TIMEOUT: Duration = Duration::from_secs(20);
/// Commands worked on at once; more wait for a slot rather than stall the poll.
const CONCURRENCY: usize = 8;
/// The largest MCP answer forwarded: the tools' own budget is 512 KiB, OpenAI's limit 10 MiB.
const MAX_ANSWER_BYTES: usize = 2 * 1024 * 1024;
/// The wait after OpenAI refused the key or does not know the tunnel: an owner has to act, so
/// asking again sooner only fills the log.
const OPERATOR_WAIT: Duration = Duration::from_secs(60);
/// The longest backoff between failed polls.
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// Attempts at delivering one response, the first included.
const POST_ATTEMPTS: u32 = 4;
/// The MCP response headers forwarded back, per the protocol's allowlist.
const RESPONSE_HEADERS: [&str; 4] = [
    "content-type",
    "mcp-session-id",
    "mcp-protocol-version",
    "www-authenticate",
];

/// One command from a poll; unknown fields are ignored, as the protocol requires.
#[derive(Deserialize)]
struct Command {
    request_id: String,
    shard_token: String,
    command_type: String,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    response_timeout: Option<Value>,
    #[serde(default)]
    headers: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    jsonrpc: Option<Value>,
}

/// A poll's `200` body.
#[derive(Deserialize)]
struct Batch {
    #[serde(default)]
    commands: Vec<Command>,
}

/// What one poll came back with.
enum Polled {
    /// `200` or `204`: OpenAI accepted the key; these commands (possibly none) are to be served.
    Commands(Vec<Command>),
    /// OpenAI refused the key (401/403).
    KeyRejected,
    /// OpenAI knows no such tunnel for this key (404).
    NotFound,
    /// No answer, a server error or a rate limit: try again after `retry_after` at the least.
    Retry { retry_after: Option<Duration> },
}

/// One response for OpenAI, before it is posted.
#[derive(Debug, PartialEq)]
struct Answer {
    resp_code: u16,
    resp_json: Option<Value>,
    resp_headers: BTreeMap<String, Vec<String>>,
    resp_type: &'static str,
}

/// What the station's MCP is, for OpenAI's routing: one remote Streamable HTTP `main` channel,
/// stateless at the transport, so neither capability is declared (protocol, "MCP server
/// information").
const SERVER_INFO: &str = r#"{"version":1,"channels":[{"name":"main"}]}"#;
/// The additive wire contract this client implements (`response_timeout` included).
const WIRE_PROTOCOL_VERSION: &str = "2026-08-25";

/// The tunnel's sender of HTTP requests to OpenAI.
struct Client {
    agent: ureq::Agent,
    tunnel_id: String,
    key: Secret,
    /// This process's ID among the clients of the tunnel, as `X-Tunnel-Client-Instance-Id`.
    instance: String,
}

impl Client {
    /// HTTPS only, no redirect followed: the key goes to OpenAI and nowhere else.
    fn new(tunnel_id: String, key: Secret) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .https_only(true)
            .max_redirects(0)
            .http_status_as_error(false)
            .user_agent(format!("MoonStation/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self {
            agent,
            tunnel_id,
            key,
            instance: auth::random().unwrap_or_else(|_| "moon-station".to_owned()),
        }
    }

    /// The headers every control-plane request carries: the key, and who this client is.
    fn identify<B>(&self, request: ureq::RequestBuilder<B>) -> ureq::RequestBuilder<B> {
        request
            .header("Authorization", format!("Bearer {}", self.key.expose()))
            .header("X-Tunnel-Client-Name", "moon-station")
            .header("X-Tunnel-Client-Version", env!("CARGO_PKG_VERSION"))
            .header("X-Tunnel-Client-Instance-Id", &self.instance)
            .header("X-Tunnel-MCP-Server-Info", SERVER_INFO)
            .header(
                "X-Tunnel-Client-Wire-Protocol-Version",
                WIRE_PROTOCOL_VERSION,
            )
    }

    /// One long poll, blocking.
    fn poll(&self) -> Polled {
        let url = format!(
            "{API}/v1/tunnels/{}/poll?limit=25&timeout_ms={POLL_WAIT_MS}",
            self.tunnel_id
        );
        let request = self.identify(
            self.agent
                .get(&url)
                .config()
                .timeout_global(Some(POLL_TIMEOUT))
                .build(),
        );
        match request.call() {
            Ok(mut response) => {
                let status = response.status().as_u16();
                log::trace!("mcp tunnel: poll -> {status}");
                let retry_after = response
                    .headers()
                    .get("retry-after")
                    .and_then(|value| value.to_str().ok())
                    .and_then(retry_after);
                let body = (status == 200)
                    .then(|| {
                        response
                            .body_mut()
                            .with_config()
                            .limit(16 * 1024 * 1024)
                            .read_to_string()
                            .ok()
                    })
                    .flatten();
                classify_poll(status, body.as_deref(), retry_after)
            }
            Err(error) => {
                log::trace!("mcp tunnel: poll failed: {error}");
                Polled::Retry { retry_after: None }
            }
        }
    }

    /// Deliver one response, blocking, retrying what the protocol says is safe to retry.
    ///
    /// Returns:
    ///     Whether OpenAI took it, or reported the command already finished (404).
    fn respond(&self, command: &CommandRef, answer: &Answer, deadline: Instant) -> bool {
        let url = format!("{API}/v1/tunnels/{}/response", self.tunnel_id);
        let mut body = json!({
            "request_id": command.request_id,
            "channel": command.channel,
            "resp_headers": answer.resp_headers,
            "resp_code": answer.resp_code,
            "resp_type": answer.resp_type,
        });
        // An acknowledgment carries no payload: the field is omitted, as the protocol says, never
        // sent as JSON null (OpenAI already refused a null in this body).
        if let Some(json) = &answer.resp_json {
            body["resp_json"] = json.clone();
        }
        let mut wait = Duration::from_millis(500);
        for attempt in 1..=POST_ATTEMPTS {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            let outcome = self
                .identify(
                    self.agent
                        .post(&url)
                        .config()
                        .timeout_global(Some(left.min(POST_TIMEOUT)))
                        .build(),
                )
                .header("X-Tunnel-Shard-Token", &command.shard_token)
                .send_json(&body);
            let mut retry_after_wait = None;
            let retry = match outcome {
                Ok(mut response) => match response.status().as_u16() {
                    200..=299 | 404 => return true,
                    status @ (408 | 429 | 502 | 503 | 504) => {
                        // A valid Retry-After is a minimum on 429 and 503, as for polls.
                        if matches!(status, 429 | 503) {
                            retry_after_wait = response
                                .headers()
                                .get("retry-after")
                                .and_then(|value| value.to_str().ok())
                                .and_then(retry_after);
                        }
                        true
                    }
                    other => {
                        // OpenAI's own error text: it names what it found invalid, and it never
                        // carries the key, which travels only in the request header.
                        let detail = response
                            .body_mut()
                            .with_config()
                            .limit(2048)
                            .read_to_string()
                            .unwrap_or_default();
                        log::warn!("mcp tunnel: OpenAI refused a response ({other}): {detail}");
                        return false;
                    }
                },
                Err(_) => true,
            };
            if !retry || attempt == POST_ATTEMPTS {
                break;
            }
            let pause = jittered(wait).max(retry_after_wait.unwrap_or_default());
            std::thread::sleep(pause.min(deadline.saturating_duration_since(Instant::now())));
            wait *= 2;
        }
        false
    }
}

/// The parts of a command its response echoes.
#[derive(Clone)]
struct CommandRef {
    request_id: String,
    shard_token: String,
    channel: String,
}

/// The logical channel a command without one belongs to.
const DEFAULT_CHANNEL: &str = "main";

/// Sort a poll's HTTP answer into what the loop does next.
fn classify_poll(status: u16, body: Option<&str>, retry_after: Option<Duration>) -> Polled {
    match status {
        204 => Polled::Commands(Vec::new()),
        200 => match body.map(serde_json::from_str::<Batch>) {
            Some(Ok(batch)) => Polled::Commands(batch.commands),
            // An unreadable batch cannot be answered command by command; poll again.
            _ => Polled::Retry { retry_after: None },
        },
        401 | 403 => Polled::KeyRejected,
        404 => Polled::NotFound,
        _ => Polled::Retry { retry_after },
    }
}

/// A `Retry-After` in delta-seconds, capped; the HTTP-date form is ignored, as the protocol allows.
fn retry_after(value: &str) -> Option<Duration> {
    value
        .trim()
        .parse::<u64>()
        .ok()
        .map(|seconds| Duration::from_secs(seconds.min(MAX_BACKOFF.as_secs())))
}

/// A command's `response_timeout`: digits and one lowercase unit. Anything else is no deadline,
/// as the protocol says a client fails open.
fn response_timeout(value: Option<&Value>) -> Option<Duration> {
    let text = value?.as_str()?;
    let split = text.find(|c: char| !c.is_ascii_digit())?;
    let (digits, unit) = text.split_at(split);
    if digits.is_empty() {
        return None;
    }
    let amount: u64 = digits.parse().ok()?;
    let nanos_per = match unit {
        "ns" => 1,
        "us" => 1_000,
        "ms" => 1_000_000,
        "s" => 1_000_000_000,
        "m" => 60_000_000_000,
        "h" => 3_600_000_000_000,
        _ => return None,
    };
    amount.checked_mul(nanos_per).map(Duration::from_nanos)
}

/// A header's first value, its name matched without regard to case, as HTTP names are.
fn first_header<'a>(headers: &'a BTreeMap<String, Vec<String>>, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .and_then(|(_, values)| values.first())
        .map(String::as_str)
}

/// The ChatGPT account behind a command: its `X-Openai-Subject`, when it is a plausible one.
fn subject(headers: &BTreeMap<String, Vec<String>>) -> Option<String> {
    first_header(headers, "x-openai-subject")
        .filter(|value| {
            !value.is_empty() && value.len() <= 256 && value.bytes().all(|b| b.is_ascii_graphic())
        })
        .map(str::to_owned)
}

/// The MCP request for one `jsonrpc` command, as the HTTPS listener would have received it, with
/// the caller's identity attached. Only the MCP protocol's own `Mcp-*` headers are carried over;
/// OpenAI's tracing and routing headers stay behind.
fn mcp_request(
    jsonrpc: &Value,
    headers: &BTreeMap<String, Vec<String>>,
    caller: Caller,
) -> Result<Request<Body>> {
    let mut request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header(header::HOST, http::TUNNEL_HOST)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream");
    for (name, values) in headers {
        if !name.to_ascii_lowercase().starts_with("mcp-") {
            continue;
        }
        let Ok(name) = HeaderName::try_from(name.as_str()) else {
            continue;
        };
        for value in values {
            if let Ok(value) = HeaderValue::from_str(value) {
                request = request.header(name.clone(), value);
            }
        }
    }
    let mut request = request
        .body(Body::from(serde_json::to_vec(jsonrpc)?))
        .context("build the MCP request")?;
    match caller {
        Caller::Paired(principal) => {
            request.extensions_mut().insert(principal);
        }
        Caller::Guest(guest) => {
            request.extensions_mut().insert(guest);
        }
    }
    Ok(request)
}

/// Who a command came from, as far as the station's grants know.
enum Caller {
    Paired(auth::Principal),
    Guest(TunnelGuest),
}

/// The response for an MCP answer: its JSON body, status and allowlisted headers. A request that
/// carried no JSON-RPC `id` is a notification, acknowledged without a body.
fn answer(jsonrpc: &Value, status: u16, headers: &axum::http::HeaderMap, body: &[u8]) -> Answer {
    let resp_headers = RESPONSE_HEADERS
        .iter()
        .filter_map(|name| {
            let values: Vec<String> = headers
                .get_all(*name)
                .iter()
                .filter_map(|value| value.to_str().ok().map(str::to_owned))
                .collect();
            (!values.is_empty()).then(|| (canonical(name), values))
        })
        .collect();
    let id = jsonrpc.get("id").filter(|id| !id.is_null());
    let Some(id) = id else {
        return Answer {
            resp_code: status,
            resp_json: None,
            resp_headers,
            resp_type: "notify_ack",
        };
    };
    let resp_json = serde_json::from_slice::<Value>(body)
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| gateway_error(id, "the station's MCP returned no JSON-RPC answer"));
    Answer {
        resp_code: status,
        resp_json: Some(resp_json),
        resp_headers,
        resp_type: "jsonrpc_response",
    }
}

/// A synthesized JSON-RPC failure, for when no MCP answer can be recovered.
fn gateway_error(id: &Value, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32603,
            "message": message,
            "data": {"tunnel_failure": {"version": 1, "source": "client_internal", "upstream_response_received": false}}
        }
    })
}

/// A response header's name as OpenAI's own client spells it.
fn canonical(name: &str) -> String {
    name.split('-')
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + chars.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// Serve the tunnel until `stop`: poll, hand each command to a worker, keep `state` current for
/// the owner's page. Never returns an error: every failure is retried or waits for the owner.
///
/// Args:
///     shared: The MCP service's state; its grants decide who a command is.
///     tunnel_id: The tunnel from OpenAI Platform.
///     key: The tunnel key, the station's credential `openai-tunnel-key`.
///     stop: Cancelled when the station stops.
pub(super) async fn run(
    shared: Arc<Shared>,
    tunnel_id: String,
    key: Secret,
    state: Arc<Mutex<TunnelState>>,
    stop: CancellationToken,
) {
    let client = Arc::new(Client::new(tunnel_id, key));
    let service = Arc::new(http::tunnel_service(shared.clone(), stop.clone()));
    let slots = Arc::new(Semaphore::new(CONCURRENCY));
    let mut backoff = Duration::from_secs(1);
    let mut problem: Option<&'static str> = None;
    log::info!("mcp tunnel: polling OpenAI");
    loop {
        let polling = client.clone();
        let polled = tokio::select! {
            () = stop.cancelled() => return,
            polled = tokio::task::spawn_blocking(move || polling.poll()) => polled,
        };
        let received = Instant::now();
        let polled = polled.unwrap_or(Polled::Retry { retry_after: None });
        let (now_problem, wait) = match polled {
            Polled::Commands(commands) => {
                backoff = Duration::from_secs(1);
                for command in commands {
                    spawn_command(
                        command,
                        received,
                        client.clone(),
                        service.clone(),
                        shared.clone(),
                        slots.clone(),
                    );
                }
                (None, Duration::ZERO)
            }
            Polled::KeyRejected => (Some(TUNNEL_KEY_REJECTED), OPERATOR_WAIT),
            Polled::NotFound => (Some(TUNNEL_NOT_FOUND), OPERATOR_WAIT),
            Polled::Retry { retry_after } => {
                let wait = jittered(backoff).max(retry_after.unwrap_or_default());
                backoff = (backoff * 2).min(MAX_BACKOFF);
                (Some(TUNNEL_UNREACHABLE), wait)
            }
        };
        if now_problem != problem {
            match now_problem {
                None => log::info!("mcp tunnel: online"),
                Some(code) => log::warn!("mcp tunnel: {code}"),
            }
            problem = now_problem;
        }
        if let Ok(mut state) = state.lock() {
            state.online = now_problem.is_none();
            state.problem = now_problem.map(str::to_owned);
            if now_problem.is_none() {
                state.last_contact_ms = Some(auth::now());
            }
        }
        if !wait.is_zero() {
            tokio::select! {
                () = stop.cancelled() => return,
                () = tokio::time::sleep(wait) => {}
            }
        }
    }
}

/// Up to a quarter more than `base`, so many stations do not retry in step.
fn jittered(base: Duration) -> Duration {
    let mut byte = [0u8; 1];
    let spread = getrandom::getrandom(&mut byte).map_or(0, |()| u64::from(byte[0]));
    base + base.mul_f64(spread as f64 / 1024.0)
}

/// Work one command on its own task, within a concurrency slot and its deadline.
fn spawn_command(
    command: Command,
    received: Instant,
    client: Arc<Client>,
    service: Arc<http::TunnelService>,
    shared: Arc<Shared>,
    slots: Arc<Semaphore>,
) {
    let timeout = response_timeout(command.response_timeout.as_ref());
    let deadline = received + timeout.unwrap_or(Duration::from_secs(120));
    let reference = CommandRef {
        request_id: command.request_id.clone(),
        shard_token: command.shard_token.clone(),
        // An absent channel is `main`, as the protocol defines; echoing JSON null instead would
        // be a response OpenAI's schema does not accept.
        channel: command
            .channel
            .clone()
            .unwrap_or_else(|| DEFAULT_CHANNEL.to_owned()),
    };
    tokio::spawn(async move {
        // An already expired command gets no MCP work and no response, per the protocol.
        if Instant::now() >= deadline {
            return;
        }
        let Ok(_slot) = slots.acquire_owned().await else {
            return;
        };
        // Again after the wait for a slot: a command that expired in the queue is dropped too.
        if Instant::now() >= deadline {
            return;
        }
        let started = Instant::now();
        let answer = match command.command_type.as_str() {
            "jsonrpc" => {
                let Some(jsonrpc) = command.jsonrpc.as_ref() else {
                    log::warn!("mcp tunnel: a jsonrpc command without its payload");
                    return;
                };
                let work = serve(jsonrpc, &command.headers, &service, &shared);
                match tokio::time::timeout_at(deadline.into(), work).await {
                    Ok(answer) => answer,
                    // Past the deadline nothing is posted: OpenAI already gave up on it.
                    Err(_) => return,
                }
            }
            // Stateless MCP keeps no session to close; acknowledge it.
            "session_termination" => Answer {
                resp_code: 204,
                resp_json: None,
                resp_headers: BTreeMap::new(),
                resp_type: "session_termination_response",
            },
            other => {
                log::warn!("mcp tunnel: unsupported command type {other:?}; ignored");
                return;
            }
        };
        // The JSON-RPC method names what ChatGPT asked for; never the arguments, which can name
        // strategies, coins and amounts. Bounded: the sender chooses it.
        let method: String = command
            .jsonrpc
            .as_ref()
            .and_then(|jsonrpc| jsonrpc.get("method"))
            .and_then(Value::as_str)
            .unwrap_or(command.command_type.as_str())
            .chars()
            .take(64)
            .collect();
        // Only the protocol-level error code: a message can quote what the caller sent.
        if let Some(code) = answer
            .resp_json
            .as_ref()
            .and_then(|json| json.get("error"))
            .and_then(|error| error.get("code"))
        {
            log::info!("mcp tunnel: {method} answered with JSON-RPC error {code}");
        }
        let code = answer.resp_code;
        let delivered =
            tokio::task::spawn_blocking(move || client.respond(&reference, &answer, deadline))
                .await
                .unwrap_or(false);
        log::info!(
            "mcp tunnel: {method} -> {code} delivered={delivered} ms={}",
            started.elapsed().as_millis()
        );
    });
}

/// Answer one JSON-RPC command through the MCP service, as the caller its subject names.
async fn serve(
    jsonrpc: &Value,
    headers: &BTreeMap<String, Vec<String>>,
    service: &http::TunnelService,
    shared: &Shared,
) -> Answer {
    let id = jsonrpc.get("id").cloned().unwrap_or(Value::Null);
    let subject = subject(headers);
    log::trace!(
        "mcp tunnel: command headers {:?}, subject={}",
        headers.keys().collect::<Vec<_>>(),
        subject.is_some()
    );
    let principal = subject.as_deref().and_then(|subject| {
        shared
            .auth
            .lock()
            .ok()
            .and_then(|mut auth| auth.tunnel_caller(subject, auth::now()))
    });
    let caller = match principal {
        Some(principal) => Caller::Paired(principal),
        None => Caller::Guest(TunnelGuest { subject }),
    };
    let request = match mcp_request(jsonrpc, headers, caller) {
        Ok(request) => request,
        Err(_) => {
            return answer(
                jsonrpc,
                400,
                &axum::http::HeaderMap::new(),
                gateway_error(&id, "the request could not be passed to the station's MCP")
                    .to_string()
                    .as_bytes(),
            );
        }
    };
    let response = service.handle(request).await;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    match axum::body::to_bytes(Body::new(response.into_body()), MAX_ANSWER_BYTES).await {
        Ok(body) => answer(jsonrpc, status, &headers, &body),
        Err(_) => answer(
            jsonrpc,
            502,
            &axum::http::HeaderMap::new(),
            gateway_error(
                &id,
                "the station's answer could not be read in full (too large or cut off)",
            )
            .to_string()
            .as_bytes(),
        ),
    }
}

#[cfg(test)]
mod tests;
