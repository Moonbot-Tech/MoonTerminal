//! Loopback HTTP: OAuth discovery, browser consent and the official MCP transport behind Caddy.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Form, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::{Engine, engine::general_purpose::STANDARD};
use moon_core::station_api::mcp::Scope;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{Shared, auth, tools};

/// OAuth vocabulary, deliberately identical to the private owner permission selectors.
const SCOPES: [&str; 4] = ["read", "strategies", "trading", "settings"];
const COOKIE: &str = "__Host-moonstation-consent";

/// Build one bounded router; only the MCP endpoint accepts bearer tokens as bot authority.
pub(super) fn router(shared: Arc<Shared>, stop: CancellationToken) -> Router {
    let factory = shared.clone();
    let host = shared.origin.trim_start_matches("https://").to_owned();
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_allowed_hosts([host])
        .with_allowed_origins([format!("{}:443", shared.origin)])
        .with_max_request_body_bytes(64 * 1024)
        .with_cancellation_token(stop);
    let service = StreamableHttpService::new(
        move || Ok(tools::Server(factory.clone())),
        Arc::new(NeverSessionManager::default()),
        config,
    );
    let mcp = Router::new()
        .route_service("/mcp", service)
        .route_layer(middleware::from_fn_with_state(shared.clone(), bearer));
    Router::new()
        .merge(mcp)
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(resource_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource",
            get(resource_metadata),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(authorization_metadata),
        )
        .route("/oauth/register", post(register))
        .route("/oauth/authorize", get(authorize))
        .route("/oauth/continue", get(continue_authorization))
        .route("/oauth/token", post(token))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn_with_state(shared.clone(), boundary))
        .with_state(shared)
}

/// Reject unexpected hosts/origins on OAuth too, and attach browser isolation/no-cache headers.
async fn boundary(State(shared): State<Arc<Shared>>, request: Request, next: Next) -> Response {
    let expected = shared.origin.trim_start_matches("https://");
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok());
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    if host != Some(expected) && host != Some(&format!("{expected}:443")) {
        return StatusCode::MISDIRECTED_REQUEST.into_response();
    }
    if origin
        .is_some_and(|origin| origin != shared.origin && origin != format!("{}:443", shared.origin))
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(_permit) = shared.http_slots.clone().try_acquire_owned() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let mut response = next.run(request).await;
    for (name, value) in [
        ("cache-control", "no-store"),
        ("pragma", "no-cache"),
        ("referrer-policy", "no-referrer"),
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        (
            "content-security-policy",
            "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        ),
    ] {
        response
            .headers_mut()
            .insert(name, HeaderValue::from_static(value));
    }
    response
}

/// A fresh bearer check precedes every MCP HTTP operation, including initialization and tool lists.
async fn bearer(State(shared): State<Arc<Shared>>, mut request: Request, next: Next) -> Response {
    let raw = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let who = raw.filter(|v| v.len() <= 128).and_then(|v| {
        shared
            .auth
            .lock()
            .ok()
            .and_then(|mut auth| auth.bearer(v, auth::now()).ok())
    });
    let Some(who) = who else {
        let mut response = StatusCode::UNAUTHORIZED.into_response();
        if let Ok(value) = HeaderValue::from_str(&format!(
            "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource/mcp\", scope=\"read strategies trading settings\"",
            shared.origin
        )) {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, value);
        }
        return response;
    };
    request.extensions_mut().insert(who);
    next.run(request).await
}

/// RFC 9728 discovery binds tokens to this station's canonical MCP resource.
async fn resource_metadata(State(shared): State<Arc<Shared>>) -> Json<Value> {
    Json(
        json!({"resource":shared.resource(),"authorization_servers":[shared.origin],"scopes_supported":SCOPES,"bearer_methods_supported":["header"]}),
    )
}

/// Advertise only implemented OAuth grants and S256; no password login or implicit grant exists.
async fn authorization_metadata(State(shared): State<Arc<Shared>>) -> Json<Value> {
    Json(
        json!({"issuer":shared.origin,"authorization_endpoint":format!("{}/oauth/authorize",shared.origin),
        "token_endpoint":format!("{}/oauth/token",shared.origin),"registration_endpoint":format!("{}/oauth/register",shared.origin),
        "response_types_supported":["code"],"grant_types_supported":["authorization_code","refresh_token"],
        "code_challenge_methods_supported":["S256"],"token_endpoint_auth_methods_supported":["none","client_secret_post","client_secret_basic"],
        "scopes_supported":SCOPES}),
    )
}

/// RFC 7591 client metadata; identity strings are labels, not proof of a provider's identity.
#[derive(Deserialize)]
struct Registration {
    client_name: Option<String>,
    redirect_uris: Vec<String>,
    token_endpoint_auth_method: Option<String>,
    grant_types: Option<Vec<String>>,
    response_types: Option<Vec<String>>,
}

/// Registration creates no grants and is bounded before persistence.
async fn register(State(shared): State<Arc<Shared>>, Json(input): Json<Registration>) -> Response {
    if input.grant_types.as_ref().is_some_and(|v| {
        v.iter()
            .any(|s| s != "authorization_code" && s != "refresh_token")
    }) || input
        .response_types
        .as_ref()
        .is_some_and(|v| v != &["code"])
    {
        return oauth_error("invalid_client_metadata", "unsupported OAuth flow");
    }
    let result = shared
        .auth
        .lock()
        .map_err(|_| anyhow::anyhow!("authorization unavailable"))
        .and_then(|mut auth| {
            auth.register(
                input.client_name.unwrap_or_else(|| "AI client".into()),
                input.redirect_uris,
                input
                    .token_endpoint_auth_method
                    .unwrap_or_else(|| "client_secret_basic".into()),
                auth::now(),
            )
        });
    match result {
        Ok((client, secret)) => (StatusCode::CREATED, Json(json!({"client_id":client.id,"client_name":client.name,"redirect_uris":client.redirects,
            "client_secret":secret,"client_secret_expires_at":0,"token_endpoint_auth_method":client.method,
            "grant_types":["authorization_code","refresh_token"],"response_types":["code"]}))).into_response(),
        Err(e) => oauth_error("invalid_client_metadata", &e.to_string()),
    }
}

/// Reject unknown scopes instead of silently turning a misspelled permission into authority.
fn parse_scopes(raw: Option<&String>) -> anyhow::Result<Vec<Scope>> {
    raw.map(String::as_str)
        .unwrap_or("read")
        .split_whitespace()
        .map(|s| match s {
            "read" => Ok(Scope::Read),
            "strategies" => Ok(Scope::Strategies),
            "trading" => Ok(Scope::Trading),
            "settings" => Ok(Scope::Settings),
            _ => anyhow::bail!("unknown scope"),
        })
        .collect()
}

/// Required OAuth resource indicator prevents a token issued for one service being used elsewhere.
fn check_resource(shared: &Shared, fields: &BTreeMap<String, String>) -> anyhow::Result<()> {
    anyhow::ensure!(
        fields.get("resource") == Some(&shared.resource()),
        "resource must equal the station MCP URL"
    );
    Ok(())
}

/// Display only a local approval code; browser access never approves its own request.
async fn authorize(
    State(shared): State<Arc<Shared>>,
    Query(fields): Query<BTreeMap<String, String>>,
) -> Response {
    let result = (|| -> anyhow::Result<_> {
        check_resource(&shared, &fields)?;
        anyhow::ensure!(
            fields.get("response_type").map(String::as_str) == Some("code")
                && fields.get("code_challenge_method").map(String::as_str) == Some("S256"),
            "authorization code with S256 is required"
        );
        let browser = auth::random()?;
        let required = |name| {
            fields
                .get(name)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("missing {name}"))
        };
        let (id, code) = shared
            .auth
            .lock()
            .map_err(|_| anyhow::anyhow!("authorization unavailable"))?
            .begin(
                required("client_id")?,
                required("redirect_uri")?,
                required("code_challenge")?,
                fields.get("state").cloned().unwrap_or_default(),
                parse_scopes(fields.get("scope"))?,
                &browser,
                auth::now(),
            )?;
        Ok((id, code, browser))
    })();
    match result {
        Ok((id, code, browser)) => {
            let mut response = page(&id, &code).into_response();
            if let Ok(cookie) = HeaderValue::from_str(&format!(
                "{COOKIE}={browser}; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age=600"
            )) {
                response.headers_mut().insert(header::SET_COOKIE, cookie);
            }
            response
        }
        Err(e) => oauth_error("invalid_request", &e.to_string()),
    }
}

/// A cookie-bound poll finishes only the owner-approved flow and never accepts a redirect argument.
async fn continue_authorization(
    State(shared): State<Arc<Shared>>,
    headers: HeaderMap,
    Query(fields): Query<BTreeMap<String, String>>,
) -> Response {
    let id = fields.get("request").map(String::as_str).unwrap_or("");
    let browser = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.split(';')
                .find_map(|pair| pair.trim().strip_prefix(&format!("{COOKIE}=")))
        })
        .unwrap_or("");
    let result = shared
        .auth
        .lock()
        .map_err(|_| anyhow::anyhow!("authorization unavailable"))
        .and_then(|mut auth| auth.continue_browser(id, browser, auth::now()));
    match result {
        Ok(Some(url)) => Redirect::to(&url).into_response(),
        Ok(None) => {
            let code = shared
                .auth
                .lock()
                .ok()
                .and_then(|auth| auth.browser_code(id, browser).map(str::to_owned))
                .unwrap_or_default();
            page(id, &code).into_response()
        }
        Err(e) => oauth_error("access_denied", &e.to_string()),
    }
}

/// Static consent screen uses no scripts or third-party assets. Only generated codes enter HTML.
fn page(id: &str, code: &str) -> Html<String> {
    // IDs come from the registry on first display; subsequent query values must be HTML escaped.
    let id = id
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    Html(format!(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta http-equiv=\"refresh\" content=\"3;url=/oauth/continue?request={id}\"><title>MoonStation — connect AI</title><style>body{{font:18px system-ui;max-width:42rem;margin:12vh auto;padding:24px;background:#101722;color:#eef3f9}}code{{font-size:32px;letter-spacing:3px}}p{{line-height:1.5}}</style><h1>Connect AI to MoonStation</h1><p>In MoonTerminal, open Station → AI access. Enter this code, select the cores and permissions, then approve:</p><p><code>{code}</code></p><p>Approve only a connection you started yourself. The terminal is the only place that can grant access. This page will continue automatically.</p></html>"
    ))
}

/// Parse confidential-client authentication without allowing two competing credential sources.
fn client_auth(
    headers: &HeaderMap,
    fields: &BTreeMap<String, String>,
) -> anyhow::Result<(String, Option<String>, &'static str)> {
    if let Some(basic) = headers.get(header::AUTHORIZATION) {
        anyhow::ensure!(
            !fields.contains_key("client_secret"),
            "multiple authentication methods"
        );
        let encoded = basic
            .to_str()?
            .strip_prefix("Basic ")
            .ok_or_else(|| anyhow::anyhow!("unsupported client authentication"))?;
        let decoded = String::from_utf8(STANDARD.decode(encoded)?)?;
        let (id, secret) = decoded
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("invalid basic authentication"))?;
        // OAuth Basic encodes each credential with application/x-www-form-urlencoded first.
        let decode = |value: &str| {
            url::form_urlencoded::parse(format!("v={value}").as_bytes())
                .next()
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default()
        };
        return Ok((decode(id), Some(decode(secret)), "client_secret_basic"));
    }
    let id = fields
        .get("client_id")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("client_id is required"))?;
    let secret = fields.get("client_secret").cloned();
    let method = if secret.is_some() {
        "client_secret_post"
    } else {
        "none"
    };
    Ok((id, secret, method))
}

/// The only token flows are bound code exchange and rotating refresh; responses are never cached.
async fn token(
    State(shared): State<Arc<Shared>>,
    headers: HeaderMap,
    Form(fields): Form<BTreeMap<String, String>>,
) -> Response {
    let result = (|| -> anyhow::Result<_> {
        check_resource(&shared, &fields)?;
        let (client, secret, method) = client_auth(&headers, &fields)?;
        let mut auth = shared
            .auth
            .lock()
            .map_err(|_| anyhow::anyhow!("authorization unavailable"))?;
        auth.authenticate_client(&client, secret.as_deref(), method)?;
        let get = |name| fields.get(name).map(String::as_str).unwrap_or("");
        match get("grant_type") {
            "authorization_code" => auth.exchange(
                &client,
                get("code"),
                get("redirect_uri"),
                get("code_verifier"),
                auth::now(),
            ),
            "refresh_token" => auth.refresh(&client, get("refresh_token"), auth::now()),
            _ => anyhow::bail!("unsupported grant type"),
        }
    })();
    match result {
        Ok(tokens) => Json(tokens).into_response(),
        Err(e) => oauth_error("invalid_grant", &e.to_string()),
    }
}

/// OAuth errors never echo request bodies, authorization headers or credentials.
fn oauth_error(code: &str, description: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error":code,"error_description":description})),
    )
        .into_response()
}

#[cfg(test)]
mod tests;
