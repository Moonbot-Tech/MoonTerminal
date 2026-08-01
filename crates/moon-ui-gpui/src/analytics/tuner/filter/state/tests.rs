//! Unit tests for persisted filter-tuner controls.

use super::{
    DEFAULT_EDGES, DEFAULT_ITERS, DEFAULT_TRAIN, SuggestJob, SuggestState, TRAIN_OPTIONS,
    TunerState, canonical_iters, edge_options, edge_options_upto, fmt_bound, iters_of, parse_num,
    persist_seed, restore_edges, restore_edges_upto, restore_enabled, restore_iters, restore_seed,
    restore_train, seed_of, staged_dirty, train_frac,
};
use moon_core::db::tuner::FIELDS;
use moon_core::db::tuner::StratFilters;
use moon_core::db::tuner::threshold_search::{
    EDGES_MAX, EDGES_MAX_LIGHT, EDGES_MIN, RESTARTS_MIN, SearchHandle, restarts_max,
};

/// `filter/state.rs:TunerState::mark_report_stale` preserves drafts but retires suggestions.
///
/// Removing its suggestion-generation bump lets an optimizer pinned before a report commit write
/// stale bounds into v1 after the commit, where they become saveable for the new report state.
#[test]
fn report_staleness_preserves_filter_drafts() {
    let mut state = TunerState::load(None, None, None, None, None, false);
    state.bounds[0][0] = ("1".into(), "2".into());
    state.staged_ignore.insert("IgnoreFilters", true);
    let (seq, hist_seq, sugg_seq, dialog_seq) =
        (state.seq, state.hist_seq, state.sugg_seq, state.dialog_seq);

    state.mark_report_stale();

    assert!(
        !state.variants()[1].is_empty(),
        "the KPI consumer must still receive the staged bound"
    );
    assert!(
        staged_dirty(&state.strat, &state.staged_ignore),
        "the Save consumer must still see the staged ignore edit"
    );
    assert_eq!(state.seq, seq);
    assert_eq!(state.hist_seq, hist_seq);
    assert_ne!(state.sugg_seq, sugg_seq);
    assert_eq!(
        state.dialog_seq, dialog_seq,
        "report data cannot invalidate a pending Save dialog"
    );
    assert!(state.needs_reload());
}

/// `filter/state.rs:TunerState::invalidate` must advance every read generation; replacing it with
/// report-only staleness lets old KPI, histogram, or suggestion completions publish in a new scope.
#[test]
fn scope_change_retires_all_filter_requests() {
    let mut state = TunerState::load(None, None, None, None, None, false);
    let (seq, hist_seq, sugg_seq) = (state.seq, state.hist_seq, state.sugg_seq);

    state.invalidate();

    assert_ne!(state.seq, seq);
    assert_ne!(state.hist_seq, hist_seq);
    assert_ne!(state.sugg_seq, sugg_seq);
    assert!(state.needs_reload());
}

/// Retiring a suggestion must STOP the search, not merely stop listening to it.
///
/// Breakage this pins: `state.rs:TunerState::invalidate_suggest` going back to advancing the
/// generation alone. Every manual v1 edit retires the running search, so one that kept going
/// would hold the whole worker pool producing an answer that can no longer be published — and
/// the user's next Search click would start a second one alongside it.
#[test]
fn retiring_a_suggestion_stops_the_search_behind_it() {
    let mut state = TunerState::load(None, None, None, None, None, false);
    let handle = SearchHandle::new();
    state.sugg = SuggestState::Running(SuggestJob::AllFields {
        handle: handle.clone(),
        total: 100,
    });

    state.invalidate_suggest();

    assert!(
        handle.is_cancelled(),
        "the retired search must be told to stop"
    );
    assert!(
        !state.sugg.is_running(),
        "and must no longer read as running"
    );
}

/// `filter/state.rs:TunerState::needs_reload` must include `hist_dirty`; checking only KPI state
/// leaves an old histogram pinned when an automatic KPI finishes after the user leaves Filters.
#[test]
fn stale_histogram_keeps_the_filter_axis_reloadable() {
    let mut state = TunerState::load(None, None, None, None, None, false);
    state.stats.apply(Ok(Vec::new()));
    state.dirty = false;
    state.hist_dirty = true;

    assert!(state.needs_reload());
}

/// `filter/state.rs:TunerState::apply_strategy_read` must preserve a confirmed Save baseline
/// when an automatic refresh cannot read the strategy row; replacing it with an unconditional
/// assignment turns the next Save preview into a diff against invented defaults.
#[test]
fn automatic_missing_strategy_read_preserves_the_save_baseline() {
    let mut state = TunerState::load(None, None, None, None, None, false);
    state.apply_strategy_read(
        StratFilters {
            found: true,
            ignore_filters: true,
            ..Default::default()
        },
        false,
    );

    state.apply_strategy_read(StratFilters::default(), true);

    assert!(state.strat.found);
    assert!(state.strat.ignore_filters);
}

/// `filter/state.rs:TunerState::mark_dialog_draft_changed` must advance `dialog_seq`; removing
/// that update lets an async Save preview open for the draft that existed before the user's edit.
#[test]
fn draft_change_retires_pending_save_preview() {
    let mut state = TunerState::load(None, None, None, None, None, false);
    let pending_preview = state.dialog_seq;

    state.mark_dialog_draft_changed();

    assert_ne!(
        state.dialog_seq, pending_preview,
        "the pending Save preview must no longer match the edited draft"
    );
}

/// Every depth the dropdown offers must be one the search will actually honour.
///
/// The oracle is the search's own accepted ceilings, read from `moon-core` — source against
/// source, not a literal restated here, and BOTH machine classes in one run. Breakage this pins:
/// someone extends `EDGE_OPTIONS_ALL` with a finer step without touching the search's own bounds.
/// The dropdown would then offer a depth that `clamp` silently rewrites, so the tuner would
/// persist and display a depth every run ignored.
#[test]
fn every_offered_depth_is_one_the_search_accepts() {
    for max in [EDGES_MAX_LIGHT, EDGES_MAX] {
        for offered in edge_options_upto(max) {
            assert!(
                (EDGES_MIN..=max).contains(offered),
                "depth {offered} is offered by the dropdown but past the ceiling {max}"
            );
        }
        assert!(
            edge_options_upto(max).contains(&DEFAULT_EDGES),
            "ceiling {max}: the fallback depth must be one the dropdown offers there too"
        );
    }
    assert!(
        (EDGES_MIN..=EDGES_MAX).contains(&DEFAULT_EDGES),
        "the fallback depth must itself be acceptable to the search"
    );
}

/// A machine below the heavy-search bar is offered a strictly smaller choice, and every depth it
/// IS offered is one a bigger machine offers too.
///
/// The oracle is the relation between the two branches, derived from what the bar is for — not
/// the option list restated. Both branches run in one test run, which a machine never does.
///
/// Breakage this pins: `state.rs:edge_options_upto` selecting by anything other than the ceiling
/// — a hand-kept index, a filter over the whole list — which can gate a depth from the MIDDLE.
/// A stored 512 would stay selectable while 128 was not, so [`edges_of`] silently rewrites a value
/// the dropdown still shows as chosen. Raising the light ceiling to the heavy one is caught here
/// too: it puts the heaviest depth back on the machines this bar exists to protect.
#[test]
fn a_small_machine_is_offered_a_prefix_of_what_a_big_one_is() {
    let light = edge_options_upto(EDGES_MAX_LIGHT);
    let heavy = edge_options_upto(EDGES_MAX);
    assert_eq!(
        light.last(),
        Some(&EDGES_MAX_LIGHT),
        "the protected dropdown must expose its exact core ceiling"
    );
    assert_eq!(
        heavy.last(),
        Some(&EDGES_MAX),
        "the powerful-PC dropdown must expose depth 512 instead of stopping early"
    );
    assert!(
        light.len() < heavy.len(),
        "the bar must actually withhold something"
    );
    assert_eq!(
        light,
        &heavy[..light.len()],
        "the withheld depths must be the heaviest ones, not a hole in the middle"
    );
    // Whatever this machine is, what it offers is a prefix of the whole list — so a retuned
    // ceiling can shorten the dropdown but can never put a hole in it.
    assert_eq!(
        edge_options(),
        &heavy[..edge_options().len()],
        "the offered set must stay a prefix on this machine too"
    );
}

/// Every default must be a value the control that shows it can actually represent.
///
/// The oracle is each control's own option set and the search's own bounds — the defaults are
/// checked against what OTHER code declares, never against a literal restated here.
///
/// Breakage this pins: retuning a default to a number the control cannot show. A `DEFAULT_EDGES`
/// of 48 makes `restore_edges(None)` hand back 48 unvalidated, so the depth dropdown opens
/// displaying 48 with no item marked as chosen, and the first click silently moves the user to a
/// different value. A `DEFAULT_ITERS` past the search ceiling would likewise display a count the
/// search would quietly clamp away.
#[test]
fn every_default_is_a_value_its_control_can_show() {
    assert!(
        edge_options().contains(&DEFAULT_EDGES),
        "the default depth {DEFAULT_EDGES} is not one the dropdown offers"
    );
    assert!(
        TRAIN_OPTIONS.contains(&DEFAULT_TRAIN),
        "the default train share {DEFAULT_TRAIN} is not one the dropdown offers"
    );
    assert_eq!(
        iters_of(&restore_iters(None)),
        DEFAULT_ITERS,
        "the restart box must open on the default the search would actually run"
    );
    assert!((RESTARTS_MIN..=restarts_max()).contains(&DEFAULT_ITERS));
}

/// The depth restored from `layout.toml` has to be one the dropdown can select again.
///
/// Breakage this pins: replacing `state.rs:restore_edges` membership validation with a range
/// clamp admits values between dropdown entries. A stored `5` would render with no highlighted
/// item, leaving the user in a state the UI cannot produce.
#[test]
fn depth_restores_only_values_the_dropdown_offers() {
    for offered in edge_options() {
        assert_eq!(
            restore_edges(Some(*offered as u32)),
            *offered,
            "a depth the dropdown offers must survive a restart unchanged"
        );
    }
    // Inside the search range but absent from the dropdown — the case a clamp would pass.
    for between in [5u32, 63, 100] {
        assert_eq!(
            restore_edges(Some(between)),
            DEFAULT_EDGES,
            "{between} is not selectable, so it must fall back to the default"
        );
    }
    for outside in [0u32, 3, 129, u32::MAX] {
        assert_eq!(restore_edges(Some(outside)), DEFAULT_EDGES);
    }
    assert_eq!(restore_edges(None), DEFAULT_EDGES);
    assert_eq!(
        restore_edges_upto(Some(512), EDGES_MAX),
        512,
        "a powerful PC must reopen its saved 512 depth"
    );
    assert_eq!(
        restore_edges_upto(Some(512), EDGES_MAX_LIGHT),
        DEFAULT_EDGES,
        "a protected PC must not inherit a 512-depth run from another machine"
    );
}

/// What gets stored must reopen as the exact count the search will run.
///
/// The oracle is the agreement of two decoupled functions — `iters_of` (raw box text → the
/// number `suggest_into_v1` runs) and `restore_iters` (stored number → box text) — not a
/// literal restated from either.
///
/// Breakage this pins: removing the clamp from `state.rs:iters_of` lets the UI store a value above
/// `RESTARTS_MAX` while the search executes the ceiling. Re-hardcoding a bound there instead of
/// reading the const drifts the same way as soon as the search moves its own. Replacing
/// `restore_iters` with `saved.unwrap_or_default()` also opens an unset knob on 0 instead of the
/// default.
#[test]
fn restarts_reopen_as_the_count_the_search_runs() {
    for typed in [
        "", "   ", "abc", "0", "1", "7", "500", "1000", "2000", "2001", "99999", "100k", "100001",
        "-5", "20k", "1.5k", " 2К ",
    ] {
        let effective = iters_of(typed);
        // Reopened text may be COMPACT ("20k"), so the agreement to check is the count itself
        // making the round trip — not the spelling.
        assert_eq!(
            iters_of(&restore_iters(Some(effective as u32))),
            effective,
            "storing what the search read from {typed:?} must reopen as that same count"
        );
        assert!((RESTARTS_MIN..=restarts_max()).contains(&effective));
    }
    // The compact form is only used where it is EXACT: a count that would lose digits stays long.
    assert_eq!(restore_iters(Some(2_000)), "2k");
    assert_eq!(restore_iters(Some(1_234)), "1234");
    // Boundary pair against THIS machine's ceiling — the search's own, so the box cannot promise
    // a count the run would silently cut down.
    let max = restarts_max();
    assert_eq!(iters_of(&max.to_string()), max);
    assert_eq!(iters_of(&(max + 1).to_string()), max);
    assert_eq!(
        canonical_iters(&(max + 1).to_string()),
        fmt_bound(max as f64)
    );
    assert_eq!(iters_of(&RESTARTS_MIN.to_string()), RESTARTS_MIN);
    assert_eq!(iters_of("0"), RESTARTS_MIN);
    // The suffix the box displays has to be one it reads back.
    assert_eq!(iters_of("2k"), 2_000);
    assert_eq!(iters_of("1.5k"), 1_500);
    // An absent value opens on the default rather than on an empty box.
    assert_eq!(restore_iters(None), DEFAULT_ITERS.to_string());
}

/// A pinned seed must reopen as the same seed, and an unusable one must not reopen as a seed.
///
/// The oracle is the agreement of two decoupled functions — `persist_seed` (box text → what is
/// stored) and `restore_seed` (stored value → box text) — read back through `seed_of`, the one
/// the search itself runs on. No literal is restated from any of them.
///
/// Breakage this pins: relaxing `state.rs:seed_of` to `parse().unwrap_or(0)`, which turns a typo
/// into the fixed seed 0 and silently pins every later search to one set of random starts;
/// or persisting the raw box text, which reopens the tuner displaying a seed no search honours.
#[test]
fn a_pinned_seed_reopens_as_the_seed_the_search_uses() {
    for typed in [
        "",
        "   ",
        "0",
        "42",
        " 7 ",
        "abc",
        "-1",
        "1.5",
        "18446744073709551615",
        "99999999999999999999999",
    ] {
        let stored = persist_seed(typed);
        let reopened = restore_seed(stored.clone());
        assert_eq!(
            seed_of(&reopened),
            seed_of(typed),
            "the seed {typed:?} reopened as {reopened:?}, a different search"
        );
        assert_eq!(
            stored.is_some(),
            seed_of(typed).is_some(),
            "only a usable seed may be stored, and every usable one must be"
        );
    }
    // An empty box is the "draw a fresh seed" state, and so is a stored value that stopped
    // parsing — a file edited by hand must not pin the search to something arbitrary.
    assert!(seed_of(&restore_seed(None)).is_none());
    assert!(seed_of(&restore_seed(Some("not-a-seed".into()))).is_none());
}

/// A bound shown in the grid must be the bound that gets applied.
///
/// A v1 threshold is STORED as its displayed string and read back through `parse_num` to build
/// the KPI query and to write the strategy, so a display form that does not survive that round
/// trip is a different filter, not a shorter number.
///
/// The oracle is the round trip itself — `fmt_bound` and `parse_num` are decoupled functions, and
/// no literal is restated from either.
///
/// Breakage this pins: `state.rs:fmt_bound` going back to returning its compact form
/// unconditionally. `1234567` would display and apply as `1.23M` = 1 230 000, silently dropping
/// every trade between the two, and a small threshold like `0.000123` would collapse to `0.0001`
/// — a fifth of the way off, on a bound the search had chosen precisely.
#[test]
fn a_displayed_bound_reads_back_as_the_same_number() {
    for v in [
        0.0,
        1.0,
        -2.5,
        0.5,
        // Four decimals is exactly what the compact form keeps — one more is not.
        0.0001,
        0.000123,
        -0.000123,
        123.4567,
        123.45678,
        // The suffix forms keep two decimals, so three significant digits survive and more do not.
        1_230_000.0,
        1_234_567.0,
        999.999,
        -1_234_567_890.0,
        1.2345e12,
    ] {
        let shown = fmt_bound(v);
        assert_eq!(
            parse_num(&shown),
            Some(v),
            "{v} displayed as {shown:?}, which reads back as a different filter"
        );
    }
}

/// A stored train share must reopen as a share the dropdown can select, and only 100 may mean
/// "no split".
///
/// The oracle is the agreement of two decoupled functions — `restore_train` (stored value → the
/// selected percentage) and `train_frac` (percentage → the fraction the search runs on) — with
/// the search's own rule that only a fraction below 1 splits anything.
///
/// Breakage this pins: replacing `state.rs:restore_train`'s membership check with a clamp. A
/// stored 0 or 5 would then survive as a real setting, and the search would fit on a handful of
/// the oldest trades while the tuner displayed a share the dropdown cannot show as selected.
#[test]
fn a_stored_train_share_reopens_as_one_the_dropdown_offers() {
    for offered in TRAIN_OPTIONS {
        assert_eq!(restore_train(Some(offered as u32)), offered);
        let frac = train_frac(offered);
        assert_eq!(
            frac < 1.0,
            offered < 100,
            "{offered}% must split the period exactly when it is not the whole period"
        );
    }
    // Values a clamp would let through: outside the offered set, so they open on the default.
    for between in [0u32, 5, 55, 99, 101, u32::MAX] {
        assert_eq!(restore_train(Some(between)), DEFAULT_TRAIN);
    }
    assert_eq!(restore_train(None), DEFAULT_TRAIN);
    // The default holds nothing back, so an untouched tuner behaves exactly as it did before the
    // split existed.
    assert_eq!(train_frac(DEFAULT_TRAIN), 1.0);
}

/// The checkbox selection must survive a reorder of the field table, not just a restart.
///
/// Breakage this pins: persisting positions (`Vec<usize>` or a bool mask) instead of column ids.
/// `FIELDS` is documented as presentation order — Base → Ping → Volume → Delta — so inserting one
/// row would shift every position after it, and the tuner would reopen with different boxes ticked
/// than the user left. The oracle is the column id looked up independently of the code under test.
#[test]
fn saved_fields_are_stored_as_column_ids_not_positions() {
    let target = FIELDS
        .iter()
        .position(|s| s.col == "dmark")
        .expect("dmark is a report column of the tuner");

    let mask = restore_enabled(Some(&["dmark".to_string()]));
    assert!(mask[target], "the saved column must reopen checked");
    assert_eq!(
        mask.iter().filter(|on| **on).count(),
        1,
        "nothing but the saved column may be checked"
    );

    let mut state = TunerState::load(None, None, None, None, None, false);
    state.enabled = mask;
    assert_eq!(
        state.enabled_cols(),
        vec!["dmark".to_string()],
        "persistence must write the column id back, not its position"
    );
}

/// A config that has never saved the selection opens on the tuner's own default.
///
/// Breakage this pins: reading the key with `saved.unwrap_or_default()`. Every field would then
/// come up unchecked on a fresh install, and the first automatic search would sweep nothing and
/// report "no suggestion" — with no visible cause.
#[test]
fn an_absent_saved_list_falls_back_to_the_mapped_default() {
    let expected: Vec<bool> = FIELDS.iter().map(|s| s.mapped()).collect();
    assert_eq!(restore_enabled(None), expected);
    assert!(
        expected.iter().any(|on| *on),
        "the default must actually enable something, or the fallback is meaningless"
    );
}

/// Unchecking every box is a statement, not a missing value.
///
/// Breakage this pins: treating an empty list as "nothing saved" (`if list.is_empty() { default }`).
/// A user who deliberately disarmed the search would find it fully re-armed after a restart.
#[test]
fn an_empty_saved_list_is_not_the_same_as_no_saved_list() {
    let empty = restore_enabled(Some(&[]));
    assert!(
        empty.iter().all(|on| !on),
        "an empty saved list must leave every field unchecked"
    );
    assert_ne!(
        empty,
        restore_enabled(None),
        "an empty list and an absent key must not restore the same selection"
    );
}

/// The mask always spans the field table, whatever the saved list holds.
///
/// Breakage this pins: building the mask from the saved list's own length or order. Three grid and
/// search call sites index `enabled` by field position, so a short mask panics out of bounds on the
/// first render — and an id from an older build is exactly how a short one would arrive.
#[test]
fn an_unknown_column_is_ignored_and_the_mask_always_spans_every_field() {
    let saved = vec!["dmark".to_string(), "col_that_no_longer_exists".to_string()];
    let mask = restore_enabled(Some(&saved));

    assert_eq!(
        mask.len(),
        FIELDS.len(),
        "the mask must cover every field the grid will index"
    );
    assert_eq!(
        mask.iter().filter(|on| **on).count(),
        1,
        "an id with no field behind it must be ignored, not counted"
    );
}

/// A field added after the last save opens unchecked rather than joining the search unannounced.
///
/// Breakage this pins: defaulting an unlisted field to `spec.mapped()` — i.e. merging the saved
/// list with the default instead of replacing it. A new tuner field would then silently widen a
/// saved search, changing both its result and its runtime for a user who changed nothing.
#[test]
fn a_field_missing_from_the_saved_list_opens_unchecked() {
    // Stands in for "the list was written before this field existed": one real id saved, and every
    // OTHER mapped field therefore absent from it.
    let mask = restore_enabled(Some(&["dmark".to_string()]));
    let unlisted_mapped = FIELDS
        .iter()
        .zip(mask.iter())
        .filter(|(s, _)| s.mapped() && s.col != "dmark")
        .collect::<Vec<_>>();

    assert!(
        !unlisted_mapped.is_empty(),
        "the table must hold another mapped field, or this proves nothing"
    );
    assert!(
        unlisted_mapped.iter().all(|(_, on)| !**on),
        "a mapped field absent from the saved list must stay unchecked"
    );
}
