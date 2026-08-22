// Explicit imports avoid pulling the parent's `gpui::*`, whose `test` shadows the built-in
// attribute and recursively expands `#[test]`.
use super::controls::selected_auto_core_name;
use super::state::{ReportFilterSet, applied_filters};
use super::{
    Period, ReportKind, ReportPeriodBucket, SideFilter, apply_period_from_prefs,
    next_prefs_for_period_pick, period_bucket_for_scope, row_scope_for, side_id,
    strategy_name_mask_enabled,
};
use crate::workspace::{RetainedCoreScope, resolve_group_scope};
use chrono::{TimeZone as _, Utc};
use moon_core::config::WorkspaceMode;
use moon_core::db::RowScope;

/// `mod.rs::row_scope_for` -- replacing its `closed_only || !show_open` guard with `&&`, dropping
/// the `!`, or swapping its branch arms makes the Report table, totals, and CSV include active
/// positions when a host or user explicitly excluded them.
///
/// The expected scopes come from the independent host/user precedence contract and the public
/// period-bound rule: only an unexcluded period ending before `now` omits active rows.
#[test]
fn row_scope_precedence_excludes_active_positions_before_consulting_the_period() {
    const NOW: i64 = 1_000;

    assert_eq!(
        row_scope_for(true, true, None, NOW),
        RowScope::Closed,
        "a host-owned closed-only scope must win even for an unbounded period"
    );
    assert_eq!(
        row_scope_for(false, false, Some(NOW), NOW),
        RowScope::Closed,
        "turning off active positions must win even when the period reaches now"
    );
    assert_eq!(
        row_scope_for(false, true, None, NOW),
        RowScope::ClosedAndOpen,
        "an unbounded period must include active positions after both exclusions are clear"
    );
    assert_eq!(
        row_scope_for(false, true, Some(NOW), NOW),
        RowScope::ClosedAndOpen,
        "a bound at now still reaches the present"
    );
    assert_eq!(
        row_scope_for(false, true, Some(NOW - 1), NOW),
        RowScope::Closed,
        "a completed period delegates to the closed-history scope"
    );
}

/// `controls::selected_auto_core_name` must prefer the live group-session name over historical
/// report metadata, while retaining that metadata as the offline fallback.
///
/// Plausible breakage: searching the DB-derived list first. A renamed core then keeps the old name
/// in Auto Report; accepting an empty live label also hides a usable historical fallback; and a
/// newly connected core with no report rows renders no server name at all.
#[test]
fn auto_core_label_prefers_live_name_and_keeps_history_as_fallback() {
    let live = vec![(7, "LIVE\nCORE".to_string())];
    let reports = vec![
        (7, "STALE CORE".to_string()),
        (8, "OFFLINE CORE".to_string()),
    ];

    assert_eq!(
        selected_auto_core_name(7, &live, &reports).as_deref(),
        Some("LIVE ¶ CORE")
    );
    assert_eq!(
        selected_auto_core_name(8, &live, &reports).as_deref(),
        Some("OFFLINE CORE")
    );
    assert_eq!(
        selected_auto_core_name(8, &[(8, " \n ".to_string())], &reports).as_deref(),
        Some("OFFLINE CORE"),
        "an empty live label must not suppress the historical fallback"
    );
    assert_eq!(selected_auto_core_name(9, &live, &reports), None);
}

/// Return one brace-delimited function or method body, including its signature.
///
/// Local mirror of `tests/theme_contract/support.rs::braced_body` — that helper lives in a
/// separate integration-test binary this crate's unit tests cannot import (`moon-ui-gpui` has no
/// `[lib]` target).
fn braced_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("expected to find `{signature}` in the source"));
    let open = source[start..]
        .find('{')
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("expected `{signature}` to have a body"));
    let mut depth = 0usize;
    for (offset, byte) in source.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..=open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("expected `{signature}` to have a matching closing brace");
}

/// Strip `//` line comments before a source-slicing assertion runs, so prose that merely NAMES a
/// call under discussion cannot satisfy an assertion whose actual code was deleted or moved.
///
/// Local mirror of `shell/workspace/tests.rs::code_only`, for the same reason as [`braced_body`].
fn code_only(body: &str) -> String {
    body.lines()
        .map(|line| match line.find("//") {
            Some(at) => &line[..at],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Replacing `report::Period::range_at`'s existing-day step with forward-clamping `day_start`
/// makes Apia Yesterday empty on December 31 instead of selecting December 29.
#[test]
fn yesterday_uses_the_previous_existing_day_across_a_dateline_skip() {
    let now = Utc
        .with_ymd_and_hms(2011, 12, 30, 12, 0, 0)
        .single()
        .expect("valid UTC instant")
        .timestamp();

    assert_eq!(
        Period::Yesterday.range_at(now, chrono_tz::Pacific::Apia),
        (Some(1_325_152_800), Some(1_325_239_199))
    );
}

/// Group Report keeps its Core shortcut passive in Auto and scopes delayed coin navigation.
///
/// Mutation: restore the Auto writer in `actions.rs` or replace the authorized Report request in
/// `columns.rs` with unconditional Main navigation. A retained transaction row could then select
/// or reveal its previous core, or lose the exact published trade-history scope.
#[test]
fn report_navigation_cannot_override_current_auto_scope() {
    let actions = include_str!("actions.rs");
    let columns = include_str!("columns.rs");

    assert!(!actions.contains("select_auto_workspace_core"));
    assert!(columns.contains("(!panel.standalone).then(|| panel.group.clone())"));
    assert!(columns.contains("b.open_report_on_main_if_authorized("));
}

/// `Period::CurWeek` is a CALENDAR preset: mid-week it must resolve to that week's own Monday, not
/// to today or to a fixed rolling offset.
///
/// Mutation: swap `today.weekday().num_days_from_monday()` for `num_days_from_sunday()` (an easy
/// Sunday-week-start slip) — on a Wednesday this shifts the boundary to Sunday instead of Monday.
///
/// Independent oracle: the expected Monday is built from a literal calendar date via
/// `chrono::Utc`, never by calling any `Period` method.
#[test]
fn cur_week_mid_week_resolves_to_that_weeks_monday() {
    // 2024-01-10 is a Wednesday; 2024-01-08 is the Monday of the same week.
    let now = Utc
        .with_ymd_and_hms(2024, 1, 10, 15, 30, 0)
        .single()
        .expect("valid UTC instant")
        .timestamp();
    let monday_start = Utc
        .with_ymd_and_hms(2024, 1, 8, 0, 0, 0)
        .single()
        .expect("valid UTC instant")
        .timestamp();

    assert_eq!(
        Period::CurWeek.range_at(now, chrono_tz::UTC),
        (Some(monday_start), None)
    );
}

/// On a Monday, `Period::CurWeek`'s lower bound is that same day, not the previous week's Monday.
///
/// Mutation: subtracting seven days when the weekday offset is zero moves the boundary to the
/// previous Monday; the literal same-day midnight is an independent oracle for that edge case.
#[test]
fn cur_week_on_a_monday_resolves_to_that_same_day() {
    let now = Utc
        .with_ymd_and_hms(2024, 1, 8, 9, 0, 0)
        .single()
        .expect("valid UTC instant")
        .timestamp();
    let monday_start = Utc
        .with_ymd_and_hms(2024, 1, 8, 0, 0, 0)
        .single()
        .expect("valid UTC instant")
        .timestamp();

    assert_eq!(
        Period::CurWeek.range_at(now, chrono_tz::UTC),
        (Some(monday_start), None)
    );
}

/// `Period::CurYear` is a CALENDAR preset anchored at January 1st, not the 1st of the current
/// month.
///
/// Mutation: the plausible copy-paste from the `CurMonth` arm right above it —
/// `calendar(today.with_day(1))` — silently drops the `with_month(1)` step and resolves to the
/// current month's 1st instead of the year's.
///
/// Independent oracle: the expected January 1st is a literal `chrono::Utc` date.
#[test]
fn cur_year_resolves_to_january_first() {
    let now = Utc
        .with_ymd_and_hms(2024, 6, 15, 12, 0, 0)
        .single()
        .expect("valid UTC instant")
        .timestamp();
    let jan_first = Utc
        .with_ymd_and_hms(2024, 1, 1, 0, 0, 0)
        .single()
        .expect("valid UTC instant")
        .timestamp();

    assert_eq!(
        Period::CurYear.range_at(now, chrono_tz::UTC),
        (Some(jan_first), None)
    );
}

/// `Period::GROUPS` is the single declaration of menu membership, grouping, AND the exact display
/// order within each group: every variant must appear exactly once, in the approved sequence, and
/// no two presets may share a menu element id.
///
/// Two independent oracles do different jobs. The exact-sequence compare below pins ORDER: a swap
/// of, say, `Today`/`Yesterday` inside their group changes nothing about membership or uniqueness,
/// so the group/uniqueness checks alone would stay green through it (an earlier version of this
/// test only NAMED "approved order" without pinning it). `expected_group`/`ALL` still does the
/// membership and duplicate-id job, and its EXHAUSTIVE match with no wildcard arm is what fails a
/// `Period` variant added without an arm to COMPILE, rather than silently passing a runtime
/// assertion that never enumerates it.
///
/// Mutations:
/// - Swap two presets within one group (e.g. `Today`/`Yesterday`, or reorder the calendar group):
///   the exact-sequence compare reddens.
/// - Drop one preset from its `GROUPS` entry (e.g. `CurYear` from the calendar group): the variant
///   stays reachable through `range_at`/`label`, so only the completeness check below (flattened
///   `GROUPS` length against `ALL`) notices it went unreachable from the menu.
/// - Give two variants the same `menu_key()` (a copy-paste of an existing arm): the uniqueness
///   check below reddens, now genuinely exercised against `GROUPS`'s own data rather than against
///   a literal list already guaranteed distinct.
#[test]
fn period_groups_home_every_variant_exactly_once_in_the_approved_order() {
    assert!(
        Period::GROUPS
            == [
                &[Period::Today, Period::Yesterday][..],
                &[Period::CurWeek, Period::CurMonth, Period::CurYear][..],
                &[Period::Days7, Period::Days30, Period::Days365][..],
                &[Period::All][..],
            ],
        "GROUPS must list every preset in exactly this approved order, group by group"
    );

    // Exhaustive: every `Period` variant must own an arm here, or the whole test binary fails to
    // compile. No wildcard arm, on purpose — that absence IS the enforcement.
    fn expected_group(period: Period) -> usize {
        match period {
            Period::Today => 0,
            Period::Yesterday => 0,
            Period::CurWeek => 1,
            Period::CurMonth => 1,
            Period::CurYear => 1,
            Period::Days7 => 2,
            Period::Days30 => 2,
            Period::Days365 => 2,
            Period::All => 3,
        }
    }

    // Explicit runtime checklist for completeness and duplicate detection. Keep it synchronized
    // with the exhaustive `expected_group` match whenever a preset is added.
    const ALL: [Period; 9] = [
        Period::Today,
        Period::Yesterday,
        Period::CurWeek,
        Period::CurMonth,
        Period::CurYear,
        Period::Days7,
        Period::Days30,
        Period::Days365,
        Period::All,
    ];

    let flat_len: usize = Period::GROUPS.iter().map(|group| group.len()).sum();
    assert_eq!(
        flat_len,
        ALL.len(),
        "GROUPS must home every Period variant exactly once"
    );

    let mut seen_keys: Vec<&str> = Vec::new();
    for (group_ix, group) in Period::GROUPS.iter().enumerate() {
        for &period in group.iter() {
            assert_eq!(
                expected_group(period),
                group_ix,
                "{} sits in the wrong GROUPS entry",
                period.menu_key()
            );
            let key = period.menu_key();
            assert!(
                !seen_keys.contains(&key),
                "menu key {key} is shared by two presets"
            );
            seen_keys.push(key);
        }
    }

    for period in ALL {
        assert!(
            seen_keys.contains(&period.menu_key()),
            "{} is unreachable from the menu",
            period.menu_key()
        );
    }
}

/// `Period::CurWeek`'s lower bound is that week's own Monday on EVERY day of the week, not just
/// the two days already pinned individually above (`cur_week_on_a_monday_resolves_to_that_same_day`,
/// `cur_week_mid_week_resolves_to_that_weeks_monday`).
///
/// The oracle is the exact literal Monday for every iterated day — not a `>=`/`>` comparison
/// against `Period::Days7`. A relative comparison is decorative against the mutation this test
/// names: aliasing `CurWeek` onto the `Days7` rolling formula yields an EQUAL bound on most days
/// of the week (and an exactly equal bound on Sunday by construction), which `>=` silently
/// accepts. The exact-Monday oracle catches the same aliasing on every day it actually differs
/// from the fixed Monday (Tuesday through Sunday), because a rolling window and a fixed calendar
/// boundary only ever coincide once, on Sunday.
///
/// Mutation: swap `today.weekday().num_days_from_monday()` for `num_days_from_sunday()`, or alias
/// the whole arm onto `Days7`'s formula.
#[test]
fn cur_week_lower_bound_is_that_weeks_monday_on_every_day_of_the_week() {
    let monday_start = Utc
        .with_ymd_and_hms(2024, 1, 8, 0, 0, 0)
        .single()
        .expect("valid UTC instant")
        .timestamp();
    for day in 8..=14 {
        let now = Utc
            .with_ymd_and_hms(2024, 1, day, 12, 0, 0)
            .single()
            .expect("valid UTC instant")
            .timestamp();
        let (cur_week_from, _) = Period::CurWeek.range_at(now, chrono_tz::UTC);
        assert_eq!(
            cur_week_from,
            Some(monday_start),
            "day {day}: CurWeek must bound at that week's own Monday, not a rolling window"
        );
    }
}

/// `side_id`, `ReportKind::id` and `Period::menu_key` are an external storage contract: every
/// filter value already written to a user's `layout.toml` must keep decoding to the same enum
/// variant forever, so the id each one emits is pinned as a LITERAL, independent of the encoder's
/// own source — not a round trip, which stays green when an encoder and its decoder are renamed
/// together in the same pass and would leave every value already on disk silently orphaned.
///
/// Every match below is EXHAUSTIVE with no wildcard arm, so a variant added to `SideFilter`,
/// `ReportKind` or `Period` without an arm here fails this file to COMPILE — that compile failure,
/// not a runtime assertion, is what enforces the id contract for a brand new variant.
///
/// Mutations:
/// - Renaming one stored id inside `side_id`/`ReportKind::id`/`Period::menu_key` (e.g. `"short"` to
///   `"sell"`) reddens the matching literal comparison below.
/// - Adding a new variant to any of the three enums without extending the matching `expected_*`
///   function here fails the BUILD.
#[test]
fn filter_ids_are_pinned_exact_strings() {
    fn expected_side_id(side: SideFilter) -> &'static str {
        match side {
            SideFilter::All => "all",
            SideFilter::Long => "long",
            SideFilter::Short => "short",
        }
    }
    for side in [SideFilter::All, SideFilter::Long, SideFilter::Short] {
        assert_eq!(side_id(side), expected_side_id(side));
    }

    fn expected_kind_id(kind: ReportKind) -> &'static str {
        match kind {
            ReportKind::All => "all",
            ReportKind::Real => "real",
            ReportKind::Emu => "emu",
        }
    }
    for kind in [ReportKind::All, ReportKind::Real, ReportKind::Emu] {
        assert_eq!(kind.id(), expected_kind_id(kind));
    }

    fn expected_period_key(period: Period) -> &'static str {
        match period {
            Period::All => "rp-all",
            Period::Today => "rp-today",
            Period::Yesterday => "rp-yesterday",
            Period::CurWeek => "rp-cur-week",
            Period::CurMonth => "rp-cur-month",
            Period::CurYear => "rp-cur-year",
            Period::Days7 => "rp-days-7",
            Period::Days30 => "rp-days-30",
            Period::Days365 => "rp-days-365",
        }
    }
    for period in [
        Period::All,
        Period::Today,
        Period::Yesterday,
        Period::CurWeek,
        Period::CurMonth,
        Period::CurYear,
        Period::Days7,
        Period::Days30,
        Period::Days365,
    ] {
        assert_eq!(period.menu_key(), expected_period_key(period));
    }
}

/// `state.rs::applied_filters` decides, per field, whether a stored id wins or the panel's
/// CURRENT value survives — the exact fallback rule `restore_persisted_filters` depends on. Calls
/// the REAL function (not a hand-written stand-in for it), so a broken fallback inside it actually
/// reddens this test.
///
/// `ReportPanel` cannot be built in a unit test: every field needs a live GPUI window, which this
/// binary crate's tests have no way to open (see `braced_body`/`code_only` above) — but
/// `applied_filters` was deliberately factored out as a free, pure function precisely so this
/// decision does not need one.
///
/// Pins: each stored field winning when present and valid; each field falling back to CURRENT
/// when its stored counterpart is absent, when the id is unknown to this build, and when the
/// entry supplies only SOME of the six fields (the rest must still resolve independently, not as
/// an all-or-nothing unit).
///
/// Mutations:
/// - Swapping `.unwrap_or(side)` for a hard-coded `SideFilter::All` inside `applied_filters`.
/// - Cross-wiring which stored field feeds one of the first four typed outputs, or omitting the
///   string mask from the returned tuple.
///
/// `current` is deliberately built from NON-default, mutually distinct values for every field
/// (never `SideFilter::All`/`ReportKind::All`/`Period::All`, the type each field's "natural"
/// hard-coded fallback would plausibly pick) — a fallback mutation that happens to coincide with
/// `current` would otherwise pass unnoticed.
#[test]
fn applied_filters_prefers_stored_values_and_falls_back_to_current_per_field() {
    let current = ReportFilterSet {
        side: SideFilter::Long,
        kind: ReportKind::Real,
        deleted_only: false,
        // The panel default is ON, so this is both the upgrade fallback and a value a hard-coded
        // false fallback cannot silently match.
        show_open: true,
        period: Period::Today,
        strategy_name_mask: "CURRENT".to_string(),
    };

    // Every field stored, valid, and DIFFERENT from `current` on every position: every one must
    // win, and a fallback-to-current or cross-wired mutation both produce a visibly wrong value.
    let full = moon_core::config::ReportFilterPrefs {
        side: Some("short".to_string()),
        kind: Some("emu".to_string()),
        deleted_only: Some(true),
        show_open: Some(false),
        period: Some("rp-cur-week".to_string()),
        period_overview: Some("rp-today".to_string()),
        strategy_name_mask: Some("EMA_".to_string()),
    };
    let ReportFilterSet {
        side,
        kind,
        deleted_only,
        show_open,
        period,
        strategy_name_mask,
        ..
    } = applied_filters(&full, ReportPeriodBucket::Single, current.clone());
    assert_eq!(
        side,
        SideFilter::Short,
        "a valid stored side must win over current"
    );
    assert!(
        kind == ReportKind::Emu,
        "a valid stored kind must win over current"
    );
    assert!(
        deleted_only,
        "a valid stored deleted_only must win over current"
    );
    assert!(
        !show_open,
        "a stored OFF value must win over the panel default of showing open positions"
    );
    assert!(
        period == Period::CurWeek,
        "a valid stored period must win over current"
    );
    let overview_applied = applied_filters(&full, ReportPeriodBucket::Overview, current.clone());
    assert!(
        overview_applied.period == Period::Today,
        "applied_filters must forward the live Overview bucket to the period decoder"
    );
    assert_eq!(strategy_name_mask, "EMA_", "a valid stored mask must win");
    // The leaf assembly a query actually reads, fed by the values applied_filters just resolved —
    // ReportKind::to_filter is exhaustive over all three variants so a swapped mapping reddens.
    assert_eq!(ReportKind::All.to_filter(), None);
    assert_eq!(ReportKind::Real.to_filter(), Some(false));
    assert_eq!(
        kind.to_filter(),
        Some(true),
        "Emu must query only emulator orders"
    );

    // Nothing stored at all: every field must fall back to current — asserted per field, so a
    // swapped pair in the returned tuple cannot hide behind one combined comparison.
    let empty = moon_core::config::ReportFilterPrefs::default();
    let ReportFilterSet {
        side,
        kind,
        deleted_only,
        show_open,
        period,
        strategy_name_mask,
        ..
    } = applied_filters(&empty, ReportPeriodBucket::Single, current.clone());
    assert_eq!(
        side, current.side,
        "an absent side must keep the current value"
    );
    assert!(
        kind == current.kind,
        "an absent kind must keep the current value"
    );
    assert_eq!(
        deleted_only, current.deleted_only,
        "an absent deleted_only must keep the current value"
    );
    assert_eq!(
        show_open, current.show_open,
        "an absent open-positions setting must keep the panel default"
    );
    assert!(
        period == current.period,
        "an absent period must keep the current value"
    );
    assert_eq!(
        strategy_name_mask, current.strategy_name_mask,
        "an absent mask must keep current"
    );

    // Every id present but unknown to this build: the same fallback as absent.
    let unknown = moon_core::config::ReportFilterPrefs {
        side: Some("diagonal".to_string()),
        kind: Some("bogus".to_string()),
        deleted_only: None,
        show_open: None,
        period: Some("rp-nonexistent".to_string()),
        period_overview: Some("rp-overview-nonexistent".to_string()),
        strategy_name_mask: None,
    };
    let ReportFilterSet {
        side,
        kind,
        deleted_only,
        show_open,
        period,
        strategy_name_mask,
        ..
    } = applied_filters(&unknown, ReportPeriodBucket::Overview, current.clone());
    assert_eq!(
        side, current.side,
        "an unknown side id must keep the current value"
    );
    assert!(
        kind == current.kind,
        "an unknown kind id must keep the current value"
    );
    assert_eq!(deleted_only, current.deleted_only);
    assert_eq!(show_open, current.show_open);
    assert!(
        period == current.period,
        "an unknown period id must keep the current value"
    );
    assert_eq!(strategy_name_mask, current.strategy_name_mask);

    // A PARTIAL entry — only side and deleted_only stored, both DIFFERENT from current — must
    // resolve each field on its own: the fields that DID win must not drag the absent ones along.
    let partial = moon_core::config::ReportFilterPrefs {
        side: Some("all".to_string()),
        kind: None,
        deleted_only: Some(true),
        show_open: None,
        period: None,
        period_overview: None,
        strategy_name_mask: Some(String::new()),
    };
    let ReportFilterSet {
        side,
        kind,
        deleted_only,
        show_open,
        period,
        strategy_name_mask,
        ..
    } = applied_filters(&partial, ReportPeriodBucket::Overview, current.clone());
    assert_eq!(side, SideFilter::All, "the stored side must win");
    assert!(
        kind == current.kind,
        "an absent kind must keep the current value even though side won"
    );
    assert!(deleted_only, "the stored deleted_only must win");
    assert_eq!(
        show_open, current.show_open,
        "an absent open-positions setting must keep the current value even though side won"
    );
    assert!(
        period == current.period,
        "an absent period must keep the current value even though side won"
    );
    assert_eq!(
        strategy_name_mask, "",
        "an explicitly cleared mask must beat the current value"
    );
}

/// `apply_period_from_prefs` must keep Overview independent while preserving the legacy fallback.
///
/// Mutation: reading only `period`, stopping after an unknown `period_overview`, or consulting the
/// Overview field for a single-server scope returns a different value in at least one assertion.
#[test]
fn period_bucket_decode_uses_overview_then_legacy_then_current() {
    let both = moon_core::config::ReportFilterPrefs {
        period: Some("rp-cur-year".to_string()),
        period_overview: Some("rp-today".to_string()),
        ..Default::default()
    };
    assert!(
        apply_period_from_prefs(&both, ReportPeriodBucket::Overview, Period::Yesterday)
            == Period::Today
    );
    assert!(
        apply_period_from_prefs(&both, ReportPeriodBucket::Single, Period::Yesterday)
            == Period::CurYear,
        "a single-server scope must ignore period_overview"
    );

    let missing_overview = moon_core::config::ReportFilterPrefs {
        period: Some("rp-cur-year".to_string()),
        ..Default::default()
    };
    assert!(
        apply_period_from_prefs(
            &missing_overview,
            ReportPeriodBucket::Overview,
            Period::Yesterday,
        ) == Period::CurYear,
        "an absent Overview value must fall back to the legacy period"
    );

    let unknown_overview = moon_core::config::ReportFilterPrefs {
        period: Some("rp-cur-year".to_string()),
        period_overview: Some("rp-unknown".to_string()),
        ..Default::default()
    };
    assert!(
        apply_period_from_prefs(
            &unknown_overview,
            ReportPeriodBucket::Overview,
            Period::Yesterday,
        ) == Period::CurYear,
        "an unknown Overview id must still try the valid legacy period"
    );

    let unknown_both = moon_core::config::ReportFilterPrefs {
        period: Some("rp-legacy-unknown".to_string()),
        period_overview: Some("rp-overview-unknown".to_string()),
        ..Default::default()
    };
    assert!(
        apply_period_from_prefs(
            &unknown_both,
            ReportPeriodBucket::Overview,
            Period::Yesterday,
        ) == Period::Yesterday,
        "unknown ids in both slots must preserve the panel's current period"
    );
}

/// `next_prefs_for_period_pick` must update only the period slot owned by the live scope.
///
/// Mutation: rebuilding from defaults or assigning both period fields loses or overwrites the
/// inactive bucket and reddens the exact before/after assertions below.
#[test]
fn period_bucket_pick_writes_only_the_live_bucket() {
    let existing = moon_core::config::ReportFilterPrefs {
        period: Some("rp-cur-year".to_string()),
        period_overview: Some("rp-yesterday".to_string()),
        ..Default::default()
    };
    let overview = next_prefs_for_period_pick(
        Some(&existing),
        ReportPeriodBucket::Overview,
        Some(Period::Today),
        &ReportFilterSet {
            side: SideFilter::Long,
            kind: ReportKind::Real,
            deleted_only: false,
            show_open: true,
            period: Period::Today,
            strategy_name_mask: "EMA_".to_string(),
        },
    );
    assert_eq!(overview.period.as_deref(), Some("rp-cur-year"));
    assert_eq!(overview.period_overview.as_deref(), Some("rp-today"));

    let before_single = moon_core::config::ReportFilterPrefs {
        period: Some("rp-cur-month".to_string()),
        period_overview: overview.period_overview.clone(),
        ..overview
    };
    let single = next_prefs_for_period_pick(
        Some(&before_single),
        ReportPeriodBucket::Single,
        Some(Period::CurYear),
        &ReportFilterSet {
            side: SideFilter::Short,
            kind: ReportKind::Emu,
            deleted_only: true,
            show_open: false,
            period: Period::CurYear,
            strategy_name_mask: "SINGLE".to_string(),
        },
    );
    assert_eq!(single.period.as_deref(), Some("rp-cur-year"));
    assert_eq!(single.period_overview.as_deref(), Some("rp-today"));
    assert_eq!(single.side.as_deref(), Some("short"));
    assert_eq!(single.kind.as_deref(), Some("emu"));
    assert_eq!(single.deleted_only, Some(true));
    assert_eq!(single.show_open, Some(false));
    assert_eq!(single.strategy_name_mask.as_deref(), Some("SINGLE"));

    let shared_only = next_prefs_for_period_pick(
        Some(&single),
        ReportPeriodBucket::Overview,
        None,
        &ReportFilterSet {
            side: SideFilter::All,
            kind: ReportKind::Real,
            deleted_only: false,
            show_open: true,
            period: Period::All,
            strategy_name_mask: "SHARED".to_string(),
        },
    );
    assert_eq!(shared_only.period, single.period);
    assert_eq!(shared_only.period_overview, single.period_overview);
    assert_eq!(shared_only.side.as_deref(), Some("all"));
    assert_eq!(shared_only.strategy_name_mask.as_deref(), Some("SHARED"));
}

/// Workspace scope changes cross the period boundary only when entering or leaving Auto Overview.
///
/// Mutation: classifying by core identity or workspace ownership alone makes AutoCore A/B differ,
/// or incorrectly gives Classic a separate bucket.
#[test]
fn period_bucket_changes_only_across_auto_overview() {
    let cores = [11, 22];
    let overview = resolve_group_scope(
        WorkspaceMode::AutoTrading,
        None,
        &cores,
        RetainedCoreScope::All,
    );
    let auto_a = resolve_group_scope(
        WorkspaceMode::AutoTrading,
        Some(11),
        &cores,
        RetainedCoreScope::All,
    );
    let auto_b = resolve_group_scope(
        WorkspaceMode::AutoTrading,
        Some(22),
        &cores,
        RetainedCoreScope::All,
    );
    let classic_all =
        resolve_group_scope(WorkspaceMode::Classic, None, &cores, RetainedCoreScope::All);
    let classic_selection = resolve_group_scope(
        WorkspaceMode::Classic,
        None,
        &cores,
        RetainedCoreScope::Explicit(&[11]),
    );

    assert_eq!(
        period_bucket_for_scope(Some(&overview)),
        ReportPeriodBucket::Overview
    );
    for scope in [&auto_a, &auto_b, &classic_all, &classic_selection] {
        assert_eq!(
            period_bucket_for_scope(Some(scope)),
            ReportPeriodBucket::Single
        );
    }
    assert_eq!(period_bucket_for_scope(None), ReportPeriodBucket::Single);
    assert_eq!(
        period_bucket_for_scope(Some(&auto_a)),
        period_bucket_for_scope(Some(&auto_b)),
        "switching single servers must stay in one bucket"
    );
    assert_ne!(
        period_bucket_for_scope(Some(&overview)),
        period_bucket_for_scope(Some(&auto_a)),
        "Overview to a server must cross buckets"
    );
}

/// The strategy-name mask belongs to the complete Auto workspace, not only a selected core.
///
/// Mutation: restoring the former `is_auto_core` gate hides and clears the mask in Full summary;
/// accepting Classic or `None` applies a retained invisible filter outside Auto.
#[test]
fn strategy_name_mask_is_enabled_for_both_auto_scopes_only() {
    let cores = [11, 22];
    let overview = resolve_group_scope(
        WorkspaceMode::AutoTrading,
        None,
        &cores,
        RetainedCoreScope::All,
    );
    let auto_core = resolve_group_scope(
        WorkspaceMode::AutoTrading,
        Some(11),
        &cores,
        RetainedCoreScope::All,
    );
    let classic_all =
        resolve_group_scope(WorkspaceMode::Classic, None, &cores, RetainedCoreScope::All);
    let classic_selection = resolve_group_scope(
        WorkspaceMode::Classic,
        None,
        &cores,
        RetainedCoreScope::Explicit(&[11]),
    );

    assert!(strategy_name_mask_enabled(Some(&overview)));
    assert!(strategy_name_mask_enabled(Some(&auto_core)));
    assert!(!strategy_name_mask_enabled(Some(&classic_all)));
    assert!(!strategy_name_mask_enabled(Some(&classic_selection)));
    assert!(!strategy_name_mask_enabled(None));
}

/// `ReportPanel::new_with_scope` must restore this host's stored filters BEFORE
/// `load_initial_metadata` runs, or a freshly opened Report always shows the panel's hard-coded
/// defaults no matter what was saved for that host context — restoration is synchronous and must
/// land before the panel's first active-render query goes out, while metadata loading is
/// backgrounded and returns later.
///
/// Pinned at the source level for the same reason as the tests above (`ReportPanel` cannot be
/// built in a unit test), using the same `find`-offset ordering technique as
/// `set_period_persists_before_the_changed_value_guard`: asserting mere PRESENCE of the call, as
/// an earlier version of this test did, still passes if the call is moved to the end of the
/// constructor.
///
/// Mutation: moving `this.restore_persisted_filters(cx);` to the end of the constructor, after
/// `this.load_initial_metadata(...)`.
#[test]
fn the_constructor_restores_persisted_filters_before_loading_metadata() {
    let state = code_only(include_str!("state.rs"));
    let body = braced_body(&state, "pub(crate) fn new_with_scope(");
    let restore_at = body
        .find("this.restore_persisted_filters(cx);")
        .expect("new_with_scope must restore this host's stored filters");
    let load_at = body
        .find("this.load_initial_metadata(")
        .expect("new_with_scope must still load background metadata");
    assert!(
        restore_at < load_at,
        "restore_persisted_filters must run before load_initial_metadata, not after"
    );
}

/// An Analytics-scoped standalone panel must neither restore stored filters nor write its own —
/// two independent guards, because either can be dropped without breaking the other.
///
/// Mutation: deleting the `if self.scoped { return false; }` guard from
/// `state.rs::restore_persisted_filters` applies stored values over the strategy Analytics asked
/// for; deleting the `if self.scoped { return; }` guard from `actions.rs::persist_filters` writes
/// that transient scope into the shared per-context store and corrupts the next ordinary Report
/// opened in that same context.
#[test]
fn a_scoped_panel_neither_restores_nor_persists_stored_filters() {
    let state = code_only(include_str!("state.rs"));
    let restore_body = braced_body(
        &state,
        "pub(super) fn restore_persisted_filters(&mut self, cx: &mut Context<Self>) -> bool {",
    );
    // `restore_persisted_filters` has a SECOND `return false;` (no stored entry for this host), so
    // the guard itself is isolated by brace-matching rather than a loose substring search.
    let scoped_guard = braced_body(restore_body, "if self.scoped {");
    assert!(
        scoped_guard.contains("return false;"),
        "restore_persisted_filters must refuse a scoped panel before reading anything from storage"
    );

    let actions = code_only(include_str!("actions.rs"));
    let persist_body = braced_body(&actions, "pub(super) fn persist_filters(");
    let scoped_guard = braced_body(persist_body, "if self.scoped {");
    assert!(
        scoped_guard.contains("return;"),
        "persist_filters must refuse to write anything for a scoped panel"
    );
}

/// Both stored-filter call sites must key their storage lookup off the panel's OWN live
/// `detached` flag, never a literal — a hard-coded `false` would make a detached window read and
/// write the docked tab's stored filters instead of its own, so a preference set in one context
/// would leak into (or be invisible from) the other.
///
/// Existence is checked per file rather than an exact total count: a legitimately added third
/// call site that still keys off `self.detached` must not redden this test — only a LITERAL
/// boolean argument is the invariant that matters.
///
/// Mutation: hard-coding `false` for `detached` in either `filters_ctx_id` caller.
#[test]
fn filters_ctx_id_call_sites_never_pass_a_literal_host_flag() {
    // The two contexts must differ — the storage contract both callers below depend on.
    assert_eq!(super::filters_ctx_id(false), "report-filters:dock");
    assert_eq!(super::filters_ctx_id(true), "report-filters:win");
    assert_ne!(super::filters_ctx_id(false), super::filters_ctx_id(true));

    let state = code_only(include_str!("state.rs"));
    let actions = code_only(include_str!("actions.rs"));
    assert!(
        state.contains("filters_ctx_id(self.detached)"),
        "restore_persisted_filters must key its lookup off self.detached"
    );
    assert!(
        actions.contains("filters_ctx_id(self.detached)"),
        "persist_filters must key its write off self.detached"
    );
    for (label, source) in [("state.rs", &state), ("actions.rs", &actions)] {
        assert!(
            !source.contains("filters_ctx_id(true)") && !source.contains("filters_ctx_id(false)"),
            "{label}: no Report filter storage call may pass a hard-coded literal instead of the \
             live host-context flag"
        );
    }
}

/// The stored period must be derived from the picked-period PARAMETER, never read off the live
/// `self.period` field: `self.period` also shows the implicit "all" that typing a manual date
/// produces, which is not a preset anybody chose from the menu and must not silently replace the
/// last real menu pick that is already stored.
///
/// This is the exact defect three reviewers found and fixed in `persist_filters`; the regression
/// it protects: type a manual date (forces `Period::All`), then change direction or order kind —
/// the stored period must stay the last real menu choice, not flip to "all".
///
/// Mutation: `persist_filters` reading `self.period` instead of `picked_period`.
#[test]
fn persist_filters_stores_the_picked_period_not_the_live_field() {
    let actions = code_only(include_str!("actions.rs"));
    let body = braced_body(&actions, "pub(super) fn persist_filters(");
    assert!(
        body.contains("period_bucket_for_scope(")
            && body.contains("next_prefs_for_period_pick(")
            && body.contains("period_bucket,")
            && body.contains("picked_period,"),
        "the live bucket and picked_period parameter must reach the persistence composer"
    );
    assert!(
        !body.contains("self.period.menu_key()"),
        "persist_filters must never read the live self.period field when computing the stored id"
    );
}

/// The workspace observer must apply a changed bucket before clearing and requerying its rows.
///
/// Mutation: deleting the conditional restore leaves the existing clear + requery path green at
/// runtime source shape while the toolbar and query retain the previous bucket's period.
#[test]
fn workspace_observer_applies_period_bucket_before_requery() {
    let state = code_only(include_str!("state.rs"));
    let observer = braced_body(&state, "cx.observe(&workspace_revision,");
    let bucket_at = observer
        .find("period_bucket_for_scope(")
        .expect("workspace observer must resolve the live period bucket");
    let restore_at = observer
        .find("apply_period_from_prefs(")
        .expect("workspace observer must restore a changed period bucket");
    let clear_at = observer
        .find("this.data = LoadState::default();")
        .expect("workspace observer must still clear rows from the previous scope");
    let requery_at = observer
        .find("this.request_requery(cx);")
        .expect("workspace observer must still requery every scope change");
    assert!(bucket_at < restore_at && restore_at < clear_at && clear_at < requery_at);
    let changed = braced_body(observer, "if period_bucket != this.last_period_bucket {");
    assert!(
        changed.contains("apply_period_from_prefs(stored, period_bucket, this.period)"),
        "period restore must conditionally apply the resolved live bucket"
    );
}

/// `restore_persisted_filters`'s `changed` result must be exactly `applied != current` — a full
/// six-field named-struct compare, not a narrowed one and not a literal — because `changed` is what
/// `mark_table_detached` gates its requery on: a caller that always sees `true` requeries on every
/// host-context switch even when nothing restored actually differs, and a caller narrowed to only
/// `side` silently skips the requery when a restored kind, deleted_only, open-positions switch,
/// period, or mask is the
/// only thing that changed, showing rows for a filter the toolbar no longer displays.
///
/// `ReportPanel` cannot be built in a unit test (needs a live GPUI window), and the comparison
/// itself is a single trivial tuple `!=` with no decision left to re-derive dynamically without
/// simply restating it — so this is pinned at the source level, the only reachable route, the same
/// technique the neighbouring tests in this file already use for un-runnable wiring.
///
/// Mutations:
/// - Replacing the comparison with a literal `true`.
/// - Narrowing it to compare only one field (e.g. `applied.0 != current.side`).
#[test]
fn restore_persisted_filters_changed_is_exactly_the_full_tuple_compare() {
    let state = code_only(include_str!("state.rs"));
    let body = braced_body(
        &state,
        "pub(super) fn restore_persisted_filters(&mut self, cx: &mut Context<Self>) -> bool {",
    );
    assert!(
        body.contains("let changed = applied != current;"),
        "changed must be the full applied-vs-current tuple compare, not a narrowed or hard-coded \
         one"
    );
    let tail = body.trim_end();
    let tail = tail.strip_suffix('}').unwrap_or(tail).trim_end();
    assert!(
        tail.ends_with("changed"),
        "the function must return the computed changed value itself, not a different expression"
    );
}

/// `set_period` must persist the picked preset even when it does not change the value displayed —
/// a menu pick that merely re-confirms an already-shown implicit "all" (from a typed manual date)
/// is still the user replacing the last stored real pick with this one.
///
/// Mutation: moving the `self.persist_filters(Some(p), cx);` call inside the
/// `if self.period != p` guard.
#[test]
fn set_period_persists_before_the_changed_value_guard() {
    let actions = code_only(include_str!("actions.rs"));
    let body = braced_body(
        &actions,
        "pub(super) fn set_period(&mut self, p: Period, cx: &mut Context<Self>) {",
    );
    let persist_at = body
        .find("self.persist_filters(Some(p), cx);")
        .expect("set_period must persist the picked preset");
    let guard_at = body
        .find("if self.period != p {")
        .expect("set_period must still guard the visible change");
    assert!(
        persist_at < guard_at,
        "persist_filters must run before the changed-value guard, not inside it"
    );
}
