//! Synthetic regressions for the batch auto-pick state: classes, supersession, busy retry.
use std::collections::HashMap;

use moon_core::db::metrics::Tally;
use moon_core::db::tuner::coin_pick::{CoinPick, CoinSuggestion, NoiseReason, PickVerdict};
use moon_core::db::{FailCode, FailKind, ReadFail};

use super::super::coins::state::PickCfg;
use super::{AutoClass, AutoKey, AutoPicks, AutoRun};

/// A coin suggestion with neutral numbers.
fn coin(token: &str) -> CoinSuggestion {
    CoinSuggestion {
        token: token.into(),
        n: 10,
        profit: -1.0,
    }
}

/// A pick with the given lists and verdict.
fn pick(black: &[&str], white: &[&str], verdict: PickVerdict) -> CoinPick {
    CoinPick {
        black: black.iter().map(|t| coin(t)).collect(),
        white: white.iter().map(|t| coin(t)).collect(),
        train_n: 70,
        holdout_n: 30,
        fact_holdout: Tally::default(),
        manual_holdout: Tally::default(),
        pick_holdout: Tally::default(),
        verdict,
    }
}

/// A launch key over the default pick settings.
fn key(scope_gen: u64) -> AutoKey {
    AutoKey {
        scope_gen,
        lists_gen: 0,
        cfg: PickCfg::default(),
    }
}

/// A database failure of the given kind.
fn fail(kind: FailKind) -> ReadFail {
    ReadFail::failed(kind, "synthetic", "reports.sqlite", "test", FailCode::None)
}

/// The column must say what the card would: a confirmed list names its size, a whitelist-only
/// suggestion reads as the whitelist, and a noise pick with candidates counts them even when it
/// touched too few fresh trades: mapping that to "few" hid coins the card offers. Too few
/// trades in the period stays "few".
#[test]
fn classes_follow_the_cards_verdict() {
    assert_eq!(
        AutoClass::of(&pick(&["XALPHA", "XBETA"], &[], PickVerdict::Useful)),
        AutoClass::Useful { n: 2, white: false }
    );
    assert_eq!(
        AutoClass::of(&pick(&[], &["XALPHA"], PickVerdict::Useful)),
        AutoClass::Useful { n: 1, white: true }
    );
    assert_eq!(
        AutoClass::of(&pick(
            &["XALPHA"],
            &[],
            PickVerdict::Noise(NoiseReason::SmallGain)
        )),
        AutoClass::Noise { n: 1, white: false }
    );
    assert_eq!(
        AutoClass::of(&pick(
            &["XALPHA", "XBETA"],
            &[],
            PickVerdict::Noise(NoiseReason::FewHoldout)
        )),
        AutoClass::Noise { n: 2, white: false }
    );
    assert_eq!(
        AutoClass::of(&pick(&[], &["XALPHA"], PickVerdict::TooFewTrades)),
        AutoClass::Few
    );
    assert_eq!(
        AutoClass::of(&pick(&[], &[], PickVerdict::TooFewTrades)),
        AutoClass::Few
    );
    assert_eq!(
        AutoClass::of(&pick(&[], &[], PickVerdict::NothingFound)),
        AutoClass::Nothing
    );
    assert_eq!(
        AutoClass::of(&pick(
            &[],
            &[],
            PickVerdict::Noise(NoiseReason::HoldoutWorse)
        )),
        AutoClass::Nothing
    );
}

/// "With a finding" must keep every pick the column draws a count for, confirmed or not, and
/// the done line must count the same strategies; "confirmed" keeps only `Useful`.
#[test]
fn the_found_filter_keeps_every_pick_with_candidates() {
    use super::AutoFilter;
    let few = AutoClass::of(&pick(
        &["XALPHA"],
        &[],
        PickVerdict::Noise(NoiseReason::FewHoldout),
    ));
    let worse = AutoClass::of(&pick(
        &["XALPHA"],
        &[],
        PickVerdict::Noise(NoiseReason::HoldoutWorse),
    ));
    for class in [few, worse] {
        assert!(AutoFilter::Found.keeps(class), "{class:?}");
        assert!(!AutoFilter::Confirmed.keeps(class), "{class:?}");
    }
    let bare = AutoClass::of(&pick(&[], &[], PickVerdict::TooFewTrades));
    assert!(!AutoFilter::Found.keeps(bare));
}

/// A reply from a batch a newer launch replaced must not overwrite the newer one, and the
/// newer launch must have asked the old one to stop.
#[test]
fn a_stale_batch_is_cancelled_and_dropped() {
    let mut auto = AutoPicks::default();
    let (old_seq, old_progress) = auto.begin(key(1));
    let (new_seq, _) = auto.begin(key(2));
    assert!(old_progress.is_cancelled());
    let mut stale = HashMap::new();
    stale.insert(
        "1@7".to_string(),
        pick(&["XALPHA"], &[], PickVerdict::Useful),
    );
    assert_eq!(auto.land(old_seq, 30, Ok(Some(stale))), (false, false));
    assert!(auto.picks.is_empty());
    assert!(matches!(auto.run, AutoRun::Running { .. }));

    let mut fresh = HashMap::new();
    fresh.insert("2@7".to_string(), pick(&[], &[], PickVerdict::NothingFound));
    assert_eq!(auto.land(new_seq, 30, Ok(Some(fresh))), (true, false));
    assert_eq!(auto.view(true).class("2@7"), AutoClass::Nothing);
    assert_eq!(auto.view(true).class("1@7"), AutoClass::Pending);
}

/// A busy database is retried by itself and says so; a corrupt one is not, and its rows read
/// as unread rather than as "still computing".
#[test]
fn only_a_busy_database_is_retried() {
    crate::test_locale::warm();
    let mut auto = AutoPicks::default();
    let (seq, _) = auto.begin(key(1));
    assert_eq!(auto.land(seq, 30, Err(fail(FailKind::Busy))), (true, true));
    assert!(auto.status(5).is_some_and(|(_, warn)| warn));

    let (seq, _) = auto.begin(key(2));
    assert_eq!(
        auto.land(seq, 30, Err(fail(FailKind::Corrupt))),
        (true, false)
    );
    assert_eq!(auto.view(true).class("1@7"), AutoClass::Unread);
}

/// A grey chip's hover must say the fresh trades did not confirm it, in every language; a
/// confirmed one must not. Without it a grey "BL 2" reads as a finding to act on.
#[test]
fn an_unconfirmed_chip_says_so_on_hover() {
    for lang in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(lang.code());
        let note = rust_i18n::t!("analytics.auto.tip_unconfirmed").to_string();
        assert!(!note.starts_with("analytics."), "{}", lang.code());
        for verdict in [
            PickVerdict::Noise(NoiseReason::FewHoldout),
            PickVerdict::Noise(NoiseReason::SmallGain),
            PickVerdict::Noise(NoiseReason::HoldoutWorse),
        ] {
            let tip = super::cell_tip(&pick(&["XALPHA"], &[], verdict.clone()), 30);
            assert!(tip.starts_with(&note), "{} {verdict:?}: {tip}", lang.code());
        }
        let tip = super::cell_tip(&pick(&["XALPHA"], &[], PickVerdict::Useful), 30);
        assert!(!tip.contains(&note), "{}: {tip}", lang.code());
    }
}

/// The filter opens on "with a finding" on By-coin, where the Auto column is the point, and on
/// "all" elsewhere; the user's menu choice wins on By-coin only. The filter is drawn on By-coin
/// alone, so on the other axes it never narrows the list, and returning to By-coin brings the
/// choice back. Defaulting to "all" everywhere buried the few strategies with something to add.
#[test]
fn the_filter_defaults_to_findings_on_by_coin_only() {
    use super::AutoFilter;
    let mut auto = AutoPicks::default();
    assert_eq!(auto.filter(true), AutoFilter::Found);
    assert_eq!(auto.filter(false), AutoFilter::All);
    assert_eq!(auto.view(true).filter, AutoFilter::Found);
    auto.chosen = Some(AutoFilter::All);
    assert_eq!(auto.filter(true), AutoFilter::All);
    auto.chosen = Some(AutoFilter::Confirmed);
    assert_eq!(auto.filter(false), AutoFilter::All);
    assert_eq!(auto.view(false).filter, AutoFilter::All);
    assert_eq!(auto.filter(true), AutoFilter::Confirmed);
}

/// The done line's "of M" is the total the list handed in, which is the header's "of Y".
/// Substituting the unfiltered strategy count made the row say "of 643" beside a header
/// that said "of 595".
#[test]
fn the_done_line_uses_the_list_total_it_is_given() {
    crate::test_locale::warm();
    let _locale = crate::test_locale::force("en");
    let mut auto = AutoPicks::default();
    let (seq, _) = auto.begin(key(1));
    let mut picks = HashMap::new();
    picks.insert(
        "1@7".to_string(),
        pick(&["XALPHA"], &[], PickVerdict::Useful),
    );
    assert_eq!(auto.land(seq, 30, Ok(Some(picks))), (true, false));
    let (text, warn) = auto.status(595).expect("done");
    assert!(!warn);
    assert!(
        text.contains("595"),
        "done line must name the list total it was given, got {text}"
    );
    assert!(
        !text.contains("643"),
        "done line must not keep a second population, got {text}"
    );
}

/// An empty filtered list says "still computing" until the batch answers, and only then the
/// "no findings" state with its "show all" button: showing that state on the default filter
/// while the batch runs told the user there was nothing before anything was counted.
#[test]
fn the_empty_state_waits_for_the_batch() {
    use super::{AutoEmpty, AutoFilter};
    let mut auto = AutoPicks::default();
    assert_eq!(
        auto.empty_face(AutoFilter::Found),
        Some(AutoEmpty::Computing)
    );
    let (seq, _) = auto.begin(key(1));
    assert_eq!(
        auto.empty_face(AutoFilter::Found),
        Some(AutoEmpty::Computing)
    );
    assert_eq!(auto.empty_face(AutoFilter::All), None);
    assert_eq!(auto.land(seq, 30, Ok(Some(HashMap::new()))), (true, false));
    assert_eq!(
        auto.empty_face(AutoFilter::Found),
        Some(AutoEmpty::NoFinding)
    );
    assert_eq!(
        auto.empty_face(AutoFilter::Confirmed),
        Some(AutoEmpty::NoFinding)
    );
    let (seq, _) = auto.begin(key(2));
    assert_eq!(
        auto.land(seq, 30, Err(fail(FailKind::Corrupt))),
        (true, false)
    );
    assert_eq!(
        auto.empty_face(AutoFilter::Found),
        Some(AutoEmpty::NoFinding)
    );
}
