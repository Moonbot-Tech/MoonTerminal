//! An empty authorization must never reach the report layer's empty-means-all convention.

use super::*;

/// Empty core selection would otherwise invoke the report layer's all-core default.
#[test]
fn empty_scope_is_rejected_before_opening_the_database() {
    let args = serde_json::json!({"filter":{"from_ms":1,"to_ms":1000}});
    let error = run("reports_aggregate", &Scope::default(), args).expect_err("empty scope");
    assert!(error.to_string().contains("explicitly authorized"));
}

/// A core id above 2^63 travels as a string and must not be rounded through a float.
#[test]
fn filter_cores_keep_large_ids_exact() {
    let args = serde_json::json!({"filter":{"cores":["18446744073709551615","3"]}});
    assert_eq!(filter_cores(&args).unwrap(), vec![u64::MAX, 3]);
    let bad = serde_json::json!({"filter":{"cores":[3]}});
    assert!(filter_cores(&bad).is_err());
}

/// Live check against a copy of a real replica: the trade pass agrees with the Report's own totals
/// on the same filter, and the answers stay within their budgets. Run by hand with
/// `MT_REPORTS_ROOT=<dir holding data/reports.sqlite>`; never touches the default data root.
#[test]
#[ignore = "needs a copy of a real replica"]
fn the_trade_pass_agrees_with_report_totals_on_a_real_replica() {
    let Ok(root) = std::env::var("MT_REPORTS_ROOT") else {
        return;
    };
    assert!(crate::config::paths::set_data_dir_override(root.into()));
    // The process lease and the cheap integrity gate every replica read requires.
    let _permit = crate::db::report_recovery::prepare();
    let conn = crate::db::open_reader().unwrap();
    let cores: Vec<u64> = crate::db::distinct_cores(&conn)
        .unwrap()
        .into_iter()
        .map(|(id, _)| id)
        .take(64)
        .collect();
    let to = crate::util::now_unix_ms_i64();
    let from = to - 30 * 86_400_000;
    let scope = Scope {
        cores: cores.clone(),
        ..Default::default()
    };
    let started = std::time::Instant::now();
    let answer = run(
        "reports_aggregate",
        &scope,
        serde_json::json!({"filter":{"from_ms":from,"to_ms":to},"group_by":["coin","strategy_name"],
            "having":[{"metric":"trades","op":">=","value":5},{"metric":"profit_usdt_sum","op":"<","value":0}],
            "sort":[{"by":"profit_usdt_sum"}],"limit":100,"metrics":["trades","profit_usdt_sum","unvalued"]}),
    )
    .unwrap();
    let bytes = answer.to_string().len();
    eprintln!(
        "aggregate: {} ms, {bytes} B, groups {}, totals {}",
        started.elapsed().as_millis(),
        answer["total_groups"],
        answer["totals"]
    );
    assert!(bytes < 70_000);
    // The independent check: the Report's own totals over the same rows.
    let all = run(
        "reports_aggregate",
        &scope,
        serde_json::json!({"filter":{"from_ms":from,"to_ms":to},"metrics":["profit_usdt_sum","trades","funding_usdt","unvalued"]}),
    )
    .unwrap();
    let snapshot = crate::db::read_snapshot(&conn).unwrap();
    let report = crate::db::query_totals(
        &snapshot,
        &crate::db::ReportFilter {
            core_uids: cores,
            date_from: Some((from + 999).div_euclid(1000)),
            date_to: Some((to - 1).div_euclid(1000)),
            emulator: Some(false),
            rows: crate::db::RowScope::Closed,
            axis: crate::db::ReportAxis::load(&snapshot, chrono_tz::UTC).unwrap(),
            ..Default::default()
        },
    )
    .unwrap();
    let report_usdt = report
        .quotes
        .valuation
        .as_ref()
        .and_then(|v| v.usdt.as_ref().map(|t| t.profit));
    eprintln!(
        "ours {} | report orders {} usdt {:?} unavailable {:?}",
        all["totals"],
        report.quotes.orders,
        report_usdt,
        report
            .quotes
            .valuation
            .as_ref()
            .map(|v| v.unavailable_orders)
    );
    if let Some(report_usdt) = report_usdt {
        let ours = all["totals"]["profit_usdt_sum"].as_f64().unwrap();
        assert!((ours - report_usdt).abs() < 0.02, "{ours} vs {report_usdt}");
    }
}
