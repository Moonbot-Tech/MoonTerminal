use super::super::state::CoinsState;
use super::{AutoCardModel, tag_line, verdict_line};
use moon_core::db::metrics::Tally;
use moon_core::db::tuner::coin_pick::{CoinPick, CoinSuggestion, NoiseReason, PickVerdict};

fn tally(profit: f64) -> Tally {
    let mut t = Tally::default();
    t.push(profit);
    t
}

fn pick(verdict: PickVerdict) -> CoinPick {
    CoinPick {
        black: vec![CoinSuggestion {
            token: "XAAA".to_string(),
            n: 48,
            profit: -995.64,
        }],
        white: Vec::new(),
        train_n: 70,
        holdout_n: 30,
        fact_holdout: tally(-130.0),
        manual_holdout: tally(-80.0),
        pick_holdout: tally(20.5),
        verdict,
    }
}

/// Every verdict reads as its own sentence, and the useful one carries the pick's figure,
/// the as-traded figure and the gain, each signed by the shared formatter.
#[test]
fn every_verdict_has_its_own_sentence() {
    let _locale = crate::test_locale::force("en");
    let verdicts = [
        PickVerdict::Useful,
        PickVerdict::Noise(NoiseReason::SmallGain),
        PickVerdict::Noise(NoiseReason::HoldoutWorse),
        PickVerdict::Noise(NoiseReason::FewHoldout),
        PickVerdict::NothingFound,
        PickVerdict::TooFewTrades,
    ];
    let lines: Vec<String> = verdicts
        .iter()
        .map(|v| verdict_line(&pick(v.clone()), 30))
        .collect();
    for (i, a) in lines.iter().enumerate() {
        assert!(!a.contains("analytics."), "raw key: {a}");
        for b in &lines[i + 1..] {
            assert_ne!(a, b);
        }
    }
    let useful = &lines[0];
    for figure in [
        moon_core::util::fmt::signed_amount(20.5, 2).0,
        moon_core::util::fmt::signed_amount(-130.0, 2).0,
        moon_core::util::fmt::signed_amount(100.5, 2).0,
    ] {
        assert!(useful.contains(&figure), "{useful} lacks {figure}");
    }
    assert!(useful.contains("30%"));
}

/// A suggested coin's tag names its full-period profit and trade count.
#[test]
fn a_tag_names_profit_and_trades() {
    let _locale = crate::test_locale::force("en");
    let c = &pick(PickVerdict::Useful).black[0];
    assert_eq!(tag_line(c), "XAAA -995.64 · 48 tr.");
}

/// Pick waits for the coin statistics and saved lists: a pick launched mid-reload would
/// score against lists about to change.
#[test]
fn pick_waits_for_the_data() {
    let _locale = crate::test_locale::force("en");
    let s = CoinsState::default();
    assert!(AutoCardModel::new(&s, true, false, true).can_pick());
    assert!(!AutoCardModel::new(&s, true, false, false).can_pick());
    assert!(!AutoCardModel::new(&s, false, false, true).can_pick());
}
