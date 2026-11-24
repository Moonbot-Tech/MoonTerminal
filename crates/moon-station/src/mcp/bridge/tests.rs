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

/// Regression target: the overview repeating every core's full status, 9.5 KB for 28 cores read
/// first in every conversation.
#[test]
fn an_overview_row_carries_only_departures_from_normal() {
    let healthy = json!({"core":"1","connection":"Ready","stale":false,"connection_epoch":3,"version":"7.73","trading":true,
        "runtime_started":true,"autodetect":false,"cpu_percent":4.0,"memory_mb":300,"transport_round_trip_ms":40,
        "exchange_order_latency_ms":90,"strategy_count":12,"report_sync":null,"problems":0});
    assert_eq!(
        brief(&healthy),
        json!({"core":"1","connection":"Ready","trading":true,"problems":0})
    );
    let mut troubled = healthy.clone();
    troubled["stale"] = json!(true);
    troubled["runtime_started"] = json!(false);
    troubled["report_sync"] = json!("ReportSyncProgress { rows: 5 }");
    let row = brief(&troubled);
    assert_eq!(row["stale"], true);
    assert_eq!(row["runtime_started"], false);
    assert_eq!(row["report_catch_up"], true);
    // A core Station has no state for keeps its unknowns as null rather than dropping out.
    let unavailable = brief(&json!({"core":"2","connection":"unavailable","stale":true}));
    assert_eq!(unavailable["connection"], "unavailable");
    assert!(unavailable["trading"].is_null());
    assert!(
        unavailable.get("runtime_started").is_none(),
        "unknown is not stopped"
    );
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

/// One sent revision of strategy `id`.
fn sent(id: u64) -> SentRevision {
    SentRevision {
        id,
        version: 1,
        last_date: 10,
    }
}

/// A batch reads as confirmed only when every strategy is: one answer missing makes the whole
/// operation unknown, and that strategy is named under it rather than dropped.
#[test]
fn a_batch_reports_its_worst_outcome_and_names_every_strategy() {
    let outcomes = BTreeMap::from([
        (1, json!({"status":"confirmed"})),
        (
            2,
            json!({"status":"adjusted","kept_by_core":[{"field":"OrderSize"}]}),
        ),
    ]);
    let result = strategy_result(&[sent(1), sent(2), sent(3)], &outcomes, Some("timeout"));
    assert_eq!(result["status"], "unknown");
    assert_eq!(result["execution_confirmed"], false);
    assert_eq!(result["strategies"]["confirmed"], json!(["1"]));
    assert_eq!(result["strategies"]["adjusted"], json!(["2"]));
    assert_eq!(result["strategies"]["unknown"], json!(["3"]));
    assert_eq!(result["adjusted"][0]["id"], "2");
    assert_eq!(result["reason"], "timeout");
    let all = BTreeMap::from([(1, json!({"status":"confirmed"}))]);
    let confirmed = strategy_result(&[sent(1)], &all, None);
    assert_eq!(confirmed["status"], "confirmed");
    assert_eq!(confirmed["execution_confirmed"], true);
}

/// Adjusted strategies past the detail bound are counted, not listed, so the result stays small.
#[test]
fn adjusted_details_are_bounded_and_counted() {
    let ids: Vec<u64> = (0..(MAX_ADJUSTED_DETAILS as u64 + 7)).collect();
    let outcomes: BTreeMap<u64, Value> = ids
        .iter()
        .map(|id| (*id, json!({"status":"adjusted","kept_by_core":[]})))
        .collect();
    let sent: Vec<SentRevision> = ids.iter().map(|id| sent(*id)).collect();
    let result = strategy_result(&sent, &outcomes, None);
    assert_eq!(
        result["adjusted"].as_array().map(Vec::len),
        Some(MAX_ADJUSTED_DETAILS)
    );
    assert_eq!(result["adjusted_details_omitted"], 7);
}

/// `strategy_compare` reads a space-separated coin list (#1448) coin by coin, the way the
/// preview does: the second strategy lacks exactly GALA, not one whole-field item.
#[test]
fn a_space_separated_list_is_compared_coin_by_coin() {
    let rest = (0..40)
        .map(|i| format!("C{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let strategy = |coins: String| BTreeMap::from([("CoinsBlackList".to_string(), coins)]);
    let found = [
        ("1:7".to_string(), strategy(format!("ANC GALA {rest}"))),
        ("2:7".to_string(), strategy(format!("ANC {rest}"))),
    ];
    let answer = compare("Night", &found, false).expect("two live strategies");
    let list = &answer["differing_fields"]["CoinsBlackList"];
    assert_eq!(list["1:7"]["items"], 42);
    assert_eq!(list["2:7"]["items"], 41);
    assert_eq!(list["2:7"]["missing_from_first"], json!(["GALA"]));
    assert_eq!(list["2:7"]["not_in_first"], json!([]));
}
