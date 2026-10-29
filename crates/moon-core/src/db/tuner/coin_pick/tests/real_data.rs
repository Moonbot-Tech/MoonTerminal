//! Opt-in diagnostics on a disposable database snapshot, never the live installation.
//!
//! Set MOON_COIN_PICK_DATA_DIR to a snapshot root with the three database families in its
//! canonical data directory, and MOON_COIN_PICK_STRATEGY to a name substring. Optional
//! MOON_COIN_PICK_CORE selects the traced core; the batch still covers all cores. Exact name
//! matches take precedence over substring matches, so numbered copies remain distinct. Run this exact
//! ignored test alone: the data-directory override is process-wide. Output stays local.

use super::*;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Print a chronological holdout tally in the selected query's monetary units.
fn print_tally(label: &str, value: &Tally) {
    println!(
        "{label}: n={} profit={:.6} wins={} wsum={:.6} lsum={:.6} pf={:.6} dd={:.6}",
        value.n,
        value.profit,
        value.wins,
        value.wsum,
        value.lsum,
        value.profit_factor(),
        value.max_dd
    );
}

/// Explain a direction's first failing production gate; rank resolves the cap boundary.
fn gate(
    s: &Moments,
    p: &CoinPickParams,
    black: bool,
    rank: Option<usize>,
    cleared: bool,
) -> &'static str {
    if s.n < p.min_trades.max(1) as usize {
        "min-trades"
    } else if if black { s.sum >= 0.0 } else { s.sum <= 0.0 } {
        "sign"
    } else if if black {
        s.score() > -T_GATE
    } else {
        s.score() < T_GATE
    } {
        "t-score"
    } else if rank.is_some_and(|r| r >= MAX_PICK) {
        "cap"
    } else if !black && cleared {
        "whitelist-cleared-by-train-check"
    } else {
        "candidate"
    }
}

/// Show every train coin, the gates, and independent train-profit oracle holdouts.
fn trace(rows: &[CoinTrade], p: &CoinPickParams, pick: &CoinPick) {
    let mut rows = rows.to_vec();
    for row in &mut rows {
        row.token = coin_match_key(&row.token);
    }
    rows.sort_by(|a, b| {
        a.close
            .cmp(&b.close)
            .then_with(|| a.profit.total_cmp(&b.profit))
            .then_with(|| a.token.cmp(&b.token))
    });
    let (train, holdout) = rows.split_at(pick.train_n);
    let mb = CoinMask::new(&p.manual_black);
    let mw = CoinMask::new(&p.manual_white);
    let manual = |r: &CoinTrade| allowed(&r.token, &mb, &mw);
    let mut scores: BTreeMap<String, Moments> = BTreeMap::new();
    for row in train {
        scores
            .entry(row.token.clone())
            .or_default()
            .push(row.profit);
    }
    let mut ranked = [Vec::new(), Vec::new()];
    for (index, black) in [true, false].into_iter().enumerate() {
        ranked[index] = scores
            .iter()
            .filter(|(token, s)| {
                allowed(token, &mb, &mw) && gate(s, p, black, None, false) == "candidate"
            })
            .map(|(token, s)| (token.clone(), s.sum))
            .collect::<Vec<_>>();
        ranked[index].sort_by(|(a, x), (b, y)| {
            (if black {
                x.total_cmp(y)
            } else {
                y.total_cmp(x)
            })
            .then_with(|| a.cmp(b))
        });
    }
    let cleared = !ranked[1].is_empty()
        && pick.white.is_empty()
        && !matches!(pick.verdict, PickVerdict::TooFewTrades);
    println!(
        "A train_n={} holdout_n={} verdict={:?} black={} white={} manual_black={} manual_white={} zero_pnl={} nonpositive_close={}",
        pick.train_n,
        pick.holdout_n,
        pick.verdict,
        pick.black.len(),
        pick.white.len(),
        p.manual_black.len(),
        p.manual_white.len(),
        rows.iter().filter(|r| r.profit == 0.0).count(),
        rows.iter().filter(|r| r.close <= 0).count()
    );
    println!(
        "GATES total_coins={} min_trades_pass={} black_t_pass={} white_t_pass={} white_cleared={cleared}",
        scores.len(),
        scores
            .iter()
            .filter(|(t, s)| allowed(t, &mb, &mw) && s.n >= p.min_trades as usize)
            .count(),
        ranked[0].len(),
        ranked[1].len()
    );
    let candidate_black: HashSet<_> = ranked[0].iter().take(MAX_PICK).map(|(t, _)| t).collect();
    let candidate_white: HashSet<_> = ranked[1].iter().take(MAX_PICK).map(|(t, _)| t).collect();
    let after_black = |r: &CoinTrade| manual(r) && !candidate_black.contains(&r.token);
    println!(
        "TRAIN_CHECK manual_profit={:.6} after_black_profit={:.6} candidate_white_profit={:.6} candidate_white_n={} cleared={cleared}",
        tally(train, manual).profit,
        tally(train, after_black).profit,
        tally(train, |r| after_black(r)
            && candidate_white.contains(&r.token))
        .profit,
        candidate_white.len()
    );
    for (token, s) in scores.iter().filter(|(_, s)| s.n >= 3) {
        let reasons: Vec<_> = [true, false]
            .into_iter()
            .enumerate()
            .map(|(i, black)| {
                if !allowed(token, &mb, &mw) {
                    "manual-list"
                } else if matches!(pick.verdict, PickVerdict::TooFewTrades) {
                    "too-few-trades-before-fit"
                } else {
                    gate(
                        s,
                        p,
                        black,
                        ranked[i].iter().position(|(t, _)| t == token),
                        cleared,
                    )
                }
            })
            .collect();
        println!(
            "COIN {} n_train={} sum={:.6} t={:.6} black={} white={}",
            token.escape_default(),
            s.n,
            s.sum,
            s.score(),
            reasons[0],
            reasons[1]
        );
    }
    print_tally("FACT", &pick.fact_holdout);
    print_tally("MANUAL", &pick.manual_holdout);
    print_tally("PICK", &pick.pick_holdout);
    let black: HashSet<_> = pick.black.iter().map(|c| &c.token).collect();
    let white: HashSet<_> = pick.white.iter().map(|c| &c.token).collect();
    let keep = |r: &CoinTrade| {
        manual(r) && !black.contains(&r.token) && (white.is_empty() || white.contains(&r.token))
    };
    let touched = holdout.iter().filter(|r| manual(r) && !keep(r)).count();
    let mut moments = Moments::default();
    for row in holdout.iter().filter(|r| manual(r)) {
        moments.push(row.profit);
    }
    let abs = NOISE_ABS_SHARE * pick.fact_holdout.profit.abs();
    let sd = moments.sd() * (touched as f64).sqrt();
    println!(
        "FLOOR touched={touched} manual_sd={:.6} abs_component={abs:.6} sd_component={sd:.6} floor={:.6} gain={:.6}",
        moments.sd(),
        abs.max(sd),
        pick.pick_holdout.profit - pick.manual_holdout.profit
    );
    let mut oracle: Vec<_> = scores
        .iter()
        .filter(|(t, _)| allowed(t, &mb, &mw))
        .collect();
    oracle.sort_by(|(a, x), (b, y)| y.sum.total_cmp(&x.sum).then_with(|| a.cmp(b)));
    for cap in [20, 50] {
        let tokens: HashSet<_> = oracle.iter().take(cap).map(|(t, _)| t.as_str()).collect();
        // An empty oracle retains no trades: it is not a disabled production whitelist.
        let score = tally(holdout, |r| manual(r) && tokens.contains(r.token.as_str()));
        println!(
            "B ORACLE top={cap} coins={} train_profit={:.6} holdout_gain={:.6}",
            tokens.len(),
            oracle.iter().take(cap).map(|(_, s)| s.sum).sum::<f64>(),
            score.profit - pick.manual_holdout.profit
        );
        print_tally("ORACLE_HOLDOUT", &score);
    }
}

/// Read and fit the same production snapshot as the cards and batch, refusing missing inputs.
#[test]
#[ignore = "requires copied databases in MOON_COIN_PICK_DATA_DIR and MOON_COIN_PICK_STRATEGY"]
fn real_data_coin_pick() {
    let root = PathBuf::from(std::env::var_os("MOON_COIN_PICK_DATA_DIR").expect("snapshot root"));
    assert!(crate::config::paths::set_data_dir_override(root));
    let _permit = crate::db::report_recovery::prepare().expect("snapshot replica lease");
    let name = std::env::var("MOON_COIN_PICK_STRATEGY").expect("strategy name substring");
    assert!(!name.is_empty(), "strategy selector must not be empty");
    let core = std::env::var("MOON_COIN_PICK_CORE")
        .ok()
        .map(|v| v.parse::<u64>().expect("numeric core uid"));
    let conn = crate::db::open_reader().expect("snapshot reader");
    let selected: Vec<(i64, Option<u64>, bool)> = conn.prepare(
        "SELECT strategy_id, core_uid, name=?1 FROM strat.strategies WHERE instr(name,?1)>0 ORDER BY core_uid,strategy_id"
    ).expect("strategy selector").query_map([&name], |r| Ok((r.get(0)?, Some(r.get::<_, i64>(1)? as u64), r.get(2)?)))
        .expect("strategy query").collect::<rusqlite::Result<_>>().expect("strategy identities");
    let mut selected: Vec<_> = selected
        .into_iter()
        .filter(|(_, uid, _)| core.is_none() || *uid == core)
        .collect();
    if selected.iter().any(|(_, _, exact)| *exact) {
        selected.retain(|(_, _, exact)| *exact);
    }
    let selected: Vec<_> = selected.into_iter().map(|(id, uid, _)| (id, uid)).collect();
    assert_eq!(
        selected.len(),
        1,
        "select exactly one strategy; set MOON_COIN_PICK_CORE if ambiguous"
    );
    drop(conn);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("UTC now")
        .as_secs() as i64;
    println!(
        "SNAPSHOT now={now} metric=Quote prefer_usdt=true min_trades=5 trace_core={core:?} batch_scope=all"
    );
    let key = format!("{}@{}", selected[0].0, selected[0].1.expect("core"));
    for days in [30, 365, 0] {
        let q = Query {
            from: if days == 0 { -1 } else { now - days * 86400 },
            to: now,
            prefer_usdt: true,
            ..Default::default()
        };
        let (rows, lists) = crate::db::tuner::read_tuner_rows(&q, batch::read_keyed)
            .expect("production batch read");
        assert!(!rows.is_empty(), "period must contain trades");
        let selected_rows: Vec<_> = rows
            .iter()
            .filter(|r| r.key == key)
            .map(|r| r.trade.clone())
            .collect();
        assert!(
            !selected_rows.is_empty(),
            "selected strategy must have trades in each period"
        );
        for pct in [10, 25, 30] {
            let settings = PickSettings {
                min_trades: 5,
                holdout_pct: pct,
                black: true,
                white: true,
            };
            let p = settings.params_for(lists.get(&key));
            let mut single = q.clone();
            single.strategies = selected.clone();
            let pick = suggest_coins(&single, &p).expect("production single read and fit");
            let in_memory = coin_pick_from_rows(&selected_rows, &p);
            assert_eq!(
                format!("{pick:?}"),
                format!("{in_memory:?}"),
                "single/batch parity"
            );
            println!("PERIOD days={days} holdout_pct={pct}");
            trace(&selected_rows, &p, &pick);
            for cap in [30, 50] {
                let picks = batch::fit_batch_with_cap(
                    rows.clone(),
                    &lists,
                    settings,
                    &BatchProgress::default(),
                    cap,
                )
                .expect("uncancelled batch");
                let mut counts: BTreeMap<String, usize> = BTreeMap::from([
                    ("Useful".into(), 0),
                    ("Noise(SmallGain)".into(), 0),
                    ("Noise(HoldoutWorse)".into(), 0),
                    ("Noise(FewHoldout)".into(), 0),
                    ("NothingFound".into(), 0),
                    ("TooFewTrades".into(), 0),
                ]);
                for value in picks.values() {
                    *counts.entry(format!("{:?}", value.verdict)).or_default() += 1;
                }
                println!(
                    "C BATCH days={days} holdout_pct={pct} cap={cap} strategies_with_trades={} counts={counts:?}",
                    picks.len()
                );
                if cap == MAX_PICK {
                    let public = coin_picks_from_rows(
                        rows.clone(),
                        &lists,
                        settings,
                        &BatchProgress::default(),
                    )
                    .expect("public batch");
                    assert_eq!(public.len(), picks.len());
                    for (k, v) in &picks {
                        assert_eq!(format!("{v:?}"), format!("{:?}", public[k]));
                    }
                }
            }
        }
    }
}
