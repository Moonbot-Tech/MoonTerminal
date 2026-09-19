use super::remembered_geometry;
use moon_core::config::layout::GeomRect;

/// New tick groups must replace a streaming snapshot without reframing or reverting to candles.
#[test]
fn progressive_trade_ticks_preserve_view_and_accept_core_completion() {
    use super::{TradeWindowState, fold_outcome};
    use moon_core::market::trade_replay::{
        TickStatus, TradeReplayOutcome, TradeReplaySeries, TradeReplaySource, replay_window_ms,
    };
    let state = TradeWindowState::Ready {
        source: TradeReplaySource::Ticks,
        tf_min: 1,
        tick_status: TickStatus::Streaming,
        bucket_ms: 0,
        partial: true,
        brand: moon_core::venue::Brand::Binance,
    };
    let mut series = TradeReplaySeries {
        source: TradeReplaySource::Ticks,
        venue: moon_core::venue::venue(3).expect("spot venue"),
        window: replay_window_ms(100_000_000, 100_060_000).expect("valid trade"),
        tf_ms: 60_000,
        candles: Vec::new(),
        ticks: vec![moon_core::feed::Tick {
            time_ms: 100_000_000.0,
            price: 10.0,
            qty: 1.0,
            side: moon_core::feed::Side::Buy,
        }],
        identity: 42,
        tick_status: TickStatus::Streaming,
        bucket_ms: 0,
        partial: true,
        side_slots: Vec::new(),
        covered: Some((99_700_000, 100_000_000)),
    };
    let next = fold_outcome(&state, true, &TradeReplayOutcome::Ready(series.clone()));
    assert!(next.accept);
    assert!(
        !next.frame,
        "a streamed group must preserve the user's pan and zoom"
    );
    series.source = TradeReplaySource::CoreTicks;
    series.tick_status = TickStatus::Served;
    let final_core = fold_outcome(&state, true, &TradeReplayOutcome::Ready(series.clone()));
    assert!(final_core.accept && final_core.restore_candle_mode);
    let awaiting = TradeWindowState::Ready {
        source: TradeReplaySource::Klines1m,
        tf_min: 1,
        tick_status: TickStatus::AwaitingCore,
        bucket_ms: 0,
        partial: false,
        brand: moon_core::venue::Brand::Bybit,
    };
    let delayed_core = fold_outcome(&awaiting, true, &TradeReplayOutcome::Ready(series.clone()));
    assert!(delayed_core.accept && delayed_core.restore_candle_mode);
    assert!(
        !delayed_core.frame,
        "a delayed archive must preserve the candle view's pan and zoom"
    );
    series.source = TradeReplaySource::Klines1m;
    series.ticks.clear();
    series.candles.push(moon_core::market::ChartCandle {
        t_open_ms: 100_000_000.0,
        open: 10.0,
        high: 11.0,
        low: 9.0,
        close: 10.0,
        volume: 1.0,
        quote_volume: 10.0,
    });
    series.tick_status = TickStatus::NoRoute;
    assert!(fold_outcome(&awaiting, true, &TradeReplayOutcome::Ready(series.clone())).accept);
    assert!(!fold_outcome(&state, true, &TradeReplayOutcome::Ready(series)).accept);
}

fn rect(x: i32, y: i32, w: u32, h: u32, display_uuid: Option<uuid::Uuid>) -> GeomRect {
    GeomRect {
        x,
        y,
        w,
        h,
        maximized: false,
        fullscreen: false,
        display_uuid,
    }
}

/// `trade_window::remembered_geometry` must undo a cascade rather than persist it. Deleting the
/// nonzero-cascade branch makes each reopened trade window remember its offset and walk off-screen
/// over time, while losing the previous display identity can restore it on the wrong monitor.
#[test]
fn remembered_trade_window_geometry_never_persists_a_cascade_offset() {
    let identity = uuid::Uuid::from_u128(0xfeed_cafe_dead_beef_0123_4567_89ab_cdef);
    let observed = rect(134, 234, 900, 600, Some(identity));

    let uncascaded = remembered_geometry(None, observed, 0.0);
    assert_eq!(
        (
            uncascaded.x,
            uncascaded.y,
            uncascaded.w,
            uncascaded.h,
            uncascaded.display_uuid
        ),
        (
            observed.x,
            observed.y,
            observed.w,
            observed.h,
            observed.display_uuid
        ),
        "an uncascaded observation must be remembered whole"
    );
    let first_cascade = remembered_geometry(None, observed, 34.0);
    assert_eq!(
        (
            first_cascade.x,
            first_cascade.y,
            first_cascade.w,
            first_cascade.h,
            first_cascade.display_uuid
        ),
        (100, 200, 900, 600, Some(identity)),
        "the first cascaded window must subtract its opening offset before saving"
    );

    let previous = rect(100, 200, 640, 480, Some(identity));
    let subsequent_cascade = remembered_geometry(Some(previous), observed, 34.0);
    assert_eq!(
        (
            subsequent_cascade.x,
            subsequent_cascade.y,
            subsequent_cascade.w,
            subsequent_cascade.h,
            subsequent_cascade.display_uuid
        ),
        (100, 200, 900, 600, Some(identity)),
        "a cascaded window keeps the remembered origin and display while retaining its new size"
    );

    let mut saved = previous;
    for _ in 0..5 {
        let observed = rect(saved.x + 34, saved.y + 34, saved.w, saved.h, Some(identity));
        saved = remembered_geometry(Some(saved), observed, 34.0);
        assert_eq!(
            (saved.x, saved.y, saved.display_uuid),
            (100, 200, Some(identity)),
            "reopening a cascaded window repeatedly must not drift its remembered origin"
        );
    }
}

/// Publishing only the focus when enabled, or the full history when disabled, hides/shows the
/// wrong arrows. Restoring the retained snapshot must also preserve neighbours outside replay.
#[test]
fn neighbour_toggle_selects_all_or_focus_without_losing_the_snapshot() {
    use moon_core::db::ChartTradeRecord;
    use std::rc::Rc;
    let focus = ChartTradeRecord {
        record_id: 1,
        core_uid: 7,
        coin: "BTC".to_owned(),
        buy_date: 100,
        close_date: 200,
        buy_ms: None,
        close_ms: None,
        sell_set_date: 0,
        sell_set_ms: None,
        buy_price: 10.0,
        sell_price: 12.0,
        quantity: 2.0,
        is_short: false,
        emulator: false,
        profit: None,
        profit_pct: None,
        report_uid: None,
        quote: None,
    };
    let neighbour = ChartTradeRecord {
        record_id: 2,
        buy_date: 100_000,
        close_date: 100_200,
        emulator: true,
        ..focus.clone()
    };
    let history = Rc::new(vec![neighbour, focus.clone()]);
    let shown = super::visible_history(&history, &focus, true);
    assert_eq!(
        shown.iter().map(|r| r.record_id).collect::<Vec<_>>(),
        vec![2, 1]
    );
    let hidden = super::visible_history(&history, &focus, false);
    assert_eq!(
        hidden.iter().map(|r| r.record_id).collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(hidden[0], focus);
    let restored = super::visible_history(&history, &focus, true);
    assert!(Rc::ptr_eq(&restored, &history));
    assert!(
        restored[0].emulator,
        "the chart's existing kind filter owns emulator visibility"
    );
}

/// The strategy block's placement rule, one arm per fact combination the window can hold.
mod strategy_presence {
    use super::super::strategy::{Presence, StrategyLookup, presence};
    use moon_core::strat_db::stats::{HeadRow, HeadStatus, VersionAt};

    fn lookup(head: Option<bool>) -> StrategyLookup {
        StrategyLookup {
            head: head.map(|deleted| HeadStatus {
                head: HeadRow {
                    core_uid: 7,
                    strategy_id: 42,
                    name: "Hook".into(),
                    kind: "Hook".into(),
                    kind_ordinal: 0,
                    folder_path: String::new(),
                    is_short: false,
                },
                deleted,
            }),
            version: VersionAt::NoHistory,
        }
    }

    #[test]
    fn a_live_name_wins_before_anything_else() {
        assert_eq!(presence(true, true, None), Presence::Live);
        assert_eq!(
            presence(true, false, Some(&lookup(Some(true)))),
            Presence::Live
        );
    }

    #[test]
    fn no_lookup_yet_is_pending_not_gone() {
        assert_eq!(presence(false, true, None), Presence::Pending);
        assert_eq!(presence(false, false, None), Presence::Pending);
    }

    #[test]
    fn a_deleted_head_is_deleted_whatever_the_core_says() {
        assert_eq!(
            presence(false, true, Some(&lookup(Some(true)))),
            Presence::Deleted
        );
        assert_eq!(
            presence(false, false, Some(&lookup(Some(true)))),
            Presence::Deleted
        );
    }

    #[test]
    fn a_saved_live_head_without_its_core_is_offline() {
        assert_eq!(
            presence(false, false, Some(&lookup(Some(false)))),
            Presence::Offline
        );
        assert_eq!(
            presence(false, false, Some(&lookup(None))),
            Presence::Offline
        );
    }

    #[test]
    fn nothing_saved_on_a_connected_core_is_gone() {
        assert_eq!(presence(false, true, Some(&lookup(None))), Presence::Gone);
    }

    /// A connected core whose list does not hold a head still saved as live: transitional, and
    /// the reveal must stay available so the Strategies window can settle it.
    #[test]
    fn a_saved_live_head_missing_from_a_connected_core_stays_pending() {
        assert_eq!(
            presence(false, true, Some(&lookup(Some(false)))),
            Presence::Pending
        );
    }

    #[test]
    fn only_a_locatable_strategy_can_be_revealed() {
        assert!(Presence::Live.can_reveal());
        assert!(Presence::Deleted.can_reveal());
        assert!(Presence::Pending.can_reveal());
        assert!(!Presence::Offline.can_reveal());
        assert!(!Presence::Gone.can_reveal());
    }
}

/// The version line prints the pane's own stamp and opens only a PAST version.
mod strategy_version_line {
    use super::super::strategy::version_line;
    use moon_core::strat_db::stats::VersionAt;

    const NOW: i64 = 1_800_000_000_000;

    #[test]
    fn a_past_version_prints_the_pane_stamp_and_opens_itself() {
        let _locale = crate::test_locale::force("en");
        let vf = NOW - 3 * 86_400_000;
        let (text, _, open) = version_line(
            VersionAt::Known {
                valid_from: vf,
                current: false,
            },
            chrono_tz::UTC,
            NOW,
        )
        .expect("a known version has a line");
        let stamp =
            moon_core::util::display_time::format_chart_clock(vf, chrono_tz::UTC, false, NOW);
        assert_eq!(text, format!("version {stamp}"));
        assert_eq!(open, Some(vf));
    }

    #[test]
    fn the_current_version_says_so_and_opens_live_mode() {
        let _locale = crate::test_locale::force("en");
        let (text, _, open) = version_line(
            VersionAt::Known {
                valid_from: NOW - 1,
                current: true,
            },
            chrono_tz::UTC,
            NOW,
        )
        .expect("a current version has a line");
        assert_eq!(text, "version: in effect");
        assert_eq!(open, None);
    }

    #[test]
    fn before_history_is_stated_and_opens_nothing() {
        let _locale = crate::test_locale::force("en");
        let (text, _, open) =
            version_line(VersionAt::BeforeHistory, chrono_tz::UTC, NOW).expect("stated");
        assert_eq!(text, "version unknown");
        assert_eq!(open, None);
    }

    #[test]
    fn no_history_has_no_line() {
        assert!(version_line(VersionAt::NoHistory, chrono_tz::UTC, NOW).is_none());
    }
}
