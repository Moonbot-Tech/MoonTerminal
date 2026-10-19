//! Test-only core-log calibration records and aggregate snapshot coverage inventory.

use super::*;

/// Fixed names make aggregate columns readable without printing log identifiers.
pub(super) const HALF_NAMES: [&str; 2] = ["fit", "test"];
pub(super) const OUTCOME_NAMES: [&str; 5] = [
    "task_started",
    "autostart_false",
    "filtercheck",
    "no_followup",
    "ambiguous",
];
const ANCHOR_NAMES: [&str; 8] = [
    "ask",
    "price_is_low",
    "daily_vol",
    "task_ms",
    "use_ask",
    "buy_price",
    "signal_pct",
    "filter",
];

/// The first terminal decision after a detect, including refusals before a task starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    TaskStarted,
    AutoStartFalse(String),
    FilterCheckRefused,
    NoFollowUp,
    Ambiguous,
}

/// One standalone detect; quoted detects in Signal lines only enrich this record.
#[derive(Clone, Debug)]
pub(super) struct LogDetect {
    pub core_label: String,
    pub core_uid: Option<u64>,
    pub strategy: String,
    pub coin: String,
    pub detect_ms: i64,
    pub ask: Option<f64>,
    pub x_price_delta: f64,
    pub price_is_low: bool,
    pub daily_vol_m: Option<f64>,
    pub outcome: Outcome,
    pub task_ms: Option<i64>,
    pub use_ask: Option<f64>,
    pub buy_price: Option<f64>,
    pub signal_pct: Option<f64>,
    pub task_id: Option<u64>,
    signal_consumed: bool,
}

/// Count unmatched relevant lines separately from unrelated log traffic.
#[derive(Default, Debug)]
pub(super) struct ParseStats {
    pub lines: usize,
    pub detects: usize,
    pub signals_unpaired: usize,
    pub unparsed: usize,
    pub other_lines: usize,
}

/// Keep parser accounting with the records so a malformed detect cannot vanish silently.
#[derive(Default)]
pub(super) struct Parsed {
    pub records: Vec<LogDetect>,
    pub stats: ParseStats,
}

/// Read a delimited angle-bracket name without interpreting its contents.
fn angle(text: &str) -> Option<&str> {
    Some(text.split_once('<')?.1.split_once('>')?.0)
}

/// Read one finite numeric token after a literal log field marker.
fn number(text: &str, marker: &str) -> Option<f64> {
    let token = text.split_once(marker)?.1.split_whitespace().next()?;
    let value: f64 = token.trim_end_matches(['%', ')', 'm']).parse().ok()?;
    value.is_finite().then_some(value)
}

/// Parse the leading millisecond clock, independently of the core's body clock.
fn clock_ms(line: &str) -> Option<i64> {
    let clock = line.split_whitespace().next()?;
    let (h, rest) = clock.split_once(':')?;
    let (m, rest) = rest.split_once(':')?;
    let (s, ms) = rest.split_once('.')?;
    if ms.len() != 3 {
        return None;
    }
    let (h, m, s, ms) = (
        h.parse::<i64>().ok()?,
        m.parse::<i64>().ok()?,
        s.parse::<i64>().ok()?,
        ms.parse::<i64>().ok()?,
    );
    ((0..24).contains(&h)
        && (0..60).contains(&m)
        && (0..60).contains(&s)
        && (0..1000).contains(&ms))
    .then_some(((h * 60 + m) * 60 + s) * 1000 + ms)
}

/// File dates are UTC, never the workstation's civil timezone.
fn midnight(date: &str) -> Option<i64> {
    Some(
        chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .ok()?
            .and_hms_opt(0, 0, 0)?
            .and_utc()
            .timestamp_millis(),
    )
}

/// Normalize only the market spelling and optional short annotation for task matching.
pub(super) fn coin_key(coin: &str) -> String {
    let coin = coin.trim_end_matches(':').trim_end_matches("<Short>");
    // USDT- is the core log grammar's quote-first marker, not a catalog market suffix.
    let coin = coin.strip_prefix("USDT-").unwrap_or(coin);
    coin_match_key(coin_of_market(coin))
}

/// Extract the task-prefix coin and task ID, not the bracketed worker index.
fn task_prefix(line: &str) -> Option<(&str, u64)> {
    let (prefix, rest) = line.split_once('[')?;
    let coin = prefix.split_whitespace().next_back()?.trim_end_matches(':');
    let coin = if coin.is_empty() {
        prefix.split_whitespace().rev().nth(1)?
    } else {
        coin
    };
    let id = rest.split_once("] (")?.1.split_once(')')?.0.parse().ok()?;
    Some((coin.trim_end_matches("<Short>"), id))
}

/// Parse one detect payload; quoted payloads use this only as a join key.
fn detect(label: &str, at: i64, payload: &str) -> Option<LogDetect> {
    let strategy = angle(payload)?;
    let rest = payload.split_once('>')?.1;
    let coin = rest.split_whitespace().next()?;
    let low = payload
        .split_once("PriceIsLow:")?
        .1
        .split_whitespace()
        .next()?;
    Some(LogDetect {
        core_label: label.to_string(),
        core_uid: None,
        strategy: strategy.to_string(),
        coin: coin.to_string(),
        detect_ms: at,
        ask: None,
        x_price_delta: number(payload, "xPriceDelta:")?,
        price_is_low: match low {
            "true" => true,
            "false" => false,
            _ => return None,
        },
        daily_vol_m: number(payload, "DailyVol:"),
        outcome: Outcome::NoFollowUp,
        task_ms: None,
        use_ask: None,
        buy_price: None,
        signal_pct: None,
        task_id: None,
        signal_consumed: false,
    })
}

/// Pair signals within one second and bind task IDs only when the candidate is unique.
#[cfg(test)]
pub(super) fn parse_core_log(label: &str, date: &str, text: &str) -> Parsed {
    parse_core_files(label, &[(date, text)])
}

/// Keep task ownership across sorted daily files of exactly one core label.
fn parse_core_files(label: &str, files: &[(&str, &str)]) -> Parsed {
    let mut out = Parsed::default();
    for &(date, text) in files {
        let Some(day) = midnight(date) else {
            out.stats.lines += text.lines().count();
            out.stats.unparsed += text.lines().count();
            continue;
        };
        let mut previous = None;
        for line in text.lines() {
            out.stats.lines += 1;
            let relevant = line.contains("DropsDetection:")
                || line.contains("FilterCheck Before Buy")
                || line.contains("Task ") && line.contains(" started;");
            out.stats.other_lines += usize::from(!relevant);
            let Some(clock) = clock_ms(line) else {
                out.stats.unparsed += usize::from(relevant);
                continue;
            };
            if previous.is_some_and(|prev| prev - clock > 43_200_000) {
                out.stats.unparsed += 1;
                continue;
            }
            previous = Some(clock);
            let at = day + clock;
            if let Some((_, payload)) = line.split_once("DropsDetection:") {
                let Some(parsed) = detect(label, at, payload) else {
                    out.stats.unparsed += 1;
                    continue;
                };
                if line.contains(" Signal ") {
                    let candidates: Vec<_> = out
                        .records
                        .iter()
                        .enumerate()
                        .filter(|(_, d)| {
                            d.strategy == parsed.strategy
                                && coin_key(&d.coin) == coin_key(&parsed.coin)
                                && (at - d.detect_ms).abs() <= 1000
                                && !d.signal_consumed
                        })
                        .map(|(i, d)| (i, (at - d.detect_ms).abs()))
                        .collect();
                    let nearest = candidates.iter().map(|(_, distance)| *distance).min();
                    let nearest: Vec<_> = candidates
                        .into_iter()
                        .filter(|(_, distance)| Some(*distance) == nearest)
                        .map(|(i, _)| i)
                        .collect();
                    let [index] = nearest.as_slice() else {
                        for i in nearest {
                            out.records[i].outcome = Outcome::Ambiguous;
                        }
                        out.stats.signals_unpaired += 1;
                        continue;
                    };
                    let row = &mut out.records[*index];
                    row.signal_consumed = true;
                    row.ask = number(line, "Ask:").filter(|p| *p > 0.0);
                    if line.contains("AutoStart: FALSE") {
                        let filter = line
                            .split_whitespace()
                            .find(|word| word.starts_with("BV_SV_"));
                        if let Some(filter) = filter {
                            row.outcome = Outcome::AutoStartFalse(filter.to_string());
                        } else {
                            row.outcome = Outcome::Ambiguous;
                            out.stats.unparsed += 1;
                        }
                    } else if !line.contains("AutoStart: TRUE") {
                        row.outcome = Outcome::Ambiguous;
                        out.stats.unparsed += 1;
                    }
                    out.stats.unparsed += usize::from(row.ask.is_none());
                } else {
                    out.records.push(parsed);
                    out.stats.detects += 1;
                }
                continue;
            }
            let Some((coin, task_id)) = task_prefix(line) else {
                if line.contains("FilterCheck Before Buy") {
                    let prefix = line.split_once('[').map_or_else(
                        || {
                            line.split_once("FilterCheck Before Buy")
                                .map_or(line, |(prefix, _)| prefix)
                        },
                        |(prefix, _)| prefix,
                    );
                    let coin = prefix
                        .split_whitespace()
                        .rev()
                        .find(|word| *word != ":")
                        .unwrap_or("")
                        .trim_end_matches(':');
                    for row in out.records.iter_mut().filter(|d| {
                        (0..=5000).contains(&(at - d.detect_ms))
                            && coin_key(&d.coin) == coin_key(coin)
                            && d.outcome == Outcome::NoFollowUp
                    }) {
                        row.outcome = Outcome::Ambiguous;
                    }
                }
                out.stats.unparsed += usize::from(relevant);
                continue;
            };
            let started = line.contains("Task ") && line.contains(" started;");
            let refused = line.contains("FilterCheck Before Buy");
            let strategy = started
                .then(|| {
                    line.split_once("(strategy ")
                        .and_then(|(_, rest)| angle(rest))
                })
                .flatten();
            let candidates: Vec<_> = out
                .records
                .iter()
                .enumerate()
                .filter(|(_, d)| {
                    (0..=5000).contains(&(at - d.detect_ms))
                        && coin_key(&d.coin) == coin_key(coin)
                        && !matches!(
                            d.outcome,
                            Outcome::AutoStartFalse(_) | Outcome::FilterCheckRefused
                        )
                        && strategy.is_none_or(|s| s == d.strategy)
                        && if refused {
                            d.task_id == Some(task_id)
                        } else {
                            d.task_id.is_none_or(|id| id == task_id)
                        }
                })
                .map(|(i, _)| i)
                .collect();
            // Existing ownership wins over unbound same-coin detects, including chatter.
            let bound: Vec<_> = candidates
                .iter()
                .copied()
                .filter(|&i| out.records[i].task_id == Some(task_id))
                .collect();
            let candidates = if bound.is_empty() { candidates } else { bound };
            if !started && !refused {
                if let [index] = candidates.as_slice() {
                    out.records[*index].task_id = Some(task_id);
                }
                continue;
            }
            if candidates.len() != 1 || started && strategy.is_none() {
                out.stats.unparsed += usize::from(started || refused);
                for i in candidates {
                    if out.records[i].outcome != Outcome::TaskStarted {
                        out.records[i].outcome = Outcome::Ambiguous;
                    }
                }
                if refused {
                    for row in out.records.iter_mut().filter(|d| {
                        (0..=5000).contains(&(at - d.detect_ms))
                            && coin_key(&d.coin) == coin_key(coin)
                            && d.task_id.is_none()
                            && d.outcome != Outcome::TaskStarted
                    }) {
                        row.outcome = Outcome::Ambiguous;
                    }
                }
                continue;
            }
            let row = &mut out.records[candidates[0]];
            // The first terminal decision owns the record; later FilterCheck is counted only.
            if row.outcome == Outcome::TaskStarted {
                continue;
            }
            // Intermediate task lines establish ownership before a refusal omits the strategy.
            row.task_id = Some(task_id);
            if started {
                row.outcome = Outcome::TaskStarted;
                row.task_ms = Some(at);
                row.use_ask = number(line, "UseAsk:");
                let buy = line.split_once("  BUY ").map(|(_, rest)| rest.trim());
                if let Some(buy) = buy {
                    if buy
                        .split_whitespace()
                        .next()
                        .is_some_and(|s| s.ends_with('%'))
                    {
                        row.signal_pct = number(line, "  BUY ");
                    } else {
                        row.buy_price = number(line, "  BUY ");
                        row.signal_pct = number(line, "(signal price ");
                    }
                }
                out.stats.unparsed += usize::from(
                    row.use_ask.is_none() || row.buy_price.is_none() && row.signal_pct.is_none(),
                );
            } else if refused {
                row.outcome = Outcome::FilterCheckRefused;
            }
        }
    }
    out
}

/// Refuse every sanitized label that was announced for more than one core UID.
pub(super) fn core_labels(app_logs: &str) -> HashMap<String, u64> {
    let mut labels: HashMap<String, Option<u64>> = HashMap::new();
    for (uid, name, _) in identity_lines(app_logs) {
        let Some(name) = name else { continue };
        labels
            .entry(crate::applog::sanitize_label(&name))
            .and_modify(|value| {
                if *value != Some(uid) {
                    *value = None;
                }
            })
            .or_insert(Some(uid));
    }
    labels
        .into_iter()
        .filter_map(|(label, uid)| Some((label, uid?)))
        .collect()
}

/// Refuse conflicting venues permanently rather than letting the final announcement win.
pub(super) fn core_venues_unique(app_logs: &str) -> HashMap<u64, Option<String>> {
    let mut venues: HashMap<u64, Option<String>> = HashMap::new();
    for (uid, _, venue) in identity_lines(app_logs) {
        venues
            .entry(uid)
            .and_modify(|value| {
                if value.as_ref() != Some(&venue) {
                    *value = None;
                }
            })
            .or_insert(Some(venue));
    }
    venues
}

/// Preserve exact duplicate-prefixed strategies before considering their base name.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Join {
    Exact(i64),
    PrefixStripped(i64),
    Ambiguous,
    Missing,
    ParamFallback,
}

/// Strip a numeric duplicate prefix only for the fallback query.
fn strip_prefix(name: &str) -> Option<&str> {
    let (n, rest) = name.strip_prefix('(')?.split_once(") ")?;
    (!n.is_empty() && n.bytes().all(|c| c.is_ascii_digit())).then_some(rest)
}

/// Query the exact core and raw name; database errors propagate instead of becoming misses.
pub(super) fn join_strategy(
    conn: &Connection,
    core_uid: u64,
    name: &str,
) -> rusqlite::Result<Join> {
    let mut stmt =
        conn.prepare("SELECT strategy_id FROM strategies WHERE core_uid = ?1 AND name = ?2")?;
    let mut historical=conn.prepare("SELECT DISTINCT strategy_id FROM strategy_versions WHERE core_uid=?1 AND json_extract(raw_json,'$.StrategyName')=?2")?;
    for (candidate, fallback) in
        std::iter::once((name, false)).chain(strip_prefix(name).map(|n| (n, true)))
    {
        let ids = stmt
            .query_map(rusqlite::params![core_uid as i64, candidate], |r| {
                r.get::<_, i64>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if ids.len() > 1 {
            return Ok(Join::Ambiguous);
        }
        let history = historical
            .query_map(rusqlite::params![core_uid as i64, candidate], |r| {
                r.get::<_, i64>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut all = ids.clone();
        all.extend(history);
        all.sort_unstable();
        all.dedup();
        if all.len() > 1 {
            return Ok(Join::ParamFallback);
        }
        match all.as_slice() {
            [] => {}
            [id] => {
                return Ok(if fallback {
                    Join::PrefixStripped(*id)
                } else {
                    Join::Exact(*id)
                });
            }
            _ => return Ok(Join::Ambiguous),
        }
    }
    Ok(Join::Missing)
}

/// Dump provenance is tracked for every model input; side is the schema's Short flag.
const INPUTS: [&str; 9] = [
    "DropsMaxTime",
    "DropsPriceMA",
    "DropsLastPriceMA",
    "DropsPriceDelta",
    "DropsPriceIsLow",
    "DropsUseLastPrice",
    "buyPrice",
    "buyPriceLastTrade",
    "Use30SecOldASK",
];

/// A parameter read either refuses short-side data or retains each field's source.
pub(super) enum Parameters {
    ShortSide,
    ParamFallback,
    PriceIsLowOn,
    Long {
        params: DropsParams,
        from_dump: [bool; 9],
    },
}

/// Resolve values at the detect rather than the current strategy head.
pub(super) fn parameters(conn: &Connection, id: i64, uid: u64, at: i64) -> Option<Parameters> {
    let mut stmt = conn.prepare("SELECT strategy_id, valid_from, valid_to, raw_json FROM strategy_versions WHERE core_uid=?1 ORDER BY valid_from").ok()?;
    let rows = stmt
        .query_map([uid as i64], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .ok()?;
    let name: String = conn
        .query_row(
            "SELECT name FROM strategies WHERE core_uid=?1 AND strategy_id=?2",
            rusqlite::params![uid as i64, id],
            |r| r.get(0),
        )
        .ok()?;
    let mut selected = None;
    let mut identities = std::collections::HashSet::new();
    let mut sides = std::collections::HashSet::new();
    let mut history = Vec::new();
    let mut names = std::collections::HashSet::from([name]);
    for row in rows {
        let (version_id, from, to, raw) = row.ok()?;
        let map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&raw).ok()?;
        let map = strategy_strings(map);
        if version_id == id {
            if let Some(name) = map.get("StrategyName") {
                names.insert(name.to_string());
            }
        }
        history.push((version_id, from, to, map));
    }
    for (version_id, from, to, map) in history {
        let historical_name = map.get("StrategyName");
        let side = StrategyValues {
            values: &map,
            defaults: &HashMap::new(),
        }
        .bool("Short", false);
        if historical_name.is_some_and(|name| names.contains(name)) || version_id == id {
            identities.insert(version_id);
            sides.insert(side);
        }
        if version_id == id && from <= at && to.is_none_or(|end| at < end) {
            if selected.is_some() {
                return Some(Parameters::ParamFallback);
            }
            selected = Some(map);
        }
    }
    if identities.len() != 1 || sides.len() != 1 || names.len() != 1 || selected.is_none() {
        return Some(Parameters::ParamFallback);
    }
    let values = selected?;
    let defaults = defaults(2.0);
    let v = StrategyValues {
        values: &values,
        defaults: &defaults,
    };
    if v.bool("Short", false) {
        return Some(Parameters::ShortSide);
    }
    if v.bool("DropsPriceIsLow", false) {
        return Some(Parameters::PriceIsLowOn);
    }
    Some(Parameters::Long {
        params: drops_params(&v),
        from_dump: INPUTS.map(|key| values.contains_key(key)),
    })
}

/// Normalize scalar dump fields once so side and model inputs use StrategyValues consistently.
fn strategy_strings(map: serde_json::Map<String, serde_json::Value>) -> HashMap<String, String> {
    map.into_iter()
        .filter_map(|(key, value)| {
            let value = match value {
                serde_json::Value::String(v) => v,
                serde_json::Value::Number(v) => v.to_string(),
                serde_json::Value::Bool(v) => if v { "YES" } else { "NO" }.into(),
                _ => return None,
            };
            Some((key, value))
        })
        .collect()
}

/// Query by coin using precisely the deal loader's venue and market ordering.
pub(super) fn tape_for_coin(
    venue: &str,
    coin: &str,
    pairs: &[(String, String)],
    spans: &Coverage,
) -> Option<(Vec<Tick>, Coverage)> {
    tape_for_coin_with(venue, coin, pairs, spans, held_ticks)
}

/// Inject only held IO so synthetic tests exercise the production selection and clipping.
pub(super) fn tape_for_coin_with(
    venue: &str,
    coin: &str,
    pairs: &[(String, String)],
    spans: &Coverage,
    mut read: impl FnMut(&str, &str, &Coverage) -> (Vec<Tick>, Coverage),
) -> Option<(Vec<Tick>, Coverage)> {
    let coin = coin_key(coin);
    pairs
        .iter()
        .filter(|(e, m)| e == venue && coin_key(m) == coin)
        .find_map(|(exchange, market)| {
            let (mut ticks, covered) = read(exchange, market, spans);
            if ticks.is_empty() && covered.clip(spans).is_empty() {
                return None;
            }
            ticks.retain(|tick| {
                spans
                    .spans()
                    .iter()
                    .any(|&(from, to)| tick.time_ms >= from as f64 && tick.time_ms <= to as f64)
            });
            ticks.sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
            Some((ticks, covered))
        })
}

/// Read all snapshot logs before readers are opened; count IO failures explicitly.
pub(super) fn snapshot_logs() -> std::io::Result<(Vec<LogDetect>, ParseStats, String, usize)> {
    let mut inputs = Vec::new();
    let mut failed = 0;
    for entry in std::fs::read_dir(paths::logs_dir_no_create())? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some((date, label)) = name.strip_suffix(".log").and_then(|n| n.split_once('_')) else {
            continue;
        };
        let text = match std::fs::read_to_string(entry.path()) {
            Ok(text) => text,
            Err(_) => {
                failed += 1;
                continue;
            }
        };
        inputs.push((date.to_string(), label.to_string(), text));
    }
    Ok(parse_log_snapshot(inputs, failed))
}

/// App-labelled files supply metadata only; their forwarded core messages are duplicates.
fn parse_log_snapshot(
    inputs: Vec<(String, String, String)>,
    failed: usize,
) -> (Vec<LogDetect>, ParseStats, String, usize) {
    let mut records = Vec::new();
    let mut stats = ParseStats::default();
    let mut app = String::new();
    let mut files: std::collections::BTreeMap<String, Vec<(String, String)>> = Default::default();
    for (date, label, text) in inputs {
        if label == "app" {
            app.push_str(&text);
            app.push('\n');
            continue;
        }
        files.entry(label).or_default().push((date, text));
    }
    for (label, mut files) in files {
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let chunks: Vec<_> = files
            .iter()
            .map(|(date, text)| (date.as_str(), text.as_str()))
            .collect();
        let parsed = parse_core_files(&label, &chunks);
        records.extend(parsed.records);
        stats.lines += parsed.stats.lines;
        stats.detects += parsed.stats.detects;
        stats.signals_unpaired += parsed.stats.signals_unpaired;
        stats.unparsed += parsed.stats.unparsed;
        stats.other_lines += parsed.stats.other_lines;
    }
    records.sort_by_key(|d| d.detect_ms);
    (records, stats, app, failed)
}

/// Accumulate only anonymous counts; neither identifiers nor prices reach stdout.
#[derive(Default)]
struct Inventory {
    core_unknown: usize,
    strategy_missing: usize,
    strategy_ambiguous: usize,
    short: usize,
    params_missing: usize,
    param_fallback: usize,
    price_is_low_on: usize,
    degenerate_params: usize,
    dump: [usize; 9],
    fallback: [usize; 9],
    coverage: [[[usize; 3]; 5]; 2],
    anchors: [usize; 8],
    kline: [usize; 4],
}

/// Select the first matching market on the requested venue, including untraded coins.
fn market_for<'a>(pairs: &'a [(String, String)], venue: &str, coin: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(e, m)| e == venue && coin_key(m) == coin_key(coin))
        .map(|(_, m)| m.as_str())
}

/// Open optional minute-candle diagnostics through one catalog reader for both benches.
pub(super) fn kline_sources() -> (Option<Vec<(String, String)>>, Option<KlineCache>) {
    let pairs =
        Connection::open_with_flags(paths::klines_db_path(), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .ok()
            .and_then(|db| {
                let mut stmt = db
                    .prepare(
                        "SELECT DISTINCT exchange, market FROM chunks_v2 ORDER BY exchange, market",
                    )
                    .ok()?;
                let rows = stmt
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                    .ok()?;
                rows.collect::<rusqlite::Result<Vec<_>>>().ok()
            });
    let cache = pairs
        .as_ref()
        .and_then(|_| KlineCache::open(paths::klines_db_path()));
    (pairs, cache)
}

/// Compare anchors with held minute candles as diagnostics, never as trade-tape evidence.
pub(super) fn kline_sanity(
    cache: Option<&KlineCache>,
    pairs: &[(String, String)],
    venue: &str,
    row: &LogDetect,
    p: &DropsParams,
    counts: &mut [usize; 4],
) {
    let (Some(cache), Some(market), Some(ask)) =
        (cache, market_for(pairs, venue, &row.coin), row.ask)
    else {
        return;
    };
    let minute = row.detect_ms.div_euclid(60_000) * 60_000;
    let from = row.detect_ms - (i64::from(p.max_time_s) + i64::from(p.price_ma_s)) * 1000;
    let Some(bars) = cache.read_range(
        venue,
        market,
        1,
        from.div_euclid(60_000) * 60_000,
        minute + 59_999,
    ) else {
        return;
    };
    if let Some(bar) = bars.iter().find(|bar| bar.t_open_ms as i64 == minute) {
        counts[1] += 1;
        counts[0] += usize::from(ask >= f64::from(bar.low) && ask <= f64::from(bar.high));
    }
    let span = Coverage::from_spans(
        bars.iter()
            .map(|bar| (bar.t_open_ms as i64, bar.t_open_ms as i64 + 59_999)),
    );
    if span.contains((from, row.detect_ms)) {
        let high = bars
            .iter()
            .map(|bar| f64::from(bar.high))
            .fold(0.0, f64::max);
        counts[3] += 1;
        counts[2] += usize::from(ask * (1.0 + row.x_price_delta / 100.0) <= high);
    }
}

/// Build the owner's coverage funnel only after the temporary data-root override is installed.
pub(super) fn inventory() {
    let Some(_permit) = crate::db::report_recovery::prepare() else {
        eprintln!("INV ERROR lease=1");
        return;
    };
    let Some(Ok(held)) =
        crate::market::trade_replay::trade_cache::handle().and_then(|c| c.inventory())
    else {
        eprintln!("INV ERROR inventory=1");
        return;
    };
    let Ok((records, stats, app, failed)) = snapshot_logs() else {
        eprintln!("INV ERROR logs=1");
        return;
    };
    let Ok(conn) = Connection::open_with_flags(
        paths::strategies_db_path(),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) else {
        eprintln!("INV ERROR strategies=1");
        return;
    };
    let labels = core_labels(&app);
    let venues = core_venues_unique(&app);
    let mut venue_ambiguous = 0;
    let mut dates: Vec<_> = records
        .iter()
        .map(|d| d.detect_ms.div_euclid(86_400_000))
        .collect();
    dates.sort_unstable();
    dates.dedup();
    let fit_dates = dates.len().div_ceil(2);
    let boundary = dates.get(fit_dates).copied();
    let fit_n = records
        .iter()
        .filter(|d| boundary.is_none_or(|b| d.detect_ms.div_euclid(86_400_000) < b))
        .count();
    eprintln!(
        "INV SPLIT fit_dates={} test_dates={} fit_detects={} test_detects={}",
        fit_dates,
        dates.len() - fit_dates,
        fit_n,
        records.len() - fit_n
    );
    let (kline_pairs, klines) = kline_sources();
    let mut tally = Inventory::default();
    for detect in records {
        let span = Coverage::one((detect.detect_ms - 600_000, detect.detect_ms + 15_000));
        let Ok(loaded) =
            heldout::load_row(detect, &conn, &labels, &venues, &held.keys, Some(&span))
        else {
            eprintln!("INV ERROR strategy_query=1");
            return;
        };
        let row = &loaded.detect;
        let side = usize::from(boundary.is_some_and(|b| row.detect_ms.div_euclid(86_400_000) >= b));
        let outcome = match &row.outcome {
            Outcome::TaskStarted => 0,
            Outcome::AutoStartFalse(_) => 1,
            Outcome::FilterCheckRefused => 2,
            Outcome::NoFollowUp => 3,
            Outcome::Ambiguous => 4,
        };
        tally.anchors[0] += usize::from(row.ask.is_some());
        tally.anchors[1] += usize::from(row.price_is_low);
        tally.anchors[2] += usize::from(row.daily_vol_m.is_some());
        tally.anchors[3] += usize::from(row.task_ms.is_some());
        tally.anchors[4] += usize::from(row.use_ask.is_some());
        tally.anchors[5] += usize::from(row.buy_price.is_some());
        tally.anchors[6] += usize::from(row.signal_pct.is_some());
        tally.anchors[7] += usize::from(
            matches!(&row.outcome, Outcome::AutoStartFalse(filter) if !filter.is_empty()),
        );
        let coverage = fit::coverage_class(&loaded.covered, &span);
        tally.coverage[side][outcome][coverage] += 1;
        match heldout::refusal(&loaded, 600) {
            Some(0) => tally.core_unknown += 1,
            Some(1) => venue_ambiguous += 1,
            Some(2) => tally.strategy_missing += 1,
            Some(3) => tally.strategy_ambiguous += 1,
            Some(4) => tally.short += 1,
            Some(5) => tally.params_missing += 1,
            Some(6) => tally.param_fallback += 1,
            Some(7) => tally.price_is_low_on += 1,
            Some(8) => tally.degenerate_params += 1,
            _ => {}
        }
        if let (Some(params), Some(from_dump)) = (&loaded.params, loaded.from_dump) {
            for (i, dump) in from_dump.into_iter().enumerate() {
                tally.dump[i] += usize::from(dump);
                tally.fallback[i] += usize::from(!dump);
            }
            if let Some(Some(venue)) = row.core_uid.and_then(|uid| venues.get(&uid)) {
                kline_sanity(
                    klines.as_ref(),
                    kline_pairs.as_deref().unwrap_or_default(),
                    venue,
                    row,
                    params,
                    &mut tally.kline,
                );
            }
        }
    }
    eprintln!(
        "INV FUNNEL parsed={} unpaired={} malformed={} other_lines={} lines={} files_unreadable={} core_unknown={} strategy_missing={} ambiguous={} short={} params_missing={} param_fallback={} price_is_low_on={} venue_ambiguous={} degenerate_params={}",
        stats.detects,
        stats.signals_unpaired,
        stats.unparsed,
        stats.other_lines,
        stats.lines,
        failed,
        tally.core_unknown,
        tally.strategy_missing,
        tally.strategy_ambiguous,
        tally.short,
        tally.params_missing,
        tally.param_fallback,
        tally.price_is_low_on,
        venue_ambiguous,
        tally.degenerate_params
    );
    for (i, (dump, default)) in tally.dump.iter().zip(tally.fallback).enumerate() {
        eprintln!(
            "INV PARAM field={} dump={dump} default={default}",
            INPUTS[i]
        );
    }
    for (half, outcomes) in tally.coverage.iter().enumerate() {
        for (outcome, [full, partial, none]) in outcomes.iter().enumerate() {
            eprintln!(
                "INV COVERAGE half={} outcome={} full={full} partial={partial} none={none}",
                HALF_NAMES[half], OUTCOME_NAMES[outcome]
            );
        }
    }
    for (field, present) in tally.anchors.into_iter().enumerate() {
        eprintln!("INV ANCHOR field={} present={present}", ANCHOR_NAMES[field]);
    }
    eprintln!(
        "INV KLINE ask_in_minute={} ask_checked={} implied_high_le_minute_high={} high_checked={} cache_missing={}",
        tally.kline[0],
        tally.kline[1],
        tally.kline[2],
        tally.kline[3],
        usize::from(klines.is_none())
    );
    drop(klines);
    drop(conn);
    eprintln!("INV DONE");
}

#[cfg(test)]
mod tests;
