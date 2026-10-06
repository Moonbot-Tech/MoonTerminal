//! Synthetic equivalence tests and manual benchmarks for Orders cache work.

use std::collections::{HashMap, HashSet};
use std::hint::black_box;
use std::time::Instant;

use moon_core::feed::OrderRow;

use super::sort::orders_signatures_from;
use super::{
    ObserveAction, OrderEntry, OrdersCacheKey, OrdersInputs, OrdersViewState, apply_main_lift,
    collect_scoped, main_highlights, orders_observe_action, orders_observe_step, prepare_entries,
};

/// Build 200 synthetic cores with 25 orders each, without reading terminal data.
fn fixture() -> Vec<(u64, String, Vec<OrderRow>)> {
    (1..=200)
        .map(|core| {
            let rows = (1..=25)
                .map(|uid| OrderRow {
                    market: format!("TOKEN{}USDT", uid % 5),
                    market_display: format!("TOKEN{}USDT", uid % 5),
                    coin: format!("TOKEN{}", uid % 5),
                    quote: "USDT".to_string(),
                    is_short: false,
                    size: 1.0,
                    remaining_size: 1.0,
                    sl_on: false,
                    ts_on: false,
                    vstop_on: false,
                    sl_fixed: false,
                    ts_fixed: false,
                    vstop_fixed: false,
                    vstop_level: 0.0,
                    vstop_vol: 0.0,
                    buy_price: 10.0,
                    sell_price: 12.0,
                    create_time_ms: uid as f64,
                    sell_create_time_ms: 0.0,
                    entry_fill_time_ms: 0.0,
                    price: 11.0,
                    fill_pct: 100.0,
                    strat: "EMA".to_string(),
                    strat_name: format!("Synthetic {uid}"),
                    strat_id: 1,
                    status: "BuyDone".to_string(),
                    uid,
                    emulator: uid % 2 == 0,
                    job_is_done: false,
                    pending: false,
                    filled: true,
                    stop_loss: None,
                    trailing: None,
                    take_profit: None,
                    vstop: None,
                    pending_cond: None,
                    liq: None,
                    panic_sell: false,
                    is_moon_shot: false,
                    corridor_price_down: 0.0,
                    corridor_price_up: 0.0,
                    buy_trace: None,
                    sell_trace: None,
                })
                .collect();
            (core, format!("Synthetic core {core}"), rows)
        })
        .collect()
}

/// Reproduce the original collect-all path as an independent equivalence oracle.
fn reference_collect(cores: &[(u64, String, Vec<OrderRow>)]) -> Vec<OrderEntry> {
    let mut entries = Vec::new();
    for (core, name, rows) in cores {
        for row in rows {
            entries.push(OrderEntry {
                core: *core,
                core_name: name.clone(),
                row: row.clone(),
            });
        }
    }
    entries
}

/// Print the same manual-loop timing unit for each benchmark.
fn measure<T>(name: &str, iterations: u32, mut work: impl FnMut() -> T) {
    let started = Instant::now();
    for _ in 0..iterations {
        black_box(work());
    }
    println!(
        "{name}: {} ns/iter",
        started.elapsed().as_nanos() / u128::from(iterations)
    );
}

/// Supply all seven independently mutable key inputs with 200 synthetic cores.
fn key_fixture() -> OrdersCacheKey {
    OrdersCacheKey {
        data_sig: 123,
        names_sig: 456,
        view: OrdersViewState::default(),
        scope_cores: (1..=200).collect(),
        current: Some((1, "TOKEN1USDT".to_string())),
        coin: "TOKEN".to_string(),
        main_open: (1..=200).map(|id| (id, format!("TOKEN{id}USDT"))).collect(),
    }
}

/// Omitting any borrowed field comparison would keep a stale row cache after that input changes.
#[test]
fn orders_key_match_agrees_with_owned_key() {
    let stored = key_fixture();
    for field in 0..=12 {
        let mut owned = key_fixture();
        match field {
            0 => owned.data_sig += 1,
            1 => owned.names_sig += 1,
            2 => owned.view.only_current_market = !owned.view.only_current_market,
            3 => {
                owned.scope_cores.pop();
            }
            4 => owned.current.as_mut().unwrap().1.push('X'),
            5 => owned.coin.push('X'),
            6 => owned.main_open[0].1.push('X'),
            7 => owned.current = None,
            8 => owned.current.as_mut().unwrap().0 += 1,
            9 => owned.main_open[0].0 += 1,
            10 => owned.scope_cores.swap(0, 1),
            11 => owned.main_open.swap(0, 1),
            _ => {}
        }
        assert_eq!(
            stored.matches(
                (owned.data_sig, owned.names_sig),
                owned.view,
                &owned.scope_cores,
                owned
                    .current
                    .as_ref()
                    .map(|(core, market)| (*core, market.as_str())),
                &owned.coin,
                &owned.main_open,
            ),
            owned == stored,
            "key field {field}"
        );
    }
}

/// Measure key construction and comparison on the unchanged observer path.
#[test]
#[ignore]
fn bench_orders_observer_key_200_cores() {
    let key = key_fixture();
    let inputs = key_fixture();
    measure(
        "bench_orders_observer_key_200_cores_owned_reference",
        20_000,
        || black_box(&inputs).clone() == *black_box(&key),
    );
    measure("bench_orders_observer_key_200_cores", 20_000, || {
        let inputs = black_box(&inputs);
        black_box(&key).matches(
            (inputs.data_sig, inputs.names_sig),
            inputs.view,
            &inputs.scope_cores,
            inputs
                .current
                .as_ref()
                .map(|(core, market)| (*core, market.as_str())),
            &inputs.coin,
            &inputs.main_open,
        )
    });
}

/// Reproduce the original first-row highlight loop independently of the production helper.
fn reference_highlights(
    entries: &[OrderEntry],
    main_open: &HashSet<(u64, String)>,
) -> HashSet<(u64, u64)> {
    let mut seen = HashSet::new();
    let mut highlights = HashSet::new();
    for entry in entries {
        let pair = (entry.core, entry.row.market.clone());
        if main_open.contains(&pair) && seen.insert(pair) {
            highlights.insert((entry.core, entry.row.uid));
        }
    }
    highlights
}

/// Restoring due-tick rebuilds would clone and sort an unchanged cache once per second.
#[test]
fn unchanged_key_due_ticks_repaint_without_rebuild() {
    let key = OrdersCacheKey {
        data_sig: 7,
        names_sig: 11,
        view: OrdersViewState::default(),
        scope_cores: vec![1, 2],
        current: None,
        coin: String::new(),
        main_open: vec![],
    };
    let mut rebuilds = 0;
    let mut repaints = 0;
    for _ in 0..60 {
        let (action, replacement) = orders_observe_step(
            Some(&key),
            OrdersInputs {
                signatures: (7, 11),
                view: OrdersViewState::default(),
                scope_ids: &[1, 2],
                current: None,
                coin: "",
                main_open: &[],
            },
            true,
        );
        assert!(replacement.is_none());
        match action {
            ObserveAction::Rebuild => {
                rebuilds += 1;
                repaints += 1;
            }
            ObserveAction::Repaint => repaints += 1,
            ObserveAction::Skip => {}
        }
    }
    assert_eq!(rebuilds, 0, "unchanged due ticks must reuse the row cache");
    assert_eq!(repaints, 60, "each due tick must still repaint");
    assert_eq!(orders_observe_action(true, false), ObserveAction::Rebuild);
    assert_eq!(orders_observe_action(false, false), ObserveAction::Skip);
    assert_eq!(orders_observe_action(false, true), ObserveAction::Repaint);
    let (action, replacement) = orders_observe_step(
        Some(&key),
        OrdersInputs {
            signatures: (7, 12),
            view: OrdersViewState::default(),
            scope_ids: &[1, 2],
            current: None,
            coin: "",
            main_open: &[],
        },
        false,
    );
    assert_eq!(action, ObserveAction::Rebuild);
    assert_eq!(replacement.unwrap().names_sig, 12);
}

/// Reverting the observer to unconditional rebuilding or dropping names from owned keys breaks wiring.
#[test]
fn orders_observer_calls_tested_step_and_preserves_names() {
    let source: String = include_str!("mod.rs")
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();
    assert!(source.contains("orders_observe_step(this.cache_key.as_ref(),OrdersInputs{"));
    assert!(source.contains("ifletSome(key)=key{this.rebuild_cache_with(b,&scope,key);}"));
    let owned = source
        .split("fncache_key_with(")
        .nth(1)
        .unwrap()
        .split("fn effective_scope")
        .next()
        .unwrap();
    assert!(owned.contains("names_sig:signatures.1"));
}

/// Dropping scoped names from the signature would keep stale Core cells after a rename.
#[test]
fn core_rename_changes_the_orders_names_sig() {
    let mut cores: Vec<_> = (1u64..=200)
        .map(|id| (id, format!("Synthetic core {id}")))
        .collect();
    let before = orders_signatures_from(
        cores
            .iter()
            .map(|(id, name)| (*id, id * 7, Some(name.as_str()))),
    );
    // Frozen formula from the original order-table revision signature, independent of names.
    let expected_data = (1u64..=200).fold(0u64, |signature, id| {
        signature
            .wrapping_mul(31)
            .wrapping_add(id)
            .wrapping_mul(31)
            .wrapping_add(id * 7)
    });
    assert_eq!(
        before.0, expected_data,
        "the gate's data signature must remain unchanged"
    );
    for index in 0..cores.len() {
        let original_len = cores[index].1.len();
        cores[index].1.push_str(" renamed");
        let after = orders_signatures_from(
            cores
                .iter()
                .map(|(id, name)| (*id, id * 7, Some(name.as_str()))),
        );
        assert_eq!(after.0, expected_data);
        assert_ne!(
            after.1, before.1,
            "rename of scoped core {}",
            cores[index].0
        );
        cores[index].1.truncate(original_len);
    }
}

/// Retain the measured rejected one-pass candidate beside the unchanged signature path.
#[test]
#[ignore]
fn bench_orders_signatures_200() {
    let sessions: Vec<_> = (1u64..=200)
        .map(|id| (id, format!("Synthetic {id}")))
        .collect();
    for size in [5, 200] {
        let ids: Vec<_> = (1..=size).collect();
        measure(&format!("orders_signatures_current_{size}"), 20_000, || {
            orders_signatures_from(black_box(&ids).iter().map(|id| {
                (
                    *id,
                    id * 7,
                    sessions
                        .iter()
                        .find(|(core, _)| core == id)
                        .map(|(_, name)| name.as_str()),
                )
            }))
        });
        measure(
            &format!("orders_signatures_rejected_{size}"),
            20_000,
            || {
                let names = rejected_scoped_session_names(
                    black_box(&ids),
                    sessions.iter().map(|(id, name)| (*id, name.as_str())),
                );
                orders_signatures_from(ids.iter().map(|id| (*id, id * 7, names.get(id).copied())))
            },
        );
    }
}

/// Reproduce the rejected map allocation and linear scope predicate for the manual benchmark.
fn rejected_scoped_session_names<'a>(
    scope: &[u64],
    sessions: impl Iterator<Item = (u64, &'a str)>,
) -> HashMap<u64, &'a str> {
    let mut names = HashMap::with_capacity(scope.len());
    for (core, name) in sessions.filter(|(core, _)| scope.contains(core)) {
        names.entry(core).or_insert(name);
    }
    names
}

/// Measure the real clone, sort, highlight, and lift work avoided by each unchanged due tick.
#[test]
#[ignore]
fn bench_orders_rebuild_5k() {
    let cores = fixture();
    let view = OrdersViewState::default();
    let overlays = HashMap::new();
    let main_open: HashSet<_> = (1u64..=200)
        .map(|core| (core, "TOKEN1USDT".to_string()))
        .collect();
    measure("bench_orders_rebuild_5k", 200, || {
        let (mut entries, real, emu) =
            prepare_entries(reference_collect(black_box(&cores)), &view, &overlays);
        let highlights = reference_highlights(&entries, &main_open);
        apply_main_lift(&mut entries, &view, &highlights, &main_open);
        black_box((real, emu, highlights));
        entries
    });
    let mut rebuilds = 0;
    let mut repaints = 0;
    for _ in 0..60 {
        match orders_observe_action(black_box(false), black_box(true)) {
            ObserveAction::Rebuild => {
                rebuilds += 1;
                repaints += 1;
            }
            ObserveAction::Repaint => repaints += 1,
            ObserveAction::Skip => {}
        }
    }
    println!("bench_orders_rebuild_5k: {rebuilds} rebuilds, {repaints} repaints / 60 due ticks");
}

/// Filtering before ownership must preserve every retained field and session-then-row order.
#[test]
fn scoped_collect_matches_collect_then_retain() {
    let cores = fixture();
    let scope: HashSet<_> = (1u64..=10).collect();
    for mode in 0..4 {
        let keep = |core, row: &OrderRow| {
            scope.contains(&core)
                && (mode != 1 || (core == 3 && row.market == "TOKEN1USDT"))
                && (mode != 2 || row.coin == "TOKEN2")
                && mode != 3
        };
        let mut expected = reference_collect(&cores);
        expected.retain(|entry| keep(entry.core, &entry.row));
        let actual = collect_scoped(
            cores
                .iter()
                .map(|(core, name, rows)| (*core, name.as_str(), rows.as_slice())),
            keep,
        );
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(&expected) {
            assert_eq!(actual.core, expected.core);
            assert_eq!(actual.core_name, expected.core_name);
            assert_eq!(format!("{:?}", actual.row), format!("{:?}", expected.row));
        }
    }
}

/// Measure collection with ten retained cores out of 200 synthetic sessions and 5k rows.
#[test]
#[ignore]
fn bench_orders_collect_5k_rows_10_of_200_cores() {
    let cores = fixture();
    let scope: HashSet<_> = (1u64..=10).collect();
    measure("bench_orders_collect_5k_rows_10_of_200_cores", 200, || {
        collect_scoped(
            black_box(&cores)
                .iter()
                .map(|(core, name, rows)| (*core, name.as_str(), rows.as_slice())),
            |core, _| black_box(&scope).contains(&core),
        )
    });
}

/// Borrowed pair keys must preserve the original first-row-per-open-pair highlight selection.
#[test]
fn main_highlight_marks_first_row_per_open_pair() {
    let entries = reference_collect(&fixture());
    let main_open: HashSet<_> = (1u64..=200)
        .map(|core| (core, "TOKEN1USDT".to_string()))
        .collect();
    let highlights = main_highlights(&entries, &main_open);
    assert_eq!(highlights, reference_highlights(&entries, &main_open));
    assert_eq!(highlights, (1u64..=200).map(|core| (core, 1)).collect());
    let reversed: Vec<_> = entries.into_iter().rev().collect();
    let reversed_highlights = main_highlights(&reversed, &main_open);
    assert_eq!(
        reversed_highlights,
        reference_highlights(&reversed, &main_open)
    );
    assert_eq!(
        reversed_highlights,
        (1u64..=200).map(|core| (core, 21)).collect()
    );
    assert!(main_highlights(&reversed, &HashSet::new()).is_empty());
}

/// Measure pair membership and first-row selection over 5k synthetic rows.
#[test]
#[ignore]
fn bench_orders_highlight_5k() {
    let entries = reference_collect(&fixture());
    let main_open: HashSet<_> = (1u64..=200)
        .map(|core| (core, "TOKEN1USDT".to_string()))
        .collect();
    measure("bench_orders_highlight_5k", 2_000, || {
        main_highlights(black_box(&entries), black_box(&main_open))
    });
}
