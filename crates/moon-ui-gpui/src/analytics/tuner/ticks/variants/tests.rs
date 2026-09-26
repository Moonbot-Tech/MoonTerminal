//! Where a search's answer lands in В1.

use std::collections::HashMap;

use super::land_answer;

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
