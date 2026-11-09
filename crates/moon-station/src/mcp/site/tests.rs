//! Which domain answers, what a failure says, and what is sent: no network, a scripted sender.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::*;

/// Replies by origin, and every URL asked, in order.
#[derive(Clone, Default)]
struct Script {
    pro: Arc<Mutex<Vec<Reply>>>,
    eu: Arc<Mutex<Vec<Reply>>>,
    asked: Arc<Mutex<Vec<String>>>,
}

impl Fetch for Script {
    fn get(&self, url: &str) -> Reply {
        self.asked.lock().unwrap().push(url.to_owned());
        let queue = if url.starts_with(ORIGINS[0]) {
            &self.pro
        } else {
            &self.eu
        };
        let mut queue = queue.lock().unwrap();
        assert!(!queue.is_empty(), "unexpected request {url}");
        queue.remove(0)
    }
}

fn ok(body: serde_json::Value) -> Reply {
    Reply::Answered {
        status: 200,
        body: body.to_string(),
    }
}

fn status(status: u16, body: &str) -> Reply {
    Reply::Answered {
        status,
        body: body.into(),
    }
}

fn scripted(pro: Vec<Reply>, eu: Vec<Reply>) -> (Client, Script) {
    let script = Script {
        pro: Arc::new(Mutex::new(pro)),
        eu: Arc::new(Mutex::new(eu)),
        ..Script::default()
    };
    (Client::with_fetch(Box::new(script.clone())), script)
}

#[test]
fn an_unreachable_pro_falls_back_to_eu_and_eu_is_asked_first_afterwards() {
    let (client, script) = scripted(
        vec![Reply::Unreachable("dns".into())],
        vec![ok(json!({"hits":[]})), ok(json!({"hits":[]}))],
    );
    client.get("/api/v1/health").unwrap();
    client.get("/api/v1/health").unwrap();
    let asked = script.asked.lock().unwrap().clone();
    assert_eq!(
        asked,
        [
            "https://moonterminal.pro/api/v1/health",
            "https://moonterminal.eu/api/v1/health",
            "https://moonterminal.eu/api/v1/health",
        ]
    );
}

#[test]
fn a_redirect_or_a_server_error_moves_on_but_a_refusal_does_not() {
    let (client, _) = scripted(vec![status(302, "")], vec![ok(json!({}))]);
    assert!(client.get("/x").is_ok());
    let (client, _) = scripted(vec![status(503, "")], vec![ok(json!({}))]);
    assert!(client.get("/x").is_ok());
    // A 404 is the site's answer: `.eu` holds the same data and is not asked.
    let (client, script) = scripted(
        vec![status(
            404,
            r#"{"error":{"code":"not_found","message":"no answer \"x\""}}"#,
        )],
        vec![],
    );
    let error = client.get("/x").unwrap_err().to_string();
    assert_eq!(error, "site: no answer \"x\"");
    assert_eq!(script.asked.lock().unwrap().len(), 1);
}

#[test]
fn both_domains_down_says_so_rather_than_finding_nothing() {
    let (client, _) = scripted(
        vec![Reply::Unreachable("timeout".into())],
        vec![status(502, "<html>")],
    );
    let error = client.get("/x").unwrap_err().to_string();
    assert!(error.contains("unreachable"), "{error}");
    assert!(error.contains("moonterminal.eu: HTTP 502"), "{error}");
}

#[test]
fn a_rate_limit_is_its_own_error_and_not_retried_elsewhere() {
    let (client, script) = scripted(vec![status(429, "<html>")], vec![]);
    assert!(
        client
            .get("/x")
            .unwrap_err()
            .to_string()
            .contains("limiting")
    );
    assert_eq!(script.asked.lock().unwrap().len(), 1);
}

#[test]
fn the_query_and_the_id_are_encoded_so_they_cannot_change_the_path() {
    let (client, script) = scripted(vec![ok(json!({})), ok(json!({}))], vec![]);
    client
        .search(&json!({"query":"ключ & lang=en #x","lang":"uk","scope":"faq","limit":3}))
        .unwrap();
    client
        .read(&json!({"kind":"screen","id":"../faq/a b|c?d","lang":"en"}))
        .unwrap();
    let asked = script.asked.lock().unwrap().clone();
    let search = url::Url::parse(&asked[0]).unwrap();
    assert_eq!(search.path(), "/api/v1/search");
    let pairs: Vec<_> = search.query_pairs().into_owned().collect();
    assert_eq!(
        pairs,
        [
            ("q".into(), "ключ & lang=en #x".into()),
            ("lang".into(), "uk".into()),
            ("scope".into(), "faq".into()),
            ("limit".into(), "3".into()),
        ]
    );
    assert_eq!(
        asked[1],
        "https://moonterminal.pro/api/v1/screen/%2E%2E%2Ffaq%2Fa%20b%7Cc%3Fd?lang=en"
    );
}

#[test]
fn a_bare_dot_segment_is_encoded_and_cannot_reach_another_endpoint() {
    let (client, script) = scripted(vec![ok(json!({}))], vec![]);
    client.read(&json!({"kind":"faq","id":".."})).unwrap();
    let asked = script.asked.lock().unwrap().clone();
    assert_eq!(
        asked[0],
        "https://moonterminal.pro/api/v1/faq/%2E%2E?lang=ru"
    );
}

#[test]
fn a_page_that_is_not_the_api_moves_on_and_is_not_remembered() {
    // A challenge page answered 200, a geo-block page 403, a JSON array: none is the API.
    for reply in [
        status(200, "<html>challenge</html>"),
        status(403, "<html>blocked</html>"),
        status(200, "[1,2]"),
    ] {
        let (client, script) = scripted(vec![reply], vec![ok(json!({"hits":[]})), ok(json!({}))]);
        assert!(client.get("/x").is_ok());
        // `.eu` answered, so it is asked first now; `.pro` is not.
        client.get("/x").unwrap();
        let asked = script.asked.lock().unwrap().clone();
        assert_eq!(asked.len(), 3);
        assert!(asked[2].starts_with(ORIGINS[1]), "{asked:?}");
    }
    let (client, _) = scripted(vec![status(200, "<html>")], vec![status(403, "<html>")]);
    let error = client.get("/x").unwrap_err().to_string();
    assert!(error.contains("not the API's JSON"), "{error}");
}

#[test]
fn the_sites_error_text_is_bounded() {
    let long = "x".repeat(5000);
    let body = json!({"error":{"code":"not_found","message":long}}).to_string();
    let (client, _) = scripted(vec![status(404, &body)], vec![]);
    let error = client.get("/x").unwrap_err().to_string();
    assert!(error.len() < 400, "{}", error.len());
}

#[test]
fn a_clip_is_passed_on_only_when_the_site_names_one() {
    let (client, _) = scripted(
        vec![
            ok(
                json!({"id":"a","question":"Q","url":"https://moonterminal.pro/ru/faq/a/","video":"https://moonterminal.pro/clips/ru/a.mp4"}),
            ),
            ok(json!({"id":"b","question":"Q","url":"https://moonterminal.pro/vi/faq/b/"})),
        ],
        vec![],
    );
    let with = client.clip(&json!({"id":"a"})).unwrap();
    assert_eq!(with["video"], "https://moonterminal.pro/clips/ru/a.mp4");
    assert_eq!(with["lang"], "ru");
    let without = client.clip(&json!({"id":"b","lang":"vi"})).unwrap();
    assert!(without["video"].is_null());
    assert_eq!(without["url"], "https://moonterminal.pro/vi/faq/b/");
}

/// The real site, once its API is up: `cargo test -p moon-station site_live -- --ignored`.
/// The one check of this client against the live API: TLS, Cloudflare and nginx in between.
#[test]
#[ignore = "asks moonterminal.pro over the network"]
fn site_live_answers_a_search_a_read_a_clip_and_a_refusal() {
    let client = Client::new();
    let found = client
        .search(&json!({"query":"подключить ядро ключ","lang":"ru","scope":"faq","limit":3}))
        .unwrap();
    assert_eq!(found["hits"][0]["kind"], "faq", "{found}");
    let id = found["hits"][0]["id"].as_str().unwrap();
    let answer = client
        .read(&json!({"kind":"faq","id":id,"lang":"ru"}))
        .unwrap();
    assert!(
        answer["url"]
            .as_str()
            .unwrap()
            .starts_with("https://moonterminal."),
        "{answer}"
    );
    let clip = client.clip(&json!({"id":id,"lang":"ru"})).unwrap();
    assert!(clip["url"].is_string(), "{clip}");
    let refused = client
        .read(&json!({"kind":"faq","id":"no-such-answer","lang":"ru"}))
        .unwrap_err()
        .to_string();
    assert!(refused.starts_with("site: "), "{refused}");
}

/// The reader gets the clip without asking for it: an answer with one tells the model to show it,
/// one without says nothing, and a search that found clips points at the read that leads there.
#[test]
fn an_answer_with_a_clip_asks_for_it_to_be_shown() {
    let (client, _) = scripted(
        vec![
            ok(json!({"id":"a","video":"https://moonterminal.pro/clips/ru/a.mp4"})),
            ok(json!({"id":"b"})),
            ok(
                json!({"hits":[{"kind":"faq","id":"a","video":"https://moonterminal.pro/clips/ru/a.mp4"}]}),
            ),
            ok(json!({"hits":[{"kind":"faq","id":"b"}]})),
        ],
        vec![],
    );
    let with = client.read(&json!({"kind":"faq","id":"a"})).unwrap();
    assert!(
        with["next"]
            .as_str()
            .unwrap()
            .contains("site_clip with id a")
    );
    let without = client.read(&json!({"kind":"faq","id":"b"})).unwrap();
    assert!(without.get("next").is_none());
    let found = client.search(&json!({"query":"x"})).unwrap();
    assert!(found["next"].as_str().unwrap().contains("site_clip"));
    let plain = client.search(&json!({"query":"x"})).unwrap();
    assert!(plain.get("next").is_none());
}
