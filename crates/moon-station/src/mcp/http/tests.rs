//! Exercise discovery, browser consent, token exchange and MCP authorization through real routers.

use super::*;
use axum::body::{Body, to_bytes};
use std::sync::{Mutex, mpsc};
use tower::ServiceExt;

/// Isolated application state with no core connections and no external knowledge downloads.
struct Fixture {
    shared: Arc<Shared>,
    router: Router,
    path: std::path::PathBuf,
    _calls: mpsc::Receiver<super::super::Call>,
}

impl Fixture {
    /// Enable only through the private store before exposing the HTTP surface.
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "moon-mcp-http-{}.json",
            auth::random().expect("random")
        ));
        let mut store = auth::Auth::open(path.clone()).expect("auth store");
        store
            .admin(
                moon_core::station_api::mcp::AdminRequest::SetEnabled { enabled: true },
                &[7],
                auth::now(),
            )
            .expect("enable");
        let (calls, receiver) = mpsc::sync_channel(4);
        let shared = Arc::new(Shared {
            origin: "https://station.example".into(),
            auth: Arc::new(Mutex::new(store)),
            calls,
            owner: std::thread::current(),
            knowledge: Arc::new(std::sync::RwLock::new(
                super::super::knowledge::Snapshot::default(),
            )),
            http_slots: Arc::new(tokio::sync::Semaphore::new(8)),
            report_slots: Arc::new(tokio::sync::Semaphore::new(2)),
        });
        Self {
            router: router(shared.clone(), CancellationToken::new()),
            shared,
            path,
            _calls: receiver,
        }
    }

    /// All test requests traverse host/origin and authentication middleware.
    async fn request(
        &self,
        method: &str,
        uri: &str,
        body: String,
        headers: &[(&str, &str)],
    ) -> (StatusCode, HeaderMap, String) {
        let mut request = axum::http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "station.example");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = self
            .router
            .clone()
            .oneshot(request.body(Body::from(body)).expect("request"))
            .await
            .expect("router");
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("body");
        (
            status,
            headers,
            String::from_utf8(bytes.to_vec()).expect("UTF8"),
        )
    }
}

impl Drop for Fixture {
    /// Credentials are test-local and never retained after a test completes.
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[tokio::test]
async fn oauth_consent_unlocks_mcp_and_revocation_blocks_it_again() {
    let f = Fixture::new();
    let (status, headers, _) = f
        .request(
            "POST",
            "/mcp",
            "{}".into(),
            &[("content-type", "application/json")],
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(
        headers["www-authenticate"]
            .to_str()
            .expect("challenge")
            .contains("oauth-protected-resource/mcp")
    );
    let requested_scopes = headers["www-authenticate"]
        .to_str()
        .unwrap()
        .split("scope=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned();
    let (status,_,body)=f.request("POST","/oauth/register",json!({"client_name":"Test AI","redirect_uris":["https://ai.example/callback"],"token_endpoint_auth_method":"none"}).to_string(),&[("content-type","application/json")]).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let client: Value = serde_json::from_str(&body).expect("client JSON");
    let verifier = "x".repeat(43);
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("response_type", "code"),
            ("client_id", client["client_id"].as_str().expect("id")),
            ("redirect_uri", "https://ai.example/callback"),
            ("resource", "https://station.example/mcp"),
            ("code_challenge_method", "S256"),
            ("code_challenge", &auth::hash(&verifier)),
            ("scope", requested_scopes.as_str()),
            ("state", "safe-state"),
        ])
        .finish();
    let (status, headers, body) = f
        .request(
            "GET",
            &format!("/oauth/authorize?{query}"),
            String::new(),
            &[],
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let code = body
        .split("<code>")
        .nth(1)
        .and_then(|s| s.split("</code>").next())
        .expect("owner code");
    let cookie = headers["set-cookie"]
        .to_str()
        .expect("cookie")
        .split(';')
        .next()
        .expect("pair");
    let pending = f.shared.auth.lock().expect("lock").status().pending[0]
        .request_id
        .clone();
    let (status, _, _) = f
        .request(
            "GET",
            &format!("/oauth/continue?request={pending}"),
            String::new(),
            &[],
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    f.shared
        .auth
        .lock()
        .expect("lock")
        .admin(
            moon_core::station_api::mcp::AdminRequest::Approve {
                code: code.into(),
                cores: vec![7],
                scopes: vec![Scope::Read, Scope::Trading],
            },
            &[7],
            auth::now(),
        )
        .expect("private approval");
    let (status, headers, _) = f
        .request(
            "GET",
            &format!("/oauth/continue?request={pending}"),
            String::new(),
            &[("cookie", cookie)],
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let redirect = url::Url::parse(headers["location"].to_str().expect("location")).expect("URL");
    let fields: BTreeMap<_, _> = redirect.query_pairs().into_owned().collect();
    assert_eq!(fields["state"], "safe-state");
    let form = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("grant_type", "authorization_code"),
            ("client_id", client["client_id"].as_str().expect("id")),
            ("redirect_uri", "https://ai.example/callback"),
            ("code", fields["code"].as_str()),
            ("code_verifier", verifier.as_str()),
            ("resource", "https://station.example/mcp"),
        ])
        .finish();
    let (status, _, body) = f
        .request(
            "POST",
            "/oauth/token",
            form,
            &[("content-type", "application/x-www-form-urlencoded")],
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let tokens: Value = serde_json::from_str(&body).expect("tokens");
    assert_eq!(tokens["scope"], "read trading");
    let bearer = format!(
        "Bearer {}",
        tokens["access_token"].as_str().expect("access")
    );
    let headers = [
        ("authorization", bearer.as_str()),
        ("content-type", "application/json"),
        ("accept", "application/json, text/event-stream"),
        ("mcp-protocol-version", "2025-11-25"),
    ];
    let (status,_,body)=f.request("POST","/mcp",json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}).to_string(),&headers).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _, body) = f
        .request(
            "POST",
            "/mcp",
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}).to_string(),
            &headers,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains("action_preview") && body.contains("logs_start"),
        "{body}"
    );
    let grant = f.shared.auth.lock().expect("lock").status().grants[0]
        .id
        .clone();
    f.shared
        .auth
        .lock()
        .expect("lock")
        .admin(
            moon_core::station_api::mcp::AdminRequest::Revoke { grant_id: grant },
            &[7],
            auth::now(),
        )
        .expect("revoke");
    assert_eq!(
        f.request("POST", "/mcp", "{}".into(), &headers).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn foreign_browser_origins_cannot_register_or_authorize() {
    let f = Fixture::new();
    let (status, _, _) = f
        .request(
            "POST",
            "/oauth/register",
            "{}".into(),
            &[
                ("content-type", "application/json"),
                ("origin", "https://evil.example"),
            ],
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = f
        .request(
            "POST",
            "/oauth/approve",
            "{}".into(),
            &[("content-type", "application/json")],
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
