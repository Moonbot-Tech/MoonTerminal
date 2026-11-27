//! The tunnel's reading of OpenAI's wire contract: polls, deadlines, the caller, the answer.

use super::*;

/// The multi-valued header map a command carries, as OpenAI sends it.
fn headers(pairs: &[(&str, &str)]) -> BTreeMap<String, Vec<String>> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), vec![(*value).to_owned()]))
        .collect()
}

/// 204 and 200 both mean the key works; only 401/403 and 404 wait for the owner; the rest retry.
#[test]
fn a_poll_status_decides_what_the_loop_does() {
    assert!(matches!(classify_poll(204, None, None), Polled::Commands(c) if c.is_empty()));
    let body = r#"{"commands":[{"request_id":"req_1","shard_token":"s","command_type":"jsonrpc","jsonrpc":{"jsonrpc":"2.0","id":1,"method":"tools/list"},"future_field":true}]}"#;
    match classify_poll(200, Some(body), None) {
        Polled::Commands(commands) => {
            assert_eq!(commands.len(), 1);
            assert_eq!(commands[0].request_id, "req_1");
            assert_eq!(commands[0].channel, None, "absent channel is not invented");
        }
        _ => panic!("a batch is commands"),
    }
    assert!(matches!(
        classify_poll(200, Some("{not json"), None),
        Polled::Retry { .. }
    ));
    assert!(matches!(
        classify_poll(401, None, None),
        Polled::KeyRejected
    ));
    assert!(matches!(
        classify_poll(403, None, None),
        Polled::KeyRejected
    ));
    assert!(matches!(classify_poll(404, None, None), Polled::NotFound));
    let wait = Some(Duration::from_secs(5));
    assert!(matches!(
        classify_poll(429, None, wait),
        Polled::Retry { retry_after } if retry_after == wait
    ));
    assert!(matches!(
        classify_poll(503, None, None),
        Polled::Retry { .. }
    ));
    assert_eq!(retry_after("7"), Some(Duration::from_secs(7)));
    assert_eq!(retry_after("100000"), Some(MAX_BACKOFF), "capped");
    assert_eq!(retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
}

/// The protocol's grammar exactly: one integer, one lowercase unit; anything else fails open.
#[test]
fn a_response_timeout_parses_only_the_protocol_grammar() {
    let parse = |text: &str| response_timeout(Some(&json!(text)));
    assert_eq!(parse("30s"), Some(Duration::from_secs(30)));
    assert_eq!(parse("4500ms"), Some(Duration::from_millis(4500)));
    assert_eq!(parse("2m"), Some(Duration::from_secs(120)));
    assert_eq!(parse("0s"), Some(Duration::ZERO));
    for bad in [
        "4.5s", "-1s", "1m30s", " 1s", "1s ", "1e3s", "30d", "s", "30",
    ] {
        assert_eq!(parse(bad), None, "{bad}");
    }
    assert_eq!(
        response_timeout(Some(&json!(30))),
        None,
        "a number is not the wire form"
    );
    assert_eq!(response_timeout(None), None);
    assert_eq!(parse("999999999999999999999h"), None, "overflow fails open");
}

/// The caller is the subject header, whatever its case; an implausible value is no caller.
#[test]
fn the_caller_is_the_subject_header() {
    let subject_of = |pairs: &[(&str, &str)]| subject(&headers(pairs));
    assert_eq!(
        subject_of(&[("X-Openai-Subject", "v1/abc")]).as_deref(),
        Some("v1/abc")
    );
    assert_eq!(
        subject_of(&[("x-openai-subject", "v1/abc")]).as_deref(),
        Some("v1/abc")
    );
    assert_eq!(subject_of(&[("X-Openai-Session", "v1/abc")]), None);
    assert_eq!(subject_of(&[("X-Openai-Subject", "")]), None);
    assert_eq!(subject_of(&[("X-Openai-Subject", "v1/a b")]), None);
    assert_eq!(subject_of(&[("X-Openai-Subject", &"x".repeat(257))]), None);
}

/// Only the MCP protocol's own headers reach the service, and the caller rides as an extension.
#[test]
fn the_mcp_request_carries_mcp_headers_and_the_caller_only() {
    let pairs = [
        ("Mcp-Protocol-Version", "2025-11-25"),
        ("X-Openai-Subject", "v1/abc"),
        ("Traceparent", "00-1-2-00"),
    ];
    let request = mcp_request(
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        &headers(&pairs),
        Caller::Guest(TunnelGuest {
            subject: Some("v1/abc".into()),
        }),
    )
    .unwrap();
    assert_eq!(request.uri(), "/mcp");
    assert_eq!(request.headers()[header::HOST], http::TUNNEL_HOST);
    assert_eq!(request.headers()["mcp-protocol-version"], "2025-11-25");
    assert!(request.headers().get("x-openai-subject").is_none());
    assert!(request.headers().get("traceparent").is_none());
    let guest = request
        .extensions()
        .get::<TunnelGuest>()
        .expect("guest mark");
    assert_eq!(guest.subject.as_deref(), Some("v1/abc"));
    assert!(request.extensions().get::<auth::Principal>().is_none());
}

/// A request's answer is its JSON-RPC body; a notification is acknowledged without one; a body
/// that is not JSON becomes a synthesized gateway error carrying the request's id.
#[test]
fn an_answer_follows_the_response_discriminators() {
    let mut mcp_headers = axum::http::HeaderMap::new();
    mcp_headers.insert("content-type", HeaderValue::from_static("application/json"));
    mcp_headers.insert("x-internal", HeaderValue::from_static("dropped"));
    let request = json!({"jsonrpc":"2.0","id":7,"method":"tools/list"});
    let body = br#"{"jsonrpc":"2.0","id":7,"result":{"tools":[]}}"#;
    let reply = answer(&request, 200, &mcp_headers, body);
    assert_eq!(reply.resp_type, "jsonrpc_response");
    assert_eq!(reply.resp_code, 200);
    assert_eq!(
        reply.resp_json.as_ref().unwrap()["result"]["tools"],
        json!([])
    );
    assert_eq!(
        reply.resp_headers,
        BTreeMap::from([(
            "Content-Type".to_owned(),
            vec!["application/json".to_owned()]
        )])
    );
    let notification = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
    let ack = answer(&notification, 202, &axum::http::HeaderMap::new(), b"");
    assert_eq!(ack.resp_type, "notify_ack");
    assert_eq!(ack.resp_json, None);
    let broken = answer(&request, 500, &axum::http::HeaderMap::new(), b"oops");
    let error = broken.resp_json.unwrap();
    assert_eq!(error["id"], 7);
    assert_eq!(error["error"]["code"], -32603);
}

/// The MCP service's state with an enabled, empty grant store and no owner thread behind it.
fn shared() -> Arc<Shared> {
    let path = std::env::temp_dir().join(format!(
        "moon-mcp-tunnel-{}.json",
        auth::random().expect("random")
    ));
    let mut store = auth::Auth::open(path).expect("auth store");
    store
        .admin(
            moon_core::station_api::mcp::AdminRequest::SetEnabled { enabled: true },
            &[7],
            auth::now(),
        )
        .expect("enable");
    // The receiver is dropped: these tests never reach a tool the owner thread would run.
    let (calls, _) = std::sync::mpsc::sync_channel(1);
    Arc::new(Shared {
        origin: String::new(),
        auth: Arc::new(Mutex::new(store)),
        calls,
        owner: std::thread::current(),
        knowledge: Arc::new(std::sync::RwLock::new(
            super::super::knowledge::Snapshot::default(),
        )),
        http_slots: Arc::new(Semaphore::new(8)),
        report_slots: Arc::new(Semaphore::new(2)),
        site: Arc::new(super::super::site::Client::new()),
        site_slots: Arc::new(Semaphore::new(1)),
    })
}

/// Through the real MCP service: an unpaired ChatGPT account lists the tools, as adding the
/// connector needs, and a tool call returns the pairing code instead of running anything.
#[tokio::test]
async fn an_unpaired_account_lists_tools_and_is_given_a_code() {
    let shared = shared();
    let service = http::tunnel_service(shared.clone(), CancellationToken::new());
    let account = headers(&[
        ("X-Openai-Subject", "v1/abc"),
        ("Mcp-Protocol-Version", "2025-11-25"),
    ]);
    let list = json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}});
    let listed = serve(&list, &account, &service, &shared).await;
    assert_eq!(listed.resp_code, 200, "{:?}", listed.resp_json);
    let tools = listed.resp_json.unwrap()["result"]["tools"].clone();
    assert!(tools.as_array().is_some_and(|tools| !tools.is_empty()));
    let call = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"station_overview","arguments":{}}});
    let called = serve(&call, &account, &service, &shared).await;
    let text = called.resp_json.unwrap().to_string();
    let code = shared
        .auth
        .lock()
        .unwrap()
        .tunnel_pairing("v1/abc", auth::now())
        .unwrap();
    assert!(text.contains(&code), "{text}");
    assert!(
        text.contains("isError"),
        "a tool error, not a result: {text}"
    );
    // Without any subject there is nobody to pair: no code is issued.
    let anonymous = serve(&call, &headers(&[]), &service, &shared).await;
    assert!(
        anonymous
            .resp_json
            .unwrap()
            .to_string()
            .contains("no ChatGPT account identity")
    );
}

/// The live run's log: every station line to stderr, so the tunnel's own account is visible.
struct Stderr;

impl log::Log for Stderr {
    /// The station's own lines, down to trace; the libraries' are left out.
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target().starts_with("moon_station")
    }

    /// One line per record, with its level, to stderr and to `moon-tunnel-live.log` in the
    /// temporary directory, where the run can be read after it ends.
    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!("[{}] {}", record.level(), record.args());
        eprintln!("{line}");
        use std::io::Write as _;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(std::env::temp_dir().join("moon-tunnel-live.log"))
        {
            let _ = writeln!(file, "{line}");
        }
    }

    /// Unbuffered.
    fn flush(&self) {}
}

/// The live run's logger instance.
static STDERR: Stderr = Stderr;

/// Live, against OpenAI: hold the tunnel named by `MOON_TUNNEL_ID` with the key in
/// `MOON_TUNNEL_KEY` for `MOON_TUNNEL_SECS` seconds (300 by default), printing the tunnel's state
/// and any pairing code. A ChatGPT connector on that tunnel should list the tools and be told to
/// enter the code. Run by hand: `cargo test -p moon-station live_openai_tunnel -- --ignored
/// --nocapture`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs a real OpenAI tunnel and its key"]
async fn live_openai_tunnel() {
    let id = std::env::var("MOON_TUNNEL_ID").expect("MOON_TUNNEL_ID");
    let key = Secret::new(std::env::var("MOON_TUNNEL_KEY").expect("MOON_TUNNEL_KEY"));
    let seconds = std::env::var("MOON_TUNNEL_SECS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(300);
    log::set_logger(&STDERR).expect("one logger per live run");
    log::set_max_level(log::LevelFilter::Trace);
    let shared = shared();
    let state = Arc::new(Mutex::new(TunnelState {
        tunnel_id: id.clone(),
        ..TunnelState::default()
    }));
    let stop = CancellationToken::new();
    let running = tokio::spawn(run(shared.clone(), id, key, state.clone(), stop.clone()));
    let until = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < until {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let tunnel = state.lock().unwrap().clone();
        let pending = shared.auth.lock().unwrap().status().pending.len();
        println!(
            "online={} problem={:?} pending={pending}",
            tunnel.online, tunnel.problem
        );
    }
    stop.cancel();
    let _ = running.await;
}

/// ChatGPT's connector setup, request for request as it arrived through a real tunnel
/// (2026-11-27), from an account not paired yet: every step must succeed, or ChatGPT refuses to
/// create the connector. The view read at the end once failed the whole setup.
#[tokio::test]
async fn chatgpt_connector_setup_succeeds_for_an_unpaired_account() {
    let shared = shared();
    let service = http::tunnel_service(shared.clone(), CancellationToken::new());
    let capabilities = json!({"experimental":{"openai/visibility":{"enabled":true}},"extensions":{"io.modelcontextprotocol/ui":{"mimeTypes":["text/html;profile=mcp-app"]}}});
    let steps = [
        (
            headers(&[
                ("Mcp-Method", "server/discover"),
                ("Mcp-Protocol-Version", "2026-07-28"),
            ]),
            json!({"jsonrpc":"2.0","id":"openai-mcp-discover","method":"server/discover","params":{"_meta":{
                "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                "io.modelcontextprotocol/clientInfo":{"name":"openai-mcp","version":"1.0.0"},
                "io.modelcontextprotocol/clientCapabilities":capabilities}}}),
        ),
        (
            headers(&[]),
            json!({"jsonrpc":"2.0","id":-1,"method":"initialize","params":{"protocolVersion":"2025-11-25","clientInfo":{"name":"openai-mcp","version":"1.0.0"},"capabilities":capabilities}}),
        ),
        (
            headers(&[("Mcp-Protocol-Version", "2025-11-25")]),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        ),
        (
            headers(&[("Mcp-Protocol-Version", "2025-11-25")]),
            json!({"jsonrpc":"2.0","id":0,"method":"tools/list"}),
        ),
        (
            headers(&[("Mcp-Protocol-Version", "2025-11-25")]),
            json!({"jsonrpc":"2.0","id":1,"method":"resources/read","params":{"uri":super::super::tools::clip_ui()}}),
        ),
    ];
    for (step_headers, body) in steps {
        let reply = serve(&body, &step_headers, &service, &shared).await;
        let method = &body["method"];
        assert!(reply.resp_code < 300, "{method}: {}", reply.resp_code);
        if let Some(json) = &reply.resp_json {
            assert!(json.get("error").is_none(), "{method}: {json}");
        }
    }
}
