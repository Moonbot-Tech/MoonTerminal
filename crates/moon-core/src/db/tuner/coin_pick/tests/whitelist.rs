//! Fixed top-20 fallback regressions, including legacy preservation and holdout isolation.

use super::*;

/// Forty coins over ten groups: noisy positive training sums evade today's t gate.
/// Twenty later winners outrank twenty low-profit coins; the last three groups are holdout.
fn ranked_rows(overfit: bool) -> Vec<CoinTrade> {
    (0..10)
        .flat_map(|round| {
            (0..40).map(move |coin| {
                let profit = if round < 5 {
                    if coin < 20 {
                        [-90.0, -90.0, 100.0, 100.0, 100.0][round]
                    } else {
                        [100.0, 100.0, -90.0, -90.0, -10.0][round]
                    }
                } else if overfit {
                    if coin < 20 { -20.0 } else { 20.0 }
                } else if coin < 20 {
                    2.0
                } else {
                    -20.0
                };
                CoinTrade {
                    token: format!("XCOIN_{coin:02}"),
                    close: round as i64,
                    profit,
                }
            })
        })
        .collect()
}

/// Whitelist-only settings keep blacklist gains from masking whitelist regressions.
fn white_params() -> CoinPickParams {
    CoinPickParams {
        white: true,
        black: false,
        ..params()
    }
}

/// Requiring a t score or clearing on train profit would miss the fallback's fresh improvement.
#[test]
fn fixed_top_twenty_adds_a_useful_whitelist() {
    let rows = ranked_rows(false);
    let pick = coin_pick_from_rows(&rows, &white_params());
    assert_eq!((pick.train_n, pick.holdout_n), (280, 120));
    assert_eq!(pick.white.len(), 20);
    assert_eq!(pick.white[0].token, "XCOIN_00");
    assert_eq!(pick.white[19].token, "XCOIN_19");
    assert_eq!(pick.manual_holdout.profit, -1080.0);
    assert_eq!(pick.pick_holdout.profit, 120.0);
    assert_eq!(pick.verdict, PickVerdict::Useful);
}

/// Scoring the train fit as Useful, or choosing another N on holdout, would hide an overfit loss.
#[test]
fn an_overfit_fallback_is_rejected_by_the_holdout_verdict() {
    let pick = coin_pick_from_rows(&ranked_rows(true), &white_params());
    assert_eq!(pick.white.len(), 20);
    assert_eq!(pick.manual_holdout.profit, 0.0);
    assert_eq!(pick.pick_holdout.profit, -1200.0);
    assert_eq!(pick.verdict, PickVerdict::Noise(NoiseReason::HoldoutWorse));
}

/// Always replacing today's whitelist with top-20 would lose three consistent winners.
#[test]
fn an_existing_gated_list_is_kept_unchanged() {
    let rows: Vec<_> = (0..10)
        .flat_map(|round| {
            (0..40).map(move |coin| CoinTrade {
                token: format!("XCOIN_{coin:02}"),
                close: round as i64,
                profit: if coin < 3 {
                    2.0
                } else if coin >= 20 {
                    -30.0
                } else if round < 5 {
                    [-90.0, -90.0, 100.0, 100.0, 100.0][round]
                } else {
                    -20.0
                },
            })
        })
        .collect();
    let pick = coin_pick_from_rows(&rows, &white_params());
    assert_eq!(
        pick.white
            .iter()
            .map(|s| s.token.as_str())
            .collect::<Vec<_>>(),
        ["XCOIN_00", "XCOIN_01", "XCOIN_02"]
    );
    assert_eq!(pick.pick_holdout.profit, 18.0);
    assert_eq!(pick.verdict, PickVerdict::Useful);
}

/// Applying the fallback's three-coin guard to the old family would erase a valid one-coin pick.
#[test]
fn an_existing_one_coin_list_is_still_kept() {
    let pick = coin_pick_from_rows(&sample(-10.0), &white_params());
    assert_eq!(pick.white.len(), 1);
    assert_eq!(pick.white[0].token, "XBBB");
    assert_eq!(pick.pick_holdout.profit, 30.0);
    assert_eq!(pick.verdict, PickVerdict::Useful);
}

/// Stopping at a train-cleared gated list would miss the fixed fallback that stays useful later.
#[test]
fn the_fallback_runs_after_the_legacy_train_check_clears_a_list() {
    let mut rows = ranked_rows(false);
    for row in &mut rows {
        if row.token.as_str() < "XCOIN_03" {
            row.profit = 1.0;
        }
    }
    let pick = coin_pick_from_rows(&rows, &white_params());
    assert_eq!(pick.white.len(), 20);
    assert_eq!(pick.pick_holdout.profit, 111.0);
    assert_eq!(pick.verdict, PickVerdict::Useful);
}

/// Ignoring manual masks or the three-coin boundary would offer unsupported tiny fallback lists.
#[test]
fn the_fallback_needs_at_least_three_eligible_coins() {
    let rows = ranked_rows(false);
    let mut p = white_params();
    p.manual_white = ["XCOIN_00".into(), "XCOIN_01".into()].into();
    assert!(coin_pick_from_rows(&rows, &p).white.is_empty());
    p.manual_white.insert("XCOIN_02".into());
    let three = coin_pick_from_rows(&rows, &p);
    assert_eq!(three.white.len(), 3);
    assert_eq!(
        three
            .white
            .iter()
            .map(|s| s.token.as_str())
            .collect::<Vec<_>>(),
        ["XCOIN_00", "XCOIN_01", "XCOIN_02"]
    );
    p.min_trades = 8;
    assert!(coin_pick_from_rows(&rows, &p).white.is_empty());
}

/// Using holdout profits to choose the candidate would swap its coins when the future flips.
#[test]
fn holdout_changes_verdict_without_changing_the_selected_list() {
    let mut rows = ranked_rows(false);
    let p = white_params();
    let before = coin_pick_from_rows(&rows, &p);
    for row in &mut rows[280..] {
        row.profit = if row.token.as_str() < "XCOIN_20" {
            -20.0
        } else {
            200.0
        };
    }
    let after = coin_pick_from_rows(&rows, &p);
    assert_eq!(
        before.white.iter().map(|s| &s.token).collect::<Vec<_>>(),
        after.white.iter().map(|s| &s.token).collect::<Vec<_>>()
    );
    assert_eq!(after.white.len(), 20);
    assert_eq!(before.verdict, PickVerdict::Useful);
    assert_eq!(after.verdict, PickVerdict::Noise(NoiseReason::HoldoutWorse));
    assert_eq!(after.pick_holdout.profit, -1200.0);
}

/// A fallback beside a legacy blacklist would drop fresh winners and erase a Useful pick.
/// A Noise blacklist must also suppress it, so the guard never selects on holdout outcomes.
#[test]
fn a_legacy_blacklist_keeps_its_entire_pick_without_trying_the_fallback() {
    // 217 TRAIN rows and 93 holdout rows make the thirty-percent cut exact.
    let mut rows: Vec<_> = (0..7)
        .flat_map(|round| {
            (0..31).map(move |coin| CoinTrade {
                token: if coin == 30 {
                    "XLOSS".into()
                } else {
                    format!("XCOIN_{coin:02}")
                },
                close: round as i64,
                profit: if coin == 30 {
                    -100.0
                } else {
                    [-90.0, -90.0, 100.0, 100.0, 100.0, 2.0, 2.0][round]
                },
            })
        })
        .collect();
    rows.extend((0..93).map(|i| CoinTrade {
        token: if i < 10 {
            "XLOSS".into()
        } else if i < 20 {
            format!("XCOIN_{:02}", i + 10)
        } else {
            "XCOIN_00".into()
        },
        close: 7 + i,
        profit: if i < 10 {
            -100.0
        } else if i < 20 {
            200.0
        } else {
            0.0
        },
    }));
    let mut with_white = params();
    with_white.white = true;
    for (fresh_loss, manual_profit, verdict) in [
        (-100.0, 1000.0, PickVerdict::Useful),
        (100.0, 3000.0, PickVerdict::Noise(NoiseReason::HoldoutWorse)),
    ] {
        for row in &mut rows[217..227] {
            row.profit = fresh_loss;
        }
        let baseline = coin_pick_from_rows(&rows, &params());
        assert_eq!((baseline.train_n, baseline.holdout_n), (217, 93));
        assert_eq!(baseline.manual_holdout.profit, manual_profit);
        assert_eq!(baseline.pick_holdout.profit, 2000.0);
        assert_eq!(baseline.verdict, verdict);
        let pick = coin_pick_from_rows(&rows, &with_white);
        assert!(pick.white.is_empty());
        assert_eq!(format!("{pick:?}"), format!("{baseline:?}"));
    }
}
