//! Bounded diagnostic leases; subscription follows the union of live consumers, not one caller.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use moon_core::config::AppConfig;
use moon_core::session::SessionManager;
use moon_core::station_api::mcp::Scope;
use serde_json::{Value, json};

use super::{Shared, auth};

/// One diagnostic window, with absolute log cursors and an explicit reason when collection ends.
struct Capture {
    who: auth::Principal,
    cores: Vec<u64>,
    started_ms: i64,
    deadline: Instant,
    end_ms: i64,
    retain_until: Instant,
    cursors: BTreeMap<u64, (u64, u64)>,
    rows: Vec<Value>,
    bytes: usize,
    ended: Option<String>,
    gaps: BTreeSet<u64>,
}

/// All active observations share one desired subscription per core connection epoch.
#[derive(Default)]
pub(super) struct Captures {
    captures: BTreeMap<String, Capture>,
    subscriptions: BTreeMap<u64, (u64, bool)>,
}

impl Captures {
    /// Start from current cursors: earlier retained lines are not misrepresented as this observation.
    pub fn start(
        &mut self,
        session: &SessionManager,
        who: auth::Principal,
        cores: Vec<u64>,
        seconds: u64,
    ) -> Result<Value> {
        ensure!(
            (5..=120).contains(&seconds),
            "capture duration must be 5..120 seconds"
        );
        ensure!(!cores.is_empty() && cores.len() <= 16, "select 1..16 cores");
        ensure!(
            self.captures.len() < 32
                && self.captures.values().filter(|c| c.ended.is_none()).count() < 8,
            "diagnostic observation limit reached"
        );
        let cursors: BTreeMap<_, _> = cores
            .iter()
            .map(|id| {
                let core = session.store().core(*id).context("core unavailable")?;
                Ok((*id, (core.log_seq, core.conn_epoch)))
            })
            .collect::<Result<_>>()?;
        let id = auth::random()?;
        let started_ms = auth::now();
        self.captures.insert(
            id.clone(),
            Capture {
                who,
                cores,
                started_ms,
                deadline: Instant::now() + Duration::from_secs(seconds),
                end_ms: started_ms + seconds as i64 * 1000,
                retain_until: Instant::now() + Duration::from_secs(seconds + 600),
                cursors,
                rows: Vec::new(),
                bytes: 0,
                ended: None,
                gaps: BTreeSet::new(),
            },
        );
        Ok(
            json!({"capture_id":id,"status":"collecting","started_ms":started_ms,"duration_seconds":seconds,
            "subscription":"requested; receipt of lines is the evidence of delivery","verbosity_changed":false,"history_available":false}),
        )
    }

    /// Collect only newly received lines inside the requested interval, recording ring loss/reconnects.
    pub fn tick(&mut self, session: &SessionManager, cfg: &AppConfig, shared: &Shared) {
        let clock = Instant::now();
        self.captures
            .retain(|_, capture| clock < capture.retain_until);
        for capture in self.captures.values_mut().filter(|c| c.ended.is_none()) {
            let authorized = shared.auth.lock().ok().is_some_and(|auth| {
                auth.check(&capture.who, Scope::Read, &capture.cores, auth::now())
                    .is_ok()
            });
            if !authorized {
                capture.ended = Some("authorization_expired_or_revoked".into());
                capture.end_ms = capture.end_ms.min(auth::now());
                continue;
            }
            for id in &capture.cores {
                let Some(core) = session.store().core(*id) else {
                    capture.gaps.insert(*id);
                    continue;
                };
                let (cursor, epoch) = capture.cursors.get(id).copied().unwrap_or_default();
                let (lines, next, gap) = core.raw_server_log_since(cursor);
                if gap
                    || core.conn_epoch != epoch
                    || core.status != moon_core::feed::ConnStatus::Ready
                {
                    capture.gaps.insert(*id);
                }
                for line in lines {
                    if line.recv_ms < capture.started_ms || line.recv_ms > capture.end_ms {
                        continue;
                    }
                    let text = redact(&line.msg, cfg);
                    if capture.bytes + text.len() > 1024 * 1024 || capture.rows.len() >= 5000 {
                        capture.ended = Some("size_limit".into());
                        break;
                    }
                    capture.bytes += text.len();
                    capture.rows.push(json!({"core":id.to_string(),"server_time_ms":line.time_ms,"server_time_axis":"core_local","received_ms":line.recv_ms,"received_time_axis":"UTC","text":text}));
                }
                capture.cursors.insert(*id, (next, core.conn_epoch));
                if capture.ended.is_some() {
                    break;
                }
            }
            if clock >= capture.deadline && capture.ended.is_none() {
                capture.ended = Some("duration_elapsed".into());
            }
            if capture.ended.is_some() {
                capture.end_ms = capture.end_ms.min(auth::now());
            }
        }
        let wanted: BTreeSet<_> = self
            .captures
            .values()
            .filter(|c| c.ended.is_none())
            .flat_map(|c| c.cores.iter().copied())
            .collect();
        for core in session.sessions() {
            let Some(data) = session.store().core(core.id) else {
                continue;
            };
            let base = cfg
                .servers
                .iter()
                .find(|s| s.id == core.id)
                .is_some_and(|s| s.feed.log_delivery);
            let desired = (data.conn_epoch, base || wanted.contains(&core.id));
            if self.subscriptions.get(&core.id) == Some(&desired) {
                continue;
            }
            if session.automation_logs(core.id, desired.1).is_ok() {
                self.subscriptions.insert(core.id, desired);
            }
        }
        self.subscriptions
            .retain(|id, _| session.sessions().iter().any(|s| s.id == *id));
    }

    /// Read or stop only the caller's own observation; shared access to a bot is not shared custody.
    pub fn get(
        &mut self,
        id: &str,
        who: &auth::Principal,
        read: Read<'_>,
        stop: bool,
        shared: &Shared,
    ) -> Result<Value> {
        let capture = self
            .captures
            .get_mut(id)
            .context("unknown or expired capture; observations do not survive Station restart")?;
        ensure!(
            capture.who.grant_id == who.grant_id,
            "capture belongs to another authorization"
        );
        shared
            .auth
            .lock()
            .map_err(|_| anyhow::anyhow!("authorization unavailable"))?
            .check(who, Scope::Read, &capture.cores, auth::now())?;
        if stop && capture.ended.is_none() {
            capture.ended = Some("stopped_by_client".into());
            capture.end_ms = auth::now();
        }
        let selected = select(&capture.rows, read.contains);
        let mut answer = json!({"capture_id":id,"status":capture.ended.as_deref().unwrap_or("collecting"),"cores":capture.cores.iter().map(u64::to_string).collect::<Vec<_>>(),
            "started_ms":capture.started_ms,"end_ms":capture.end_ms,"received_lines":capture.rows.len(),"gap_cores":capture.gaps.iter().map(u64::to_string).collect::<Vec<_>>(),
            "data_is_untrusted":true,"time_axes":{"received_ms":"UTC","server_time_ms":"core_local"},
            "interpretation":"No lines can mean a quiet bot, missing subscription support, disconnection or insufficient core verbosity. ServerLog has no replay or reliable delivery; gap_cores cannot detect every packet lost before Station. File-only logs and historical archives are not included."});
        if let Some(needle) = read.contains {
            answer["contains"] = json!(needle);
            answer["matched_lines"] = json!(selected.len());
        }
        if read.summary {
            // Paged like the lines are, so every distinct line stays reachable.
            let groups = summarize(&selected, capture.cores.len() > 1);
            let page: Vec<_> = groups
                .iter()
                .skip(read.offset)
                .take(read.lines.clamp(1, SUMMARY_PAGE))
                .collect();
            answer["distinct_lines"] = json!(groups.len());
            answer["offset"] = json!(read.offset);
            answer["next_offset"] = json!(read.offset + page.len());
            answer["groups"] = json!(page);
        } else {
            let rows: Vec<_> = selected
                .iter()
                .skip(read.offset)
                .take(read.lines.clamp(1, 100))
                .collect();
            answer["offset"] = json!(read.offset);
            answer["next_offset"] = json!(read.offset + rows.len());
            answer["lines"] = json!(
                rows.iter()
                    .map(|row| compact(row, capture.cores.len() > 1))
                    .collect::<Vec<_>>()
            );
        }
        Ok(answer)
    }
}

/// Most distinct lines one summary page holds; the rest is reached by `next_offset`. A summary is
/// read to see what a log is busy with, which the first page answers.
const SUMMARY_PAGE: usize = 20;
/// Characters of a sample a summary keeps; the full line is one `contains` away.
const SUMMARY_TEXT: usize = 300;

/// One captured line as an answer carries it. The time axes are stated once in the answer, not on
/// every line, and the core only when the observation has more than one: measured live, the
/// labels were about 110 bytes of every line.
fn compact(row: &Value, several_cores: bool) -> Value {
    let mut line = json!({"received_ms":row["received_ms"],"server_time_ms":row["server_time_ms"],"text":row["text"]});
    if several_cores {
        line["core"] = row["core"].clone();
    }
    line
}

/// How `logs_get` reads an observation.
pub(super) struct Read<'a> {
    /// First line to return, counted over the lines that match `contains`; in a summary, the
    /// first distinct line.
    pub offset: usize,
    /// Lines to return; in a summary, distinct lines.
    pub lines: usize,
    /// Keep only lines holding this text, ignoring case.
    pub contains: Option<&'a str>,
    /// One entry per distinct line instead of the lines.
    pub summary: bool,
}

/// The captured lines holding `contains`, ignoring case; all of them without it.
fn select<'a>(rows: &'a [Value], contains: Option<&str>) -> Vec<&'a Value> {
    let needle = contains.map(str::to_lowercase);
    rows.iter()
        .filter(|row| {
            needle.as_deref().is_none_or(|needle| {
                row["text"]
                    .as_str()
                    .is_some_and(|text| text.to_lowercase().contains(needle))
            })
        })
        .collect()
}

/// A line with every run of digits folded to one `#`, so the same message about two prices or two
/// order numbers counts as one.
fn pattern(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_digits = false;
    for character in text.chars() {
        if character.is_ascii_digit() {
            if !in_digits {
                out.push('#');
            }
            in_digits = true;
        } else {
            out.push(character);
            in_digits = false;
        }
    }
    out
}

/// One entry per distinct pattern, most frequent first, with a real line as its sample.
///
/// The folded pattern itself is not returned: it repeats the sample with its digits replaced, and
/// on a live log it doubled the page.
fn summarize(rows: &[&Value], several_cores: bool) -> Vec<Value> {
    struct Group<'a> {
        count: usize,
        first: &'a Value,
        last: &'a Value,
        cores: BTreeSet<&'a str>,
    }
    let mut groups: BTreeMap<String, Group<'_>> = BTreeMap::new();
    for row in rows {
        let text = row["text"].as_str().unwrap_or_default();
        let group = groups.entry(pattern(text)).or_insert(Group {
            count: 0,
            first: row,
            last: row,
            cores: BTreeSet::new(),
        });
        group.count += 1;
        group.last = row;
        if let Some(core) = row["core"].as_str() {
            group.cores.insert(core);
        }
    }
    let mut groups: Vec<_> = groups.into_iter().collect();
    // Most frequent first; equal counts by first appearance, so the order is the same every read.
    groups.sort_by(|(_, a), (_, b)| {
        b.count.cmp(&a.count).then_with(|| {
            a.first["received_ms"]
                .as_i64()
                .cmp(&b.first["received_ms"].as_i64())
        })
    });
    groups
        .into_iter()
        .map(|(_, group)| {
            let sample: String = group.first["text"]
                .as_str()
                .unwrap_or_default()
                .chars()
                .take(SUMMARY_TEXT)
                .collect();
            let mut entry = json!({"count":group.count,"sample":sample,
            "first_received_ms":group.first["received_ms"],"last_received_ms":group.last["received_ms"]});
            if several_cores {
                entry["cores"] = json!(group.cores);
            }
            entry
        })
        .collect()
}

/// Known keys and credential-labelled lines cannot reach an AI; keep timestamps and an explicit marker.
fn redact(text: &str, cfg: &AppConfig) -> String {
    let lower = text.to_ascii_lowercase();
    if [
        "api_key",
        "apikey",
        "api key",
        "secret",
        "password",
        "passphrase",
        "authorization:",
        "token=",
        "token:",
        "mbkey",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
        || cfg.servers.iter().any(|server| {
            let key = server.key.expose();
            key.len() >= 8 && text.contains(key)
        })
    {
        return "[credential-bearing line redacted]".into();
    }
    moon_core::applog::redact::addresses(text)
        .chars()
        .take(4096)
        .collect()
}

#[cfg(test)]
mod tests;
