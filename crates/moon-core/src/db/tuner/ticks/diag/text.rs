//! The report as text: one header, the funnel, the cores, the segments — each line built only
//! from counts, relative times and per-cent deviations ([`super`]'s module doc says why).

use std::collections::BTreeSet;
use std::fmt::Write;

use super::super::settings::ModelSettings;
use super::super::verify::RuleFlags;
use super::{CoreLine, REPORT_VERSION, Report, ReportInput, Segment, TapeClass};

/// Below this share of the table's rows with tape, the report opens by saying so: what follows
/// describes a sample too thin to say anything about the model.
const THIN_TAPE_SHARE: f64 = 0.5;

/// The report's text.
pub(super) fn render(input: &ReportInput, report: &Report) -> String {
    let mut out = String::new();
    header(&mut out, input, report);
    funnel(&mut out, input, report);
    cores(&mut out, input, report);
    segments(&mut out, report);
    out
}

/// The format, the build, the scope and the model settings that differ from the defaults.
fn header(out: &mut String, input: &ReportInput, report: &Report) {
    let _ = writeln!(
        out,
        "MoonTerminal Entry/Exit tuner report v{REPORT_VERSION} · build {}",
        input.build
    );
    let kinds: BTreeSet<&str> = input.rows.iter().map(|r| r.deal.kind.as_str()).collect();
    let period = input
        .period_days
        .map_or_else(|| "open period".to_string(), |d| format!("{d:.1} days"));
    let _ = writeln!(
        out,
        "scope: {period} · {} core(s) · {} row(s) · kinds: {}",
        report.cores.len(),
        report.funnel.rows,
        kinds.into_iter().collect::<Vec<_>>().join(", ")
    );
    let changes = model_changes(&input.model);
    if changes.is_empty() {
        let _ = writeln!(out, "model settings: default");
    } else {
        let _ = writeln!(out, "model settings changed: {}", changes.join(" · "));
    }
    let covered = report
        .funnel
        .tape
        .get(&TapeClass::Covered)
        .copied()
        .unwrap_or(0);
    if (covered as f64) < report.funnel.rows as f64 * THIN_TAPE_SHARE {
        let _ = writeln!(
            out,
            "NOTE: tape covers {covered} of {} row(s) — load the tape on the axis first, then \
             make the report again",
            report.funnel.rows
        );
    }
}

/// Every model setting that differs from its default, as `name value (default x)`.
fn model_changes(model: &ModelSettings) -> Vec<String> {
    let (Ok(serde_json::Value::Object(now)), Ok(serde_json::Value::Object(default))) = (
        serde_json::to_value(model),
        serde_json::to_value(ModelSettings::default()),
    ) else {
        return Vec::new();
    };
    now.iter()
        .filter(|(key, value)| default.get(*key) != Some(value))
        .map(|(key, value)| match default.get(key) {
            Some(was) => format!("{key} {value} (default {was})"),
            None => format!("{key} {value}"),
        })
        .collect()
}

/// From the scope's rows to the judged sample.
fn funnel(out: &mut String, input: &ReportInput, report: &Report) {
    let f = &report.funnel;
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "left out before the table: no ms stamps {} · service rows {} · kind or exit not tunable {}",
        input.without_ms, input.service, input.untunable
    );
    let tape: Vec<String> = f
        .tape
        .iter()
        .map(|(class, n)| match f.tape_venues.get(class) {
            Some(venues) if !venues.is_empty() => format!(
                "{} {n} [{}]",
                class.label(),
                venues.iter().cloned().collect::<Vec<_>>().join(", ")
            ),
            _ => format!("{} {n}", class.label()),
        })
        .collect();
    let _ = writeln!(out, "tape of {} row(s): {}", f.rows, tape.join(" · "));
    if f.holed > 0 {
        let _ = writeln!(out, "covered with a hole in the tape: {}", f.holed);
    }
    if f.unanswered > 0 {
        let _ = writeln!(
            out,
            "covered, no verdict yet (in the shares, not in the segments): {}",
            f.unanswered
        );
    }
    let share = |hits: usize, n: usize| match n {
        0 => "—".to_string(),
        n => format!("{hits}/{n} ({:.1}%)", hits as f64 / n as f64 * 100.0),
    };
    let _ = writeln!(
        out,
        "accuracy over covered rows: entry ✓ {} · exit ✓ {}",
        share(f.entry.hits, f.entry.n),
        share(f.exit.hits, f.exit.n)
    );
}

/// One line per core, numbered C1…CN by rows.
fn cores(out: &mut String, input: &ReportInput, report: &Report) {
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "cores (fill stamp − tape: the report's buy stamp minus the nearest print at its price, ms):"
    );
    for (i, core) in report.cores.iter().enumerate() {
        let _ = writeln!(out, "  C{} {}", i + 1, core_line(core, input));
    }
}

/// A core's line, after its number.
fn core_line(core: &CoreLine, input: &ReportInput) -> String {
    let mut parts = vec![core.venue.clone().unwrap_or_else(|| "?".to_string())];
    match input.cores.get(&core.uid) {
        Some(facts) if !facts.connected => {
            parts.push("offline".to_string());
            if let Some(secs) = facts.tz_offset_secs {
                parts.push(format!("tz {:+}h", f64::from(secs) / 3600.0));
            }
        }
        Some(facts) => {
            parts.push(match facts.version {
                Some(v) => format!("build {}", crate::util::fmt::core_build(v)),
                None => "build not reported".to_string(),
            });
            parts.push(match facts.tz_offset_secs {
                Some(secs) => format!("tz {:+}h", f64::from(secs) / 3600.0),
                None => "tz not adopted".to_string(),
            });
        }
        // Not in the session at all: nothing it reported can be read.
        None => parts.push("not in the session".to_string()),
    }
    parts.push(format!("rows {} · covered {}", core.rows, core.covered));
    // Only rows the model answered for carry the clock reading and the calibration.
    if !core.clock.is_empty() || core.clock_unmatched > 0 {
        parts.push(if core.step_lag_ms > 0.0 {
            format!("step lag {:.0} ms", core.step_lag_ms)
        } else {
            "step lag not calibrated".to_string()
        });
        parts.push(match core.round_trip_ms {
            Some(ms) => format!("round trip {ms:.0} ms"),
            None => "round trip not calibrated".to_string(),
        });
        parts.push(
            match (
                quantile_i(&core.clock, 0.5),
                quantile_i(&core.clock, 0.1),
                quantile_i(&core.clock, 0.9),
            ) {
                (Some(med), Some(p10), Some(p90)) => format!(
                    "fill stamp − tape med {med:+} · p10 {p10:+} · p90 {p90:+} (n {}, no print {})",
                    core.clock.len(),
                    core.clock_unmatched
                ),
                _ => format!(
                    "fill stamp − tape: no print at the fill price ({})",
                    core.clock_unmatched
                ),
            },
        );
    }
    parts.join(" · ")
}

/// Every segment, most ✗ and · first.
fn segments(out: &mut String, report: &Report) {
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "segments (kind · venue · side · core closed by): covered rows"
    );
    for seg in &report.segments {
        segment(out, seg);
    }
}

/// One segment's block.
fn segment(out: &mut String, seg: &Segment) {
    let k = &seg.key;
    let side = if k.short { "short" } else { "long" };
    let _ = writeln!(
        out,
        "{} · {} · {side} · {}: {}",
        k.kind,
        k.venue,
        k.close.label(),
        seg.n
    );
    let e = &seg.entry;
    if e.fact < seg.n {
        let mut line = format!("  entry ✓{} ✗{}", e.hits, e.unfilled + e.off.len());
        let mut why = Vec::new();
        if e.unfilled > 0 {
            why.push(format!("unfilled {}", e.unfilled));
        }
        if !e.off.is_empty() {
            why.push(format!(
                "off {}{}",
                e.off.len(),
                quantile_f(&e.off, 0.5).map_or(String::new(), |m| format!(", dev med {m:+.3}%"))
            ));
        }
        if !why.is_empty() {
            let _ = write!(line, " ({})", why.join(" · "));
        }
        let _ = writeln!(out, "{line}");
    }
    let x = &seg.exit;
    let unjudged: usize = x.unjudged.values().sum();
    let _ = writeln!(out, "  exit ✓{} ✗{} ·{}", x.hits, x.misses, unjudged);
    let mut misses = Vec::new();
    if x.stop_not_fired > 0 {
        misses.push(format!("stop not fired {}", x.stop_not_fired));
    }
    if x.no_level > 0 {
        misses.push(format!("no level at close {}", x.no_level));
    }
    let level_n = x.level.len() + x.level_no_dev;
    if level_n > 0 {
        misses.push(format!(
            "level {level_n}{}",
            match (quantile_f(&x.level, 0.5), quantile_f(&x.level, 0.9)) {
                (Some(med), Some(p90)) => format!(" (|dev| med {med:.3}% · p90 {p90:.3}%)"),
                _ => String::new(),
            }
        ));
    }
    if !x.late.is_empty() {
        misses.push(format!(
            "moment {}{}",
            x.late.len(),
            match (quantile_i(&x.late, 0.5), quantile_i(&x.late, 0.9)) {
                (Some(med), Some(p90)) =>
                    format!(" (model − core med {med:+} ms · p90 {p90:+} ms)"),
                _ => String::new(),
            }
        ));
    }
    if x.line_at_take + x.line_later > 0 {
        misses.push(format!(
            "line {} (at the take {} · at a later move {})",
            x.line_at_take + x.line_later,
            x.line_at_take,
            x.line_later
        ));
    }
    if !misses.is_empty() {
        let _ = writeln!(out, "    ✗ {}", misses.join(" · "));
    }
    if unjudged > 0 {
        let why: Vec<String> = x
            .unjudged
            .iter()
            .map(|(label, n)| format!("{label} {n}"))
            .collect();
        let _ = writeln!(out, "    · {}", why.join(" · "));
    }
    let rules: Vec<String> = RuleFlags::default()
        .named()
        .iter()
        .zip(seg.rules)
        .filter(|(_, n)| *n > 0)
        .map(|((name, _), n)| format!("{name} {n}"))
        .collect();
    if !rules.is_empty() {
        let _ = writeln!(out, "  rules on: {}", rules.join(" · "));
    }
    if !seg.outside.is_empty() {
        let fields: Vec<String> = seg
            .outside
            .iter()
            .map(|(field, n)| format!("{field} {n}"))
            .collect();
        let _ = writeln!(out, "  fields outside the model: {}", fields.join(" · "));
    }
}

/// The `q` quantile of the finite values, read at [`quantile_index`]; `None` without one.
fn quantile_f(values: &[f64], q: f64) -> Option<f64> {
    let mut sorted: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    sorted.sort_by(f64::total_cmp);
    quantile_index(sorted.len(), q).map(|i| sorted[i])
}

/// The `q` quantile, read at [`quantile_index`]; `None` for no values.
fn quantile_i(values: &[i64], q: f64) -> Option<i64> {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    quantile_index(sorted.len(), q).map(|i| sorted[i])
}

/// The index of the `q` quantile among `n` sorted values: `(n − 1)·q` rounded half away from
/// zero, so the value is always one that occurred and an even count's median is its upper middle
/// — a diagnostic reads the shape, and no interpolated value stands for a trade. `None` for none.
fn quantile_index(n: usize, q: f64) -> Option<usize> {
    (n > 0).then(|| (((n - 1) as f64) * q.clamp(0.0, 1.0)).round() as usize)
}
