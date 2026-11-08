//! Log redaction must happen before a diagnostic line enters any model-facing buffer.

use super::*;

fn line(core: &str, received_ms: i64, text: &str) -> Value {
    json!({"core":core,"received_ms":received_ms,"text":text})
}

/// Regression target: the filter matching case-sensitively. A core writes «Order» and «order».
#[test]
fn contains_keeps_matching_lines_ignoring_case() {
    let rows = [
        line("1", 10, "Order 42 placed"),
        line("1", 20, "ticker update"),
        line("2", 30, "order 43 cancelled"),
    ];
    let kept = select(&rows, Some("ORDER"));
    assert_eq!(kept.len(), 2);
    assert_eq!(select(&rows, None).len(), 3);
}

/// A repeated message about different numbers is one line in a summary, most frequent first.
#[test]
fn a_summary_folds_numbers_and_counts_repeats() {
    let rows = [
        line("1", 10, "Order 42 cancelled at 0.1234"),
        line("2", 20, "Order 43 cancelled at 0.1250"),
        line("1", 30, "Connected"),
        line("1", 40, "Order 44 cancelled at 0.1300"),
    ];
    let refs: Vec<&Value> = rows.iter().collect();
    let groups = summarize(&refs, true);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0]["count"], 3);
    assert_eq!(
        groups[0]["sample"], "Order 42 cancelled at 0.1234",
        "a real line"
    );
    assert_eq!(groups[0]["first_received_ms"], 10);
    assert_eq!(groups[0]["last_received_ms"], 40);
    assert_eq!(groups[0]["cores"], json!(["1", "2"]));
    assert_eq!(groups[1]["count"], 1);
    // One core: the list of cores says nothing and is left out.
    let single = summarize(&refs, false);
    assert!(single[0].get("cores").is_none());
}

/// Regression target: repeating the time-axis labels and the core on every line, about 110 bytes of
/// each one on a live log.
#[test]
fn a_line_carries_only_what_differs_from_line_to_line() {
    let row = json!({"core":"1","server_time_ms":5,"server_time_axis":"core_local","received_ms":6,"received_time_axis":"UTC","text":"x"});
    assert_eq!(
        compact(&row, false),
        json!({"received_ms":6,"server_time_ms":5,"text":"x"})
    );
    assert_eq!(compact(&row, true)["core"], "1");
}

/// Secret-labelled lines must be removed before storage without erasing useful order evidence.
#[test]
fn credential_lines_are_removed_but_order_diagnostics_remain_readable() {
    let cfg = AppConfig::headless(Vec::new());
    assert_eq!(
        redact("API_KEY=do-not-send", &cfg),
        "[credential-bearing line redacted]"
    );
    assert_eq!(
        redact("Authorization: Bearer do-not-send", &cfg),
        "[credential-bearing line redacted]"
    );
    assert_eq!(
        redact("Order 42 BTCUSDT cancelled: insufficient balance", &cfg),
        "Order 42 BTCUSDT cancelled: insufficient balance"
    );
}
