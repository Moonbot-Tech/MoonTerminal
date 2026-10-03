//! HTTP regressions for Mini App cancel and panic writes.
//!
//! The oracle is Telegram's documented initData contract plus the write-route
//! limits: a launch older than one hour is not a credential, a body field the
//! route does not name is not a command, and a market longer than 64 bytes is
//! not sent. Status codes come from those rules, not from a value this module
//! computed and then read back.

use std::io::Read;
use std::sync::{Arc, RwLock, atomic::AtomicBool};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use sha2::Sha256;
use touche::body::HttpBody;
use touche::{Body, Method, Request, StatusCode};

use super::dto::{StrategyDto, id_text};
use super::{
    Admission, App, CancelOrderBody, MiniAppApiError, MiniAppApiRequest, StrategyToggleBody,
};
use crate::config::Secret;

const BOT_TOKEN: &str = "123456:authoring-fixture-token";
const PAIRED_USER: i64 = 41;
/// Documented Mini App launch window. One second past this is not a credential.
const INIT_DATA_MAX_AGE_SECS: u64 = 3600;
const MAX_MARKET_BYTES: usize = 64;

/// Signs a percent-safe initData fixture with Telegram's two-stage HMAC-SHA256.
///
/// Args:
///     fields: Key/value pairs excluding `hash`.
///
/// Returns:
///     The query string a WebApp would send, with `hash` appended.
fn signed_init_data(fields: &[(&str, &str)]) -> String {
    let mut sorted = fields.to_vec();
    sorted.sort_unstable_by_key(|(key, _)| *key);
    let data_check_string = sorted
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\n");

    let mut token_key =
        Hmac::<Sha256>::new_from_slice(b"WebAppData").expect("HMAC accepts the WebAppData key");
    token_key.update(BOT_TOKEN.as_bytes());
    let secret_key = token_key.finalize().into_bytes();

    let mut signature = Hmac::<Sha256>::new_from_slice(&secret_key)
        .expect("a SHA-256 HMAC digest is a valid HMAC key");
    signature.update(data_check_string.as_bytes());
    let hash = signature
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();

    fields
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .chain(std::iter::once(format!("hash={hash}")))
        .collect::<Vec<_>>()
        .join("&")
}

/// Returns the same signed input with one flipped hash nibble.
fn with_forged_hash(init_data: &str) -> String {
    let (fields, hash) = init_data
        .rsplit_once("&hash=")
        .expect("the fixture signer always appends hash last");
    let replacement = if hash.ends_with('0') { '1' } else { '0' };
    format!("{fields}&hash={}{}", &hash[..hash.len() - 1], replacement)
}

/// Unix seconds, moved off the last 300 ms of the current second so a one-second
/// boundary cannot flip an at-limit launch into the next second mid-request.
fn stable_unix_now() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the test clock is after the Unix epoch");
    if now.subsec_millis() > 700 {
        thread::sleep(Duration::from_millis(350));
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the test clock is after the Unix epoch")
        .as_secs()
}

/// Signs a paired user launch at `auth_date`.
fn signed_at(auth_date: u64) -> String {
    let auth_date = auth_date.to_string();
    signed_init_data(&[
        ("auth_date", auth_date.as_str()),
        ("query_id", "AAEAAQ"),
        ("user", "{\"id\":41,\"first_name\":\"Ada\"}"),
    ])
}

/// Builds the write handler and a thread that accepts every authenticated command.
///
/// A refused auth path returns before this thread is used. An accepted command
/// must not sit in the five-second reply wait, or a dropped guard looks like a timeout.
fn handler_with_acceptor() -> App {
    let (events_tx, events_rx) = std::sync::mpsc::sync_channel(8);
    thread::spawn(move || {
        while let Ok(event) = events_rx.recv() {
            accept(event);
        }
    });
    App {
        admission: Arc::new(RwLock::new(Admission {
            chats: vec![PAIRED_USER],
            labels: std::collections::BTreeMap::new(),
        })),
        token: Arc::new(Secret::new(BOT_TOKEN)),
        events_tx,
        stop: Arc::new(AtomicBool::new(false)),
    }
}

/// Answers one authenticated event so the HTTP handler can finish.
fn accept(event: MiniAppApiRequest) {
    match event {
        MiniAppApiRequest::CancelOrder { reply, .. } => {
            let _ = reply.send(Ok(super::dto::CommandResultDto {
                ok: true,
                armed: None,
                error: None,
            }));
        }
        MiniAppApiRequest::PanicSell { reply, .. } => {
            let _ = reply.send(Ok(super::dto::CommandResultDto {
                ok: true,
                armed: Some(false),
                error: None,
            }));
        }
        MiniAppApiRequest::Session { reply, .. } => {
            let _ = reply.send(Ok(()));
        }
        MiniAppApiRequest::Report { reply, .. } => {
            let _ = reply.send(Err(MiniAppApiError::Rejected));
        }
        MiniAppApiRequest::Cores { reply, .. } => {
            let _ = reply.send(Err(MiniAppApiError::Rejected));
        }
        MiniAppApiRequest::Orders { reply, .. } => {
            let _ = reply.send(Err(MiniAppApiError::Rejected));
        }
        MiniAppApiRequest::Trades { reply, .. } => {
            let _ = reply.send(Err(MiniAppApiError::Rejected));
        }
        MiniAppApiRequest::Strategies { reply, .. } => {
            let _ = reply.send(Err(MiniAppApiError::Rejected));
        }
        MiniAppApiRequest::CoreSwitch { reply, .. }
        | MiniAppApiRequest::CancelAllOrders { reply, .. }
        | MiniAppApiRequest::StrategyToggle { reply, .. }
        | MiniAppApiRequest::CoreReconnect { reply, .. } => {
            let _ = reply.send(Ok(super::dto::CommandResultDto {
                ok: true,
                armed: None,
                error: None,
            }));
        }
        MiniAppApiRequest::CoresSwitch { cores, reply, .. } => {
            let n = u32::try_from(cores.len()).unwrap_or(u32::MAX);
            let _ = reply.send(Ok(super::dto::ScopeResultDto {
                ok: true,
                sent: n,
                requested: n,
                already: 0,
                offline: 0,
                error: None,
            }));
        }
        MiniAppApiRequest::Notify { reply, .. } | MiniAppApiRequest::NotifySave { reply, .. } => {
            let _ = reply.send(Ok(notify_ok()));
        }
    }
}

/// The settings document the acceptor returns for both notify routes.
fn notify_ok() -> super::dto::NotifyDto {
    super::dto::NotifyDto {
        settings: crate::telegram::notify::NotifySettings::default(),
        cores: vec![super::dto::NotifyCoreDto {
            id: 1,
            name: "A".into(),
            exchange: "Binance".into(),
        }],
        zone: "UTC".into(),
        revision: 0,
        error: None,
        fault: None,
    }
}

/// Posts one JSON write and returns the status plus the raw body.
fn post(app: &App, path: &str, init_data: Option<&str>, body: &str) -> (u16, String) {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(format!("http://127.0.0.1{path}"))
        .header("host", "127.0.0.1")
        .header("content-type", "application/json");
    if let Some(init_data) = init_data {
        builder = builder.header("x-telegram-init-data", init_data);
    }
    let request = builder
        .body(Body::from(body.as_bytes().to_vec()))
        .expect("the fixture request is valid HTTP");
    let response = app.handle(request);
    let status = response.status();
    let mut reader = response.into_body().into_reader();
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .expect("the response body is readable");
    let text = String::from_utf8(bytes).expect("the response body is UTF-8 JSON");
    assert_ne!(
        status,
        StatusCode::GATEWAY_TIMEOUT,
        "{path} waited out the reply timeout; the acceptor did not run. body {text}"
    );
    (status.as_u16(), text)
}

/// Reads the compact `error` string from a JSON error body.
fn error_code(body: &str) -> Option<&str> {
    let rest = body.split_once("\"error\":\"")?.1;
    Some(rest.split_once('"')?.0)
}

/// `telegram/init_data.rs:verify_init_data` must keep a launch older than one hour
/// out of `POST /api/order/cancel` and `POST /api/panic`.
///
/// Mutation: drop the `auth_date` age check (`now - auth_date > 3600` returns
/// `Stale`). A captured Mini App launch then cancels orders and arms Panic Sell
/// an hour after the user closed the page. A missing or forged header stays 401
/// either way; those asserts are the controls that the route is not hard-coded
/// to reject.
#[test]
fn telegram_write_auth_date_past_one_hour_is_rejected() {
    let app = handler_with_acceptor();
    let now = stable_unix_now();
    let fresh = signed_at(now);
    let at_limit = signed_at(now.saturating_sub(INIT_DATA_MAX_AGE_SECS));
    let one_past = signed_at(now.saturating_sub(INIT_DATA_MAX_AGE_SECS + 1));
    let forged = with_forged_hash(&fresh);
    let routes = [
        ("/api/order/cancel", r#"{"core":1,"uid":"2"}"#),
        ("/api/panic", r#"{"core":1,"market":"BTCUSDT","on":true}"#),
    ];

    for (path, body) in routes {
        let (status, text) = post(&app, path, None, body);
        assert_eq!(status, 401, "{path} without initData body {text}");
        assert_eq!(
            error_code(&text),
            Some("missing_init_data"),
            "{path} must name a missing credential, body {text}"
        );

        let (status, text) = post(&app, path, Some(&forged), body);
        assert_eq!(status, 401, "{path} forged hash body {text}");
        assert_eq!(
            error_code(&text),
            Some("hash"),
            "{path} must reject a forged hash, body {text}"
        );

        let (status, text) = post(&app, path, Some(&fresh), body);
        assert_eq!(
            status, 200,
            "{path} a launch from this second must reach the command, body {text}"
        );

        let (status, text) = post(&app, path, Some(&at_limit), body);
        assert_eq!(
            status, 200,
            "{path} a launch exactly {INIT_DATA_MAX_AGE_SECS}s old is still inside the hour, body {text}"
        );

        let (status, text) = post(&app, path, Some(&one_past), body);
        assert_eq!(
            status, 401,
            "{path} one second past the hour must not cancel or arm panic, body {text}"
        );
        assert_eq!(
            error_code(&text),
            Some("stale"),
            "{path} one second past the hour is a stale credential, body {text}"
        );
    }
}

/// `web.rs:CancelOrderBody` must reject a field the cancel route does not name.
///
/// Mutation: drop `deny_unknown_fields` on `CancelOrderBody`. A client can then
/// smuggle an extra field into `POST /api/order/cancel` and the order is still
/// cancelled. The same-shaped panic body is not this assertion.
#[test]
fn telegram_cancel_unknown_field_is_rejected() {
    let app = handler_with_acceptor();
    let init_data = signed_at(stable_unix_now());
    let (status, text) = post(
        &app,
        "/api/order/cancel",
        Some(&init_data),
        r#"{"core":1,"uid":"2"}"#,
    );
    assert_eq!(
        status, 200,
        "a named cancel body must reach the command, body {text}"
    );

    let (status, text) = post(
        &app,
        "/api/order/cancel",
        Some(&init_data),
        r#"{"core":1,"uid":"2","note":1}"#,
    );
    assert_eq!(
        status, 400,
        "an unknown cancel field must be refused before the order is touched, body {text}"
    );
    assert_eq!(
        error_code(&text),
        Some("json"),
        "an unknown cancel field is a JSON refusal, body {text}"
    );
}

/// `web.rs:PanicSellBody` must reject a field the panic route does not name.
///
/// Mutation: drop `deny_unknown_fields` on `PanicSellBody`. `POST /api/panic`
/// then accepts a body the page never sends and still arms Panic Sell.
#[test]
fn telegram_panic_unknown_field_is_rejected() {
    let app = handler_with_acceptor();
    let init_data = signed_at(stable_unix_now());
    let (status, text) = post(
        &app,
        "/api/panic",
        Some(&init_data),
        r#"{"core":1,"market":"BTCUSDT","on":true}"#,
    );
    assert_eq!(
        status, 200,
        "a named panic body must reach the command, body {text}"
    );

    let (status, text) = post(
        &app,
        "/api/panic",
        Some(&init_data),
        r#"{"core":1,"market":"BTCUSDT","on":true,"note":1}"#,
    );
    assert_eq!(
        status, 400,
        "an unknown panic field must be refused before Panic Sell changes, body {text}"
    );
    assert_eq!(
        error_code(&text),
        Some("json"),
        "an unknown panic field is a JSON refusal, body {text}"
    );
}

/// `web.rs:App::handle` must refuse a panic `market` longer than 64 bytes and
/// accept one of exactly 64 bytes.
///
/// Mutation: drop the `parsed.market.len() > MAX_MARKET_BYTES` guard. A 65-byte
/// market then arms Panic Sell. The 64-byte request is the at-limit half: it
/// must still reach the command, so the refusal is the length rather than a
/// blanket 400.
#[test]
fn telegram_panic_market_over_64_bytes_is_rejected() {
    let app = handler_with_acceptor();
    let init_data = signed_at(stable_unix_now());
    let at_limit = "A".repeat(MAX_MARKET_BYTES);
    let one_past = "A".repeat(MAX_MARKET_BYTES + 1);

    let (status, text) = post(
        &app,
        "/api/panic",
        Some(&init_data),
        &format!(r#"{{"core":1,"market":"{at_limit}","on":true}}"#),
    );
    assert_eq!(
        status, 200,
        "a market of exactly {MAX_MARKET_BYTES} bytes must reach Panic Sell, body {text}"
    );

    let (status, text) = post(
        &app,
        "/api/panic",
        Some(&init_data),
        &format!(r#"{{"core":1,"market":"{one_past}","on":false}}"#),
    );
    assert_eq!(
        status, 400,
        "a market of {MAX_MARKET_BYTES} + 1 bytes must not arm Panic Sell, body {text}"
    );
    assert_eq!(
        error_code(&text),
        Some("json"),
        "an overlong market is the same JSON refusal as a bad body, body {text}"
    );
}

/// `web.rs` route arms `/api/core/switch`, `/api/cores/switch`, `/api/core/cancel_all`,
/// `/api/strategy/toggle` and `/api/core/reconnect` must authenticate through `handle_api`.
///
/// Mutation: one arm parses its body and sends the command without the
/// initData HMAC, freshness and paired-chat check. Anyone holding the tunnel URL
/// can then stop trading, cancel every order, toggle strategies or reconnect the owner's cores. The fresh
/// paired launch reaching 200 is the control that the routes are not hard-coded
/// to reject.
#[test]
fn telegram_core_control_routes_require_fresh_paired_init_data() {
    let app = handler_with_acceptor();
    let now = stable_unix_now();
    let fresh = signed_at(now);
    let one_past = signed_at(now.saturating_sub(INIT_DATA_MAX_AGE_SECS + 1));
    let forged = with_forged_hash(&fresh);
    let now_text = now.to_string();
    let stranger = signed_init_data(&[
        ("auth_date", now_text.as_str()),
        ("query_id", "AAEAAQ"),
        ("user", "{\"id\":42,\"first_name\":\"Eve\"}"),
    ]);
    let routes = [
        (
            "/api/core/switch",
            r#"{"core":1,"switch":"trading","on":false}"#,
        ),
        (
            "/api/cores/switch",
            r#"{"cores":[1,2],"switch":"auto_detect","on":false}"#,
        ),
        ("/api/core/cancel_all", r#"{"core":1}"#),
        ("/api/strategy/toggle", r#"{"core":1,"id":"7","on":true}"#),
        ("/api/core/reconnect", r#"{"core":1}"#),
    ];

    for (path, body) in routes {
        let (status, text) = post(&app, path, None, body);
        assert_eq!(status, 401, "{path} without initData body {text}");
        assert_eq!(
            error_code(&text),
            Some("missing_init_data"),
            "{path} body {text}"
        );

        let (status, text) = post(&app, path, Some(&forged), body);
        assert_eq!(status, 401, "{path} forged hash body {text}");
        assert_eq!(error_code(&text), Some("hash"), "{path} body {text}");

        let (status, text) = post(&app, path, Some(&one_past), body);
        assert_eq!(status, 401, "{path} stale launch body {text}");
        assert_eq!(error_code(&text), Some("stale"), "{path} body {text}");

        let (status, text) = post(&app, path, Some(&stranger), body);
        assert_ne!(
            status, 200,
            "{path} a validly signed but unpaired user must not reach the core, body {text}"
        );

        let (status, text) = post(&app, path, Some(&fresh), body);
        assert_eq!(
            status, 200,
            "{path} a fresh paired launch must reach the command, body {text}"
        );
    }
}

/// `dto.rs:id_text` carries ids past 2^53 through the DTO JSON and back through the command
/// bodies unchanged.
///
/// Mutation: drop `serialize_with` on `StrategyDto.id` or `deserialize_with` on the bodies. The id
/// then goes out as a JSON number the page rounds, or a string body is rejected as `json`, and the
/// strategy toggle never reaches its handler. Oracle: `u64::MAX` and a negative stored id
/// -1234567890123456789 read as `u64` (17212176183586094827) survive both directions.
#[test]
fn big_ids_round_trip_as_decimal_strings() {
    for id in [u64::MAX, (-1_234_567_890_123_456_789_i64) as u64] {
        let dto = StrategyDto {
            id,
            name: "S".into(),
            checked: true,
            wanted: None,
            pending: None,
        };
        let json = serde_json::to_value(&dto).expect("serialize");
        let text = id.to_string();
        assert_eq!(json["id"], serde_json::Value::String(text.clone()));

        let body = format!(r#"{{"core":1,"id":"{text}","on":true}}"#);
        let toggle: StrategyToggleBody = serde_json::from_str(&body).expect("toggle body");
        assert_eq!(toggle.id, id);
        let body = format!(r#"{{"core":1,"uid":"{text}"}}"#);
        let cancel: CancelOrderBody = serde_json::from_str(&body).expect("cancel body");
        assert_eq!(cancel.uid, id);
    }
    assert!(serde_json::from_str::<StrategyToggleBody>(r#"{"core":1,"id":7,"on":true}"#).is_err());
    let mut out = Vec::new();
    id_text::serialize(&7, &mut serde_json::Serializer::new(&mut out)).expect("serialize");
    assert_eq!(out, br#""7""#);
}

/// One `POST /api/session` over a real socket; returns the HTTP status code.
fn session_over_socket(port: u16, init_data: &str) -> u16 {
    use std::io::Write;
    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", port)).expect("the listener accepts");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    // Telegram sends initData URL-encoded; raw JSON quotes are not a valid header value.
    let init_data: String = init_data
        .chars()
        .map(|c| match c {
            '{' | '}' | '"' | ',' | ':' => format!("%{:02X}", c as u32),
            c => c.to_string(),
        })
        .collect();
    let request = [
        "POST /api/session HTTP/1.1".to_string(),
        "Host: 127.0.0.1".to_string(),
        "Content-Type: application/json".to_string(),
        format!("X-Telegram-Init-Data: {init_data}"),
        "Content-Length: 2".to_string(),
        "Connection: close".to_string(),
        String::new(),
        "{}".to_string(),
    ]
    .join("\r\n");
    stream.write_all(request.as_bytes()).expect("request sent");
    let mut answer = String::new();
    let _ = stream.read_to_string(&mut answer);
    answer
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .expect("an HTTP status line")
}

/// A pairing or an unpairing reaches the running listener in place (STATION.md §9, question 36:
/// restarting it for a new chat gave the tunnel a new address): the next request of a removed chat
/// is refused, and of a newly paired one admitted — on the same port.
#[test]
fn admission_changes_reach_the_running_listener() {
    let mut server = super::MiniAppServer::bind(super::MiniAppServerConfig {
        token: Secret::new(BOT_TOKEN),
        authorized_chat_ids: vec![PAIRED_USER],
    })
    .expect("loopback bind");
    let events = server.take_events().expect("event receiver");
    thread::spawn(move || {
        while let Ok(event) = events.recv() {
            accept(event);
        }
    });
    let port = server.port();
    let launch = signed_at(stable_unix_now());
    assert_eq!(session_over_socket(port, &launch), 200);
    server.update(Vec::new(), std::collections::BTreeMap::new());
    assert_eq!(session_over_socket(port, &launch), 403);
    server.update(vec![PAIRED_USER], std::collections::BTreeMap::new());
    assert_eq!(session_over_socket(port, &launch), 200);
    server.stop();
}

/// The Mini App nav is report, cores, deals, strategies, settings, and the balances tab is gone.
///
/// Mutation: put the balances section back, point the page at `/api/balances`, or reorder the
/// nav buttons. The bottom bar would open a removed screen or hide Settings.
/// Oracle: the nav buttons' `data-tab` values in document order, and the absence of the
/// balances tab name and route in `index.html` and `app.js`.
#[test]
fn mini_app_nav_is_report_cores_deals_strategies_settings() {
    const HTML: &str = include_str!("index.html");
    const JS: &str = include_str!("app.js");
    assert!(!HTML.contains("data-tab=\"balances\""));
    assert!(!JS.contains("data-tab=\"balances\""));
    assert!(!HTML.contains("/api/balances"));
    assert!(!JS.contains("/api/balances"));
    assert_eq!(
        nav_tabs(HTML),
        ["report", "cores", "deals", "strategies", "settings"]
    );
}

/// `data-tab` values of the bottom-bar buttons, in document order.
fn nav_tabs(html: &str) -> Vec<&str> {
    let start = html
        .find("<nav id=\"app-nav\"")
        .expect("index.html has #app-nav");
    let nav = &html[start..];
    let end = nav.find("</nav>").expect("nav is closed");
    let nav = &nav[..end];
    let mut tabs = Vec::new();
    let mut rest = nav;
    let needle = "data-tab=\"";
    while let Some(at) = rest.find(needle) {
        let after = &rest[at + needle.len()..];
        let quote = after.find('"').expect("data-tab is quoted");
        tabs.push(&after[..quote]);
        rest = &after[quote + 1..];
    }
    tabs
}

/// Builds a handler whose channel is not consumed, so a request that built an event is visible.
fn handler_quiet() -> (App, std::sync::mpsc::Receiver<MiniAppApiRequest>) {
    let (events_tx, events_rx) = std::sync::mpsc::sync_channel(8);
    let app = App {
        admission: Arc::new(RwLock::new(Admission {
            chats: vec![PAIRED_USER],
            labels: std::collections::BTreeMap::new(),
        })),
        token: Arc::new(Secret::new(BOT_TOKEN)),
        events_tx,
        stop: Arc::new(AtomicBool::new(false)),
    };
    (app, events_rx)
}

/// GET one path. Notify routes are POST-only.
fn get(app: &App, path: &str) -> (u16, String) {
    let request = Request::builder()
        .method(Method::GET)
        .uri(format!("http://127.0.0.1{path}"))
        .header("host", "127.0.0.1")
        .body(Body::from(Vec::new()))
        .expect("the fixture request is valid HTTP");
    let response = app.handle(request);
    let status = response.status();
    let mut reader = response.into_body().into_reader();
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .expect("the response body is readable");
    let text = String::from_utf8(bytes).expect("the response body is UTF-8 JSON");
    (status.as_u16(), text)
}

/// `POST /api/notify` and `POST /api/notify/save` refuse a bad launch, a non-object, an oversize
/// body, and a save body with no `settings`, and they do that before an event is built.
///
/// Mutation: parse the save body after `try_send`, or skip the initData check on these two arms.
/// A stranger then writes another chat's notification file, or a non-object becomes a command.
/// Oracle: the statuses below, and `try_recv` stays empty.
#[test]
fn notify_routes_reject_bad_input_before_an_event_is_built() {
    let (app, events) = handler_quiet();
    let now = stable_unix_now();
    let fresh = signed_at(now);
    let one_past = signed_at(now.saturating_sub(INIT_DATA_MAX_AGE_SECS + 1));
    let forged = with_forged_hash(&fresh);
    let now_text = now.to_string();
    let stranger = signed_init_data(&[
        ("auth_date", now_text.as_str()),
        ("query_id", "AAEAAQ"),
        ("user", "{\"id\":42,\"first_name\":\"Eve\"}"),
    ]);
    let routes = ["/api/notify", "/api/notify/save"];

    for path in routes {
        let (status, text) = get(&app, path);
        assert_eq!(status, 405, "{path} GET body {text}");
        assert_eq!(error_code(&text), Some("method"), "{path} body {text}");
        assert!(events.try_recv().is_err(), "{path} GET built an event");

        let (status, text) = post(&app, path, None, "{}");
        assert_eq!(status, 401, "{path} without initData body {text}");
        assert_eq!(error_code(&text), Some("missing_init_data"));
        assert!(
            events.try_recv().is_err(),
            "{path} missing initData built an event"
        );

        let (status, text) = post(&app, path, Some(&forged), "{}");
        assert_eq!(status, 401, "{path} forged hash body {text}");
        assert_eq!(error_code(&text), Some("hash"));
        assert!(
            events.try_recv().is_err(),
            "{path} forged hash built an event"
        );

        let (status, text) = post(&app, path, Some(&one_past), "{}");
        assert_eq!(status, 401, "{path} stale launch body {text}");
        assert_eq!(error_code(&text), Some("stale"));
        assert!(
            events.try_recv().is_err(),
            "{path} stale launch built an event"
        );

        let (status, text) = post(&app, path, Some(&stranger), "{}");
        assert_eq!(status, 403, "{path} unpaired body {text}");
        assert_eq!(error_code(&text), Some("unpaired"));
        assert_ne!(status, 200);
        assert!(events.try_recv().is_err(), "{path} unpaired built an event");

        let (status, text) = post(&app, path, Some(&fresh), "[]");
        assert_eq!(status, 400, "{path} non-object body {text}");
        assert_eq!(error_code(&text), Some("json"));
        assert!(
            events.try_recv().is_err(),
            "{path} non-object built an event"
        );

        let over = "x".repeat(usize::try_from(super::MAX_BODY_BYTES).unwrap() + 1);
        let (status, text) = post(&app, path, Some(&fresh), &over);
        assert_eq!(status, 413, "{path} oversize body {text}");
        assert_eq!(error_code(&text), Some("body"));
        assert!(events.try_recv().is_err(), "{path} oversize built an event");
    }

    let (status, text) = post(&app, "/api/notify/save", Some(&fresh), "{}");
    assert_eq!(status, 400, "save without settings body {text}");
    assert_eq!(error_code(&text), Some("json"));
    assert!(
        events.try_recv().is_err(),
        "missing settings built an event"
    );

    let (status, text) = post(&app, "/api/notify/save", Some(&fresh), r#"{"settings":{}}"#);
    assert_eq!(status, 400, "save without revision body {text}");
    assert_eq!(error_code(&text), Some("json"));
    assert!(
        events.try_recv().is_err(),
        "missing revision built an event"
    );

    let (status, text) = post(
        &app,
        "/api/notify/save",
        Some(&fresh),
        r#"{"settings":{},"revision":0,"extra":1}"#,
    );
    assert_eq!(status, 400, "save with an unknown field body {text}");
    assert_eq!(error_code(&text), Some("json"));
    assert!(
        events.try_recv().is_err(),
        "an unknown field built an event"
    );
}

/// A fresh paired launch reaches both notify routes. They are not hard-coded rejects.
///
/// Mutation: answer 403 before the consumer, or leave the new variants out of `accept`.
/// The page then cannot load or store settings. Oracle: 200 and the acceptor's document.
#[test]
fn notify_routes_accept_a_fresh_paired_launch() {
    let app = handler_with_acceptor();
    let fresh = signed_at(stable_unix_now());
    let expected = serde_json::to_string(&notify_ok()).expect("acceptor document");

    let (status, text) = post(&app, "/api/notify", Some(&fresh), "{}");
    assert_eq!(status, 200, "read body {text}");
    assert_eq!(text, expected);

    let (status, text) = post(
        &app,
        "/api/notify/save",
        Some(&fresh),
        r#"{"settings":{},"revision":0}"#,
    );
    assert_eq!(status, 200, "save body {text}");
    assert_eq!(text, expected);
}

/// Compact JSON the Mini App reads. Field order is the struct order `serde_json::to_vec` writes.
///
/// Mutation: skip `error` when it is null, or rename a settings field. The form then misses a
/// key or reads a different default. Oracle: these exact strings.
#[test]
fn notify_dto_default_json_is_the_compact_document() {
    use crate::telegram::notify::{CoreScope, NotifySettings};

    let settings = NotifySettings::default();
    assert_eq!(
        serde_json::to_string(&settings).expect("settings"),
        r#"{"trades":{"on":false,"cores":{"kind":"all"},"min_volume_usd":null,"profit_at_least_usd":null,"loss_at_least_usd":null},"down":{"on":false,"after_minutes":5},"daily":{"on":false,"hour":21,"minute":0}}"#
    );
    assert_eq!(
        serde_json::to_string(&CoreScope::Only(vec![1, 2])).expect("only"),
        r#"{"kind":"only","ids":[1,2]}"#
    );

    let dto = super::dto::NotifyDto {
        settings,
        cores: vec![super::dto::NotifyCoreDto {
            id: 1,
            name: "A".into(),
            exchange: "Binance".into(),
        }],
        zone: "UTC".into(),
        revision: 0,
        error: None,
        fault: None,
    };
    assert_eq!(
        serde_json::to_string(&dto).expect("dto"),
        r#"{"settings":{"trades":{"on":false,"cores":{"kind":"all"},"min_volume_usd":null,"profit_at_least_usd":null,"loss_at_least_usd":null},"down":{"on":false,"after_minutes":5},"daily":{"on":false,"hour":21,"minute":0}},"cores":[{"id":1,"name":"A","exchange":"Binance"}],"zone":"UTC","revision":0,"error":null,"fault":null}"#
    );
}
