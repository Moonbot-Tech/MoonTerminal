//! Workspace retargeting, detachment, and construction routing regressions for ChartTabs.

// NOT `use super::*`: the parent imports `gpui::*`, whose `test` macro shadows `#[test]`.
use super::{
    AutoWorkspaceChartState, Tab, coin_search_bucket, preferred_auto_workspace_market,
    prune_coin_selection_to_scope, windows::chart_detach_allowed,
};
use moon_core::config::{ChartBucket, WorkspaceMode};
use moon_core::market::MarketLabel;
use moon_core::session::CoreId;
use std::collections::HashSet;

/// Build one catalog-backed market label for selection-policy tests.
fn label(coin: &str, quote: &str) -> MarketLabel {
    MarketLabel {
        coin: coin.to_string(),
        canonic: String::new(),
        quote: quote.to_string(),
        contract: None,
    }
}

/// Removing or reordering any branch in `chart_tabs/mod.rs:preferred_auto_workspace_market` must
/// fail: a rail click would choose another quote or contract even though the exact instrument is
/// available on the selected core.
#[test]
fn auto_market_selection_prefers_exact_then_pair_then_coin() {
    let current = label("AAVE_RP", "USD");
    let candidates = vec![
        ("AAVEUSDT".to_string(), label("AAVE", "USDT")),
        ("AAVEUSD_PERP".to_string(), label("AAVE_RP", "USD")),
        ("AAVEUSD".to_string(), label("AAVE", "USD")),
    ];

    assert_eq!(
        preferred_auto_workspace_market("AAVEUSD", &current, &candidates).as_deref(),
        Some("AAVEUSD")
    );
    assert_eq!(
        preferred_auto_workspace_market("MISSING", &current, &candidates).as_deref(),
        Some("AAVEUSD_PERP")
    );
    assert_eq!(
        preferred_auto_workspace_market("MISSING", &label("AAVE_RP", "BTC"), &candidates)
            .as_deref(),
        Some("AAVEUSDT")
    );
    assert_eq!(
        preferred_auto_workspace_market("MISSING", &label("SOL", "USDT"), &candidates),
        None
    );
}

/// Recording `applied_core` inside `chart_tabs/mod.rs:AutoWorkspaceChartState::candidate` must
/// fail: an initially unavailable target catalog would consume the rail click and never retry.
#[test]
fn auto_chart_target_commits_only_after_success_and_retries_catalog_events() {
    let current = Some((7, "BTCUSDT".to_string()));
    let mut restored = AutoWorkspaceChartState::seeded(Some(41), current.clone());
    assert_eq!(
        restored.candidate(Some(41), current.clone()),
        Some(41),
        "a persisted selection must not suppress retargeting when Main belongs to another core"
    );

    let restored_target = Some((41, "BTCUSDT".to_string()));
    let mut matching = AutoWorkspaceChartState::seeded(Some(41), restored_target.clone());
    assert_eq!(
        matching.candidate(Some(41), restored_target.clone()),
        None,
        "an actually matching restored Main target must not be replayed"
    );
    assert_eq!(
        matching.candidate(Some(41), current.clone()),
        Some(41),
        "moving Main away from the selected Auto core must re-arm retargeting"
    );

    let mut pending = AutoWorkspaceChartState::default();
    assert_eq!(pending.candidate(Some(41), current.clone()), Some(41));
    assert_eq!(
        pending.candidate(Some(41), current.clone()),
        None,
        "one unresolved snapshot must not be searched on every backend notification"
    );
    // Initial list and a full slot replacement can have identical counters. Both events must retry.
    for _ in 0..2 {
        pending.catalog_changed();
        assert_eq!(pending.candidate(Some(41), current.clone()), Some(41));
        assert_eq!(pending.candidate(Some(41), current.clone()), None);
    }
    pending.commit(41);
    pending.catalog_changed();
    assert_eq!(
        pending.candidate(Some(41), Some((41, "BTCUSDT".to_string())),),
        None,
        "focus-only revisions must not replace a successfully applied target"
    );
    assert_eq!(pending.candidate(None, None), None);
    assert_eq!(pending.applied_core, None);
}

/// Removing the catalog notification in the feed drain or the `ChartTabs` observer must
/// fail: a selected core whose catalog becomes ready later would remain stuck behind the previous
/// chart.
///
/// The loop moved to `startup/boot.rs` when the login window split startup in two, so the anchor
/// is matched without its leading indentation — the code is one nesting level shallower there and
/// nothing about this contract depends on how deeply it sits.
#[test]
fn catalog_revisions_wake_pending_auto_chart_retargets_without_price_walks() {
    let main = include_str!("../main.rs");
    let startup = include_str!("../startup/boot.rs");
    let backend = include_str!("../backend/workspace_entities.rs");
    let chart_tabs = include_str!("mod.rs");
    assert!(main.contains("market_catalog_revision: Entity<MarketCatalogRevision>"));
    assert!(startup.contains(
        "if drain.market_catalog {\n                        b.market_catalog_revision.update(cx, |_, cx| cx.notify());"
    ));
    assert!(backend.contains("pub(crate) fn market_catalog_revision("));
    assert!(chart_tabs.contains(
        "cx.observe(&market_catalog_revision, |this, _revision, cx| {\n            // A failed rail retarget must wake"
    ));
    let chart = include_str!("../panels/chart/construct.rs");
    assert!(!chart.contains("cx.observe(&market_data_revision"));
    assert!(!chart_tabs.contains("cx.observe(&market_data_revision"));
    assert!(chart_tabs.contains("this.auto_workspace_chart_state.catalog_changed();"));
}

/// Replacing `main_stack.rs:replace_or_focus`'s indexed insertion with `push` must fail: selecting
/// successive Auto rail cores would accumulate one Main chart per click instead of replacing the
/// current slot.
#[test]
fn auto_retarget_replaces_the_active_main_slot_without_appending() {
    let source = include_str!("main_stack/open.rs");
    let body = source
        .split("pub(in crate::chart_tabs) fn replace_or_focus(")
        .nth(1)
        .and_then(|tail| {
            tail.split("/// Remove panels whose own panes are empty")
                .next()
        })
        .expect("Auto Main replacement method must precede empty-panel pruning");

    assert!(body.contains("self.remove_chart_at(active, cx);"));
    // Matched in two pieces because rustfmt splits the call across lines: what this pins is the
    // INDEXED insertion, not the formatting it happens to have today.
    assert!(body.contains("self.charts.insert("));
    assert!(body.contains("ChartStackEntry::new(core, market, panel,"));
    assert!(!body.contains("self.charts.push("));
}

/// Allowing `WorkspaceMode::AutoTrading` in `chart_tabs/windows.rs:chart_detach_allowed`, or
/// moving its caller below geometry/window work, must fail: Auto would mutate Classic detached
/// chart persistence before refusing the operation.
#[test]
fn auto_refuses_chart_detach_before_window_or_persistence_work() {
    assert!(chart_detach_allowed(WorkspaceMode::Classic));
    assert!(!chart_detach_allowed(WorkspaceMode::AutoTrading));

    let source = include_str!("windows.rs");
    let body = source
        .split("pub(super) fn detach(")
        .nth(1)
        .and_then(|tail| tail.split("/// Open an OS window").next())
        .expect("ChartTabs detach method must precede window creation");
    let guard = body
        .find("chart_detach_allowed")
        .expect("Auto detachment guard must exist");
    for mutation in ["spec_geom", "open_chart_window", "upsert_spec"] {
        assert!(
            guard
                < body
                    .find(mutation)
                    .expect("detach mutation anchor must exist"),
            "Auto guard must run before {mutation}"
        );
    }
}

/// Removing the construction drain from `chart_tabs/mod.rs:ChartTabs::new` would strand a Main
/// request emitted before its group window and observer existed.
#[test]
fn construction_consumes_an_already_pending_request() {
    let source = include_str!("mod.rs");
    let constructor = source
        .split("pub fn new(")
        .nth(1)
        .and_then(|tail| tail.split("fn handle_open_request(").next())
        .expect("ChartTabs constructor must precede its request handler");

    assert!(constructor.contains("this.handle_open_request(false, cx);"));
}

/// Changing the constructor argument in `chart_tabs/mod.rs:ChartTabs::new` from `false` to `true`
/// would let an old activating request steal OS focus during startup; live observer delivery must
/// still preserve the request's activation semantics.
#[test]
fn startup_drain_suppresses_activation_while_live_delivery_preserves_it() {
    let source = include_str!("mod.rs");

    assert!(source.contains("this.handle_open_request(false, cx);"));
    assert!(source.contains("this.handle_open_request(true, cx);"));
    assert!(source.contains("if activate && allow_window_activation"));
}

/// `chart_tabs/mod.rs::coin_search_bucket` scopes the coin search popup to the Auto rail's
/// selected core while the active tab is Main or a custom tab.
///
/// Mutation: replacing `auto_core.map(ChartBucket::Core)` with `None` on the `Main | Custom` arm.
/// The popup would then fall back to the unscoped bucket and offer markets from every core in the
/// group's Auto workspace instead of only the one selected on the rail.
#[test]
fn coin_search_scopes_to_the_selected_auto_core_on_main_and_custom() {
    assert_eq!(
        coin_search_bucket(&Tab::Main, Some(41)),
        Some(ChartBucket::Core(41))
    );
    assert_eq!(coin_search_bucket(&Tab::Main, None), None);
    assert_eq!(
        coin_search_bucket(&Tab::Custom(100_000, ChartBucket::Shared), Some(7)),
        Some(ChartBucket::Core(7))
    );
}

/// `chart_tabs/mod.rs::prune_coin_selection_to_scope` drops accumulated coin checkboxes that fall
/// outside a newly selected Auto rail core.
///
/// Mutation: deleting the `selected.retain(...)` line while keeping the early `true`. A rail move
/// would then leave invisible markets of the previous core in the accumulated selection, inflating
/// the "Open in new tab" footer count with coins the popup no longer shows.
#[test]
fn prune_coin_selection_drops_markets_outside_the_new_scope() {
    let mut selected: HashSet<(CoreId, String)> = HashSet::from([
        (7, "BTCUSDT".to_string()),
        (7, "ETHUSDT".to_string()),
        (9, "SOLUSDT".to_string()),
    ]);

    let pruned = prune_coin_selection_to_scope(&mut selected, Some(7));

    assert!(
        pruned,
        "a selection spanning more than the new scope must report a change"
    );
    assert_eq!(
        selected,
        HashSet::from([(7, "BTCUSDT".to_string()), (7, "ETHUSDT".to_string())])
    );
}

/// Every settings tooltip on both chart toolbars must reuse its popup's title key.
///
/// Breakage: reintroducing a separate `*.tip` key lets the hover wording drift from the window
/// title in one or more locales.
#[test]
fn chart_settings_tooltips_reuse_popup_title_keys() {
    let cases = [
        (
            "chart-layout-settings",
            "detached-layout-settings",
            "chart.layout.title",
        ),
        (
            "chart-candle-settings",
            "detached-candle-settings",
            "chart.candles.title",
        ),
        (
            "chart-graphics-settings",
            "detached-graphics-settings",
            "chart.graphics.title",
        ),
        (
            "chart-history-settings",
            "detached-history-settings",
            "chart.history.title",
        ),
        (
            "chart-labels-settings",
            "detached-labels-settings",
            "chart_labels.title",
        ),
    ];
    for (docked_id, detached_id, title_key) in cases {
        for (source, button_id) in [
            (include_str!("strip.rs"), docked_id),
            (include_str!("detached_host/render.rs"), detached_id),
        ] {
            let button = source
                .split(button_id)
                .nth(1)
                .and_then(|tail| tail.split(".on_click").next())
                .unwrap_or_else(|| panic!("missing chart toolbar button {button_id}"));
            assert!(
                button.contains(&format!("t!(\"{title_key}\")")),
                "{button_id} must use {title_key} for its tooltip"
            );
        }
    }
}

/// Closing the layout popup must commit its name through the same host path as Enter and blur.
///
/// Breakage: removing the close-settlement call, bypassing the shared subscription on either host,
/// or dropping the non-empty guard would respectively lose a typed name or erase a stored one.
#[test]
fn layout_close_commits_custom_name_on_both_hosts() {
    let common = include_str!("common.rs");
    let settle = common
        .split("fn settle_closed_popup(")
        .nth(1)
        .and_then(|tail| tail.split("fn release_layout_field_focus(").next())
        .expect("layout popup close settlement must exist");
    assert!(settle.contains("self.commit_layout_popup(cx);"));
    assert!(settle.contains("self.commit_rename_input(cx);"));

    for host in [include_str!("mod.rs"), include_str!("detached_host/mod.rs")] {
        assert!(host.contains("subscribe_rename_commit(&custom_name_input, cx);"));
    }
    for rename in [
        include_str!("custom.rs"),
        include_str!("detached_host/mod.rs"),
    ] {
        assert!(rename.contains("let name = name.trim().to_string();"));
        assert!(rename.contains("if name.is_empty()"));
    }
}

/// Frozen replay shared by chart tab-selection and new-market frame benches.
fn chart_tab_replay() -> std::rc::Rc<moon_core::market::trade_replay::TradeReplaySeries> {
    use moon_core::feed::{Side, Tick};
    use moon_core::market::side_slots_of_ticks;
    use moon_core::market::trade_replay::venue_caps::TickValue;
    use moon_core::market::trade_replay::{
        Coverage, TickStatus, TickThinning, TradeReplaySeries, TradeReplaySource, replay_window_ms,
    };

    let candles = crate::test_support::fixtures::candles(100_000);
    let ticks: Vec<_> = (0..100_000)
        .map(|index| Tick {
            time_ms: (crate::test_support::fixtures::EPOCH_MS + index * 60_000) as f64,
            price: 10.0 + (index % 100) as f32 / 100.0,
            qty: 1.0,
            side: if index % 2 == 0 {
                Side::Buy
            } else {
                Side::Sell
            },
        })
        .collect();
    let from_ms = candles.first().expect("candles").t_open_ms as i64;
    let to_ms = candles.last().expect("candles").t_open_ms as i64 + 60_000;
    let side_slots = side_slots_of_ticks(&ticks, TickValue::Base);
    std::rc::Rc::new(TradeReplaySeries {
        source: TradeReplaySource::CoreTicks,
        venue: None,
        window: replay_window_ms(from_ms, to_ms, 30_000).expect("replay window"),
        tf_ms: 60_000,
        candles,
        ticks,
        identity: 1,
        tick_status: TickStatus::Served,
        thinning: TickThinning::Raw,
        partial: false,
        side_slots,
        covered: Coverage::one((from_ms, to_ms)),
    })
}

/// Construct a chart for one market over the shared frozen replay.
fn chart_tab_engine(
    market: &str,
    replay: std::rc::Rc<moon_core::market::trade_replay::TradeReplaySeries>,
) -> crate::chartdx::ChartEngine {
    use moon_core::config::ChartTheme;
    use moon_core::market::{MarketDataSource, MarketStore};

    assert_eq!(replay.candles.len(), 100_000);
    assert_eq!(replay.ticks.len(), 100_000);
    let left = replay.window.from_ms as f64;
    let right = replay.window.to_ms as f64;
    let mut engine = crate::chartdx::ChartEngine::new(
        crate::test_support::fixtures::EPOCH_MS as f64,
        ChartTheme::default(),
    );
    engine.open(1, market);
    engine.set_historical(true);
    engine.set_trade_replay(Some(replay));
    engine.set_follow(false, right);
    engine.show_time_range(left, right, 0.0);
    engine.set_scene_visible(true);
    engine.set_market_source(Some(MarketDataSource::new(MarketStore::shared(
        crate::test_support::fixtures::EPOCH_MS as f64,
    ))));
    engine
}

/// Prepare the chart frame immediately following a chart-tab action.
fn chart_tab_frame(engine: &mut crate::chartdx::ChartEngine) {
    use std::time::Instant;

    use gpui::{Bounds, GpuFrameInfo, point, px, size};

    engine.set_scene_visible(false);
    engine.set_scene_visible(true);
    engine.with_container_mut(|_| {});
    let decision = engine.prepare_frame_for_tests(GpuFrameInfo {
        now: Instant::now(),
        bounds: Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(1_000.0), px(600.0)),
        },
        scale_factor: 1.0,
        content_zoom: 1.0,
        presentable: true,
    });
    std::hint::black_box(decision);
}

/// Time alternating selection of two existing populated charts and their following frames.
#[test]
#[ignore = "bench: S-smooth"]
fn bench_s_chart_tab_select() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            crate::test_locale::warm();
            let _window = crate::test_support::SharedWindowGuard::take();
            let replay = chart_tab_replay();
            let mut engines = [
                chart_tab_engine("XALPHAUSDT", replay.clone()),
                chart_tab_engine("XBETAUSDT", replay),
            ];
            chart_tab_frame(&mut engines[0]);
            chart_tab_frame(&mut engines[1]);
            let mut step = 0;
            crate::test_support::bench::measure(
                "bench_s_chart_tab_select stimulus=existing_tab_plus_cpu_frame trades=100000 candles=100000 partial_(no_Backend_wiring)",
                5,
                || {
                    step += 1;
                    let tab = if step % 2 == 0 {
                        Tab::Main
                    } else {
                        Tab::Custom(super::CUSTOM_NUM_BASE, ChartBucket::Shared)
                    };
                    assert_eq!(coin_search_bucket(&tab, Some(1)), Some(ChartBucket::Core(1)));
                    chart_tab_frame(&mut engines[step % 2]);
                },
            );
        })
        .expect("bench thread")
        .join()
        .expect("bench result");
}

/// Time a new-market chart replacement and the following frozen-history frame.
#[test]
#[ignore = "bench: S-smooth"]
fn bench_s_chart_new_market_switch() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            crate::test_locale::warm();
            let _window = crate::test_support::SharedWindowGuard::take();
            let replay = chart_tab_replay();
            let mut step = 0;
            crate::test_support::bench::measure(
                "bench_s_chart_new_market_switch stimulus=new_market_plus_history_cpu_frame trades=100000 candles=100000 partial_(no_Backend_wiring)",
                3,
                || {
                    step += 1;
                    let market = if step % 2 == 0 { "XALPHAUSDT" } else { "XBETAUSDT" };
                    let mut engine = chart_tab_engine(market, replay.clone());
                    chart_tab_frame(&mut engine);
                    assert!(engine.uses_market(1, market));
                    engine
                },
            );
        })
        .expect("bench thread")
        .join()
        .expect("bench result");
}
