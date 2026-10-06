// Not `super::*`: the parent's `gpui::*` brings gpui's own `test` attribute, which `#[test]` would
// then name, and it expands into itself.
use super::{Accuracy, GroupCount, pct_text};
use moon_core::db::tuner::ticks::Verdict;

fn verdict(entry: Option<bool>, exit: Option<bool>) -> Verdict {
    Verdict {
        entry,
        entry_dev_pct: None,
        exit,
        exit_dev_pct: None,
        fill: None,
        exit_kind: None,
        line_points: None,
        entry_finding: match entry {
            None => moon_core::db::tuner::ticks::verify::EntryFinding::Fact,
            Some(true) => moon_core::db::tuner::ticks::verify::EntryFinding::Hit,
            Some(false) => moon_core::db::tuner::ticks::verify::EntryFinding::Off,
        },
        exit_finding: match exit {
            None => moon_core::db::tuner::ticks::verify::ExitFinding::Unjudged(
                moon_core::db::tuner::ticks::verify::Unjudged::OtherRule,
            ),
            Some(true) => moon_core::db::tuner::ticks::verify::ExitFinding::Hit,
            Some(false) => moon_core::db::tuner::ticks::verify::ExitFinding::Miss(
                moon_core::db::tuner::ticks::verify::ExitMiss::NoLevel,
            ),
        },
        fill_clock_ms: None,
        rules: moon_core::db::tuner::ticks::verify::RuleFlags::default(),
        stop: moon_core::db::tuner::ticks::verify::StopFacts::default(),
    }
}

/// A miss and a trade the model cannot judge both count against it: the share is over every
/// trade with tape, not over the judged ones the KPI caption counts.
#[test]
fn the_share_is_over_every_trade_with_tape() {
    let verdicts = [
        verdict(Some(true), Some(true)),
        verdict(Some(true), Some(false)),
        verdict(Some(false), None),
        verdict(Some(true), Some(true)),
    ];
    let acc = Accuracy::of(verdicts.iter().map(Some).chain([None]));
    assert_eq!(acc.n(), 5);
    assert_eq!(
        acc.entry,
        GroupCount {
            hits: 3,
            misses: 1,
            unjudged: 1
        }
    );
    assert_eq!(
        acc.exit,
        GroupCount {
            hits: 2,
            misses: 1,
            unjudged: 2
        }
    );
    assert!((acc.entry.pct().unwrap() - 60.0).abs() < 1e-9);
    assert!((acc.exit.pct().unwrap() - 40.0).abs() < 1e-9);
}

#[test]
fn nothing_to_count_prints_a_dash() {
    let acc = Accuracy::of(std::iter::empty());
    assert_eq!(acc.n(), 0);
    assert_eq!(acc.exit.pct(), None);
    assert_eq!(pct_text(None), "—");
    assert_eq!(pct_text(Some(85.54)), "85.5 %");
}
