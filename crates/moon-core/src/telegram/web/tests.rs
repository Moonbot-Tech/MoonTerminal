//! HTTP regressions for Mini App cancel and panic writes.
//!
//! The oracle is Telegram's documented initData contract plus the write-route
//! limits: a launch older than one hour is not a credential, a body field the
//! route does not name is not a command, and a market longer than 64 bytes is
//! not sent. Status codes come from those rules, not from a value this module
//! computed and then read back.

use std::io::Read;
use std::sync::{Arc, atomic::AtomicBool};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use sha2::Sha256;
use touche::body::HttpBody;
use touche::{Body, Method, Request, StatusCode};

use super::{App, MiniAppApiError, MiniAppApiRequest};
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
        labels: Arc::new(std::collections::BTreeMap::new()),
        token: Arc::new(Secret::new(BOT_TOKEN)),
        authorized_chat_ids: Arc::new(vec![PAIRED_USER]),
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
        MiniAppApiRequest::Balances { reply, .. } => {
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
                error: None,
            }));
        }
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
        ("/api/order/cancel", r#"{"core":1,"uid":2}"#),
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
        r#"{"core":1,"uid":2}"#,
    );
    assert_eq!(
        status, 200,
        "a named cancel body must reach the command, body {text}"
    );

    let (status, text) = post(
        &app,
        "/api/order/cancel",
        Some(&init_data),
        r#"{"core":1,"uid":2,"note":1}"#,
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
        ("/api/strategy/toggle", r#"{"core":1,"id":7,"on":true}"#),
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
