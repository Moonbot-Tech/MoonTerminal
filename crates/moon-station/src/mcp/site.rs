//! The MoonTerminal site's search API, for questions about the terminal itself.
//!
//! MoonDoc (`knowledge.rs`) knows the core: parameters, concepts, strategy examples. Where a button
//! is, how to connect a core or what a version changed is on the site — the FAQ, the interface map
//! and the release history — and the site answers it through `GET /api/v1/*` (`server/site-api.mjs`
//! in the site's repository). Every call is a real request: the site counts its use, and an answer
//! is always the one the site shows now, with links to its pages and the answer's clip.
//!
//! The query leaves the station, so the tools say never to put private bot data in it. Only `GET`
//! of fixed paths on two fixed origins is sent, with no cookie and nothing about the bots.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

/// The domains in the order they are tried: `.eu` serves Russia and its neighbours, where `.pro`
/// may be unreachable, and the site may send such a station there with a 3xx.
pub(super) const ORIGINS: [&str; 2] = ["https://moonterminal.pro", "https://moonterminal.eu"];

/// The languages the site's reference ships: the terminal's own eight.
pub(super) const LANGS: [&str; 8] = ["ru", "en", "es", "uk", "tr", "pt", "vi", "zh"];

/// The language when a call names none: the FAQ is written in Russian and translated from it.
const DEFAULT_LANG: &str = "ru";

/// How long a domain that answered is asked first, so a station that only reaches `.eu` does not
/// pay `.pro`'s timeout on every call.
const REMEMBER: Duration = Duration::from_secs(30 * 60);

/// One attempt's whole budget. Two attempts, `.pro` then `.eu`, fit inside the ten seconds the MCP
/// side gives a site call (`tools.rs`), so the worker and its permit are free by the time the
/// caller hears "timed out". The API answers in milliseconds; this only bounds a dead domain.
const TIMEOUT: Duration = Duration::from_secs(4);

/// How much of the site's own error message reaches the model.
const MAX_ERROR_CHARS: usize = 300;

/// The largest answer read. The API's biggest, a screen with its controls, is tens of kilobytes.
const MAX_BYTES: u64 = 1024 * 1024;

/// How one request to one domain ended, as far as choosing the next domain goes.
#[derive(Debug)]
pub(super) enum Reply {
    /// The status and the body, whatever the status.
    Answered { status: u16, body: String },
    /// No HTTP answer at all: DNS, TCP, TLS or the timeout.
    Unreachable(String),
}

/// What sends a request; the tests replace it.
pub(super) trait Fetch: Send + Sync {
    fn get(&self, url: &str) -> Reply;
}

/// The real sender: HTTPS only, no redirect followed, bounded in time and size.
struct Http(ureq::Agent);

impl Fetch for Http {
    fn get(&self, url: &str) -> Reply {
        match self.0.get(url).call() {
            Ok(mut response) => {
                let status = response.status().as_u16();
                match response
                    .body_mut()
                    .with_config()
                    .limit(MAX_BYTES)
                    .read_to_string()
                {
                    Ok(body) => Reply::Answered { status, body },
                    Err(error) => Reply::Unreachable(format!("reading the answer failed: {error}")),
                }
            }
            Err(error) => Reply::Unreachable(error.to_string()),
        }
    }
}

/// The client: which domain answered last, and the sender.
pub(super) struct Client {
    fetch: Box<dyn Fetch>,
    /// The index into `ORIGINS` that answered, and when.
    preferred: Mutex<Option<(usize, Instant)>>,
}

impl Client {
    /// The real client. A redirect is never followed: a 3xx means "ask the other domain", and the
    /// address in `Location` is never read, so the site cannot send the station anywhere else.
    pub fn new() -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .https_only(true)
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_global(Some(TIMEOUT))
            .user_agent(format!("MoonStation/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self::with_fetch(Box::new(Http(agent)))
    }

    /// A client over any sender, for the tests.
    pub(super) fn with_fetch(fetch: Box<dyn Fetch>) -> Self {
        Self {
            fetch,
            preferred: Mutex::new(None),
        }
    }

    /// The domains to try, the one that answered within `REMEMBER` first.
    fn order(&self) -> [usize; 2] {
        let remembered = self
            .preferred
            .lock()
            .ok()
            .and_then(|preferred| *preferred)
            .filter(|(_, at)| at.elapsed() < REMEMBER)
            .map(|(index, _)| index);
        if remembered == Some(1) {
            [1, 0]
        } else {
            [0, 1]
        }
    }

    /// One API path on the first domain that answers it with the API's own JSON.
    ///
    /// No answer, a 5xx, a 3xx, or a body that is not the API's JSON object — a Cloudflare
    /// challenge or a geo-block page, whatever its status — moves on to the next domain, and a
    /// domain is remembered only once it answered properly. A 4xx carrying the API's own error is
    /// the site's word: the other domain serves the same data and would say the same.
    pub(super) fn get(&self, path: &str) -> Result<Value> {
        let mut failures = Vec::new();
        for index in self.order() {
            let origin = ORIGINS[index];
            let (status, body) = match self.fetch.get(&format!("{origin}{path}")) {
                Reply::Answered { status, body } => (status, body),
                Reply::Unreachable(error) => {
                    failures.push(format!("{origin}: {error}"));
                    continue;
                }
            };
            if status == 429 {
                bail!("the site is limiting requests; retry in a few seconds");
            }
            let parsed = serde_json::from_str::<Value>(&body)
                .ok()
                .filter(Value::is_object);
            match parsed {
                Some(mut value) if (200..300).contains(&status) => {
                    if let Ok(mut preferred) = self.preferred.lock() {
                        *preferred = Some((index, Instant::now()));
                    }
                    value["content_is_reference_data"] = json!(true);
                    return Ok(value);
                }
                Some(value) if (400..500).contains(&status) => {
                    if let Some(message) = value["error"]["message"].as_str() {
                        let message: String = message.chars().take(MAX_ERROR_CHARS).collect();
                        bail!("site: {message}");
                    }
                    failures.push(format!("{origin}: HTTP {status}"));
                }
                Some(_) => failures.push(format!("{origin}: HTTP {status}")),
                None => failures.push(format!("{origin}: HTTP {status}, not the API's JSON")),
            }
        }
        bail!(
            "the MoonTerminal site is unreachable ({}); answer from MoonDoc or say so",
            failures.join("; ")
        )
    }

    /// `site_search`: the arguments were checked against the tool's schema.
    pub fn search(&self, arguments: &Value) -> Result<Value> {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("q", required(arguments, "query")?);
        query.append_pair("lang", lang(arguments));
        if let Some(scope) = arguments["scope"].as_str() {
            query.append_pair("scope", scope);
        }
        if let Some(limit) = arguments["limit"].as_u64() {
            query.append_pair("limit", &limit.to_string());
        }
        let mut value = self.get(&format!("/api/v1/search?{}", query.finish()))?;
        // A model may answer from the excerpts alone and never reach the answer's clip.
        let has_clip = value["hits"]
            .as_array()
            .is_some_and(|hits| hits.iter().any(|hit| hit["video"].is_string()));
        if has_clip {
            value["next"] = json!(
                "Read the answer you use with site_read; when it has video, call site_clip unasked so the reader sees it."
            );
        }
        Ok(value)
    }

    /// `site_read`: one answer, screen or version, by the id a search returned.
    ///
    /// An answer with a finished clip says what to do with it in the result itself: the reader
    /// should see the clip without having to ask for it, and a step named in the result is
    /// followed far more reliably than one in the server's opening instructions.
    pub fn read(&self, arguments: &Value) -> Result<Value> {
        let endpoint = match required(arguments, "kind")? {
            "faq" => "faq",
            "screen" => "screen",
            "release" => "release",
            other => bail!("kind {other} is not faq, screen or release"),
        };
        let id = required(arguments, "id")?;
        let mut value = self.get(&path_of(endpoint, id, lang(arguments)))?;
        if endpoint == "faq" && value["video"].is_string() {
            // The id asked for, not the one the answer echoes: a missing field must not send the
            // model to site_clip with an empty id.
            value["next"] = json!(format!(
                "This answer has a clip: call site_clip with id {id} and lang {} now, without being asked, so the reader sees it in the chat.",
                lang(arguments)
            ));
        }
        Ok(value)
    }

    /// `site_clip`: an answer's clip for the chat's player, and the answer's page beside it.
    ///
    /// The structured result is what the player reads (`clip.html`); a client with no player shows
    /// the same fields as text, so the link is never lost.
    pub fn clip(&self, arguments: &Value) -> Result<Value> {
        let lang = lang(arguments);
        let answer = self.get(&path_of("faq", required(arguments, "id")?, lang))?;
        let video = answer["video"].as_str().map(str::to_owned);
        let note = if video.is_some() {
            "The clip plays in the chat where the client shows apps; otherwise give the reader the video link."
        } else {
            "This answer has no clip in this language yet; give the reader url instead."
        };
        Ok(json!({
            "id": answer["id"],
            "question": answer["question"],
            "url": answer["url"],
            "video": video,
            "lang": lang,
            "note": note,
            "site": answer["site"],
            "content_is_reference_data": true,
        }))
    }
}

/// A required string argument.
fn required<'a>(arguments: &'a Value, key: &str) -> Result<&'a str> {
    arguments[key]
        .as_str()
        .ok_or_else(|| anyhow!("{key} must be a string"))
}

/// The call's language, one of `LANGS` by the schema, or the default.
fn lang(arguments: &Value) -> &str {
    arguments["lang"].as_str().unwrap_or(DEFAULT_LANG)
}

/// `/api/v1/<endpoint>/<id>?lang=`: the id as one encoded path segment, so it cannot climb or add
/// a query of its own. The dots are encoded too: form encoding leaves them, and `..` as a bare
/// segment would be collapsed by any URL normaliser on the way into another endpoint.
fn path_of(endpoint: &str, id: &str, lang: &str) -> String {
    let segment: String = url::form_urlencoded::byte_serialize(id.as_bytes()).collect();
    format!(
        "/api/v1/{endpoint}/{}?lang={lang}",
        segment.replace('+', "%20").replace('.', "%2E")
    )
}

#[cfg(test)]
mod tests;
