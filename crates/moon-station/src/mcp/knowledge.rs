//! Revision-pinned public MoonDoc snapshots. No bot data is sent to the documentation source.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock, mpsc};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::auth::now;

/// Maximum uncompressed knowledge bytes, bounding both download and retained cache memory.
const MAX_BYTES: usize = 12 * 1024 * 1024;
const SOURCE: &str = "https://api.github.com/repos/Moonbot-Tech/MoonDoc";

/// A complete set from exactly one public commit, atomically published to readers.
#[derive(Default, Serialize, Deserialize)]
pub(super) struct Snapshot {
    pub revision: String,
    pub checked_ms: i64,
    documents: BTreeMap<String, String>,
    #[serde(skip)]
    pub last_error: Option<String>,
}

/// Shared readers never wait for a network call or see a partially downloaded revision.
pub(super) type SharedKnowledge = Arc<RwLock<Snapshot>>;

/// Background refresh lifecycle; stopping wakes the six-hour wait immediately.
pub(super) struct Worker {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Worker {
    /// Load the last good snapshot, then check the public repository outside the owner thread.
    pub fn start() -> Result<(Self, SharedKnowledge)> {
        let path = moon_core::config::paths::station_mcp_knowledge();
        let saved = match std::fs::metadata(&path) {
            Ok(meta) if meta.len() <= (MAX_BYTES * 2) as u64 => std::fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default(),
            _ => Snapshot::default(),
        };
        let shared = Arc::new(RwLock::new(saved));
        let output = shared.clone();
        let (stop, receiver) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("mcp-moondoc".into())
            .spawn(move || {
                let agent: ureq::Agent = ureq::Agent::config_builder()
                    .https_only(true)
                    .max_redirects(0)
                    .timeout_global(Some(Duration::from_secs(15)))
                    .build()
                    .into();
                loop {
                    let current = output
                        .read()
                        .map(|s| s.revision.clone())
                        .unwrap_or_default();
                    let result = refresh(&agent, &current, &receiver);
                    match result {
                        Ok(Some(snapshot)) => {
                            let save = serde_json::to_vec(&snapshot)
                                .map_err(anyhow::Error::from)
                                .and_then(|bytes| {
                                    moon_core::config::write_file_atomic(
                                        &path,
                                        &bytes,
                                        "MoonDoc snapshot",
                                    )
                                });
                            match save {
                                Ok(()) => {
                                    if let Ok(mut active) = output.write() {
                                        *active = snapshot;
                                    }
                                }
                                Err(e) => {
                                    if let Ok(mut active) = output.write() {
                                        active.last_error =
                                            Some(format!("cache write failed: {e}"));
                                    }
                                }
                            }
                        }
                        Ok(None) => {
                            if let Ok(mut active) = output.write() {
                                active.checked_ms = now();
                                active.last_error = None;
                            }
                        }
                        Err(e) => {
                            if let Ok(mut active) = output.write() {
                                active.last_error = Some(e.to_string());
                            }
                        }
                    }
                    match receiver.recv_timeout(Duration::from_secs(6 * 60 * 60)) {
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        _ => break,
                    }
                }
            })?;
        Ok((
            Self {
                stop: Some(stop),
                thread: Some(thread),
            },
            shared,
        ))
    }
}

impl Drop for Worker {
    /// An in-flight fetch is bounded by the HTTP timeout; no new file starts after cancellation.
    fn drop(&mut self) {
        self.stop.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Fetch bounded public data only; redirects cannot turn a fixed GitHub source into arbitrary URLs.
fn fetch(agent: &ureq::Agent, url: &str, limit: u64) -> Result<String> {
    let mut response = agent
        .get(url)
        .header("User-Agent", "MoonStation-MoonDoc")
        .call()?;
    ensure!(
        response.status().is_success(),
        "MoonDoc fetch returned {}",
        response.status()
    );
    Ok(response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_string()?)
}

/// Pin the tree and every raw document to one SHA; failure keeps the entire old snapshot.
fn refresh(
    agent: &ureq::Agent,
    current: &str,
    stop: &mpsc::Receiver<()>,
) -> Result<Option<Snapshot>> {
    let head: Value = serde_json::from_str(&fetch(
        agent,
        &format!("{SOURCE}/commits/main"),
        256 * 1024,
    )?)?;
    let revision = head["sha"].as_str().context("MoonDoc commit has no SHA")?;
    ensure!(
        revision.len() == 40 && revision.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid MoonDoc SHA"
    );
    if revision == current {
        return Ok(None);
    }
    let tree: Value = serde_json::from_str(&fetch(
        agent,
        &format!("{SOURCE}/git/trees/{revision}?recursive=1"),
        1024 * 1024,
    )?)?;
    ensure!(
        tree["truncated"].as_bool() == Some(false),
        "incomplete MoonDoc tree"
    );
    let entries = tree["tree"].as_array().context("MoonDoc tree missing")?;
    let mut documents = BTreeMap::new();
    let mut total = 0;
    for entry in entries {
        let Some(path) = entry["path"].as_str() else {
            continue;
        };
        if entry["type"] != "blob" || !published_document(path) {
            continue;
        }
        ensure!(
            matches!(stop.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "MoonDoc refresh stopped"
        );
        ensure!(
            entry["size"]
                .as_u64()
                .is_some_and(|size| size <= 1024 * 1024),
            "MoonDoc file exceeds limit"
        );
        ensure!(documents.len() < 256, "MoonDoc file count exceeds limit");
        let url =
            format!("https://raw.githubusercontent.com/Moonbot-Tech/MoonDoc/{revision}/{path}");
        let text = fetch(agent, &url, 1024 * 1024)?;
        total += text.len();
        ensure!(total <= MAX_BYTES, "MoonDoc snapshot exceeds limit");
        documents.insert(path.to_owned(), text);
    }
    ensure!(
        documents.contains_key("llms.txt") && documents.contains_key("reference/params/index.tsv"),
        "MoonDoc required indexes missing"
    );
    Ok(Some(Snapshot {
        revision: revision.into(),
        checked_ms: now(),
        documents,
        last_error: None,
    }))
}

/// Cache public reference, trading explanations and strategy examples; never follow document links.
fn published_document(path: &str) -> bool {
    !path.contains("..")
        && !path.contains(['?', '#', '%', '\\'])
        && (path == "llms.txt"
            || path == "README.md"
            || (["reference/", "concepts/", "examples/strategies/"]
                .iter()
                .any(|prefix| path.starts_with(prefix))
                && (path.ends_with(".md") || path.ends_with(".tsv"))))
}

impl Snapshot {
    /// Attach the relevant maintained workflow to live facts instead of relying on model memory.
    /// Missing guidance is explicit while an installation waits for its first public snapshot.
    pub fn context(&self, topic: &str) -> Value {
        let path = "reference/station-mcp-guide.md";
        let heading = format!("## {topic}");
        let section = self.documents.get(path).and_then(|document| {
            let first = document.lines().position(|line| line == heading)?;
            let count = 1 + document
                .lines()
                .skip(first + 1)
                .take_while(|line| !line.starts_with("## "))
                .count();
            Some((first, count.min(40)))
        });
        match section {
            Some((first, count)) => self
                .read(path, first + 1, count)
                .unwrap_or_else(|_| json!({"available":false,"path":path})),
            None => {
                json!({"available":false,"path":path,"topic":topic,"instruction":"Use knowledge_search for current reference; do not invent missing capabilities or field meanings."})
            }
        }
    }

    /// Every answer carries revision, age and refresh failure so stale knowledge is visible.
    pub fn metadata(&self) -> Value {
        json!({"source":"Moonbot-Tech/MoonDoc", "revision":self.revision, "checked_at_ms":self.checked_ms,
            "age_ms":now().saturating_sub(self.checked_ms), "refresh_error":self.last_error,
            "content_is_reference_data":true})
    }

    /// Exact parameter spelling wins, then lexical matches; results include navigable line ranges.
    pub fn search(&self, query: &str) -> Result<Value> {
        ensure!(
            !self.documents.is_empty(),
            "MoonDoc is not available yet; retry after synchronization"
        );
        ensure!(
            !query.trim().is_empty() && query.len() <= 256,
            "query must contain 1..256 bytes"
        );
        let needle = query.to_lowercase();
        let terms: Vec<_> = needle.split_whitespace().collect();
        let mut matches = Vec::new();
        for (path, text) in &self.documents {
            for (index, line) in text.lines().enumerate() {
                let lower = line.to_lowercase();
                let exact = line
                    .split('\t')
                    .next()
                    .is_some_and(|field| field.eq_ignore_ascii_case(query));
                let score = if exact {
                    100
                } else if lower.contains(&needle) {
                    20
                } else {
                    terms.iter().filter(|term| lower.contains(**term)).count()
                };
                if score == 0 {
                    continue;
                }
                matches.push((score, path, index + 1, line));
            }
        }
        matches.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.cmp(b.1))
                .then_with(|| a.2.cmp(&b.2))
        });
        let hits: Vec<_> = matches.into_iter().take(12).map(|(_, path, line, text)| json!({
            "path":path,"line":line,"excerpt":text.chars().take(800).collect::<String>(),"url":self.url(path,line)
        })).collect();
        Ok(
            json!({"knowledge":self.metadata(),"hits":hits,"next":"Use knowledge_read for the surrounding section; preserve exact field names."}),
        )
    }

    /// Read a bounded neighborhood, including surrounding causal context rather than isolated hits.
    pub fn read(&self, path: &str, first: usize, count: usize) -> Result<Value> {
        let document = self
            .documents
            .get(path)
            .context("document is not in the public snapshot")?;
        let first = first.max(1);
        let mut lines = Vec::new();
        let mut bytes = 0;
        let mut truncated_line = None;
        for line in document.lines().skip(first - 1).take(count.clamp(1, 100)) {
            if bytes + line.len() > 20_000 {
                if lines.is_empty() {
                    let end = line.floor_char_boundary(20_000);
                    lines.push(&line[..end]);
                    truncated_line = Some(first);
                }
                break;
            }
            bytes += line.len();
            lines.push(line);
        }
        Ok(
            json!({"knowledge":self.metadata(),"path":path,"first_line":first,"next_line":first+lines.len(),
            "text":lines.join("\n"),"truncated_line":truncated_line,"url":self.url(path,first)}),
        )
    }

    /// Link to the exact cached revision so later publication cannot change the cited evidence.
    fn url(&self, path: &str, line: usize) -> String {
        format!(
            "https://github.com/Moonbot-Tech/MoonDoc/blob/{}/{path}#L{line}",
            self.revision
        )
    }
}

#[cfg(test)]
mod tests;
