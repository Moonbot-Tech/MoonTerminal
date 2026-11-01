//! Regression coverage for tuner mode persistence and workspace-constrained retained selection.

use super::{STRAT_MODES, StratMode, strategy_selection_visible};
use moon_core::config::layout::StratColsByMode;

/// Leaving an axis cancels its ordinary work, without claiming searches or another axis' reads.
#[test]
fn axis_navigation_retires_only_hidden_ordinary_reads_and_preserves_drafts() {
    use super::super::bg::{LatestReads, ReadLane};
    use super::coins::state::PickState;
    use super::filter::state::{SuggestJob, SuggestState};
    use super::{CoinListsState, CoinsState, TicksState, TimeTunerState, TunerState};
    use moon_core::db::tuner::threshold_search::SearchHandle;

    let ordinary = [
        ReadLane::FilterKpi,
        ReadLane::FilterHistogram,
        ReadLane::Time,
        ReadLane::Coins,
        ReadLane::CoinKpi,
        ReadLane::CoinPicked,
        ReadLane::Ticks,
        ReadLane::TicksReplay,
        ReadLane::TicksVariants,
    ];
    let protected = [
        ReadLane::Summary,
        ReadLane::StrategyBase,
        ReadLane::Calendar,
        ReadLane::CoinPick,
        ReadLane::CoinPickAll,
        ReadLane::TicksSearch,
        ReadLane::TicksReady,
    ];
    for mode in STRAT_MODES {
        let mut reads = LatestReads::default();
        let tokens: Vec<_> = ordinary
            .iter()
            .chain(&protected)
            .map(|lane| (*lane, reads.replace(&[*lane])))
            .collect();
        let mut tuner = TunerState::load(None, None, None, None, None, false);
        let mut time = TimeTunerState::load();
        let mut coins = CoinsState::default();
        let mut lists = CoinListsState::default();
        let mut ticks = TicksState::default();
        tuner.dirty = false;
        tuner.hist_dirty = false;
        time.dirty = false;
        coins.dirty = false;
        ticks.dirty = false;
        tuner.bounds[0][0] = ("1".into(), "2".into());
        tuner.staged_ignore.insert("IgnoreFilters", true);
        time.bounds[0][0] = ("1".into(), "2".into());
        time.ignore_staged = Some(true);
        time.sugg_busy = true;
        coins.work.black.insert("XALPHA".into());
        coins.saved.white.insert("XBETA".into());
        coins.picked.insert("XGAMMA".into());
        coins.pick = PickState::Running { seq: 7 };
        coins.pick_seq = 7;
        ticks.variant.insert("TakeProfit".into(), "1.25".into());
        let filter_search = SearchHandle::new();
        tuner.sugg = SuggestState::Running(SuggestJob::Compose {
            handle: filter_search.clone(),
            axis_moved: false,
        });
        let tick_search = SearchHandle::new();
        ticks.start_search(
            tick_search.clone(),
            8,
            1,
            std::time::Instant::now(),
            None,
            (10, 2),
        );
        let search_generations = (tuner.sugg_seq, time.sugg_seq, ticks.sugg_seq);
        let dialog_generation = tuner.dialog_seq;

        super::retire_hidden_axis_reads(
            mode, &mut reads, &mut tuner, &mut time, &mut coins, &mut lists, &mut ticks,
        );

        for (lane, token) in &tokens {
            let expected = match mode {
                StratMode::Filters => {
                    matches!(lane, ReadLane::FilterKpi | ReadLane::FilterHistogram)
                }
                StratMode::Time => *lane == ReadLane::Time,
                StratMode::Coins => {
                    matches!(
                        lane,
                        ReadLane::Coins | ReadLane::CoinKpi | ReadLane::CoinPicked
                    )
                }
                StratMode::Ticks => matches!(
                    lane,
                    ReadLane::Ticks | ReadLane::TicksReplay | ReadLane::TicksVariants
                ),
            };
            assert_eq!(token.is_cancelled(), expected);
            assert_eq!(reads.contains(*lane), !expected);
        }
        assert_eq!(tuner.dirty, mode == StratMode::Filters);
        assert_eq!(time.dirty, mode == StratMode::Time);
        assert_eq!(coins.dirty, mode == StratMode::Coins);
        assert_eq!(ticks.dirty, mode == StratMode::Ticks);
        assert_eq!(tuner.seq != 0, mode == StratMode::Filters);
        assert_eq!(tuner.hist_seq != 0, mode == StratMode::Filters);
        assert_eq!(time.seq != 0, mode == StratMode::Time);
        assert_eq!(coins.seq != 0, mode == StratMode::Coins);
        assert_eq!(coins.kpi_seq != 0, mode == StratMode::Coins);
        assert_eq!(coins.picked_seq != 0, mode == StratMode::Coins);
        assert_eq!(lists.seq != 0, mode == StratMode::Coins);
        assert_eq!(ticks.seq != 0, mode == StratMode::Ticks);
        assert_eq!(ticks.tape_seq != 0, mode == StratMode::Ticks);
        assert_eq!(ticks.var_seq != 0, mode == StratMode::Ticks);
        assert_eq!(tuner.bounds[0][0], ("1".into(), "2".into()));
        assert_eq!(tuner.staged_ignore.get("IgnoreFilters"), Some(&true));
        assert_eq!(time.bounds[0][0], ("1".into(), "2".into()));
        assert_eq!(time.ignore_staged, Some(true));
        assert!(coins.work.black.contains("XALPHA"));
        assert!(coins.saved.white.contains("XBETA"));
        assert!(coins.picked.contains("XGAMMA"));
        assert_eq!(coins.pick, PickState::Running { seq: 7 });
        assert_eq!(coins.pick_seq, 7);
        assert_eq!(
            ticks.variant.get("TakeProfit").map(String::as_str),
            Some("1.25")
        );
        assert!(tuner.sugg.is_running());
        assert!(time.sugg_busy);
        assert!(matches!(
            &ticks.sugg,
            super::ticks::state::SuggState::Running {
                total: 8,
                parallel: 1,
                sample: (10, 2),
                stopping: false,
                ..
            }
        ));
        assert!(!filter_search.is_cancelled());
        assert!(!tick_search.is_cancelled());
        assert_eq!(
            search_generations,
            (tuner.sugg_seq, time.sugg_seq, ticks.sugg_seq)
        );
        assert_eq!(tuner.dialog_seq, dialog_generation);
    }
}

/// A compound read retires both guarded projections; returning before its acknowledgement stays owed.
#[test]
fn axis_navigation_retires_compound_generations_and_keeps_a_rapid_return_dirty() {
    use super::super::bg::{LatestReads, ReadLane};
    use super::{CoinListsState, CoinsState, TicksState, TimeTunerState, TunerState};

    let mut reads = LatestReads::default();
    let compound = reads.replace(&[ReadLane::FilterKpi, ReadLane::FilterHistogram]);
    let mut tuner = TunerState::load(None, None, None, None, None, false);
    let mut time = TimeTunerState::load();
    let mut coins = CoinsState::default();
    let mut lists = CoinListsState::default();
    let mut ticks = TicksState::default();
    tuner.dirty = false;
    tuner.hist_dirty = false;
    tuner.hist_loading = true;
    let old = (tuner.seq, tuner.hist_seq);
    super::retire_hidden_axis_reads(
        StratMode::Filters,
        &mut reads,
        &mut tuner,
        &mut time,
        &mut coins,
        &mut lists,
        &mut ticks,
    );
    assert!(compound.is_cancelled());
    assert_ne!(old.0, tuner.seq);
    assert_ne!(old.1, tuner.hist_seq);
    assert!(!tuner.hist_loading);
    assert!(tuner.needs_reload());
    super::retire_hidden_axis_reads(
        StratMode::Time,
        &mut reads,
        &mut tuner,
        &mut time,
        &mut coins,
        &mut lists,
        &mut ticks,
    );
    reads.finish(&compound);
    assert!(tuner.needs_reload(), "A-B-A must reread cancelled A");
    let source = include_str!("mod.rs");
    let setter = source
        .split_once("fn set_strat_mode(")
        .unwrap()
        .1
        .split_once("\n    }")
        .unwrap()
        .0;
    let retire = setter
        .find("self.retire_hidden_tuner_reads();")
        .expect("mode changes retire the same ordinary reads as root-tab navigation");
    let select = setter.find("self.strat_mode = mode;").unwrap();
    assert!(retire < select);
    let wrapper = source
        .split_once("fn retire_hidden_tuner_reads(")
        .unwrap()
        .1
        .split_once("\n    }")
        .unwrap()
        .0;
    assert!(wrapper.contains("retire_hidden_axis_reads("));
    assert!(wrapper.contains("self.strat_mode,"));
    assert_eq!(source.matches("&mut self.latest_reads,").count(), 1);
    assert!(setter.contains("self.request_axis_if_stale(mode, cx);"));
    assert!(!setter.contains("self.reload_axis("));
}

/// A pending edit rescore stays owed on re-entry; a retained spent handle costs no new read.
#[test]
fn axis_navigation_distinguishes_pending_and_spent_debounce_tasks() {
    use super::super::bg::LatestReads;
    use super::{CoinListsState, CoinsState, TicksState, TimeTunerState, TunerState};

    let executor = gpui::BackgroundExecutor::new(std::sync::Arc::new(gpui::TestDispatcher::new(0)));
    for mode in [StratMode::Coins, StratMode::Ticks] {
        for pending in [false, true] {
            let mut reads = LatestReads::default();
            let mut tuner = TunerState::load(None, None, None, None, None, false);
            let mut time = TimeTunerState::load();
            let mut coins = CoinsState::default();
            let mut lists = CoinListsState::default();
            let mut ticks = TicksState::default();
            coins.dirty = false;
            ticks.dirty = false;
            let task = if pending {
                executor.spawn(std::future::pending::<()>())
            } else {
                gpui::Task::ready(())
            };
            match mode {
                StratMode::Coins => coins.kpi_task = Some(task),
                StratMode::Ticks => ticks.var_task = Some(task),
                _ => unreachable!(),
            }
            super::retire_hidden_axis_reads(
                mode, &mut reads, &mut tuner, &mut time, &mut coins, &mut lists, &mut ticks,
            );
            assert!(coins.kpi_task.is_none());
            assert!(ticks.var_task.is_none());
            assert_eq!(coins.dirty, pending && mode == StratMode::Coins);
            assert_eq!(ticks.dirty, pending && mode == StratMode::Ticks);
            assert_eq!(coins.kpi_seq != 0, pending && mode == StratMode::Coins);
            assert_eq!(ticks.var_seq != 0, pending && mode == StratMode::Ticks);
        }
    }
}

/// A hidden search completion must rescore its Strategies period after a Summary period edit.
///
/// An AnalyticsView requires a live Backend/window, so this pins the production query boundary
/// and its callback routes. Restoring the active-tab query would make a completed hidden search
/// publish Summary-period KPIs as fresh under the retained Strategies period and enable Save.
#[test]
fn hidden_tuner_continuations_keep_the_strategies_period_and_read_authority() {
    let tuner = include_str!("mod.rs");
    let filter = include_str!("filter/mod.rs");
    let ticks = include_str!("ticks/mod.rs");
    let method = |source: &'static str, name: &str| {
        source
            .split_once(&format!("fn {name}("))
            .unwrap_or_else(|| panic!("missing method {name}"))
            .1
            .split("\n    }")
            .next()
            .expect("method body")
    };
    assert!(method(tuner, "tuner_query_all").contains("self.query_for(self.strat_period)"));
    for (source, name) in [
        (filter, "tuner_query"),
        (tuner, "open_strategy_report"),
        (ticks, "deal_target"),
    ] {
        assert!(
            method(source, name).contains("self.tuner_query_all()"),
            "{name} must use the Strategies-owned query even from a delayed callback"
        );
    }
    for source in [tuner, filter, ticks] {
        assert!(
            !source.contains("self.query()"),
            "tuner consumers must not inherit the visible page's independent period"
        );
    }
    assert!(
        method(filter, "tuner_query")
            .contains("q.strategies = self.visible_target_keys(self.read_core_ids());"),
        "period ownership must retain the full read-visible strategy set"
    );
    assert!(
        method(tuner, "selected_targets")
            .contains("self.targets_visible_in(self.action_core_ids())"),
        "query ownership must not broaden write authority"
    );
    assert!(
        method(include_str!("filter/actions.rs"), "suggest_into_v1")
            .contains("this.reload_tuner(cx);"),
        "a retained search must rescore through the period-owned query"
    );
    assert!(
        method(include_str!("coins/load.rs"), "run_coin_kpi").contains("self.tuner_query()"),
        "a delayed coin KPI must use the same period-owned query"
    );
    assert!(
        method(include_str!("time/grid.rs"), "time_suggest")
            .contains("this.rescore_time_draft(cx);"),
        "a time suggestion must rescore through the period-owned query"
    );
    assert!(
        method(include_str!("time/mod.rs"), "rescore_time_draft")
            .contains("let query = self.tuner_query();"),
        "the reduced time rescore must retain the exact Strategies-owned read query"
    );
}

/// A retained figure may render during a refresh but cannot seed a new strategy write.
#[test]
fn pending_tuner_read_keeps_figures_without_enabling_writes() {
    use super::fresh_tuner_data;
    use crate::load_state::LoadState;

    let mut result = LoadState::default();
    result.apply(Ok(vec![41, 42]));
    assert!(fresh_tuner_data(&result, false));
    assert!(!fresh_tuner_data(&result, true));

    result.begin();
    assert_eq!(result.data().unwrap().as_slice(), &[41, 42]);
    assert!(!fresh_tuner_data(&result, false));

    result.apply(Err(moon_core::db::ReadFail::NotReady));
    assert!(!fresh_tuner_data(&result, false));
    result.apply(Ok(Vec::new()));
    assert!(fresh_tuner_data(&result, false));
}

/// Axis reads use local progress, while stale-render Save/Copy clicks recheck fresh inputs.
#[test]
fn nonblocking_tuner_reads_keep_write_entry_points_guarded() {
    for (source, count) in [
        (include_str!("filter/mod.rs"), 3),
        (include_str!("time/mod.rs"), 1),
        (include_str!("coins/load.rs"), 1),
        (include_str!("ticks/load.rs"), 1),
    ] {
        assert_eq!(
            source
                .matches("let show_overlay = self.axis_read_overlay(show_overlay);")
                .count(),
            count,
        );
    }
    for (source, methods, kind) in [
        (
            include_str!("filter/actions.rs"),
            &["open_save_dialog", "open_copy_dialog"][..],
            "Filter",
        ),
        (
            include_str!("time/save.rs"),
            &["time_open_save_dialog", "time_open_copy_dialog"][..],
            "Time",
        ),
        (
            include_str!("coins/save.rs"),
            &["coins_open_save_dialog", "coins_open_copy_dialog"][..],
            "Coins",
        ),
        (
            include_str!("ticks/variants.rs"),
            &["ticks_open_copy_dialog"][..],
            "Ticks",
        ),
        (
            include_str!("ticks/summary.rs"),
            &["ticks_begin_write"][..],
            "Ticks",
        ),
    ] {
        for method in methods {
            let entry = source
                .split(&format!("fn {method}("))
                .nth(1)
                .and_then(|tail| tail.split("\n    }").next())
                .expect("write entry point must exist");
            assert!(
                entry.contains(&format!(
                    "if !self.axis_write_ready(super::super::shared::TunerKind::{kind})"
                )),
                "{method} must guard a click delivered from an older frame"
            );
        }
    }
    assert!(include_str!("shell.rs").contains("&& self.axis_write_ready(kind)"));
    assert!(
        include_str!("ticks/summary.rs")
            .contains("&& self.axis_write_ready(super::super::shared::TunerKind::Ticks)")
    );
    // The matrix's reads show local progress in its title caption, never a blocking overlay.
    assert!(
        include_str!("kpi.rs").contains("let caption = refreshing_caption(caption, refreshing);")
    );
}

/// Each axis must address its OWN slot. Two axes sharing one is the copy-paste that makes
/// the whole per-axis layout pointless — and it would look like "my columns keep changing
/// when I switch tabs", which is exactly what this feature exists to stop.
#[test]
fn each_axis_owns_its_column_slot() {
    let mut cols = StratColsByMode::default();
    for (i, mode) in STRAT_MODES.into_iter().enumerate() {
        *mode.cols_slot(&mut cols) = i as u16 + 1;
    }
    assert_eq!((cols.filter, cols.coins, cols.time), (1, 2, 3));
    // And reading back returns what that axis wrote, not a neighbour's.
    for (i, mode) in STRAT_MODES.into_iter().enumerate() {
        assert_eq!(*mode.cols_slot(&mut cols), i as u16 + 1);
    }
}

/// Only the coin axis spends width on the coin-list columns — that difference is the
/// reason the mask is per axis at all.
#[test]
fn coin_axis_defaults_to_showing_the_lists() {
    assert_ne!(
        StratMode::Coins.default_cols(),
        StratMode::Filters.default_cols()
    );
    assert_eq!(
        StratMode::Filters.default_cols(),
        StratMode::Time.default_cols()
    );
}

/// The Auto column is drawn on "By coin" only, and hiding it elsewhere never edits the saved
/// mask: the other bits pass through untouched and the By-coin mask keeps the Auto bit, so the
/// column (and its saved width slot) comes back on return.
#[test]
fn the_auto_column_is_drawn_on_by_coin_only() {
    use super::{COL_BIT_AUTO, COL_BIT_READY, STRAT_COLS_ALL};
    for mode in STRAT_MODES {
        let drawn = mode.drawn_cols(STRAT_COLS_ALL);
        assert_eq!(drawn & COL_BIT_AUTO != 0, mode == StratMode::Coins);
        assert_eq!(drawn & COL_BIT_READY != 0, mode == StratMode::Ticks);
        assert_eq!(drawn | COL_BIT_AUTO | COL_BIT_READY, STRAT_COLS_ALL);
        assert_eq!(mode.shows_auto(), mode == StratMode::Coins);
        assert_eq!(mode.shows_ready(), mode == StratMode::Ticks);
    }
    assert_eq!(StratMode::Coins.drawn_cols(0), 0);
}

/// Routing `tuner/mod.rs:selected_targets` through the read authority would keep a stale owner
/// strategy writable after the singleton Auto workspace moves to another core. The By-coin
/// renderer wiring is pinned independently of that selector: the binary-only crate's cards
/// must agree on placeholders when the retained anchor is hidden by the read scope.
#[test]
fn stale_owner_strategy_selection_is_hidden_without_erasing_classic_state() {
    let retained = "41@11";
    assert!(!strategy_selection_visible(retained, Some(&[22])));
    assert!(strategy_selection_visible(retained, Some(&[11])));
    assert!(strategy_selection_visible(retained, None));

    let tuner = include_str!("mod.rs");
    let visible = tuner
        .split("fn visible_targets<'a>(")
        .nth(1)
        .and_then(|tail| tail.split("\n    }").next())
        .expect("visible_targets must exist");
    assert!(visible.contains("strategy_selection_visible(key, workspace)"));

    // `selected_targets` feeds Save/Copy/purge, so it must filter through the WRITE authority
    // (`action_core_ids`), not the read filter — the two differ on Auto+Overview, where the core
    // dropdown is unpinned but write access still stays confined to the focused group.
    let selected = tuner
        .split("fn selected_targets(&self)")
        .nth(1)
        .and_then(|tail| tail.split("\n    }").next())
        .expect("selected_targets must exist");
    assert!(selected.contains("self.action_core_ids()"));

    for (source, renderer) in [
        (include_str!("coins/mod.rs"), "fn coins_card("),
        (include_str!("coins/mod.rs"), "fn coins_kpi("),
        (include_str!("coins/picker.rs"), "fn coins_lists_card("),
        (include_str!("coins/auto.rs"), "fn coins_auto_card("),
    ] {
        let render = source
            .split(renderer)
            .nth(1)
            .and_then(|tail| tail.split("\n    }").next())
            .expect("coin renderer must exist");
        assert!(
            render.contains("self.visible_target_count(self.read_core_ids()) > 0"),
            "{renderer} must share the side gate's read-visible selection"
        );
    }

    let save = include_str!("save.rs");
    assert!(save.contains("if !self.save_target_in_workspace(target)"));
    assert!(save.contains("if !this.save_authority_is_current(&authority, &targets, cx)"));
    assert!(save.contains("resolve_complete_target_cores(&targets, &live)"));
}

/// Tuner report and strategy navigation must derive the current singleton owner at dispatch.
///
/// Mutation: omit the owner lookup or its argument to `open_goto`. A row retained after a rail
/// switch could reveal a strategy or report from the previously selected core.
#[test]
fn tuner_navigation_revalidates_singleton_workspace_authority() {
    let source = include_str!("mod.rs");

    assert!(source.matches(".singleton_workspace()").count() >= 2);
    assert!(source.contains(".workspace_action_allows_core(workspace_group.as_deref(), core_uid)"));
    assert!(source.contains("strategy_id,\n            workspace_group,"));
}
