//! Synthetic deal reasons protect sample membership, translation coverage and independent marks.

use crate::analytics::tuner::ticks::tail::Admission;
use std::collections::HashSet;
use std::sync::Arc;

use super::{DealWhy, Tone};
use crate::analytics::tuner::ticks::state::{DealRow, RowAddress, TapeStatus};
use crate::analytics::tuner::ticks::tape::PackedTape;
use moon_core::db::tuner::ticks::exit::UnmodelledRule;
use moon_core::db::tuner::ticks::gap::TapeGap;
use moon_core::db::tuner::ticks::verify::{
    EntryFinding, ExitFinding, ExitMiss, MissParts, Unjudged,
};
use moon_core::db::tuner::ticks::{Deal, Deltas, Verdict};
use moon_core::market::trade_replay::TickStatus;

/// Build a coherent finding/verdict pair, rather than testing contradictory synthetic records.
fn verdict(entry_finding: EntryFinding, exit_finding: ExitFinding) -> Verdict {
    Verdict {
        entry: entry_finding.verdict(),
        exit: exit_finding.verdict(),
        entry_finding,
        exit_finding,
        entry_dev_pct: None,
        exit_dev_pct: None,
        fill: None,
        exit_kind: None,
        line_points: None,
        fill_clock_ms: None,
        rules: Default::default(),
        stop: Default::default(),
    }
}

/// A fully replayable synthetic trade; each test varies only its own exclusion axis.
fn row() -> DealRow {
    DealRow {
        deal: Deal {
            report_uid: 1,
            core_uid: 1,
            core_name: "CORE_A".into(),
            strategy_id: 1,
            kind: "Spread".into(),
            coin: "XALPHA".into(),
            buy_ms: 1_000,
            close_ms: 2_000,
            buy_price: 1.0,
            sell_price: 1.01,
            spent: 100.0,
            is_short: false,
            sell_reason: String::new(),
            fact_pnl: 0.0,
            sizing: None,
            pnl_pct: false,
            profit: None,
            deltas: Deltas::default(),
            tick: None,
            pre_spike_ask: None,
            archived_take: None,
            fact_modifier: None,
            hook_depth_pct: None,
            hook_stated_take_pct: None,
            strike_detect: None,
            step_lag_ms: 0.0,
            round_trip_ms: None,
            stop_anchor: None,
            delta_track: None,
            bars: None,
            own_entry: None,
            buy_set_ms: None,
            corridor: None,
            entry_placed: None,
            gap: None,
        },
        tape: TapeStatus::Covered,
        verdict: Some(verdict(EntryFinding::Hit, ExitFinding::Hit)),
        address: None,
        ticks: Some(PackedTape::pack(Vec::new())),
        entry_line: None,
        held: Some((0, 3_600_000)),
    }
}

/// A gap's length comes from its endpoints, never from the held post-close tail.
fn gap() -> TapeGap {
    TapeGap {
        from_ms: 1_100,
        to_ms: 120_100,
        fact_line: None,
        fact_stop: None,
    }
}

/// Explicit cases make new upstream variants fail the compiler or the locale inventory check.
fn statuses() -> [TickStatus; 12] {
    [
        TickStatus::Pending,
        TickStatus::Streaming,
        TickStatus::AwaitingCore,
        TickStatus::ContextUnavailable,
        TickStatus::NoRoute,
        TickStatus::ArchivePending {
            retry_at_ms: 216_000_000,
        },
        TickStatus::Disabled,
        TickStatus::OutOfRetention {
            retention_ms: 259_200_000,
        },
        TickStatus::NoTrades,
        TickStatus::RateLimited { retry_in_s: 7 },
        TickStatus::Failed,
        TickStatus::Served,
    ]
}

/// All named exits are represented, including same-rule disagreements and each unmodelled rule.
fn exits() -> Vec<ExitFinding> {
    vec![
        ExitFinding::Hit,
        ExitFinding::Miss(ExitMiss::StopNotFired),
        ExitFinding::Miss(ExitMiss::NoLevel),
        ExitFinding::Miss(ExitMiss::Off(MissParts {
            level: true,
            late_ms: Some(250),
            first_unmatched: Some(1),
        })),
        ExitFinding::Unjudged(Unjudged::Rule(UnmodelledRule::SellShot)),
        ExitFinding::Unjudged(Unjudged::Rule(UnmodelledRule::SellSpread)),
        ExitFinding::Unjudged(Unjudged::Rule(UnmodelledRule::NoAutoSell)),
        ExitFinding::Unjudged(Unjudged::InGap),
        ExitFinding::Unjudged(Unjudged::TakeUnknown),
        ExitFinding::Unjudged(Unjudged::OtherRule),
    ]
}

/// Changing exclusion order or sample eligibility must disagree with the existing fit oracle.
#[test]
fn fit_matches_the_search_oracle_across_tape_findings_tail_gaps_and_retention() {
    let floor = crate::analytics::tuner::ticks::tail::effective_s();
    let mut tapes = vec![
        TapeStatus::Covered,
        TapeStatus::Missing,
        TapeStatus::Fetching,
        TapeStatus::NoAddress,
    ];
    tapes.extend(statuses().map(TapeStatus::Refused));
    for tape in tapes {
        for entry in [
            EntryFinding::Fact,
            EntryFinding::Hit,
            EntryFinding::Unfilled,
            EntryFinding::Off,
        ] {
            for exit in exits() {
                for held in [
                    None,
                    Some((0, -1)),
                    Some((0, i64::from(floor) * 1_000 - 1)),
                    Some((0, i64::from(floor) * 1_000)),
                ] {
                    for has_gap in [false, true] {
                        for ticks in [false, true] {
                            let mut row = row();
                            row.tape = tape;
                            row.verdict = Some(verdict(entry, exit));
                            row.held = held;
                            row.deal.gap = has_gap.then(gap);
                            if !ticks {
                                row.ticks = None;
                            }
                            assert_eq!(
                                DealWhy::of_with(&row, &Admission::at(floor, 0)) == DealWhy::Fit,
                                row.fit_with(&Admission::current()) && row.ticks.is_some()
                            );
                        }
                    }
                }
            }
        }
    }
    let mut pending = row();
    pending.verdict = None;
    assert_eq!(
        DealWhy::of_with(&pending, &Admission::at(floor, 0)),
        DealWhy::NoVerdict
    );
}

/// Distinct source reasons cannot collapse into debug output or silently use English fallback.
#[test]
fn every_reason_has_distinct_keys_and_resolves_in_every_language() {
    let mut reasons: Vec<_> = statuses().map(DealWhy::Refused).into_iter().collect();
    reasons.extend(
        exits()
            .into_iter()
            .filter_map(|exit| DealWhy::of_exit(Some(&verdict(EntryFinding::Hit, exit)))),
    );
    reasons.extend([
        DealWhy::Fit,
        DealWhy::AcceptedShortTail,
        DealWhy::NoTape { fetchable: false },
        DealWhy::NoTape { fetchable: true },
        DealWhy::NoAddress,
        DealWhy::Fetching,
        DealWhy::EntryMissed(EntryFinding::Unfilled),
        DealWhy::EntryMissed(EntryFinding::Off),
        DealWhy::NoVerdict,
        DealWhy::ShortTail {
            trail_ms: 39_000,
            min_s: 60,
        },
        DealWhy::Hole { len_ms: 119_000 },
        DealWhy::LostTape,
    ]);
    assert_eq!(
        reasons
            .iter()
            .map(|reason| reason.key())
            .collect::<HashSet<_>>()
            .len(),
        reasons.len()
    );
    for language in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../locales/{0}/analytics.{0}.yml",
            language.code()
        ));
        let dictionary = std::fs::read_to_string(path).expect("language dictionary");
        for reason in &reasons {
            for suffix in ["", ".why"] {
                let key = format!("analytics.ticks.why.{}{suffix}:", reason.key());
                assert!(
                    dictionary.lines().any(|line| line.starts_with(&key)),
                    "{key} {}",
                    language.code()
                );
            }
            let text = reason.tape_tip().text();
            assert!(
                !text.contains("analytics.") && !text.contains("%{"),
                "{text}"
            );
            for debug in [
                "ArchivePending",
                "OutOfRetention",
                "RateLimited",
                "Unjudged(",
                "StopNotFired",
                "NoLevel",
                "TakeUnknown",
                "NoAutoSell",
            ] {
                assert!(!text.contains(debug), "{text}");
            }
        }
    }
}

/// Only the four approved actionable families offer a next step; every refusal reassures users.
#[test]
fn actions_and_reassurance_follow_the_contract() {
    let _locale = crate::test_locale::force("en");
    let short = DealWhy::ShortTail {
        trail_ms: 30_000,
        min_s: 60,
    };
    assert!(
        !short.tape_tip().not_your_fault,
        "the floor is the user's own setting"
    );
    assert_eq!(short.tape_glyph(), ("●", Tone::Warning));
    for reason in [
        DealWhy::NoTape { fetchable: true },
        DealWhy::NoAddress,
        DealWhy::ShortTail {
            trail_ms: 39_000,
            min_s: 60,
        },
        DealWhy::Refused(TickStatus::ArchivePending { retry_at_ms: 0 }),
    ] {
        assert!(reason.tape_tip().todo.is_some());
    }
    for reason in statuses()
        .map(DealWhy::Refused)
        .into_iter()
        .filter(|reason| !matches!(reason, DealWhy::Refused(TickStatus::ArchivePending { .. })))
        .chain([
            DealWhy::Fit,
            DealWhy::Fetching,
            DealWhy::NoTape { fetchable: false },
            DealWhy::NoVerdict,
            DealWhy::LostTape,
            DealWhy::Hole { len_ms: 1_000 },
        ])
    {
        assert!(reason.tape_tip().todo.is_none());
    }
    for entry in [
        EntryFinding::Fact,
        EntryFinding::Hit,
        EntryFinding::Unfilled,
        EntryFinding::Off,
    ] {
        for exit in exits() {
            let mut row = row();
            row.verdict = Some(verdict(entry, exit));
            let why = DealWhy::of_with(&row, &Admission::at(60, 0));
            if !matches!(entry, EntryFinding::Fact | EntryFinding::Hit) || exit != ExitFinding::Hit
            {
                assert!(why.model_tip(row.verdict.as_ref()).not_your_fault);
            } else {
                assert!(!why.model_tip(row.verdict.as_ref()).not_your_fault);
            }
        }
    }
}

/// Entry exclusion must not hide the exit reason, nor may an unjudged exit hide a tape gap.
#[test]
fn model_and_tape_projections_remain_independent() {
    let mut row = row();
    row.verdict = Some(verdict(
        EntryFinding::Off,
        ExitFinding::Unjudged(Unjudged::InGap),
    ));
    row.deal.gap = Some(gap());
    assert_eq!(
        DealWhy::of_with(&row, &Admission::at(60, 0)),
        DealWhy::EntryMissed(EntryFinding::Off)
    );
    assert_eq!(
        DealWhy::of_exit(row.verdict.as_ref()),
        Some(DealWhy::Unjudged(Unjudged::InGap))
    );
    assert_eq!(
        DealWhy::tape_of_with(&row, &Admission::at(60, 0)),
        DealWhy::Hole { len_ms: 119_000 }
    );
    assert_eq!(
        DealWhy::tape_of_with(&row, &Admission::at(60, 0)).tape_glyph(),
        ("◐", Tone::Warning)
    );
    row.verdict = Some(verdict(
        EntryFinding::Hit,
        ExitFinding::Unjudged(Unjudged::InGap),
    ));
    assert_eq!(
        DealWhy::of_with(&row, &Admission::at(60, 0)),
        DealWhy::Unjudged(Unjudged::InGap)
    );
    assert_eq!(
        DealWhy::tape_of_with(&row, &Admission::at(60, 0)).tape_glyph(),
        ("◐", Tone::Warning)
    );
}

/// An actionable missing-tape reason follows the real fetchability predicate, not tape colour.
#[test]
fn missing_tape_offers_fetch_only_with_a_resolved_source() {
    let _locale = crate::test_locale::force("en");
    let mut row = row();
    row.tape = TapeStatus::Missing;
    assert_eq!(
        DealWhy::of_with(&row, &Admission::at(60, 0)),
        DealWhy::NoTape { fetchable: false }
    );
    row.address = Some(Arc::new(RowAddress {
        core_uid: 1,
        venue: Some(moon_core::venue::Venue {
            brand: moon_core::venue::Brand::Bybit,
            kind: moon_core::venue::MarketKind::Spot,
        }),
        exchange_key: "synthetic".into(),
        market: "XALPHAUSDT".into(),
        btc_market: None,
    }));
    assert!(row.fetchable_with(&Admission::at(60, 0)));
    assert_eq!(
        DealWhy::of_with(&row, &Admission::at(60, 0)),
        DealWhy::NoTape { fetchable: true }
    );
    assert!(
        DealWhy::of_with(&row, &Admission::at(60, 0))
            .tape_tip()
            .todo
            .is_some()
    );
    for exit in exits() {
        if let Some(reason) = DealWhy::of_exit(Some(&verdict(EntryFinding::Hit, exit))) {
            assert!(reason.tape_tip().todo.is_none());
        }
    }
}

/// Restoring a strict tail gate would lose old deals; relaxing every tail would skip fresh refills.
#[test]
fn unfillable_short_tail_is_in_the_sample_but_fresh_tape_waits() {
    use crate::analytics::tuner::ticks::readiness::Readiness;
    use crate::analytics::tuner::ticks::tail::fillable_at;
    let mut row = row();
    row.address = Some(Arc::new(RowAddress {
        core_uid: 1,
        venue: None,
        exchange_key: "EX-XX".into(),
        market: "XALPHA".into(),
        btc_market: None,
    }));
    row.held = Some((30_000, 30_000));
    let old_now = row.deal.close_ms + 49 * 3_600_000;
    let fresh_now = row.deal.close_ms + 3_600_000;
    assert!(!fillable_at(
        row.address.as_ref().unwrap(),
        1,
        row.deal.close_ms,
        30_000,
        old_now,
        (false, true)
    ));
    assert!(fillable_at(
        row.address.as_ref().unwrap(),
        1,
        row.deal.close_ms,
        30_000,
        fresh_now,
        (false, true)
    ));
    assert_eq!(
        DealWhy::of_with(&row, &Admission::at(60, old_now)),
        DealWhy::AcceptedShortTail
    );
    assert_eq!(
        DealWhy::of_with(&row, &Admission::at(60, fresh_now)),
        DealWhy::ShortTail {
            trail_ms: 30_000,
            min_s: 60
        }
    );
    assert!(row.fit_with(&Admission::current()));
    let old = Readiness::of_rows([&row], old_now, 60);
    assert_eq!((old.covered, old.fit, old.fetchable), (1, 1, 0));
    let fresh = Readiness::of_rows([&row], fresh_now, 60);
    assert_eq!((fresh.covered, fresh.fit, fresh.fetchable), (1, 0, 1));
    row.deal.gap = Some(gap());
    assert!(
        !row.fit_with(&Admission::current()),
        "short-tail admission never admits a pre-close gap"
    );
    row.deal.gap = None;
    row.tape = TapeStatus::Missing;
    assert!(
        !row.fit_with(&Admission::current()),
        "a tail exception cannot create missing tape"
    );
}
