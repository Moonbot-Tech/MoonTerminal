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
/// suggestion reads as the whitelist, and "few holdout trades" is "few", not a noise finding.
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
        AutoClass::Noise { n: 1 }
    );
    assert_eq!(
        AutoClass::of(&pick(
            &["XALPHA"],
            &[],
            PickVerdict::Noise(NoiseReason::FewHoldout)
        )),
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
    assert_eq!(auto.view().class("2@7"), AutoClass::Nothing);
    assert_eq!(auto.view().class("1@7"), AutoClass::Pending);
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
    assert_eq!(auto.view().class("1@7"), AutoClass::Unread);
}
