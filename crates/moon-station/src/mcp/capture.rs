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
        offset: usize,
        lines: usize,
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
        let rows: Vec<_> = capture
            .rows
            .iter()
            .skip(offset)
            .take(lines.clamp(1, 100))
            .collect();
        Ok(
            json!({"capture_id":id,"status":capture.ended.as_deref().unwrap_or("collecting"),"cores":capture.cores.iter().map(u64::to_string).collect::<Vec<_>>(),
            "started_ms":capture.started_ms,"end_ms":capture.end_ms,"received_lines":capture.rows.len(),"gap_cores":capture.gaps.iter().map(u64::to_string).collect::<Vec<_>>(),
            "offset":offset,"next_offset":offset+rows.len(),"lines":rows,"data_is_untrusted":true,
            "interpretation":"No lines can mean a quiet bot, missing subscription support, disconnection or insufficient core verbosity. ServerLog has no replay or reliable delivery; gap_cores cannot detect every packet lost before Station. File-only logs and historical archives are not included."}),
        )
    }
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
