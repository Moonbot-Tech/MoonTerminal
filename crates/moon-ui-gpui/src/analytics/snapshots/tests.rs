//! Shared payload restoration and safety-critical snapshot publication wiring.

use std::sync::Arc;

use moon_core::db::analytics::Query;
use moon_core::db::{ProfitUnit, QuoteBreakdown, QuoteCurrency};

use super::retained_snapshot_needs_refresh;
use crate::analytics::ProfitLoadState;
use crate::analytics::cache::{CacheKey, CacheRevisions, CachedScope};
use crate::load_state::Note;

/// Omitting Clone makes a deep-copy requirement fail to compile rather than escape the assertion.
struct NonClonePayload {
    rows: Vec<u64>,
}

/// Limit source contracts to a documented method, excluding unrelated implementations and comments.
fn method<'a>(source: &'a str, signature: &str) -> &'a str {
    source
        .split_once(signature)
        .unwrap_or_else(|| panic!("missing method {signature}"))
        .1
        .split("\n    ///")
        .next()
        .expect("method body")
}

/// Read one explicit guard's balanced body so contracts cannot accept unguarded invalidations.
fn guarded_block<'a>(source: &'a str, opening: &str) -> (&'a str, &'a str) {
    let body = source
        .split_once(opening)
        .unwrap_or_else(|| panic!("missing guard {opening}"))
        .1;
    let mut depth = 1;
    for (index, character) in body.char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return (&body[..index], &body[index + 1..]);
                }
            }
            _ => {}
        }
    }
    panic!("unclosed guard {opening}");
}

/// Repeated hits preserve render preparation only for the same allocation under the same unit.
#[test]
fn shared_snapshot_identity_checks_arc_currency_and_empty_scope_without_clone() {
    let original = Arc::new(NonClonePayload { rows: vec![5, 11] });
    let other = Arc::new(NonClonePayload { rows: vec![5, 11] });
    let unit = ProfitUnit::Quote(QuoteCurrency::btc());
    let cached = CachedScope::Comparable {
        unit,
        data: Arc::clone(&original),
    };
    let mut state = ProfitLoadState::default();
    assert!(!state.shares_snapshot(&cached));
    state.restore(&cached);
    assert!(state.shares_snapshot(&cached));
    assert!(!state.shares_snapshot(&CachedScope::Comparable { unit, data: other }));
    assert!(!state.shares_snapshot(&CachedScope::Comparable {
        unit: ProfitUnit::Percent,
        data: Arc::clone(&original)
    }));
    assert!(!state.shares_snapshot(&CachedScope::Empty(Arc::clone(&original))));
    let empty = CachedScope::Empty(Arc::clone(&original));
    state.restore(&empty);
    assert!(state.shares_snapshot(&empty));
    assert!(!state.shares_snapshot(&cached));
    let split = CachedScope::Split(Arc::new(QuoteBreakdown::default()));
    state.restore(&split);
    assert!(!state.shares_snapshot(&split));
}

/// Cache-hit restoration must not discard derived work until its shared payload or range changed.
#[test]
fn repeated_snapshot_restore_guards_summary_and_strategy_render_invalidations() {
    let restore = method(include_str!("../snapshots.rs"), "fn restore_snapshot_hit(");
    let summary = restore
        .split_once("CachedResult::Summary(read) if tab == Tab::Summary => {")
        .expect("summary restore")
        .1
        .split_once("CachedResult::Strategies(read)")
        .expect("summary boundary")
        .0;
    assert!(
        summary.contains(
            "let snapshot_changed = range_moved || !self.data.shares_snapshot(&read.data);"
        )
    );
    let (range_reset, _) = guarded_block(summary, "if range_moved {");
    assert!(range_reset.contains("self.hover_profit_bucket = None;"));
    assert!(range_reset.contains("self.summary_popup_hover.reset_for_reload(true);"));
    let (changed, after_changed) = guarded_block(summary, "if snapshot_changed {");
    assert!(changed.contains("self.summary_derived = None;"));
    assert!(changed.contains("self.summary_ui.retain_picks(&present);"));
    assert_eq!(summary.matches("self.summary_derived = None;").count(), 1);
    assert_eq!(summary.matches("self.summary_ui.retain_picks(").count(), 1);
    assert!(summary.contains("self.data.restore(&read.data);"));
    assert!(summary.contains("self.data_as_of = Some(hit.as_of);"));
    assert!(summary.contains("self.data_dirty = !hit.fresh;"));
    assert!(after_changed.contains("self.restore_snapshot_metadata("));

    let strategies = restore
        .split_once("CachedResult::Strategies(read) if tab == Tab::Strategies => {")
        .expect("strategy restore")
        .1
        .split_once("CachedResult::Calendar(read)")
        .expect("strategy boundary")
        .0;
    let (changed, after_changed) = guarded_block(
        strategies,
        "if !self.strategy_data.shares_snapshot(&read.data) {",
    );
    for invalidation in [
        "tuner::core_names_changed(",
        "self.strat_core_w = None;",
        "self.strat_metric_w = None;",
        "self.strat_visible = None;",
    ] {
        assert!(
            changed.contains(invalidation),
            "{invalidation} must stay inside changed-payload guard"
        );
        assert_eq!(strategies.matches(invalidation).count(), 1);
    }
    for required in [
        "self.strategy_data.restore(&read.data);",
        "self.strategy_data_period = self.strat_period;",
        "self.strategy_dirty = !hit.fresh;",
        "self.restore_snapshot_metadata(",
    ] {
        assert!(
            after_changed.contains(required),
            "{required} must run even for unchanged snapshots"
        );
    }
}

/// A cache hit must share the original allocation and retain its exact currency without Clone.
#[test]
fn restoring_borrowed_nonclone_payload_preserves_arc_and_currency() {
    let original = Arc::new(NonClonePayload { rows: vec![13, 29] });
    let unit = ProfitUnit::Quote(QuoteCurrency::btc());
    let cached = CachedScope::Comparable {
        unit,
        data: Arc::clone(&original),
    };
    let mut state = ProfitLoadState::default();

    state.restore(&cached);

    let restored = state.data().expect("comparable payload restored");
    assert!(Arc::ptr_eq(restored, &original));
    assert_eq!(state.unit(), Some(unit));
    assert_eq!(restored.rows.as_slice(), &[13, 29]);
    assert!(state.split().is_none());
}

/// Reusing a view across currency-split and empty scopes must remove the previous scalar unit.
#[test]
fn restoring_split_then_empty_cannot_retain_comparable_money() {
    let comparable = Arc::new(NonClonePayload { rows: vec![7] });
    let mut state = ProfitLoadState::default();
    state.restore(&CachedScope::Comparable {
        unit: ProfitUnit::Quote(QuoteCurrency::btc()),
        data: Arc::clone(&comparable),
    });
    let split =
        QuoteBreakdown::from_groups([(Some(1), 12.5, 2), (Some(8), -3.0, 1), (None, 99.0, 4)]);
    state.restore(&CachedScope::Split(Arc::new(split.clone())));
    assert_eq!(state.split(), Some(&split));
    assert!(state.data().is_none());
    assert_eq!(state.unit(), None);
    assert!(matches!(
        state.view(|_| false),
        Err(Note::IncomparableQuote)
    ));

    let empty = Arc::new(NonClonePayload { rows: Vec::new() });
    state.restore(&CachedScope::Empty(Arc::clone(&empty)));
    assert!(Arc::ptr_eq(
        state.data().expect("empty scalar payload"),
        &empty
    ));
    assert_eq!(state.unit(), None);
    assert!(state.split().is_none());
    assert!(matches!(
        state.view(|data| data.rows.is_empty()),
        Err(Note::Empty)
    ));
}

/// Expired retained rows must require refresh and an age label whether their LRU entry survived.
#[test]
fn retained_expired_snapshot_is_labelled_on_both_hit_and_miss() {
    let source = include_str!("../snapshots.rs");
    let restore = method(source, "fn restore_or_join_snapshot(");
    let hit = restore
        .split_once("if let Some(mut hit) = hit {")
        .expect("cache hit branch")
        .1;
    let (hit, miss) = hit
        .split_once("} else if !initial")
        .expect("retained cache miss branch");
    assert!(hit.contains("if fresh || initial {"));
    assert!(hit.contains("self.mark_snapshot_stale(request.tab, as_of);"));
    assert!(miss.contains("retained_snapshot_needs_refresh("));
    assert!(miss.contains("let Some(as_of) = self.snapshot_as_of[request.tab.snapshot_index()]"));
    assert!(miss.contains("self.mark_snapshot_stale(request.tab, as_of);"));
    let stale = method(source, "fn mark_snapshot_stale(");
    assert!(stale.contains("self.cached_refreshing[tab.snapshot_index()] = Some(as_of);"));
    for dirty in [
        "self.data_dirty = true",
        "self.strategy_dirty = true",
        "self.cal_dirty = true",
    ] {
        assert!(stale.contains(dirty), "an expired page must set {dirty}");
    }
}

/// An evicted page cannot remain clean after any independent publication changes its retained key.
#[test]
fn retained_cache_miss_refreshes_changed_publications_before_the_age_limit() {
    let query = Query {
        from: 100,
        to: 200,
        ..Query::default()
    };
    let revisions = CacheRevisions {
        reports: 7,
        valuation: 3,
        strategies: 2,
        read_state: Some(1),
        valuation_attachment: Some(1),
    };
    let retained = CacheKey::summary(&query, None, revisions, true);
    assert!(!retained_snapshot_needs_refresh(
        Some(&retained),
        &retained,
        100,
        159
    ));
    assert!(retained_snapshot_needs_refresh(
        Some(&retained),
        &retained,
        100,
        160
    ));
    for changed in [
        CacheRevisions {
            reports: 8,
            ..revisions
        },
        CacheRevisions {
            valuation: 4,
            ..revisions
        },
        CacheRevisions {
            strategies: 3,
            ..revisions
        },
    ] {
        let requested = CacheKey::summary(&query, None, changed, true);
        assert!(retained_snapshot_needs_refresh(
            Some(&retained),
            &requested,
            100,
            101
        ));
    }
}

/// Visiting another tab cannot overwrite the retained page's freshness provenance or age label.
#[test]
fn snapshot_markers_and_rendered_age_are_owned_by_each_page() {
    let snapshot = include_str!("../snapshots.rs");
    let restore = method(snapshot, "fn restore_snapshot_hit(");
    assert!(restore.contains("self.cached_refreshing[tab.snapshot_index()] ="));
    assert!(restore.contains("self.snapshot_as_of[tab.snapshot_index()] = Some(hit.as_of);"));
    assert!(
        restore.contains("self.snapshot_keys[tab.snapshot_index()] = Some(request.key.clone());")
    );
    let render = method(include_str!("../render.rs"), "fn render(");
    assert!(render.contains("self.cached_refreshing[self.tab.snapshot_index()]"));
    let view = include_str!("../mod.rs")
        .split_once("pub struct AnalyticsView")
        .expect("view state")
        .1;
    let state = view
        .split_once("impl crate::controls")
        .map_or(view, |(state, _)| state);
    for field in [
        "cached_refreshing: [Option<i64>; 3]",
        "snapshot_as_of: [Option<i64>; 3]",
        "snapshot_keys: [Option<cache::CacheKey>; 3]",
    ] {
        assert!(
            state.contains(field),
            "page-independent marker storage: {field}"
        );
    }
}

/// Restoring or attaching metadata must preserve its actual observation clock without renewal.
#[test]
fn throttled_core_metadata_restores_its_original_timestamp() {
    let source = include_str!("../snapshots.rs");
    let metadata = method(source, "fn restore_snapshot_metadata(");
    assert!(metadata.contains("self.last_cores_at = cores_at;"));
    assert!(!metadata.contains("Instant::now()"));
    let fallback = method(source, "fn with_snapshot_cores(");
    assert!(fallback.contains("let cores_at = self.last_cores_at;"));
    assert_eq!(
        fallback.matches("cores_at,").count(),
        3,
        "every page must carry the metadata clock"
    );
    assert!(!fallback.contains("Instant::now()"));
    let restore = method(source, "fn restore_snapshot_hit(");
    assert_eq!(
        restore.matches("read.cores_at").count(),
        3,
        "every page restores its own metadata clock"
    );
}

/// A due metadata refresh already belongs to the foreground read; timer zero would repeat its axis.
#[test]
fn snapshot_restore_arms_core_timer_only_for_a_future_deadline() {
    let restore = method(include_str!("../snapshots.rs"), "fn restore_snapshot_hit(");
    let guarded = restore
        .split_once("if !wait.is_zero() {")
        .expect("future deadline guard")
        .1;
    let guarded = guarded
        .split_once("\n        }")
        .expect("deadline guard boundary")
        .0;
    assert!(guarded.contains("self.schedule_core_metadata_refresh(wait, cx);"));
    assert_eq!(
        restore
            .matches("self.schedule_core_metadata_refresh(")
            .count(),
        1
    );
}

/// Transient split reads must fall through the raw-result path and central catch-up predicate.
#[test]
fn base_completions_keep_transient_split_publication_under_the_shared_policy() {
    let load = include_str!("../load.rs");
    for signature in ["fn reload_summary(", "fn reload_strategy_base("] {
        let body = method(load, signature);
        assert!(body.contains("CachedRead::from_analytics("));
        assert!(body.contains("Err(result) => *result,"));
        assert!(body.contains("CatchUpOutcome::of_scope(&data)"));
        assert!(body.contains("this.keep_on_catch_up(after_report, data_outcome, report_req)"));
    }
    let calendar = method(
        include_str!("../calendar/mod.rs"),
        "fn reload_calendar_inner(",
    );
    assert!(calendar.contains("CachedCalendar::from_calendar("));
    assert!(calendar.contains("Err(read) => *read,"));
    assert!(calendar.contains("CatchUpOutcome::of_scope(&data.period)"));
    assert!(calendar.contains("this.keep_on_catch_up(after_report, period_outcome, report_req)"));
}

/// Initial and retiring replaceable reads must leave navigation usable; writes remain modal.
#[test]
fn overlay_distinguishes_retiring_replaceable_reads_from_blocking_operations() {
    let overlay = method(include_str!("../load.rs"), "fn busy_overlay_due(");
    assert!(overlay.contains("blocking_overlay_due("));
    assert!(overlay.contains("self.busy_ops,"));
    assert!(overlay.contains("self.latest_overlay_ops,"));
    assert!(overlay.contains("self.busy_since.map(|since| since.elapsed())"));
    assert!(!overlay.contains("visible_first_load_pending()"));
    let worker = method(include_str!("../bg.rs"), "fn spawn_latest_db<");
    let admission = worker
        .split_once("cx.spawn(")
        .expect("latest read admission")
        .0;
    assert!(admission.contains("if overlay {"));
    assert!(admission.contains("self.latest_overlay_ops += 1;"));
    let finish = worker
        .split_once("store(this, result, cx);")
        .expect("result publication")
        .1;
    let finish = finish
        .split_once("this.observe_report_axis(cx);")
        .expect("completion accounting boundary")
        .0;
    assert!(finish.contains("if overlay {"));
    assert!(
        finish.contains("this.latest_overlay_ops = this.latest_overlay_ops.saturating_sub(1);")
    );
    assert!(finish.contains("this.op_finished(cx);"));
}

/// Restoring or completing a snapshot must not bypass a revoked reader or changed money basis.
#[test]
fn money_read_certification_and_scope_guard_both_reuse_and_publication() {
    let source = include_str!("../snapshots.rs");
    let revisions = method(source, "fn cache_revisions(");
    assert!(revisions.contains("moon_core::db::read_state_revision()"));
    assert!(revisions.contains("moon_core::db::valuation::attach_epoch()"));
    let reuse = method(source, "fn restore_or_join_snapshot(");
    for required in [
        "request.key.is_reusable()",
        "key.same_read_state(&request.key)",
        "self.retire_snapshot(request.tab)",
    ] {
        assert!(
            reuse.contains(required),
            "cache reuse must check {required}"
        );
    }
    let publish = method(source, "fn publish_snapshot(");
    let before_restore = publish
        .split_once("self.restore_snapshot_hit(")
        .expect("typed publication")
        .0;
    for required in [
        "request.key.same_scope(&current.key)",
        "request.key.same_read_state(&current.key)",
        "self.retire_snapshot(request.tab)",
        "RefreshUrgency::User",
    ] {
        assert!(
            before_restore.contains(required),
            "publication must check {required} before figures are restored"
        );
    }
}

/// A deferred tab entry must not repeat an axis that became current while its request waited.
#[test]
fn queued_axis_entry_rechecks_freshness_after_the_inflight_read_finishes() {
    let refresh = method(
        include_str!("../observe.rs"),
        "fn refresh_visible_report_data(",
    );
    let axis = refresh
        .split_once("VisibleRefresh::StrategyAxis => {")
        .expect("axis dispatch")
        .1
        .split_once("VisibleRefresh::Calendar")
        .expect("axis boundary")
        .0;
    let guard = axis
        .find("if self.axis_needs_reload(self.strat_mode)")
        .expect("freshness recheck");
    let read = axis
        .find("self.reload_axis_after_report(")
        .expect("axis read");
    assert!(guard < read);
}
