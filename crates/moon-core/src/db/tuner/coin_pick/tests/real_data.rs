//! Opt-in diagnostics on a disposable database snapshot, never the live installation.
//!
//! Set MOON_COIN_PICK_DATA_DIR to a snapshot root with the three database families in its
//! canonical data directory, and MOON_COIN_PICK_STRATEGY to a name substring. Optional
//! MOON_COIN_PICK_CORE selects the traced core; the batch still covers all cores. Exact name
//! matches take precedence over substring matches. MOON_COIN_PICK_NOW pins the UTC cutoff for
//! before/after comparisons. Run this exact ignored test alone: the root override is process-wide.

use super::*;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Print fresh-deal profit and drawdown without publishing any strategy or coin identities.
fn print_tally(label: &str, value: &Tally) {
    println!(
        "{label}: n={} profit={:.6} pf={:.6} dd={:.6}",
        value.n,
        value.profit,
        value.profit_factor(),
        value.max_dd
    );
}

/// Explain whether the legacy fit or its fixed train-only fallback supplied the whitelist.
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
    let scores = score_rows(train, manual);
    let fitted_black = if p.black {
        gated_candidates(&scores, p, true, MAX_PICK)
    } else {
        HashSet::new()
    };
    let legacy = super::super::whitelist::legacy(train, &scores, p, &fitted_black, MAX_PICK);
    let fallback = if p.white && legacy.is_empty() && fitted_black.is_empty() {
        super::super::whitelist::fallback(&scores, p.min_trades, MAX_PICK)
    } else {
        HashSet::new()
    };
    let source = if !p.white {
        "disabled"
    } else if !legacy.is_empty() {
        "legacy"
    } else if !fitted_black.is_empty() {
        "legacy-blacklist"
    } else if !pick.white.is_empty() {
        "top20"
    } else {
        "none"
    };
    println!(
        "WHITELIST source={source} fixed_n={} legacy_coins={} fallback_coins={}",
        super::super::whitelist::TOP_N,
        legacy.len(),
        fallback.len()
    );
    println!(
        "A train_n={} holdout_n={} verdict={:?} black={} white={}",
        pick.train_n,
        pick.holdout_n,
        pick.verdict,
        pick.black.len(),
        pick.white.len()
    );
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
    let floor = (NOISE_ABS_SHARE * pick.fact_holdout.profit.abs())
        .max(moments.sd() * (touched as f64).sqrt());
    println!(
        "FLOOR touched={touched} floor={floor:.6} gain={:.6}",
        pick.pick_holdout.profit - pick.manual_holdout.profit
    );
}

/// Read and fit the production single/batch paths on the same copied, fixed-cutoff snapshot.
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
    let now = std::env::var("MOON_COIN_PICK_NOW")
        .ok()
        .map(|v| v.parse::<i64>().expect("fixed UTC now"))
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("UTC now")
                .as_secs() as i64
        });
    println!("SNAPSHOT now={now} metric=Quote prefer_usdt=true min_trades=5 batch_scope=all");
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
        for pct in [25, 30] {
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
            let started = Instant::now();
            let in_memory = coin_pick_from_rows(&selected_rows, &p);
            println!(
                "TIMING_SINGLE days={days} holdout_pct={pct} ms={:.3}",
                started.elapsed().as_secs_f64() * 1000.0
            );
            assert_eq!(
                format!("{pick:?}"),
                format!("{in_memory:?}"),
                "single/batch parity"
            );
            println!("PERIOD days={days} holdout_pct={pct}");
            trace(&selected_rows, &p, &pick);
            let started = Instant::now();
            let picks =
                coin_picks_from_rows(rows.clone(), &lists, settings, &BatchProgress::default())
                    .expect("uncancelled batch");
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            println!(
                "TIMING_BATCH days={days} holdout_pct={pct} ms={ms:.3} per_strategy_ms={:.3}",
                ms / picks.len() as f64
            );
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
            assert_eq!(
                format!("{pick:?}"),
                format!("{:?}", picks[&key]),
                "public batch/single parity"
            );
            println!(
                "C BATCH days={days} holdout_pct={pct} strategies_with_trades={} counts={counts:?}",
                picks.len()
            );
        }
    }
}
