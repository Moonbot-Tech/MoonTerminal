//! Station-owner tool state: previews, receipts, diagnostic leases and a bounded event journal.

use std::collections::{BTreeMap, VecDeque};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use moon_core::automation::{Action, Authorization, Receipt, Submission};
use moon_core::config::AppConfig;
use moon_core::session::SessionManager;
use moon_core::station_api::mcp::{AuditEntry, Scope};
use serde_json::{Value, json};

use super::{
    Shared, auth,
    capture::Captures,
    tools::{number, string},
};

/// One preview is also its idempotency key; applying it twice cannot submit twice.
struct Operation {
    who: auth::Principal,
    core: u64,
    epoch: u64,
    action: Action,
    expected: Value,
    expires: Instant,
    retain_until: Instant,
    submitted: bool,
    receipt: Option<mpsc::Receiver<std::result::Result<Receipt, String>>>,
    admission: Option<moon_core::automation::OrderActionTicket>,
    result: Value,
}

impl Operation {
    /// A matching strategy event proves the saved values; a timeout never resubmits an edit.
    fn strategy_resolution(&self, session: &SessionManager) -> Option<Value> {
        if self.result["status"] != "submitted" || self.result["expected"].is_null() {
            return None;
        }
        let data = session.store().core(self.core)?;
        if data.conn_epoch != self.epoch {
            return Some(
                json!({"status":"unknown","execution_confirmed":false,"reason":"core reconnected before resolution"}),
            );
        }
        session.automation_strategy_resolution(self.core, &self.result["expected"]).or_else(|| {
            (Instant::now() >= self.expires).then(|| json!({"status":"unknown","execution_confirmed":false,
                "reason":"No matching strategy resolution before timeout; read actual values before another edit."}))
        })
    }

    /// Feed and ActiveLib receipts are polled without blocking the Station's session owner.
    fn take_receipt(&mut self) -> Option<Value> {
        if let Some(receipt) = &self.receipt {
            match receipt.try_recv() {
                Ok(Ok(receipt)) => {
                    if receipt.value["pending"].as_bool() != Some(true) {
                        self.receipt = None;
                    }
                    self.admission = receipt.admission;
                    return Some(receipt.value);
                }
                Ok(Err(reason)) => {
                    self.receipt = None;
                    return Some(
                        json!({"status":"rejected","reason":reason,"execution_confirmed":false}),
                    );
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.receipt = None;
                    return Some(
                        json!({"status":"unknown","reason":"core worker stopped before a receipt; do not retry automatically"}),
                    );
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some(ticket) = &self.admission {
            let result = match ticket.try_result() {
                Ok(Some(admitted)) => {
                    json!({"status":if admitted { "submitted" } else { "rejected" },"execution_confirmed":false,
                    "detail":if admitted { "guard matched and ActiveLib dispatched the action; observe exchange state" } else { "order changed or action was inapplicable; no guarded command was dispatched" }})
                }
                Ok(None) => return None,
                Err(_) => {
                    json!({"status":"unknown","reason":"ActiveLib stopped before reporting admission; do not retry automatically"})
                }
            };
            self.admission = None;
            return Some(result);
        }
        None
    }
}

/// Events are scoped on read; only a grant's own operation events are visible to that grant.
struct Event {
    seq: u64,
    core: u64,
    grant: Option<String>,
    data: Value,
}

/// Mutable state lives alongside the existing SessionManager owner, not on HTTP workers.
#[derive(Default)]
pub(super) struct Owner {
    operations: BTreeMap<String, Operation>,
    captures: Captures,
    states: BTreeMap<u64, Value>,
    events: VecDeque<Event>,
    sequence: u64,
}

impl Owner {
    /// Maintain bounded leases and collect receipts; no polling of heavy snapshots is performed here.
    pub fn tick(&mut self, session: &SessionManager, cfg: &AppConfig, shared: &Shared) {
        self.captures.tick(session, cfg, shared);
        self.operations
            .retain(|_, operation| Instant::now() < operation.retain_until);
        let mut events = Vec::new();
        for (id, operation) in &mut self.operations {
            let Some(result) = operation
                .take_receipt()
                .or_else(|| operation.strategy_resolution(session))
            else {
                continue;
            };
            operation.result = result.clone();
            events.push((
                operation.core,
                Some(operation.who.grant_id.clone()),
                json!({"kind":"operation_receipt","operation_id":id,"result":result}),
            ));
            if let Ok(mut auth) = shared.auth.lock() {
                let outcome = result["status"].as_str().unwrap_or("unknown");
                if let Err(e) = auth.audit_outcome(id, outcome) {
                    log::error!("MCP audit receipt could not be saved: {e}");
                }
            }
        }
        for core in session.sessions() {
            let Some(data) = session.store().core(core.id) else {
                continue;
            };
            let state = json!({"connection":format!("{:?}",data.status),"trading":data.strategies_running,
                "runtime_started":data.runtime_state.map(|s| s.is_started),"autodetect":data.runtime_state.map(|s| s.auto_detect_active)});
            if self.states.get(&core.id) != Some(&state) {
                self.states.insert(core.id, state.clone());
                events.push((
                    core.id,
                    None,
                    json!({"kind":"core_state_changed","state":state}),
                ));
            }
        }
        self.states
            .retain(|id, _| session.sessions().iter().any(|s| s.id == *id));
        for (core, grant, data) in events {
            self.event(core, grant, data);
        }
    }

    /// Append one event on change, retaining only the latest 512 and an absolute sequence cursor.
    fn event(&mut self, core: u64, grant: Option<String>, mut data: Value) {
        self.sequence = self.sequence.saturating_add(1);
        data["at_ms"] = json!(auth::now());
        self.events.push_back(Event {
            seq: self.sequence,
            core,
            grant,
            data,
        });
        while self.events.len() > 512 {
            self.events.pop_front();
        }
    }

    /// Dispatch only named, bounded semantic tools; all core selections are checked before reading.
    pub fn call(
        &mut self,
        session: &SessionManager,
        cfg: &AppConfig,
        shared: &Shared,
        who: &auth::Principal,
        name: &str,
        args: Value,
    ) -> Result<Value> {
        let grant = shared
            .auth
            .lock()
            .map_err(|_| anyhow::anyhow!("authorization unavailable"))?
            .check(who, Scope::Read, &[], auth::now())?;
        let core = args
            .get("core")
            .map(moon_core::automation::ids::parse)
            .transpose()?;
        if let Some(core) = core {
            authorize(shared, who, Scope::Read, &[core])?;
        }
        let require_core = || core.context("core must be a stable core ID");
        let mut result = match name {
            "station_overview" => {
                let cores: Vec<_> = cfg
                    .servers
                    .iter()
                    .filter_map(|server| {
                        let scopes = grant.scopes_on(server.id)?;
                        let mut state = brief(&core_status(session, server.id));
                        state["name"] = json!(server.name);
                        state["permissions"] = json!(scopes);
                        Some(state)
                    })
                    .collect();
                json!({"cores":cores,"now_ms":auth::now(),"log_observations_started":false,
                    "workflow":"For a deeper diagnosis use logs_start, wait the requested interval, logs_get, and relevant MoonDoc sections. Keep facts separate from hypotheses."})
            }
            "core_state" => state(
                session,
                cfg,
                require_core()?,
                string(&args, "section")?,
                number(&args, "offset", 0) as usize,
            )?,
            "strategy_list" => session.automation_strategies(
                require_core()?,
                None,
                number(&args, "offset", 0) as usize,
            )?,
            "strategy_read" => {
                let mut read = session.automation_strategies(
                    require_core()?,
                    Some(moon_core::automation::ids::parse(&args["strategy_id"])?),
                    0,
                )?;
                if let Some(names) = args.get("fields").and_then(Value::as_array) {
                    let names: Vec<&str> = names.iter().filter_map(Value::as_str).collect();
                    keep_fields(&mut read, &names);
                }
                read
            }
            "strategy_schema" => session.automation_schema(
                require_core()?,
                args.get("strategy_kind")
                    .and_then(Value::as_u64)
                    .map(|v| v as u8),
                args.get("field").and_then(Value::as_str),
                number(&args, "offset", 0) as usize,
            )?,
            "market_read" => {
                session.automation_market(require_core()?, string(&args, "market")?)?
            }
            "settings_read" => session.automation_settings(
                require_core()?,
                args.get("field").and_then(Value::as_str),
                number(&args, "offset", 0) as usize,
            )?,
            "action_preview" => {
                let core = require_core()?;
                let action: Action = serde_json::from_value(
                    args.get("action").cloned().context("action required")?,
                )?;
                authorize(shared, who, action.scope(), &[core])?;
                if self.operations.len() >= 256 {
                    let retired = self
                        .operations
                        .iter()
                        .filter(|(_, operation)| {
                            operation.submitted
                                && operation.receipt.is_none()
                                && operation.admission.is_none()
                                && !(operation.result["status"] == "submitted"
                                    && !operation.result["expected"].is_null())
                        })
                        .min_by_key(|(_, operation)| operation.retain_until)
                        .map(|(id, _)| id.clone());
                    if let Some(retired) = retired {
                        self.operations.remove(&retired);
                    }
                }
                ensure!(
                    self.operations.len() < 256,
                    "too many active operations or previews; wait for their results or expiry"
                );
                let preview = session.automation_preview(core, &action)?;
                ensure!(
                    serde_json::to_vec(&preview)?.len() <= 128 * 1024,
                    "preview exceeds the response budget; select fewer fields or a smaller strategy"
                );
                let epoch = session
                    .store()
                    .core(core)
                    .context("core unavailable")?
                    .conn_epoch;
                let id = auth::random()?;
                self.operations.insert(
                    id.clone(),
                    Operation {
                        who: who.clone(),
                        core,
                        epoch,
                        action,
                        expected: preview.clone(),
                        expires: Instant::now() + Duration::from_secs(120),
                        retain_until: Instant::now() + Duration::from_secs(120),
                        submitted: false,
                        receipt: None,
                        admission: None,
                        result: json!({"status":"preview"}),
                    },
                );
                json!({"preview_id":id,"core":core.to_string(),"expires_in_seconds":120,"preview":preview,"submitted":false})
            }
            "action_apply" => self.apply(session, shared, who, string(&args, "preview_id")?)?,
            "operation_get" => {
                let id = string(&args, "operation_id")?;
                let operation = self.operations.get(id).context("operation unknown or expired; a restart clears local receipts, not already submitted core commands")?;
                ensure!(
                    operation.who.grant_id == who.grant_id,
                    "operation belongs to another authorization"
                );
                authorize(shared, who, Scope::Read, &[operation.core])?;
                let observed = match moon_core::automation::ids::parse(
                    &operation.result["strategy_id"],
                )
                .ok()
                {
                    Some(strategy) => session
                        .automation_strategies(operation.core, Some(strategy), 0)
                        .ok(),
                    None => None,
                };
                json!({"operation_id":id,"core":operation.core.to_string(),"result":operation.result,"observed_strategy":observed,
                    "core_state":core_status(session,operation.core),"instruction":"Compare the observed core state with the requested effect. A receipt alone is not proof of execution. Do not automatically retry unknown trading outcomes."})
            }
            "logs_start" => {
                let cores = cores(&args)?;
                authorize(shared, who, Scope::Read, &cores)?;
                self.captures
                    .start(session, who.clone(), cores, number(&args, "seconds", 30))?
            }
            "logs_get" | "logs_stop" => self.captures.get(
                string(&args, "capture_id")?,
                who,
                super::capture::Read {
                    offset: number(&args, "offset", 0) as usize,
                    lines: number(&args, "lines", 60) as usize,
                    contains: args.get("contains").and_then(Value::as_str),
                    summary: args.get("summary").and_then(Value::as_bool) == Some(true),
                },
                name == "logs_stop",
                shared,
            )?,
            "events_read" => {
                let after = number(&args, "after", 0);
                let events: Vec<_> = self
                    .events
                    .iter()
                    .filter(|e| {
                        e.seq > after
                            && grant.scopes_on(e.core).is_some()
                            && e.grant.as_ref().is_none_or(|id| *id == who.grant_id)
                    })
                    .take(100)
                    .map(|e| json!({"cursor":e.seq,"core":e.core.to_string(),"event":e.data}))
                    .collect();
                let cursor = events
                    .last()
                    .and_then(|e| e["cursor"].as_u64())
                    .unwrap_or(self.sequence);
                json!({"events":events,"next_cursor":cursor,"gap":after > self.sequence || self.events.front().is_some_and(|first| after>0 && after.saturating_add(1)<first.seq),
                    "delivery":"poll while connected; the AI host must schedule observation and notifications"})
            }
            super::tools::REPORT_SCOPE => {
                let cores = readable(cfg, &grant, &args["filter"]["cores"])?;
                authorize(shared, who, Scope::Read, &cores)?;
                let names: Vec<Value> = cores
                    .iter()
                    .filter_map(|id| session.store().core(*id).map(|core| (*id, core)))
                    .flat_map(|(id, core)| {
                        core.strategies
                            .iter()
                            .map(|s| json!([id.to_string(), s.id.to_string(), s.name]))
                            .collect::<Vec<_>>()
                    })
                    .collect();
                json!({"cores":cores.iter().map(u64::to_string).collect::<Vec<_>>(),"names":names})
            }
            "strategy_compare" => {
                let cores = readable(cfg, &grant, &args["cores"])?;
                authorize(shared, who, Scope::Read, &cores)?;
                let found: Vec<(String, BTreeMap<String, String>)> = cores
                    .iter()
                    .filter_map(|id| session.store().core(*id).map(|core| (*id, core)))
                    .flat_map(|(id, core)| {
                        core.strategies
                            .iter()
                            .filter(|s| {
                                s.name.eq_ignore_ascii_case(
                                    string(&args, "strategy_name").unwrap_or_default(),
                                )
                            })
                            .map(|s| (format!("{id}:{}", s.id), s.fields.iter().cloned().collect()))
                            .collect::<Vec<_>>()
                    })
                    .collect();
                compare(
                    string(&args, "strategy_name")?,
                    &found,
                    args.get("full_lists").and_then(Value::as_bool) == Some(true),
                )?
            }
            _ => bail!("unknown owner tool"),
        };
        if let Ok(knowledge) = shared.knowledge.read() {
            result["knowledge"] = knowledge.metadata();
            // The guide's section rides only where an edit is being prepared. Elsewhere it repeated
            // two kilobytes on every answer, and a client reads the guide with knowledge_read.
            let topic = match name {
                "strategy_schema" => Some("Strategies"),
                "action_preview" => Some("Commands"),
                _ => None,
            };
            if let Some(topic) = topic {
                result["task_context"] = knowledge.context(topic);
            }
        }
        Ok(result)
    }

    /// One admitted preview produces one operation. Repeated calls return its existing receipt.
    fn apply(
        &mut self,
        session: &SessionManager,
        shared: &Shared,
        who: &auth::Principal,
        id: &str,
    ) -> Result<Value> {
        let operation = self
            .operations
            .get_mut(id)
            .context("preview not found or expired")?;
        ensure!(
            operation.who.grant_id == who.grant_id,
            "preview belongs to another authorization"
        );
        authorize(shared, who, operation.action.scope(), &[operation.core])?;
        if operation.submitted {
            return Ok(json!({"operation_id":id,"result":operation.result,"reused":true}));
        }
        ensure!(
            Instant::now() < operation.expires,
            "preview expired; read state and preview again"
        );
        ensure!(
            session
                .store()
                .core(operation.core)
                .is_some_and(|c| c.conn_epoch == operation.epoch),
            "core reconnected since preview"
        );
        let auth = shared.auth.clone();
        let principal = who.clone();
        let core = operation.core;
        let scope = operation.action.scope();
        let authorization = Authorization(std::sync::Arc::new(move || {
            auth.lock()
                .ok()
                .is_some_and(|auth| auth.check(&principal, scope, &[core], auth::now()).is_ok())
        }));
        let summary =
            moon_core::automation::journal::summarize(&operation.action, &operation.expected);
        shared
            .auth
            .lock()
            .map_err(|_| anyhow::anyhow!("authorization unavailable"))?
            .audit(AuditEntry {
                at_ms: auth::now(),
                grant_id: who.grant_id.clone(),
                action: operation.action.name().into(),
                cores: vec![core],
                outcome: "admitted".into(),
                operation: id.to_owned(),
                client_name: String::new(),
                subject: summary.subject,
                changes: summary.changes,
                changes_omitted: summary.omitted,
            })?;
        let (reply, receipt) = mpsc::sync_channel(2);
        let submitted = session.automation_submit(
            core,
            Submission {
                action: operation.action.clone(),
                expected: operation.expected.clone(),
                expires: operation.expires,
                authorization,
                reply,
            },
        );
        if let Err(error) = submitted {
            // Nothing reached the core: "admitted" would stay in the journal forever otherwise.
            if let Ok(mut auth) = shared.auth.lock() {
                if let Err(e) = auth.audit_outcome(id, "rejected") {
                    log::error!("MCP audit receipt could not be saved: {e}");
                }
            }
            return Err(error);
        }
        operation.submitted = true;
        operation.expires = Instant::now() + Duration::from_secs(120);
        operation.receipt = Some(receipt);
        operation.retain_until = Instant::now() + Duration::from_secs(24 * 60 * 60);
        operation.result = json!({"status":"queued_in_station","execution_confirmed":false});
        Ok(json!({"operation_id":id,"result":operation.result,"reused":false}))
    }
}

/// A nonempty explicit core list is mandatory wherever an operation spans several bots.
pub(super) fn cores(args: &Value) -> Result<Vec<u64>> {
    let values = args["cores"]
        .as_array()
        .context("cores must be a nonempty array")?;
    ensure!(
        !values.is_empty() && values.len() <= 64,
        "select 1..64 cores"
    );
    let mut cores: Vec<_> = values
        .iter()
        .map(moon_core::automation::ids::parse)
        .collect::<Result<_>>()?;
    cores.sort_unstable();
    cores.dedup();
    Ok(cores)
}

/// Recheck capability without retaining the mutex over a core operation.
fn authorize(shared: &Shared, who: &auth::Principal, scope: Scope, cores: &[u64]) -> Result<()> {
    shared
        .auth
        .lock()
        .map_err(|_| anyhow::anyhow!("authorization unavailable"))?
        .check(who, scope, cores, auth::now())?;
    Ok(())
}

/// Compact core facts with unknowns kept as null and retained disconnected state labelled stale.
fn core_status(session: &SessionManager, id: u64) -> Value {
    let Some(core) = session.store().core(id) else {
        return json!({"core":id.to_string(),"connection":"unavailable","stale":true});
    };
    json!({"core":id.to_string(),"connection":format!("{:?}",core.status),"stale":core.status!=moon_core::feed::ConnStatus::Ready,
        "connection_epoch":core.conn_epoch,"version":core.server_version,"trading":core.strategies_running,
        "runtime_started":core.runtime_state.map(|s|s.is_started),"autodetect":core.runtime_state.map(|s|s.auto_detect_active),
        "cpu_percent":core.sys.process_cpu_percent,"memory_mb":core.sys.used_memory_mb,"transport_round_trip_ms":core.sys.round_trip_ms,
        "exchange_order_latency_ms":core.sys.order_api_latency_ms,"strategy_count":core.strategies.len(),"report_sync":core.report_sync.as_ref().map(|p|format!("{p:?}")),
        "problems":core.problems.supported.then_some(core.problems.items.len())})
}

/// The cores a tool names, or every core the grant reaches when it names none.
///
/// Errors:
///     A value that is not a decimal core id. A named core outside the grant is refused later by
///     the authorization check, never dropped quietly.
fn readable(
    cfg: &AppConfig,
    grant: &moon_core::station_api::mcp::GrantInfo,
    named: &Value,
) -> Result<Vec<u64>> {
    let named: Vec<u64> = named
        .as_array()
        .into_iter()
        .flatten()
        .map(|core| {
            core.as_str()
                .and_then(|s| s.parse().ok())
                .context("cores hold decimal core ids as strings")
        })
        .collect::<Result<_>>()?;
    Ok(if named.is_empty() {
        cfg.servers
            .iter()
            .map(|server| server.id)
            .filter(|id| grant.scopes_on(*id).is_some())
            .collect()
    } else {
        named
    })
}

/// The fields that differ between same-named live strategies.
///
/// A whole strategy is about 9 KB, so only the differences travel. A coin list runs to kilobytes;
/// unless asked for whole it is shown as its size and its difference from the first strategy.
///
/// Args:
///     name: The strategy name asked for.
///     found: Each strategy as `core:id` and its fields as the core formats them.
///     full_lists: Return long values whole.
///
/// Errors:
///     No live strategy of that name on the cores.
fn compare(
    name: &str,
    found: &[(String, BTreeMap<String, String>)],
    full_lists: bool,
) -> Result<Value> {
    ensure!(
        !found.is_empty(),
        "no live strategy named {name} on the authorized cores"
    );
    const LONG: usize = 100;
    let items = |value: &str| -> std::collections::BTreeSet<String> {
        value
            .split(',')
            .map(|item| item.trim().to_ascii_uppercase())
            .filter(|item| !item.is_empty())
            .collect()
    };
    let fields: std::collections::BTreeSet<&String> =
        found.iter().flat_map(|(_, fields)| fields.keys()).collect();
    let mut differing = serde_json::Map::new();
    let mut same = 0;
    for field in fields {
        let values: Vec<Option<&String>> = found.iter().map(|(_, f)| f.get(field)).collect();
        if values.windows(2).all(|pair| pair[0] == pair[1]) {
            same += 1;
            continue;
        }
        let long = !full_lists && values.iter().flatten().any(|v| v.len() > LONG);
        let first = values
            .first()
            .copied()
            .flatten()
            .map(|v| items(v))
            .unwrap_or_default();
        let per: serde_json::Map<String, Value> = found
            .iter()
            .zip(&values)
            .map(|((label, _), value)| {
                let shown = match value {
                    None => Value::Null,
                    Some(value) if long => {
                        let own = items(value);
                        json!({"items": own.len(),
                            "not_in_first": own.difference(&first).take(20).collect::<Vec<_>>(),
                            "missing_from_first": first.difference(&own).take(20).collect::<Vec<_>>()})
                    }
                    Some(value) => json!(value),
                };
                (label.clone(), shown)
            })
            .collect();
        differing.insert(field.clone(), per.into());
    }
    Ok(json!({
        "strategy_name": name,
        "strategies": found.iter().map(|(label, _)| label).collect::<Vec<_>>(),
        "differing_fields": differing,
        "same_fields": same,
        "null_means": "the core did not send the field: it equals its schema default or does not belong to that strategy's kind",
        "values_are_data": true,
    }))
}

/// The overview row of one core: what decides which core to look at, and only the departures from
/// normal beside it.
///
/// The overview is read first in every conversation and lists every core, so its size is paid
/// whatever the task: measured live, the full status was about 330 bytes a core, 9.5 KB for 28.
/// Load, latencies and version stay one `core_state` status away.
///
/// Args:
///     full: What `core_status` answers for the core.
fn brief(full: &Value) -> Value {
    let mut row = json!({"core":full["core"],"connection":full["connection"],"trading":full["trading"],"problems":full["problems"]});
    if full["stale"] == true {
        row["stale"] = json!(true);
    }
    // Unknown is not stopped: only a runtime the core says is not started is a departure.
    if full["runtime_started"] == false {
        row["runtime_started"] = json!(false);
    }
    if !full["report_sync"].is_null() {
        row["report_catch_up"] = json!(true);
    }
    row
}

/// Keep only the named fields of a strategy read, and say which named ones the core did not send.
///
/// A whole strategy is about 9 KB, most of it coin lists, so a client asking for one stop value
/// should not pay for all of it. The core omits a field whose value equals its schema default,
/// which is why an absent name is reported as such rather than as unknown: only the schema tells
/// the two apart.
///
/// Args:
///     read: The answer of `automation_strategies` for one strategy.
///     names: Field names the client asked for, matched ignoring case.
fn keep_fields(read: &mut Value, names: &[&str]) {
    let Some(strategies) = read.get_mut("strategies").and_then(Value::as_array_mut) else {
        return;
    };
    for strategy in strategies {
        let Some(fields) = strategy.get_mut("fields").and_then(Value::as_object_mut) else {
            continue;
        };
        // Matched ignoring case, as strategy_schema matches a field name, so `stoploss` finds
        // `StopLoss` instead of reading as a default-valued field.
        let wanted = |name: &str| names.iter().any(|asked| asked.eq_ignore_ascii_case(name));
        fields.retain(|name, _| wanted(name));
        let absent: Vec<&str> = names
            .iter()
            .copied()
            .filter(|asked| !fields.keys().any(|name| name.eq_ignore_ascii_case(asked)))
            .collect();
        if !absent.is_empty() {
            strategy["absent_fields"] = json!(absent);
            strategy["absent_fields_mean"] = json!(
                "not sent by the core: either equal to the schema default, or no field of this strategy's kind; strategy_schema with field set tells which"
            );
        }
    }
}

/// What the core's own detectors confirmed about it, kept apart from anything Station measured.
///
/// A list that never arrived means nothing is known, which must not read as healthy: the core
/// sends only confirmed findings, and an older core sends no list at all. The store keeps the last
/// list across a plain disconnect, so a list from a core that is not connected is marked stale
/// rather than passed off as current.
///
/// Args:
///     found: The core's last list.
///     connected: Whether the core is connected now.
///     scrub: The credential filter log lines pass through; the core's text is model-visible too.
fn problems(
    found: &moon_core::feed::CoreProblems,
    connected: bool,
    scrub: impl Fn(&str) -> String,
) -> Value {
    let items: Vec<_> = found
        .items
        .iter()
        .map(|problem| {
            json!({"kind":problem.kind_name,"category":format!("{:?}",problem.category),"title":scrub(&problem.title),
            "message":scrub(&problem.message),"details":scrub(&problem.technical_details),"first_seen_ms":problem.first_seen_ms,
            "confirmed_ms":problem.confirmed_ms,"confirmations":problem.confirmations})
        })
        .collect();
    json!({"known":found.supported,"stale":!connected,"count":found.supported.then_some(items.len()),"problems":items,"data_is_untrusted":true,
    "interpretation":match (found.supported, connected) {
        (true, true) => "Only what the core's own detectors confirmed. None listed means none confirmed, not that nothing is wrong.",
        (true, false) => "The last list the core sent before it disconnected; it may no longer hold.",
        (false, _) => "The core has delivered no problem list yet, or does not send one: nothing is known, which is not the same as healthy.",
    }})
}

/// Remove what must not reach an AI client from a core's own diagnostic text: addresses and any
/// configured core key appearing in it.
///
/// Not the log-line filter, which blanks a whole line at a word like "secret" or "api key": a
/// problem about an exchange key is exactly where those words appear, and blanking it would lose
/// the core's diagnosis where it matters most.
fn scrub(text: &str, cfg: &AppConfig) -> String {
    let mut out = moon_core::applog::redact::addresses(text).into_owned();
    for server in &cfg.servers {
        let key = server.key.expose();
        if key.len() >= 8 && out.contains(key) {
            out = out.replace(key, "[core key]");
        }
    }
    out
}

/// Read account values using their established unit/freshness contract rather than formatting UI text.
fn state(
    session: &SessionManager,
    cfg: &AppConfig,
    id: u64,
    section: &str,
    offset: usize,
) -> Result<Value> {
    let core = session.store().core(id).context("core unavailable")?;
    match section {
        "status" => Ok(core_status(session, id)),
        "problems" => Ok(problems(
            &core.problems,
            core.status == moon_core::feed::ConnStatus::Ready,
            |text| scrub(text, cfg),
        )),
        "orders" => session.automation_orders(id, offset),
        "positions" => session.automation_positions(id, offset),
        "balances" => {
            let assets = &core.assets;
            let rows: Vec<_> = assets
                .rows
                .iter()
                .skip(offset)
                .take(100)
                .map(|r| {
                    json!({"market":r.market,"coin":r.coin,"quote":r.quote,
                "quantity":r.qty,"quantity_full":r.qty_full,"value_usdt":r.value_usdt,
                "pnl_usdt":r.pnl_usdt,"pnl_is_live":r.pnl_live})
                })
                .collect();
            Ok(
                json!({"stale":core.assets_stale,"base_currency":assets.base_currency,"futures":assets.futures_account,"rows":rows,"next_offset":offset+rows.len(),
                "free_usdt":assets.global.free_usdt,"total_usdt":assets.global.total_usdt,"usd_valuation_complete":assets.global.usd_rate_known,
                "native_pnl":assets.global.total_pnl,"native_pnl_currency":assets.base_currency,
                "positions":"Use core_state section=positions for exact per-market quantities and both hedge legs."}),
            )
        }
        "settings" => Ok(
            json!({"stale":core.client_settings_stale,"manual_trading":core.client_settings.as_ref().map(|s|json!({
                "take_profit_percent":s.take_profit_pct,"stop_loss_percent":s.stop_loss_pct,"trailing_percent":s.trailing_drop_pct,
                "global_take_profit_enabled":s.use_global_take_profit,"global_take_profit_percent":s.global_take_profit_pct,"panic_if_price_drop":s.panic_if_price_drop,
                "emulator":s.emu_mode,"buy_iceberg":s.buy_iceberg,"sell_iceberg":s.sell_iceberg,"use_stop_market":s.use_stop_market,
                "use_manual_strategy":s.use_manual_strategy,"manual_strategy_id":s.manual_strategy_id.to_string()
            }))}),
        ),
        _ => bail!("unknown core state section"),
    }
}

#[cfg(test)]
mod tests;
