use super::super::state::CoinsState;
use super::{AutoCardModel, AutoView, settings_summary, tag_line, verdict_line};
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

/// Keeping an adopted result forever would strand the card after its saved baseline catches up.
#[test]
fn adopted_result_keeps_verdict_until_saved_baseline_arrives() {
    let _locale = crate::test_locale::force("en");
    let mut s = CoinsState::default();
    let seq = s.begin_pick();
    s.land_pick(
        seq,
        s.lists_rev,
        s.report_gen,
        30,
        Ok(pick(PickVerdict::Useful)),
    );
    s.adopt_pick();
    let m = AutoCardModel::new(&s, true, false, true);
    assert!(matches!(
        m.view,
        AutoView::Result(super::ResultModel { adopted: true, .. })
    ));
    assert!(m.pending.is_some());
    assert!(!s.retire_saved_pick(), "unsaved edits retain their verdict");
    s.adopt(s.work.duplicate());
    assert!(s.retire_saved_pick());
    assert!(s.suggested().is_none());
    let m = AutoCardModel::new(&s, true, false, true);
    assert!(matches!(m.view, AutoView::Settings));
    assert!(m.pending.is_none());
    s.add_tokens(true, "XBETA");
    let m = AutoCardModel::new(&s, true, false, true);
    assert!(
        matches!(m.view, AutoView::Settings),
        "a later manual edit must not resurrect the old verdict"
    );
    assert!(m.pending.is_some());
}

/// A hard-coded count or plural would misdescribe the settings the user is about to reuse.
#[test]
fn compact_settings_use_live_values_and_russian_plurals() {
    let _locale = crate::test_locale::force("ru");
    let mut cfg = super::PickCfg {
        min_trades: 3,
        holdout_pct: 50,
        black: true,
        white: false,
    };
    assert_eq!(
        settings_summary(cfg),
        "мин. 3 сделки · проверка на свежих 50% · ищет ЧС"
    );
    cfg.min_trades = 1;
    assert!(settings_summary(cfg).contains("1 сделка"));
    cfg.min_trades = 5;
    assert!(settings_summary(cfg).contains("5 сделок"));
}

/// A generic list label must not replace the approved per-list confirmation sentence.
#[test]
fn adopted_hint_uses_the_approved_black_and_white_sentences() {
    let _locale = crate::test_locale::force("ru");
    let mut s = CoinsState::default();
    let mut pick = pick(PickVerdict::Useful);
    pick.black[0].token = "XBETA".into();
    let seq = s.begin_pick();
    s.land_pick(seq, s.lists_rev, s.report_gen, 30, Ok(pick));
    s.adopt_pick();
    let AutoView::Result(mut result) = AutoCardModel::new(&s, true, false, true).view else {
        panic!("result")
    };
    assert_eq!(
        super::adopted_hint(&result),
        "XBETA добавлена в чёрный список ниже."
    );
    result.white = std::mem::take(&mut result.black);
    assert_eq!(
        super::adopted_hint(&result),
        "XBETA добавлена в белый список ниже."
    );
}
