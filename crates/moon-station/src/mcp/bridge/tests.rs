//! What the owner-side tools put into an answer, without a live core.

use super::*;

/// Regression target: returning the whole strategy whatever was asked, the 9 KB a stop value cost.
#[test]
fn a_strategy_read_keeps_only_the_named_fields() {
    let mut read = json!({"strategies":[{"id":"7","fields":{"StopLoss":-1.5,"CoinsBlackList":"A,B,C","SellPrice":0.3}}]});
    keep_fields(&mut read, &["StopLoss", "TakeProfit"]);
    let strategy = &read["strategies"][0];
    assert_eq!(strategy["fields"], json!({"StopLoss":-1.5}));
    assert_eq!(strategy["absent_fields"], json!(["TakeProfit"]));
    assert_eq!(strategy["id"], "7", "the rest of the strategy stays");
}

/// Every named field present: nothing to explain.
#[test]
fn nothing_absent_says_nothing() {
    let mut read = json!({"strategies":[{"fields":{"StopLoss":-1.5}}]});
    keep_fields(&mut read, &["StopLoss"]);
    assert!(read["strategies"][0].get("absent_fields").is_none());
}

/// A field name in another case is the same field, as strategy_schema treats it.
#[test]
fn a_field_name_in_another_case_still_matches() {
    let mut read = json!({"strategies":[{"fields":{"StopLoss":-1.5,"SellPrice":0.3}}]});
    keep_fields(&mut read, &["stoploss"]);
    assert_eq!(read["strategies"][0]["fields"], json!({"StopLoss":-1.5}));
    assert!(read["strategies"][0].get("absent_fields").is_none());
}

/// Regression target: scrubbing the core's text with the log-line filter, which blanked a problem
/// about an exchange key whole, the one place its words must stay.
#[test]
fn a_problem_about_an_api_key_keeps_its_words() {
    let cfg = AppConfig::headless(Vec::new());
    let text = "Exchange rejected the API key: IP not whitelisted, check key secret permissions";
    assert_eq!(scrub(text, &cfg), text);
}

/// A list that never came is not a clean core.
#[test]
fn no_problem_list_is_unknown_not_healthy() {
    let keep = |text: &str| text.to_owned();
    let unknown = problems(&moon_core::feed::CoreProblems::default(), true, keep);
    assert_eq!(unknown["known"], false);
    assert!(unknown["count"].is_null());
    let none = problems(
        &moon_core::feed::CoreProblems {
            supported: true,
            items: Vec::new(),
        },
        true,
        keep,
    );
    assert_eq!(none["known"], true);
    assert_eq!(none["count"], 0);
}

/// Regression target: a disconnected core's last list read as current. The store keeps it across a
/// plain disconnect.
#[test]
fn a_disconnected_cores_list_is_marked_stale() {
    let list = moon_core::feed::CoreProblems {
        supported: true,
        items: Vec::new(),
    };
    let answer = problems(&list, false, |text| text.to_owned());
    assert_eq!(answer["stale"], true);
    assert!(
        answer["interpretation"]
            .as_str()
            .unwrap()
            .contains("disconnected")
    );
}
