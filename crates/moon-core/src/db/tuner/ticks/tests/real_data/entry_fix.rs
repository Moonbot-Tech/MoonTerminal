//! Archived-deal evidence for the entry and take-rule decisions, confined to the bench.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::{Deal, StrategyValues, Verdict, verify};
use crate::db::tuner::ticks::exit::sell_order::archived_take;
use crate::db::tuner::ticks::hook::parse_hook_detect;

/// Modifier-free rule hits with a separate denominator for the YES rule's required buy level.
#[derive(Default)]
struct StrikeHits {
    n: usize,
    yes_n: usize,
    hits: [usize; 3],
    current_only: usize,
    skipped_modifier: usize,
}

/// Modifier-free stated hook takes compared to the depth and spike-price formulas.
#[derive(Default)]
struct HookHits {
    n: usize,
    depth: usize,
    spike: usize,
    skipped_modifier: usize,
}

/// Bounded examples and aggregate counts over the bench's covered population.
#[derive(Default)]
pub(super) struct EntryFixDiag {
    comments: HashMap<(u64, i64), String>,
    acb_values: BTreeMap<String, usize>,
    n_acb: usize,
    no_stamp: usize,
    late_creation: usize,
    late_move: usize,
    creation_examples: Vec<i64>,
    move_examples: Vec<i64>,
    strikes: BTreeMap<&'static str, StrikeHits>,
    hooks: BTreeMap<(&'static str, &'static str), HookHits>,
    arb: [usize; 2],
    post_only: [usize; 2],
}

impl EntryFixDiag {
    /// Read raw hook comments once, keyed as in `deals::overlay_hook_detect`.
    /// Missing columns are allowed; database failures abort the bench rather than hide data.
    pub(super) fn new(deals: &[Deal]) -> Self {
        let wanted: HashSet<_> = deals
            .iter()
            .filter(|d| d.kind == "MoonHook")
            .map(|d| (d.core_uid, d.report_uid))
            .collect();
        let mut diag = Self::default();
        if wanted.is_empty() {
            return diag;
        }
        let conn = crate::db::open_reader().expect("entry diagnostics report reader");
        for src in crate::db::read_sources_res(&conn).expect("entry diagnostics report sources") {
            if !["core_uid", "reportuid", "comment"]
                .iter()
                .all(|col| src.cols.contains(*col))
            {
                continue;
            }
            let sql = format!(
                "SELECT core_uid, reportuid, comment FROM \"{}\" WHERE comment LIKE '%Depth:%'",
                src.table
            );
            let mut stmt = conn
                .prepare(&sql)
                .expect("entry diagnostics comments query");
            let mut rows = stmt.query([]).expect("entry diagnostics comments");
            while let Some(row) = rows.next().expect("entry diagnostics comment row") {
                let key = (
                    row.get::<_, Option<i64>>(0)
                        .expect("comment core uid")
                        .unwrap_or(0) as u64,
                    row.get::<_, Option<i64>>(1)
                        .expect("comment report uid")
                        .unwrap_or(0),
                );
                if wanted.contains(&key) {
                    let comment = row
                        .get::<_, Option<String>>(2)
                        .expect("raw hook comment")
                        .unwrap_or_default();
                    if parse_hook_detect(&comment).is_some() {
                        diag.comments.entry(key).or_insert(comment);
                    }
                }
            }
        }
        diag
    }

    /// Count one covered deal using its raw settings, archived lines and archived verdict.
    pub(super) fn add(
        &mut self,
        deal: &Deal,
        values: &StrategyValues<'_>,
        entry_points: Option<&[(i64, f64)]>,
        exit_points: Option<&[(i64, f64)]>,
        verdict: &Verdict,
    ) {
        match deal.kind.as_str() {
            "MoonShot" => self.shot(deal, values, entry_points, verdict.entry),
            "MoonStrike"
                if deal
                    .sell_reason
                    .trim()
                    .eq_ignore_ascii_case(verify::REASON_TAKE) =>
            {
                self.strike(deal, values, exit_points);
            }
            "MoonHook" => self.hook(deal, values),
            _ => {}
        }
    }

    /// Compare raw creation and last archived replacement against the 15-second ACB floor.
    fn shot(
        &mut self,
        deal: &Deal,
        v: &StrategyValues<'_>,
        points: Option<&[(i64, f64)]>,
        entry: Option<bool>,
    ) {
        let acb = v.num("AutoCancelBuy", f64::NAN);
        let label = if acb.is_nan() {
            "unset".to_string()
        } else {
            acb.to_string()
        };
        *self.acb_values.entry(label).or_default() += 1;
        if acb > 0.0 {
            if let Some(created) = deal.buy_set_ms {
                self.n_acb += 1;
                let limit = acb.max(15.0) * 1000.0;
                let last = points
                    .map(verify::archived_replacements)
                    .and_then(|moves| moves.into_iter().rev().find(|(t, _)| *t <= deal.buy_ms))
                    .map_or(created, |(t, _)| t);
                if (deal.buy_ms - created) as f64 > limit {
                    self.late_creation += 1;
                    if self.creation_examples.len() < 10 {
                        self.creation_examples.push(deal.report_uid);
                    }
                }
                if (deal.buy_ms - last) as f64 > limit {
                    self.late_move += 1;
                    if self.move_examples.len() < 10 {
                        self.move_examples.push(deal.report_uid);
                    }
                }
            } else {
                self.no_stamp += 1;
            }
        }
        for (enabled, counts) in [
            (v.num("MShotAddArbRange", 0.0) != 0.0, &mut self.arb),
            (v.bool("UsePostOnly", false), &mut self.post_only),
        ] {
            if enabled {
                counts[0] += 1;
                counts[1] += usize::from(entry == Some(true));
            }
        }
    }

    /// Compare modifier-free strike predictions to the archived take, falling back to sale.
    fn strike(&mut self, deal: &Deal, v: &StrategyValues<'_>, points: Option<&[(i64, f64)]>) {
        let Some(d) = deal.strike_detect else {
            return;
        };
        let level = v.num("MStrikeSellLevel", f64::NAN);
        if !level.is_finite() {
            return;
        }
        let counts = self
            .strikes
            .entry(split(v, "MStrikeSellAdjust"))
            .or_default();
        if v.num("SellModifier", 0.0) != 0.0 {
            counts.skipped_modifier += 1;
            return;
        }
        let s = if deal.is_long() { 1.0 } else { -1.0 };
        let top = d.trade_price * (1.0 + s * d.depth_pct / 100.0);
        let f = |x: f64| x + level / 100.0 * (top - x);
        let deeper = |a: f64, b: f64| if deal.is_long() { a.min(b) } else { a.max(b) };
        let no = f(d.trade_price);
        let current = f(deeper(d.trade_price, deal.buy_price));
        let bl = v.num("MStrikeBuyLevel", f64::NAN);
        let yes = bl.is_finite().then(|| {
            let buy = if v.bool("MStrikeBuyRelative", true) {
                d.trade_price + bl / 100.0 * (top - d.trade_price)
            } else if deal.is_long() {
                top * (1.0 + bl / 100.0)
            } else {
                top / (1.0 + bl / 100.0)
            };
            deeper(deeper(no, f(buy)), f(deal.buy_price))
        });
        let reference = archived_take(points).unwrap_or(deal.sell_price);
        let hit = |p: f64| ((p / reference - 1.0) * 100.0).abs() <= 0.05;
        let hits = [hit(no), hit(current), yes.is_some_and(hit)];
        counts.n += 1;
        counts.yes_n += usize::from(yes.is_some());
        for (count, matched) in counts.hits.iter_mut().zip(hits) {
            *count += usize::from(matched);
        }
        // An unknown buy level cannot establish that the YES rule missed.
        counts.current_only += usize::from(yes.is_some() && hits[1] && !hits[0] && !hits[2]);
    }

    /// Compare modifier-free raw depth and stated take before `prepare_deal` changes the depth.
    fn hook(&mut self, deal: &Deal, v: &StrategyValues<'_>) {
        let Some(comment) = self.comments.get(&(deal.core_uid, deal.report_uid)) else {
            return;
        };
        let Some(d) = parse_hook_detect(comment) else {
            return;
        };
        let (Some(stated), Some(high), Some(min)) = (
            d.stated_take_pct,
            number_after(comment, "High:"),
            number_after(comment, "Min:"),
        ) else {
            return;
        };
        let level = v.num("HookSellLevel", f64::NAN);
        if !level.is_finite()
            || !high.is_finite()
            || !min.is_finite()
            || min <= 0.0
            || high <= min
            || (deal.buy_price / min - 1.0).abs() <= 0.001
        {
            return;
        }
        let spike = if deal.is_long() {
            ((min + level / 100.0 * (high - min)) / deal.buy_price - 1.0) * 100.0
        } else {
            (1.0 - (high - level / 100.0 * (high - min)) / deal.buy_price) * 100.0
        };
        let fixed = if v.bool("HookSellFixed", false) {
            "YES"
        } else {
            "NO/unset"
        };
        let side = if deal.is_long() { "long" } else { "short" };
        let counts = self.hooks.entry((fixed, side)).or_default();
        if v.num("SellModifier", 0.0) != 0.0 {
            counts.skipped_modifier += 1;
            return;
        }
        counts.n += 1;
        counts.depth += usize::from((stated - level * d.depth_pct / 100.0).abs() <= 0.01);
        counts.spike += usize::from((stated - spike).abs() <= 0.01);
    }

    /// Print ASCII summaries without changing the pairing script's per-deal line.
    pub(super) fn print(&self) {
        eprintln!("[M8] AutoCancelBuy_seconds {:?}", self.acb_values);
        eprintln!(
            "[M8] n_acb={} late_from_creation={} late_from_last_move={} no_stamp={}",
            self.n_acb, self.late_creation, self.late_move, self.no_stamp
        );
        eprintln!(
            "[M8] late_from_creation_uids={:?} late_from_last_move_uids={:?}",
            self.creation_examples, self.move_examples
        );
        for split in ["YES", "NO", "unset"] {
            let empty = StrikeHits::default();
            let c = self.strikes.get(split).unwrap_or(&empty);
            eprintln!(
                "[M2] Adjust={split} n={} no_hits={} current_hits={} yes_hits={} yes_n={} current_only={} skipped_modifier {}",
                c.n, c.hits[0], c.hits[1], c.hits[2], c.yes_n, c.current_only, c.skipped_modifier
            );
        }
        for fixed in ["NO/unset", "YES"] {
            for side in ["long", "short"] {
                let empty = HookHits::default();
                let c = self.hooks.get(&(fixed, side)).unwrap_or(&empty);
                eprintln!(
                    "[M1] Fixed={fixed} side={side} n={} depth_hits={} spike_hits={} skipped_modifier {}",
                    c.n, c.depth, c.spike, c.skipped_modifier
                );
            }
        }
        eprintln!(
            "[M6] ArbRange_nonzero={} archived_entry_true={} UsePostOnly_YES={} archived_entry_true={}",
            self.arb[0], self.arb[1], self.post_only[0], self.post_only[1]
        );
    }
}

/// Preserve explicit YES/NO versus unset, using the harness's schema defaults when present.
fn split(v: &StrategyValues<'_>, key: &str) -> &'static str {
    if !v.values.contains_key(key) && !v.defaults.contains_key(&key.to_ascii_lowercase()) {
        "unset"
    } else if v.bool(key, false) {
        "YES"
    } else {
        "NO"
    }
}

/// Read a bare number only at a word-boundary label, matching `strike::number_after`.
fn number_after(text: &str, label: &str) -> Option<f64> {
    let at = text.match_indices(label).map(|(i, _)| i).find(|&i| {
        text[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric())
    })?;
    let rest = text[at + label.len()..].trim_start();
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
        .unwrap_or(rest.len());
    rest[..end].parse::<f64>().ok()
}
