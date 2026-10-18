//! Retention capability, deletion estimates and station-reported maintenance copy.

use super::{PASS_EVERY_MS, Tile, TileHint, TileValue, clock, plural_key};
use chrono_tz::Tz;
use moon_core::station_api::{PurgeFail, TapeCatchup, TapeKeep, TapePurge, TapeWindow};
use rust_i18n::t;

/// One write-bar message; widening warns even when station counts or capabilities are absent.
pub(super) fn bar_message(
    ed: &super::TapeEd,
    c: Option<&TapeCatchup>,
    busy: bool,
) -> (String, bool) {
    let changed = ed.policy_changed();
    let warning = ed.seen.zip(ed.draft).and_then(|(s, d)| {
        widen_warning(
            s,
            d,
            c.and_then(|c| c.tape_trades),
            c.and_then(|c| c.catchup_rate_per_hour),
            c.and_then(reported_keep).is_some(),
        )
    });
    if matches!(keep_state(c), KeepState::Purging { .. }) || ed.awaiting_keep.is_some() {
        return (t!("telegram.server.keep_bar_purging").to_string(), false);
    }
    if ed.confirm {
        let msg = t!("telegram.server.keep_bar_confirm").to_string();
        return warning.map_or((msg.clone(), false), |warning| {
            (format!("{msg} {warning}"), true)
        });
    }
    let line = super::tape_status(super::TapeFacts {
        known: ed.draft.is_some(),
        dirty: changed,
        busy,
        older_service: c.is_some(),
    });
    if line == super::TapeStatus::Busy {
        return (t!("telegram.server.keep_bar_busy").to_string(), false);
    }
    if line == super::TapeStatus::Clean && warning.is_none() {
        return (t!("telegram.server.tape_clean").to_string(), false);
    }
    if let Some((seen, draft)) = ed.seen_keep.zip(ed.draft_keep)
        && removes_tape(seen, draft)
    {
        let why = deletion_reason(Some(seen), draft);
        let counts = c
            .and_then(|c| purge_counts(c, draft))
            .filter(|_| !seen.drops_untunable(draft));
        let msg = match counts {
            Some(counts) => t!(
                &plural_key("telegram.server.keep_bar_remove", counts.total),
                n = counts.total,
                why = why
            )
            .to_string(),
            None => t!("telegram.server.keep_bar_remove_unknown", why = why).to_string(),
        };
        return warning.map_or((msg.clone(), false), |warning| {
            (format!("{msg} {warning}"), true)
        });
    }
    if let Some(warning) = warning {
        return (warning, true);
    }
    if ed.seen_keep != ed.draft_keep {
        return (t!("telegram.server.keep_bar_lengthen").to_string(), false);
    }
    (t!("settings.savebar.dirty_other").to_string(), false)
}

/// Only complete capability reports authorize writing both retention keys.
pub(super) fn reported_keep(c: &TapeCatchup) -> Option<TapeKeep> {
    Some(TapeKeep {
        max_age_days: c.max_age_days?,
        record_emulator: c.record_emulator?,
        tuner_only: c.tuner_only,
        min_tail_s: c.min_tail_s,
    })
}

/// Unsupported stations never receive retention keys, even with a retained draft.
pub(super) fn job_keep(seen: Option<TapeKeep>, draft: Option<TapeKeep>) -> Option<TapeKeep> {
    let seen = seen?;
    let mut draft = draft?;
    if seen.tuner_only.is_none() {
        draft.tuner_only = None;
    }
    draft.min_tail_s = seen.min_tail_s.map(|_| crate::analytics::tape_min_tail_s());
    Some(draft)
}

/// Zero means infinity; emulator removal is independent of the age horizon.
pub(super) fn removes_tape(seen: TapeKeep, draft: TapeKeep) -> bool {
    seen.removes_tape(draft)
}

/// Emulator removal owns that category regardless of age, avoiding overlapping deletion counts.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct PurgeCounts {
    pub older: u64,
    pub emulator: u64,
    pub total: u64,
}

/// Missing histograms are unknown, not a promise that deletion removes zero trades.
pub(super) fn purge_counts(c: &TapeCatchup, draft: TapeKeep) -> Option<PurgeCounts> {
    c.tape_trades?;
    if c.tape_ages.is_empty() {
        return None;
    }
    let mut counts = PurgeCounts {
        older: 0,
        emulator: 0,
        total: 0,
    };
    for age in &c.tape_ages {
        if draft.max_age_days != 0 && age.days >= draft.max_age_days {
            counts.older += u64::from(age.plain)
                + if draft.record_emulator {
                    u64::from(age.emulator)
                } else {
                    0
                };
        }
        if !draft.record_emulator {
            counts.emulator += u64::from(age.emulator);
        }
    }
    counts.total = counts.older + counts.emulator;
    Some(counts)
}

/// Maintenance state remains independent of automatic catch-up being disabled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum KeepState {
    Old,
    Editable,
    Purging { frac: f32 },
    Done { freed: u64, at: i64 },
    Failed { reason: PurgeFail },
}

/// Done and Deferred yield to newer catch-up passes; actual failures stay visible.
pub(super) fn keep_state(c: Option<&TapeCatchup>) -> KeepState {
    let Some(c) = c else {
        return KeepState::Old;
    };
    if reported_keep(c).is_none() {
        return KeepState::Old;
    }
    match c.purge {
        Some(TapePurge::Running { done, total }) => KeepState::Purging {
            frac: if total == 0 {
                0.0
            } else {
                (done as f64 / total as f64).clamp(0.0, 1.0) as f32
            },
        },
        Some(TapePurge::Done { at_ms, freed_bytes })
            if c.last_pass_ms.is_none_or(|pass| pass < at_ms) =>
        {
            KeepState::Done {
                freed: freed_bytes,
                at: at_ms,
            }
        }
        Some(TapePurge::Failed { reason, at_ms })
            if reason != PurgeFail::Deferred || c.last_pass_ms.is_none_or(|pass| pass < at_ms) =>
        {
            KeepState::Failed { reason }
        }
        _ => KeepState::Editable,
    }
}

/// Localized periods use the approved year presets, otherwise the language's day plural.
pub(super) fn period(days: u16) -> String {
    match days {
        0 => t!("telegram.server.keep_forever").to_string(),
        365 => t!("telegram.server.keep_year").to_string(),
        1825 => t!("telegram.server.keep_five_years").to_string(),
        n => t!(&plural_key("telegram.server.keep_days", n.into()), n = n).to_string(),
    }
}

/// Retention may exceed one year; the note never promises an older catch-up source.
pub(super) fn keep_note(days: u16) -> String {
    if days == 0 {
        return t!("telegram.server.keep_note_forever").to_string();
    }
    let reach = if days < 365 {
        period(days)
    } else {
        t!("telegram.server.keep_reach_year").to_string()
    };
    t!(
        "telegram.server.keep_note_finite",
        period = period(days),
        reach = reach
    )
    .to_string()
}

/// The draft bar names only settings that actually narrow the acknowledged policy.
pub(super) fn deletion_reason(seen: Option<TapeKeep>, draft: TapeKeep) -> String {
    let mut parts = Vec::new();
    if seen.is_some_and(|s| s.narrows_age(draft)) {
        parts.push(
            t!(
                "telegram.server.keep_older",
                period = period(draft.max_age_days)
            )
            .to_string(),
        );
    }
    if seen.is_some_and(|s| s.drops_emulator(draft)) {
        parts.push(t!("telegram.server.keep_emu_reason").to_string());
    }
    if seen.is_some_and(|s| s.drops_untunable(draft)) {
        parts.push(t!("telegram.server.keep_tuner_reason").to_string());
    }
    parts.join(&t!("telegram.server.keep_join"))
}

/// Every wider window warns; only retention-capable stations can promise the CPU cap.
pub(super) fn widen_warning(
    seen: TapeWindow,
    draft: TapeWindow,
    trades: Option<u32>,
    rate: Option<u32>,
    supports_cpu_cap: bool,
) -> Option<String> {
    if draft.margin_s <= seen.margin_s && draft.long_position_min <= seen.long_position_min {
        return None;
    }
    if !supports_cpu_cap {
        return Some(t!("telegram.server.keep_widen_no_cap").to_string());
    }
    let Some(n) = trades else {
        return Some(t!("telegram.server.keep_widen_unknown").to_string());
    };
    let term = rate
        .filter(|rate| *rate > 0)
        .map(|rate| {
            let duration =
                crate::display_text::fmt_duration_short(f64::from(n) / f64::from(rate) * 3600.0);
            t!("telegram.server.keep_widen_term", duration = duration).to_string()
        })
        .unwrap_or_default();
    Some(
        t!(
            &plural_key("telegram.server.keep_widen", n.into()),
            n = n,
            term = term
        )
        .to_string(),
    )
}

/// A rolled-back deletion differs from committed deletion whose file did not shrink.
pub(super) fn purge_sentence(c: &TapeCatchup, zone: Tz) -> Option<String> {
    match keep_state(Some(c)) {
        KeepState::Purging { .. } => Some(t!("telegram.server.keep_purging").to_string()),
        KeepState::Done { at, .. } if c.auto == Some(false) => Some(
            t!(
                "telegram.server.keep_done_off",
                at = clock(at, zone),
                n = c.pending
            )
            .to_string(),
        ),
        KeepState::Done { at, .. } => Some(
            t!(
                &plural_key("telegram.server.keep_done", c.pending.into()),
                at = clock(at, zone),
                n = c.pending,
                next = c
                    .last_pass_ms
                    .map(|pass| clock(pass + PASS_EVERY_MS, zone))
                    .unwrap_or_else(|| t!("telegram.server.keep_next_pass").to_string())
            )
            .to_string(),
        ),
        KeepState::Failed {
            reason: PurgeFail::DeleteFailed,
        } => Some(t!("telegram.server.keep_delete_failed").to_string()),
        KeepState::Failed {
            reason: PurgeFail::DeleteNoRoom,
        } => Some(t!("telegram.server.keep_delete_no_room").to_string()),
        KeepState::Failed {
            reason: PurgeFail::Deferred,
        } => Some(t!("telegram.server.keep_compact_pending").to_string()),
        KeepState::Failed { reason } => {
            let key = match reason {
                PurgeFail::NoRoom => "telegram.server.keep_fail_no_room",
                PurgeFail::NotShrinkable => "telegram.server.keep_fail_not_shrinkable",
                _ => "telegram.server.keep_fail_other",
            };
            Some(t!("telegram.server.keep_failed", reason = t!(key)).to_string())
        }
        _ => None,
    }
}

/// A reported database size includes the WAL; unknown sizes remain a dash.
pub(super) fn db_tile(c: &TapeCatchup) -> Tile {
    let size = |bytes: u64| {
        t!(
            "telegram.server.keep_mb",
            n = moon_core::util::fmt::group_thousands(&bytes.div_ceil(1_048_576).to_string())
        )
        .to_string()
    };
    let tape = c
        .tape_trades
        .map(|n| {
            t!(
                &plural_key("telegram.server.keep_db_trades", n.into()),
                n = n
            )
            .to_string()
        })
        .unwrap_or_else(|| t!("telegram.server.keep_db_unknown").to_string());
    let state = keep_state(Some(c));
    let hint = match state {
        KeepState::Purging { frac } => t!(
            "telegram.server.keep_db_progress",
            pct = (frac * 100.0).round() as u32
        )
        .to_string(),
        KeepState::Done { freed, .. } => t!(
            "telegram.server.keep_db_freed",
            size = size(freed),
            tape = tape
        )
        .to_string(),
        KeepState::Failed {
            reason: PurgeFail::DeleteFailed | PurgeFail::DeleteNoRoom,
        } => t!("telegram.server.keep_db_delete_failed").to_string(),
        KeepState::Failed {
            reason: PurgeFail::Deferred,
        } => t!("telegram.server.keep_compact_pending").to_string(),
        KeepState::Failed { .. } => t!("telegram.server.keep_db_not_shrunk").to_string(),
        _ => tape,
    };
    Tile {
        value: TileValue::Text(c.db_bytes.map(size).unwrap_or_else(|| "—".into())),
        label: "telegram.server.keep_db",
        hint: TileHint::Text(hint),
        accent: matches!(state, KeepState::Done { .. }),
    }
}
