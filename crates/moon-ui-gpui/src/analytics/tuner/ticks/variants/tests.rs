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

/// "Search" on `UseTakeProfit`: the switch lands, and so does the `TakeProfit` the answer
/// completed for it — Save would otherwise write the switch with no per cent behind it — while
/// every other cell of В1 stays.
#[test]
fn a_search_of_one_field_lands_the_values_it_completed() {
    let mut v1 = cells(&[("SellPrice", "1.5"), ("StopLoss", "-3")]);
    land_answer(
        &mut v1,
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

/// A search of one field that left it at its base answers without it: В1's cell stays.
#[test]
fn a_field_left_at_its_base_keeps_what_v1_had() {
    let mut v1 = cells(&[("SellPrice", "1.5")]);
    land_answer(&mut v1, &[]);
    assert_eq!(v1, cells(&[("SellPrice", "1.5")]));
}

/// "Search all" lays its answer over В1 and never empties it (the developer, 2026-09-25): a
/// search runs from В1 as it stands, so a cell it did not move is still В1's, and В1 is cleared
/// only by the user.
#[test]
fn a_search_of_every_field_keeps_what_v1_had() {
    let mut v1 = cells(&[("SellPrice", "1.5"), ("StopLoss", "-3")]);
    land_answer(&mut v1, &answer(&[("SellPrice", "2")]));
    assert_eq!(v1, cells(&[("SellPrice", "2"), ("StopLoss", "-3")]));
    land_answer(&mut v1, &[]);
    assert_eq!(v1, cells(&[("SellPrice", "2"), ("StopLoss", "-3")]));
}
