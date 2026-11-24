//! The client's view of a strategy edit carries what differs and nothing it already knows.

use moon_core::automation::{Action, StrategyFieldEdit};
use serde_json::{Value, json};

use super::*;

/// An edit action naming `ids`; the view reads the preview, not the action's fields.
fn edit(ids: &[u64]) -> Action {
    Action::StrategyEdit {
        edits: ids
            .iter()
            .map(|id| StrategyFieldEdit {
                id: *id,
                fields: Default::default(),
                checked: None,
                path: None,
            })
            .collect(),
        apply_to_orders: false,
    }
}

/// A retained preview row of a strategy carrying `coins` as its CoinsBlackList and a filter value.
fn row(name: &str, filter_max: f64, coins: &str, version: i32) -> Value {
    json!({"id":"7","version":version,"last_date":100,"kind":3,"checked":true,"path":"a",
        "fields":{"StrategyName":name,"FilterMax":filter_max,"FilterBy":"24h","CoinsBlackList":coins}})
}

/// The case from the report: two fields edited on a strategy whose coin list is kilobytes long.
/// The view must name the strategy and the two fields, and must not carry the coin list or the
/// revision stamps that move on every edit.
#[test]
fn a_changed_strategy_shows_its_name_and_only_the_fields_that_differ() {
    let coins = (0..400)
        .map(|i| format!("C{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let preview = json!({"strategies":[{"id":"7","unchanged":false,
        "before":row("Night",2.0,&coins,1),"after":row("Night",3.0,&coins,1)}],
        "apply_to_orders":false,"limits":"x"});
    let view = client_view(&edit(&[7]), &preview, false);
    let only = &view["strategies"][0];
    assert_eq!(only["name"], "Night");
    assert_eq!(
        only["changes"],
        json!({"FilterMax":{"before":2.0,"after":3.0}})
    );
    let text = view.to_string();
    assert!(!text.contains("C399"), "the coin list leaked: {text}");
    assert!(text.len() < 1_000, "{} bytes", text.len());
}

/// A changed coin list is a count and what entered and left it, unless asked for whole.
#[test]
fn a_long_list_is_shown_as_its_difference_unless_asked_whole() {
    let old = (0..60)
        .map(|i| format!("c{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let new = format!("{old}, NEWCOIN");
    let preview = json!({"strategies":[{"id":"7","unchanged":false,
        "before":row("Night",2.0,&old,1),"after":row("Night",2.0,&new,1)}]});
    let short = client_view(&edit(&[7]), &preview, false);
    let coins = &short["strategies"][0]["changes"]["CoinsBlackList"];
    assert_eq!(coins["before"]["items"], 60);
    assert_eq!(coins["after"]["items"], 61);
    assert_eq!(coins["added"], json!(["NEWCOIN"]));
    assert_eq!(coins["removed_count"], 0);
    let whole = client_view(&edit(&[7]), &preview, true);
    assert_eq!(
        whole["strategies"][0]["changes"]["CoinsBlackList"]["after"],
        json!(new)
    );
}

/// A strategy that already holds the values is reported apart, so the client is not told an
/// unsent edit is part of the submission.
#[test]
fn a_strategy_left_out_is_named_apart_from_the_changed_ones() {
    let same = row("Day", 2.0, "A", 1);
    let preview = json!({"strategies":[
        {"id":"7","unchanged":false,"before":row("Night",2.0,"A",1),"after":row("Night",3.0,"A",1)},
        {"id":"8","unchanged":true,"before":same,"after":same}]});
    let view = client_view(&edit(&[7, 8]), &preview, false);
    assert_eq!(view["strategies_changed"], 1);
    assert_eq!(view["strategies"].as_array().map(Vec::len), Some(1));
    assert_eq!(view["unchanged"], json!([{"id":"8","name":"Day"}]));
}

/// Other actions keep the preview they always had: an order's limits and size are the point.
#[test]
fn another_action_is_shown_as_previewed() {
    let preview = json!({"market_limits":{"tick_size":0.1},"before":{"x":1}});
    assert_eq!(
        client_view(&Action::RuntimeRestart, &preview, false),
        preview
    );
}

/// Spacing and case are not differences in a coin list.
#[test]
fn list_items_ignore_spacing_and_case() {
    assert_eq!(
        list_items("CoinsWhiteList", "btc, Eth,,BTC "),
        list_items("CoinsWhiteList", "ETH,BTC")
    );
}

/// A coin list separated by spaces, as MoonBot accepts it (#1448), is several coins: lifting
/// GALA out of "ANC GALA ..." is one coin removed, not one whole-field item swapped for another.
#[test]
fn a_space_separated_list_is_diffed_coin_by_coin() {
    let rest = (0..40)
        .map(|i| format!("C{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let preview = json!({"strategies":[{"id":"7","unchanged":false,
        "before":text_row("CoinsBlackList",&format!("ANC GALA {rest}")),
        "after":text_row("CoinsBlackList",&format!("ANC {rest}"))}]});
    let view = client_view(&edit(&[7]), &preview, false);
    let coins = &view["strategies"][0]["changes"]["CoinsBlackList"];
    assert_eq!(coins["before"]["items"], 42);
    assert_eq!(coins["after"]["items"], 41);
    assert_eq!(coins["removed"], json!(["GALA"]));
    assert_eq!(coins["added_count"], 0);
}

/// A retained preview row of strategy 7 whose one field besides its name is `field`.
fn text_row(field: &str, value: &str) -> Value {
    json!({"id":"7","version":1,"last_date":100,"kind":3,"checked":true,"path":"a",
        "fields":{"StrategyName":"Night",field:value}})
}

/// A long free-text field is not a coin list: a comment of words without commas stays one item,
/// so a reworded comment is shown as its old text removed and its new text added, as before #1448.
#[test]
fn a_long_comment_keeps_its_whole_text_as_one_item() {
    let old = "watch the big green candles before the night session and do not chase anything that has already doubled today";
    let new = "do not chase anything that has already doubled today and watch the big green candles before the night session";
    let preview = json!({"strategies":[{"id":"7","unchanged":false,
        "before":text_row("Comment",old),"after":text_row("Comment",new)}]});
    let view = client_view(&edit(&[7]), &preview, false);
    let shown = &view["strategies"][0]["changes"]["Comment"];
    assert_eq!(shown["before"]["items"], 1);
    assert_eq!(shown["after"]["items"], 1);
    assert_eq!(shown["added"], json!([new.to_ascii_uppercase()]));
    assert_eq!(shown["removed"], json!([old.to_ascii_uppercase()]));
}

/// Every separator a strategy coin list may use counts alike, whatever the name's case; a field
/// that is not a coin list keeps splitting on commas only.
#[test]
fn list_items_split_on_every_strategy_separator() {
    assert_eq!(
        list_items("coinsblacklist", "anc gala;btc\teth\nxrp"),
        list_items("CoinsBlackList", "ANC,GALA,BTC,ETH,XRP")
    );
    assert_eq!(list_items("Comment", "anc gala, btc").len(), 2);
}
