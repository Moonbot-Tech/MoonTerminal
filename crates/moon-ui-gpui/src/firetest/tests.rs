use super::*;

#[test]
fn chart_smoke_stage_plan_covers_every_runtime_phase() {
    assert_eq!(
        STAGE_PLAN.len(),
        Phase::StageCount as usize,
        "FireTest Phase changed; update STAGE_PLAN so chart-smoke stays one explicit scenario"
    );
    assert!(
        !STAGE_PLAN.contains(&Phase::StageCount),
        "phase count sentinel must never be part of the runtime stage plan"
    );
}

#[test]
fn chart_smoke_stage_plan_is_one_contiguous_scenario() {
    let names: Vec<&'static str> = STAGE_PLAN.iter().map(|phase| phase.stage_name()).collect();
    assert_eq!(
        names,
        vec![
            "start",
            "open_chart",
            "wait_chart_probe",
            "settle_live_chart",
            "baseline",
            "mouse_storm",
            "static_text_gap",
            "static_text_warmup",
            "static_text_storm",
            "command_error_contract",
            "tool_windows_open",
            "tool_windows_verify_open",
            "tool_windows_dedup",
            "tool_windows_verify_dedup",
            "root_overlay_contract",
            "locale_switch",
            "locale_switch_verify",
            "price_scale_50",
            "price_scale_20",
            "price_scale_auto",
            "price_scale_verify_auto",
            "order_cancel_lag",
            "cooldown",
            "result",
        ],
        "chart-smoke must remain one ordered run; do not add side tests outside this stage plan"
    );
}

#[test]
fn order_cancel_lag_script_is_a_narrow_order_only_run() {
    let config = Config::from_args([
        "moonterminal".to_string(),
        "--debug-script".to_string(),
        "order-cancel-lag".to_string(),
    ])
    .expect("order-cancel-lag args must parse")
    .expect("order-cancel-lag must create FireTest config");

    assert_eq!(config.script, Script::OrderCancelLag);
    assert!(
        config.order_cancel_lag,
        "order-cancel-lag must enable the real order-lag stage itself"
    );
    assert_eq!(
        phase_after_settle(config.script),
        Phase::OrderCancelLag,
        "order-cancel-lag must skip mouse/static/tool-window stages"
    );

    let names: Vec<&'static str> = ORDER_CANCEL_LAG_STAGE_PLAN
        .iter()
        .map(|phase| phase.stage_name())
        .collect();
    assert_eq!(
        names,
        vec![
            "start",
            "open_chart",
            "wait_chart_probe",
            "settle_live_chart",
            "order_cancel_lag",
            "cooldown",
            "result",
        ],
        "order-cancel-lag must remain a focused order-path diagnostic"
    );
    assert!(
        !ORDER_CANCEL_LAG_STAGE_PLAN.contains(&Phase::Storm)
            && !ORDER_CANCEL_LAG_STAGE_PLAN.contains(&Phase::StaticTextStorm)
            && !ORDER_CANCEL_LAG_STAGE_PLAN.contains(&Phase::ToolWindowsOpen),
        "order-cancel-lag must not pull in unrelated cursor/text/tool-window stages"
    );
}
