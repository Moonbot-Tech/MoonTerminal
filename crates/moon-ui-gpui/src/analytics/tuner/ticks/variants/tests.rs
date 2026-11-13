//! Where a search's answer lands in В1.

use std::collections::HashMap;

use super::land_answer;

/// Search hover text must wait with the card rather than claim a finished zero-fit sample.
#[test]
fn search_tooltips_wait_for_the_selected_tape_read() {
    use super::super::state::{TicksData, TicksState};
    let mut state = TicksState::default();
    // The scope's rows are on screen and judged once; a reload of the same scope reads the
    // tape again. (A first load, before any judgement, says it is loading instead:
    // `the_search_waits_for_the_selected_strategies_deals`.)
    state.publish(Ok(TicksData::default()), false);
    state.mark_rows_judged();
    state.tape_reading = true;
    for language in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        assert_eq!(
            super::search_status_tip(&state).unwrap(),
            rust_i18n::t!("analytics.ticks.fetch_reading")
        );
    }
    let _locale = crate::test_locale::force("en");
    state.tape_reading = false;
    assert_eq!(
        super::search_status_tip(&state).unwrap(),
        "Too few fit deals: 0 of 20 — the search does not run"
    );
}

fn cells(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

fn answer(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

fn keys(names: &[&str]) -> Vec<String> {
    names.iter().map(|k| (*k).to_string()).collect()
}

/// "Search" on `UseTakeProfit`: the switch lands, and so does the `TakeProfit` the answer
/// completed for it — Save would otherwise write the switch with no per cent behind it — while
/// every cell the search did not vary stays.
#[test]
fn a_search_of_one_field_lands_the_values_it_completed() {
    let mut v1 = cells(&[("SellPrice", "1.5"), ("StopLoss", "-3")]);
    land_answer(
        &mut v1,
        &keys(&["UseTakeProfit"]),
        &answer(&[("TakeProfit", "1"), ("UseTakeProfit", "YES")]),
    );
    assert_eq!(
        v1,
        cells(&[
            ("SellPrice", "1.5"),
            ("StopLoss", "-3"),
            ("UseTakeProfit", "YES"),
            ("TakeProfit", "1"),
        ])
    );
}

/// A searched field is searched anew from the strategies (LinKvo, 2026-09-25): an answer that
/// leaves it at the strategies' own value empties its cell, whatever В1 had put there.
#[test]
fn a_searched_field_left_at_the_strategy_empties_its_cell() {
    let mut v1 = cells(&[("SellPrice", "1.5")]);
    land_answer(&mut v1, &keys(&["SellPrice"]), &[]);
    assert!(v1.is_empty(), "{v1:?}");
}

/// "Search all" replaces every searched cell by its answer and leaves the cells it did not
/// search as they were.
#[test]
fn a_search_of_every_ticked_field_replaces_them_and_keeps_the_rest() {
    let mut v1 = cells(&[
        ("SellPrice", "1.5"),
        ("StopLoss", "-3"),
        ("MaxModifier", "5"),
    ]);
    land_answer(
        &mut v1,
        &keys(&["SellPrice", "StopLoss"]),
        &answer(&[("SellPrice", "2")]),
    );
    assert_eq!(v1, cells(&[("SellPrice", "2"), ("MaxModifier", "5")]));
}

/// The search buttons wait for the selected strategies' deals: until the scope's rows are
/// published and judged by their tape stage there is no sample, and "too few" cannot say so
/// yet, so the tooltip says they are loading — and a selection change brings that back.
/// Breaks when the loading check leaves the tip, and with it the buttons' gate.
#[test]
fn the_search_waits_for_the_selected_strategies_deals() {
    let _locale = crate::test_locale::force("en");
    let loading = rust_i18n::t!("analytics.ticks.pick.loading").to_string();
    let mut state = super::super::state::TicksState::default();
    assert_eq!(super::search_status_tip(&state), Some(loading.clone()));
    state.publish(Ok(super::super::state::TicksData::default()), false);
    assert_eq!(
        super::search_status_tip(&state),
        Some(loading.clone()),
        "published, not judged"
    );
    state.mark_rows_judged();
    assert_ne!(super::search_status_tip(&state), Some(loading.clone()));
    state.invalidate();
    assert_eq!(
        super::search_status_tip(&state),
        Some(loading.clone()),
        "another selection"
    );
    state.publish(Err(moon_core::db::ReadFail::NotReady), false);
    assert_ne!(
        super::search_status_tip(&state),
        Some(loading),
        "a failed read is not loading"
    );
    let cfg = include_str!("../cfg.rs");
    assert!(cfg.contains("all_enabled: loaded && !running && !too_few,"));
}
