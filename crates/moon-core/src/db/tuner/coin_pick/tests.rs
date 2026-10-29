//! Synthetic regressions for fitting isolation, list precedence and fresh-trade assessment.
use super::*;

mod real_data;

/// A smaller cap silently truncates both independently fitted lists before fifty entries.
#[test]
fn both_lists_allow_fifty_candidates() {
    let rows: Vec<_> = (0..1_000)
        .map(|i| CoinTrade {
            token: format!("COIN_{:02}", i % 60),
            close: i,
            profit: -1.0,
        })
        .collect();
    let black = coin_pick_from_rows(&rows, &params());
    assert_eq!(black.black.len(), 50);
    let mut rows = rows;
    for row in &mut rows {
        row.profit = 1.0;
    }
    // An unqualified loser makes restricting to the fifty winners improve train profit.
    rows.extend((0..700).map(|i| CoinTrade {
        token: "COIN_LOSS".into(),
        close: i,
        profit: -10.0,
    }));
    let mut p = params();
    p.black = false;
    p.white = true;
    let white = coin_pick_from_rows(&rows, &p);
    assert_eq!(white.white.len(), 50);
    assert_eq!(fit_with_cap(&rows, &p, 30).white.len(), 30);
}

/// Default fixture settings reserve exactly thirty of one hundred trades.
fn params() -> CoinPickParams {
    CoinPickParams {
        min_trades: 5,
        holdout_pct: 30,
        black: true,
        white: false,
        manual_black: HashSet::new(),
        manual_white: HashSet::new(),
    }
}

/// Literal-only matching loses manual mask restrictions before fitting and holdout scoring.
#[test]
fn manual_masks_match_by_prefix_or_suffix() {
    let rows: Vec<_> = (0..100)
        .map(|i| CoinTrade {
            token: ["XPERP_RP", "XALPHA", "YBETA", "YPERP"][i as usize % 4].into(),
            close: i,
            profit: 1.0,
        })
        .collect();
    let mut p = params();
    p.black = false;
    p.manual_white.insert(" *perp ".into());
    let suffix = coin_pick_from_rows(&rows, &p);
    assert_eq!(suffix.manual_holdout.n, 15);
    assert_eq!(suffix.manual_holdout.profit, 15.0);
    p.manual_white.clear();
    p.manual_black.insert("x*".into());
    let prefix = coin_pick_from_rows(&rows, &p);
    assert_eq!(prefix.manual_holdout.n, 16);
    assert_eq!(prefix.pick_holdout.profit, 16.0);
    p.manual_white.insert("*PERP".into());
    assert_eq!(coin_pick_from_rows(&rows, &p).manual_holdout.n, 8);
    for unsupported in ["*X*", "*", "X*P"] {
        p.manual_white = [unsupported.to_string()].into();
        assert_eq!(coin_pick_from_rows(&rows, &p).manual_holdout.n, 0);
    }
}

/// Two alternating tokens with an independently controlled train and fresh loss coin.
fn sample(fresh: f64) -> Vec<CoinTrade> {
    (0..100)
        .map(|i| CoinTrade {
            token: if i % 2 == 0 { "XAAA" } else { "XBBB" }.into(),
            close: i,
            profit: if i % 2 != 0 {
                2.0
            } else if i < 70 {
                -10.0
            } else {
                fresh
            },
        })
        .collect()
}

/// Reading fresh profits during fitting would erase the losing-train suggestion.
#[test]
fn train_loser_remains_suggested_when_holdout_wins() {
    let pick = coin_pick_from_rows(&sample(100.0), &params());
    assert_eq!(
        pick.black
            .iter()
            .map(|s| s.token.as_str())
            .collect::<Vec<_>>(),
        ["XAAA"]
    );
    assert_eq!(pick.black[0].profit, 1150.0);
    assert_eq!(pick.verdict, PickVerdict::Noise(NoiseReason::HoldoutWorse));
    assert_eq!(
        pick.pick_holdout.profit - pick.manual_holdout.profit,
        -1500.0
    );
}

/// Removing the fresh-size guard would label a tiny validation sample useful.
#[test]
fn too_few_fresh_trades_refuses_selection() {
    let rows = sample(-10.0);
    assert_eq!(
        coin_pick_from_rows(&rows[..60], &params()).verdict,
        PickVerdict::TooFewTrades
    );
    assert_eq!(
        coin_pick_from_rows(&rows[..67], &params()).verdict,
        PickVerdict::Useful
    );
}

/// Ignoring the per-token count guard would pick a sparsely observed train loser.
#[test]
fn minimum_count_blocks_candidate() {
    let mut p = params();
    p.min_trades = 36;
    assert!(coin_pick_from_rows(&sample(-10.0), &p).black.is_empty());
    p.min_trades = 35;
    assert_eq!(coin_pick_from_rows(&sample(-10.0), &p).black.len(), 1);
}

/// Reversing the incremental-gain sign would reject stable losses removed on fresh rows.
#[test]
fn persistent_loser_is_useful() {
    let pick = coin_pick_from_rows(&sample(-10.0), &params());
    assert_eq!(pick.verdict, PickVerdict::Useful);
    assert_eq!(pick.manual_holdout.profit, -120.0);
    assert_eq!(pick.pick_holdout.profit, 30.0);
}

/// Pooling losses across tokens, or a sign-only gate, would invent a list from twenty coins
/// that each pass the count guard but lose only by noise.
#[test]
fn diffuse_sparse_losses_find_nothing() {
    let rows: Vec<_> = (0..400)
        .map(|i| CoinTrade {
            token: format!("X{:03}", i % 20),
            close: i,
            profit: if (i / 20) % 2 == 0 { -3.0 } else { 2.9 },
        })
        .collect();
    let pick = coin_pick_from_rows(&rows, &params());
    // 280 train rows over 20 coins: 14 each, well past the 5-trade guard.
    assert_eq!(pick.train_n, 280);
    assert_eq!(pick.verdict, PickVerdict::NothingFound);
    assert!(pick.black.is_empty());
}

/// A sum-only gate would pick the alternating coin; a gate without the mean would miss the
/// steady loser whose profits vary.
#[test]
fn noisy_profits_are_judged_by_their_t_statistic() {
    let rows: Vec<_> = (0..100_i32)
        .map(|i| {
            let k = i / 4;
            let (token, profit) = match i % 4 {
                0 => ("XNEG", -10.0 + f64::from(k % 3 - 1) * 4.0),
                1 => ("XALT", if k % 2 == 0 { -30.0 } else { 28.0 }),
                _ => ("XPOS", 1.0),
            };
            CoinTrade {
                token: token.into(),
                close: i64::from(i),
                profit,
            }
        })
        .collect();
    let pick = coin_pick_from_rows(&rows, &params());
    // XALT on train: 18 trades, sum -18 < 0, mean -1, t ~ -0.15, so the gate rejects it.
    assert_eq!(
        pick.black
            .iter()
            .map(|s| s.token.as_str())
            .collect::<Vec<_>>(),
        ["XNEG"]
    );
}

/// The verdict's gain is incremental over Manual: a manual blacklist removing a fresh loser
/// is not credited to the pick, which is scored only on the train loser it found itself.
#[test]
fn gain_is_pick_over_manual_when_both_remove_losers() {
    let rows: Vec<_> = (0..100)
        .map(|i| {
            let (token, profit) = match i % 3 {
                0 => ("XAAA", -10.0),
                1 => ("XBBB", 2.0),
                _ => ("XCCC", if i < 70 { 1.0 } else { -5.0 }),
            };
            CoinTrade {
                token: token.into(),
                close: i,
                profit,
            }
        })
        .collect();
    let mut p = params();
    p.manual_black.insert("XCCC".into());
    let pick = coin_pick_from_rows(&rows, &p);
    assert_eq!(pick.black[0].token, "XAAA");
    assert_eq!(pick.black.len(), 1);
    assert_eq!(pick.fact_holdout.profit, -130.0);
    assert_eq!(pick.manual_holdout.profit, -80.0);
    assert_eq!(pick.pick_holdout.profit, 20.0);
    assert_eq!(
        pick.pick_holdout.profit - pick.manual_holdout.profit,
        100.0,
        "the gain counts XAAA only, never the manual XCCC removal"
    );
    assert_eq!(pick.verdict, PickVerdict::Useful);
}

/// Accepting an equal-profit whitelist would offer a redundant restriction after BL.
#[test]
fn whitelist_must_beat_blacklist_on_train() {
    let mut p = params();
    p.white = true;
    let pick = coin_pick_from_rows(&sample(-10.0), &p);
    assert_eq!(pick.black.len(), 1);
    assert!(pick.white.is_empty());
}

/// Filtering before splitting would shift the fresh interval when manual removes recent rows.
#[test]
fn manual_blacklist_never_moves_split_or_gets_credit() {
    let mut rows = sample(-10.0);
    for row in &mut rows[70..] {
        row.token = "XCCC_RP".into();
    }
    let mut p = params();
    p.manual_black.insert("xccc".into());
    p.manual_black.insert("XAAA".into());
    let pick = coin_pick_from_rows(&rows, &p);
    assert_eq!((pick.train_n, pick.holdout_n), (70, 30));
    assert_eq!(pick.fact_holdout.n, 30);
    assert_eq!(pick.manual_holdout.n, 0);
    assert_eq!(pick.pick_holdout.profit, 0.0);
    assert!(pick.black.is_empty());
    assert_eq!(pick.verdict, PickVerdict::NothingFound);
}

/// Cutting tied timestamps or using input-order sums would make reordered rows change picks.
#[test]
fn equal_time_permutations_have_identical_picks() {
    let mut rows = sample(-10.0);
    for row in &mut rows {
        row.close /= 4;
    }
    let a = coin_pick_from_rows(&rows, &params());
    rows.reverse();
    let b = coin_pick_from_rows(&rows, &params());
    assert_eq!((a.train_n, a.holdout_n), (72, 28));
    assert_eq!((a.train_n, a.holdout_n), (b.train_n, b.holdout_n));
    assert_eq!(a.black[0].token, b.black[0].token);
    assert_eq!(a.black[0].profit, b.black[0].profit);
    assert_eq!(a.pick_holdout.profit, b.pick_holdout.profit);
    assert_eq!(a.verdict, b.verdict);
}
