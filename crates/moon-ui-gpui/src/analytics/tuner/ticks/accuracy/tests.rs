// Not `super::*`: the parent's `gpui::*` brings gpui's own `test` attribute, which `#[test]` would
// then name, and it expands into itself.
use super::{Accuracy, GroupCount, UncheckedWhy, accuracy_sentence, pct_text};

/// The checked exits and the unchecked ones are two numbers, the unchecked with their main
/// reason; the search's fit sample is a third.
#[test]
fn the_footer_splits_checked_from_unchecked_and_names_the_independent_fit_sample() {
    let _locale = crate::test_locale::force("en");
    let verdicts = [
        verdict(Some(true), Some(true)),
        verdict(Some(false), Some(false)),
        verdict(None, None),
    ];
    let acc = Accuracy::of(verdicts.iter().map(Some).chain([None]));
    let text = accuracy_sentence(&acc, false, 3);
    // One unchecked by another rule, one with no verdict yet: the tie goes to the first listed.
    assert_eq!(
        text,
        "Model repeats: entry — taken from the fact · exit — 1 of 2 checked (50.0 %), 2 not checked: closed by another rule. The search learns from 3 usable trades."
    );
    assert_eq!(acc.none_checked_note(), None, "an exit was checked");
}

/// A strategy whose every exit went unchecked reads as unchecked, with the reason — never as
/// "0 of N", which reads as N misses.
#[test]
fn no_exit_checked_reads_as_unchecked_not_as_missed() {
    let _locale = crate::test_locale::force("en");
    let unknown = Verdict {
        exit_finding: moon_core::db::tuner::ticks::verify::ExitFinding::Unjudged(
            moon_core::db::tuner::ticks::verify::Unjudged::TakeUnknown,
        ),
        ..verdict(None, None)
    };
    let verdicts = [unknown, unknown, unknown, verdict(None, None)];
    let acc = Accuracy::of(verdicts.iter().map(Some));
    assert_eq!(acc.main_unchecked_why(), Some(UncheckedWhy::TakeUnknown));
    assert_eq!(
        accuracy_sentence(&acc, false, 0),
        "Model repeats: entry — taken from the fact · exit — 4 not checked: the take level is unknown. The search learns from 0 usable trades."
    );
    assert_eq!(
        acc.none_checked_note().as_deref(),
        Some("no exit could be checked: the take level is unknown")
    );
}
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

/// An entry miss must not remove the exit's independent unchecked reason from the footer.
#[test]
fn an_entry_miss_keeps_the_unchecked_exit_reason() {
    let mut row = verdict(Some(false), None);
    row.exit_finding = moon_core::db::tuner::ticks::verify::ExitFinding::Unjudged(
        moon_core::db::tuner::ticks::verify::Unjudged::InGap,
    );
    let accuracy = Accuracy::of([Some(&row)]);
    assert_eq!(accuracy.entry.misses, 1);
    assert_eq!(accuracy.exit.unjudged, 1);
    assert_eq!(accuracy.main_unchecked_why(), Some(UncheckedWhy::Gap));
}
