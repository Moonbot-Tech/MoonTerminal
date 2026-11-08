//! Validate model input against the advertised schema before an owner-thread command can run.

use super::*;

/// Optional invalid values must not silently turn into defaults or truncate across integer widths.
#[test]
fn schemas_reject_wrong_types_extra_keys_and_out_of_range_arguments() {
    for (name, arguments) in [
        ("strategy_schema", json!({"core":"1","strategy_kind":256})),
        ("strategy_list", json!({"core":"1","offset":"0"})),
        ("station_overview", json!({"cores":[42]})),
        // The pre-filter shape is gone: cores and the window live in `filter` now.
        ("reports_query", json!({"cores":[],"from_ms":1,"to_ms":2})),
        ("reports_aggregate", json!({"filter":{"from_ms":1}})),
        (
            "reports_aggregate",
            json!({"filter":{"from_ms":1,"to_ms":2,"cores":[3]}}),
        ),
        (
            "reports_aggregate",
            json!({"filter":{"from_ms":1,"to_ms":2},"group_by":["sellreason_like"]}),
        ),
        (
            "reports_whatif",
            json!({"filter":{"from_ms":1,"to_ms":2},"variant":{"sql":"1=1"}}),
        ),
        ("logs_start", json!({"cores":["1"],"seconds":1000})),
        (
            "action_preview",
            json!({"core":"1","action":{"kind":"order_place","market":"BTCUSDT","short":false,"price":100,"base_quantity":1,"use_market_stop":false}}),
        ),
    ] {
        let (_, validator) = catalog()
            .iter()
            .find(|(tool, _)| tool.name == name)
            .unwrap();
        assert!(!validator.is_valid(&arguments), "{name}");
    }
}

/// The schema the client sees and the parser behind it accept the same arguments: a field the
/// schema allows but the parser rejects would fail every call that uses it, and the reverse
/// would let the parser accept what the client is never told about.
#[test]
fn report_schemas_agree_with_their_parsers() {
    let filter = json!({"cores":["1","18446744073709551615"],"from_ms":1,"to_ms":2,"date_field":"buy","coins":["JCT"],
        "exclude_coins":["ACE"],"coin_like":"J%","strategy_ids":["0","7"],"strategy_name":"MainShot !test","side":"short",
        "currency":"USDT","reason":["stoploss"],"exclude_reason":["funding"],"lev_min":1,"lev_max":20,"profit_usdt_min":-5,
        "profit_usdt_max":5,"profit_pct_min":-1,"profit_pct_max":1,"hold_sec_min":0,"hold_sec_max":60,
        "entry":[{"field":"d1m","min":0,"max":3}],"include_emulator":true});
    let cases = [
        (
            "reports_aggregate",
            json!({"filter":filter,"group_by":["coin","strategy_name","bucket","entry_quantile"],"bucket_size":"week",
            "tz":"Europe/Kyiv","entry_quantile":{"field":"d1m","bins":4},"metrics":["trades","native_profit"],
            "having":[{"metric":"trades","op":">=","value":5}],"sort":[{"by":"profit_usdt_sum","dir":"asc"}],"limit":100,"offset":0}),
        ),
        (
            "reports_query",
            json!({"filter":filter,"fields":["coin","d1m"],"sort":[{"by":"coin"}],"limit":10,"offset":5}),
        ),
        (
            "reports_whatif",
            json!({"filter":filter,"variant":{"coins":["JCT"],"entry":[{"field":"d1m","max":1}]},"holdout_pct":30}),
        ),
        (
            "blacklist_candidates",
            json!({"filter":filter,"min_trades":5,"holdout_pct":30}),
        ),
        (
            "strategy_effect",
            json!({"core":"1","strategy_id":"7","valid_from_ms":5,"window_days":14}),
        ),
        (
            "strategy_versions",
            json!({"core":"1","strategy_id":"7","from_ms":1,"to_ms":2,"at_ms":2,"fields":["StopLoss"],"offset":0}),
        ),
    ];
    for (name, arguments) in cases {
        let (_, validator) = catalog()
            .iter()
            .find(|(tool, _)| tool.name == name)
            .unwrap();
        assert!(validator.is_valid(&arguments), "schema rejects {name}");
        moon_core::automation::reports::check_args(name, arguments)
            .unwrap_or_else(|e| panic!("parser rejects {name}: {e}"));
    }
}

/// Regression target: one `bytes=-` for every failure. The over-budget answer is the one that
/// says a tool needs narrowing, so it must not look like a refused permission in the log.
#[test]
fn a_failure_is_logged_by_its_kind() {
    let class = |text: &str| error_class(&anyhow::anyhow!(text.to_owned()));
    assert_eq!(
        class("response exceeds the budget; narrow the query or page size"),
        "over_budget"
    );
    assert_eq!(class("invalid tool arguments: x"), "invalid_arguments");
    assert_eq!(class("unknown tool"), "unknown_tool");
    assert_eq!(class("report read failed: disk"), "failed");
}

/// The narrowing arguments are bounded: an empty field name or an unbounded list would let one call
/// ask for everything again.
#[test]
fn narrowing_arguments_are_bounded() {
    let refused = [
        (
            "strategy_read",
            json!({"core":"1","strategy_id":"2","fields":[]}),
        ),
        (
            "strategy_read",
            json!({"core":"1","strategy_id":"2","fields":[""]}),
        ),
        ("logs_get", json!({"capture_id":"x","contains":""})),
        ("core_state", json!({"core":"1","section":"everything"})),
    ];
    for (name, arguments) in refused {
        let (_, validator) = catalog()
            .iter()
            .find(|(tool, _)| tool.name == name)
            .unwrap();
        assert!(!validator.is_valid(&arguments), "{name} {arguments}");
    }
    let accepted = [
        (
            "strategy_read",
            json!({"core":"1","strategy_id":"2","fields":["StopLoss"]}),
        ),
        (
            "logs_get",
            json!({"capture_id":"x","contains":"order","summary":true}),
        ),
        ("core_state", json!({"core":"1","section":"problems"})),
    ];
    for (name, arguments) in accepted {
        let (_, validator) = catalog()
            .iter()
            .find(|(tool, _)| tool.name == name)
            .unwrap();
        assert!(validator.is_valid(&arguments), "{name} {arguments}");
    }
}

/// The real order interface accepts a balance-currency amount and excludes the old incorrect unit.
#[test]
fn order_amount_schema_agrees_with_the_typed_action() {
    let (_, validator) = catalog()
        .iter()
        .find(|(tool, _)| tool.name == "action_preview")
        .unwrap();
    let arguments = json!({"core":"1","action":{"kind":"order_place","market":"BTCUSDT","short":false,"price":100,"balance_amount":25,"use_market_stop":false}});
    assert!(validator.is_valid(&arguments));
    let action: Action = serde_json::from_value(arguments["action"].clone()).unwrap();
    assert!(matches!(
        action,
        Action::OrderPlace {
            balance_amount: 25.0,
            ..
        }
    ));
}
