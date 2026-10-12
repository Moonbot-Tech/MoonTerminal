//! Confirmation and layout invariants from the approved station cores mockup.
use super::{
    ActionSurface, CoreCache, CoresFilter, CoresSummary, TABLE_MAX_H, action_id, compact_name_cell,
    compact_row, core_address_cell, core_name_cell, cores_summary, differ_card, filter_rows,
    list_controls_el, push_els, removable, retained_arm, show_all_el, state_label, summary_el,
    table_el, table_fits, table_layout,
};
use crate::backend::station::cores_sync::{self, LocalCore, Row, RowState};
use crate::design;
use gpui::{
    AnyElement, App, AppContext, ClickEvent, Context, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, ScrollHandle, Styled, TextOverflow, WhiteSpace, Window, div, px,
};
use moon_core::config::Language;
use moon_core::station_api::ListedCore;
use moon_ui::{MoonButton, MoonGroupBox, MoonInputState, MoonTone, h_flex};
use rust_i18n::t;
use std::rc::Rc;

/// A differing card keeps the whole name wrapped; the full list truncates it with a tooltip.
#[test]
fn core_names_wrap_with_unlimited_height() {
    let mut cell = core_name_cell(0, "Demo Fund ~ F-BN / SUBACCOUNT (SUB 7)".into());
    let style = cell.style();
    assert_eq!(style.text.white_space, Some(WhiteSpace::Normal));
    assert_eq!(style.text.text_overflow, None);
    assert_eq!(style.text.line_clamp, None);
    assert_eq!(style.size.height, None);
    assert_eq!(style.max_size.height, None);
    let mut compact = compact_name_cell(0, "Demo Fund ~ F-BN / SUBACCOUNT (SUB 7)");
    let style = compact.style();
    assert_eq!(style.text.white_space, Some(WhiteSpace::Nowrap));
    assert!(matches!(
        style.text.text_overflow,
        Some(TextOverflow::Truncate(_))
    ));
    assert_eq!(style.flex_shrink, Some(1.0));
}

/// A synthetic comparison row.
fn row(state: RowState, index: usize) -> Row {
    let net = if index.is_multiple_of(2) {
        "198.51.100"
    } else {
        "203.0.113"
    };
    Row {
        state,
        name: format!("Synthetic Core {index:02} ~ F-XX / SUBACCOUNT LONG NAME {index}"),
        station_name: (state == RowState::NameDiffers).then(|| format!("Old Name {index}")),
        address: Some(format!("{net}.{}:4510", 10 + index)),
        terminal_uid: (state != RowState::OnlyOnStation).then_some(index as u64 + 1),
        station_uid: (state != RowState::OnlyHere).then_some(index as u64 + 1),
    }
}

/// Sixty synthetic cores: four differing ones on top, the rest matching.
fn rows() -> Vec<Row> {
    let mut out = vec![
        row(RowState::OnlyOnStation, 0),
        row(RowState::OnlyHere, 1),
        row(RowState::KeyDiffers, 2),
        row(RowState::NameDiffers, 3),
    ];
    out.extend((4..60).map(|index| row(RowState::Same, index)));
    out
}

/// Station-only and terminal-only cores both differ; the total is the whole union.
#[test]
fn summary_counts_one_sided_cores_as_differing() {
    let rows = rows();
    let summary = |total, differ| CoresSummary { total, differ };
    assert_eq!(cores_summary(&rows), summary(60, 4));
    assert_eq!(cores_summary(&rows[..2]), summary(2, 2));
    assert_eq!(cores_summary(&rows[4..]), summary(56, 0));
}

/// Search hits an address substring or any-case name; Differ drops matching cores.
#[test]
fn full_list_filters_by_address_name_and_state() {
    let rows = rows();
    let indexes = |hits: Vec<(usize, &Row)>| hits.into_iter().map(|(i, _)| i).collect::<Vec<_>>();
    assert_eq!(
        indexes(filter_rows(&rows, "203.0.113.15", CoresFilter::All)),
        [5]
    );
    assert_eq!(
        indexes(filter_rows(&rows, "  synthetic core 07 ", CoresFilter::All)),
        [7]
    );
    assert_eq!(
        indexes(filter_rows(&rows, "OLD NAME 3", CoresFilter::All)),
        [3]
    );
    assert_eq!(
        indexes(filter_rows(&rows, "", CoresFilter::Differ)),
        [0, 1, 2, 3]
    );
    assert!(indexes(filter_rows(&rows, "synthetic core 07", CoresFilter::Differ)).is_empty());
    assert_eq!(filter_rows(&rows, "", CoresFilter::All).len(), 60);
    assert!(filter_rows(&rows, "198.51.100.15", CoresFilter::All).is_empty());
}

/// Allowing address shrink or ellipsis would hide the port beside a long common name prefix.
#[test]
fn core_addresses_keep_intrinsic_width_without_ellipsis() {
    for address in [Some("203.0.113.60:50000".into()), None] {
        let mut cell = core_address_cell(address);
        let style = cell.style();
        assert_eq!(style.flex_shrink, Some(0.0));
        assert_eq!(style.size.width, None);
        assert_eq!(style.text.white_space, Some(WhiteSpace::Nowrap));
        assert_eq!(style.text.text_overflow, None);
    }
}

/// A reused uid, changed identity or newly local match must discard destructive consent.
#[test]
fn confirmation_is_cleared_when_identity_changes_or_becomes_local() {
    let listing = [3, 9].map(|uid| ListedCore {
        endpoint_override: None,
        uid,
        name: format!("Core {uid}"),
        address: Some(format!("198.51.100.{uid}:4510")),
        key_fp: None,
    });
    let rows = cores_sync::reconcile(&[], &listing, None);
    let arm = (9, listing[1].address.clone(), listing[1].name.clone());
    assert_eq!(retained_arm(Some(arm.clone()), &rows), Some(arm.clone()));
    assert_eq!(retained_arm(Some(arm.clone()), &rows[1..]), None);
    assert!(removable(&rows[1], 2));
    let mut changed = listing.clone();
    changed[1].name = "Reused".into();
    assert_eq!(
        retained_arm(
            Some(arm.clone()),
            &cores_sync::reconcile(&[], &changed, None)
        ),
        None
    );
    changed[1].name = listing[1].name.clone();
    changed[1].address = Some("198.51.100.99:4510".into());
    assert_eq!(
        retained_arm(
            Some(arm.clone()),
            &cores_sync::reconcile(&[], &changed, None)
        ),
        None
    );
    let local = LocalCore {
        endpoint_override: String::new(),
        key_address: listing[1].address.clone(),
        uid: 17,
        name: "Now local".into(),
        address: listing[1].address.clone(),
        key_fp: "fixture".into(),
    };
    assert_eq!(
        retained_arm(Some(arm), &cores_sync::reconcile(&[local], &listing, None)),
        None
    );
}

/// The mockup reserves positive tone for equal cores and warning/info for push actions.
#[test]
fn actionable_states_follow_mockup_invariants() {
    for state in [
        RowState::Same,
        RowState::OnlyHere,
        RowState::OnlyOnStation,
        RowState::NameDiffers,
        RowState::KeyDiffers,
    ] {
        let (_, tone) = state_label(state);
        if matches!(tone, MoonTone::Warning | MoonTone::Info) {
            assert!(matches!(
                state,
                RowState::OnlyHere | RowState::NameDiffers | RowState::KeyDiffers
            ));
        }
        if state == RowState::OnlyOnStation {
            assert!(!matches!(tone, MoonTone::Positive));
        }
        if state == RowState::Same {
            assert!(!matches!(
                tone,
                MoonTone::Warning | MoonTone::Info | MoonTone::Danger
            ));
        }
    }
}

/// A longer translated action must raise the table breakpoint instead of overflowing.
#[test]
fn layout_switches_at_the_measured_content_boundary() {
    assert!(table_fits(600.0, 200.0, 100.0, 100.0, 150.0, 50.0));
    assert!(!table_fits(599.0, 200.0, 100.0, 100.0, 150.0, 50.0));
    assert!(!table_fits(600.0, 300.0, 100.0, 100.0, 150.0, 50.0));
    assert!(table_fits(700.0, 300.0, 100.0, 100.0, 150.0, 50.0));
}

/// Stale cached rows would keep a removed core actionable or hide a saved local rename.
#[test]
fn cache_invalidates_on_listing_and_local_revision() {
    let mut cache = CoreCache::default();
    let remote = ListedCore {
        endpoint_override: None,
        uid: 9,
        name: "Core".into(),
        address: Some("198.51.100.9:4510".into()),
        key_fp: Some("fp".into()),
    };
    let local = LocalCore {
        endpoint_override: String::new(),
        key_address: remote.address.clone(),
        uid: 3,
        name: "Core".into(),
        address: remote.address.clone(),
        key_fp: "fp".into(),
    };
    let listing = Some(Some(vec![remote]));
    cache.update(&listing, vec![local.clone()], None);
    assert_eq!(cache.rows[0].state, RowState::Same);
    let mut renamed = local;
    renamed.name = "Renamed".into();
    cache.update(&listing, vec![renamed], None);
    assert_eq!(cache.rows[0].state, RowState::NameDiffers);
    cache.update(&Some(Some(vec![])), vec![], None);
    assert!(cache.rows.is_empty());
}

/// Retaining cached allocations after a floor change would offer a uid from retired history.
#[test]
fn cache_invalidates_when_only_retirement_floor_changes() {
    let mut cache = CoreCache::default();
    let local = LocalCore {
        endpoint_override: String::new(),
        key_address: Some("198.51.100.3:4510".into()),
        uid: 3,
        name: "New".into(),
        address: Some("198.51.100.3:4510".into()),
        key_fp: "fixture".into(),
    };
    let listing = Some(Some(vec![]));
    cache.update(&listing, vec![local.clone()], Some(7));
    assert_eq!(cache.rows[0].station_uid, Some(8));
    cache.update(&listing, vec![local], Some(12));
    assert_eq!(cache.rows[0].station_uid, Some(13));
}

/// The production row actions' visual shape, with the station-only core's removal armed.
fn probe_actions(row: &Row, index: usize) -> AnyElement {
    let button = |id: String, key: &str| {
        MoonButton::new(gpui::SharedString::from(id))
            .size(design::CONTROL_TIER)
            .label(t!(key).to_string())
    };
    let actions = h_flex().w_full().min_w_0().flex_wrap().gap(px(6.0));
    let actions = match row.state {
        RowState::OnlyHere => {
            actions.child(button(format!("add-{index}"), "telegram.server.cores_add").render())
        }
        RowState::NameDiffers => {
            actions.child(button(format!("name-{index}"), "telegram.server.cores_name").render())
        }
        RowState::KeyDiffers => {
            actions.child(button(format!("key-{index}"), "telegram.server.cores_key").render())
        }
        RowState::OnlyOnStation => actions
            .child(
                button(
                    format!("rm-{index}"),
                    "telegram.server.cores_remove_confirm",
                )
                .danger()
                .render(),
            )
            .child(button(format!("cancel-{index}"), "dialogs.cancel").render()),
        RowState::Same => actions,
    };
    actions.into_any_element()
}

/// The cores block's production pieces, backend-free, with the full list open.
struct CoresProbe {
    rows: Vec<Row>,
    width: f32,
    /// Cores the push button would send; `None` when nothing differs.
    push: Option<usize>,
    query: Entity<MoonInputState>,
    scroll: ScrollHandle,
}

impl Render for CoresProbe {
    /// Stack the summary, differing cards, caret, list controls and full list as the block does.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = cores_summary(&self.rows);
        let layout = table_layout(&self.rows, self.width - design::ui_value(cx, 24.0), cx);
        let mut section = MoonGroupBox::new("probe-cores")
            .padding(12.0)
            .gap(10.0)
            .child(summary_el(summary, cx));
        for (index, row) in self.rows.iter().enumerate() {
            if row.state != RowState::Same {
                section = section.child(differ_card(index, row, probe_actions(row, index), cx));
            }
        }
        let shown = filter_rows(&self.rows, "", CoresFilter::All)
            .into_iter()
            .map(|(index, row)| compact_row(index, row, probe_actions(row, index), layout, cx))
            .collect();
        section = section
            .child(show_all_el(true, summary.total, Rc::new(|_, _| {}), cx))
            .child(list_controls_el(
                &self.query,
                CoresFilter::All,
                summary,
                Rc::new(|_, _, _| {}),
                cx,
            ))
            .child(table_el(layout, shown, &self.scroll, cx));
        for el in push_els(
            self.push
                .map(|n| (n, |_: &ClickEvent, _: &mut Window, _: &mut App| {})),
            false,
            true,
            None,
            cx,
        ) {
            section = section.child(el);
        }
        div()
            .w(px(self.width))
            .debug_selector(|| "probe-col".into())
            .child(section)
    }
}

/// `debug_bounds` takes a `'static` selector; the probe's selectors are built per run.
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// Every action stays inside the panel, no address is clipped and the full list is capped, in
/// every language, at a wide and a narrow column and both zooms, the removal armed.
#[gpui::test]
fn the_cores_block_fits_every_language_width_and_zoom(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    for language in Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        for zoom in [1.0, 1.25] {
            for (column, push) in [
                (860.0, None),
                (860.0, Some(3)),
                (420.0, None),
                (420.0, Some(3)),
            ] {
                let width = column * zoom;
                let window =
                    cx.open_window(gpui::size(px(width), px(4000.0)), |window, cx| CoresProbe {
                        rows: rows(),
                        width,
                        push,
                        query: cx.new(|cx| MoonInputState::new(window, cx)),
                        scroll: ScrollHandle::new(),
                    });
                let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                visual.update(|window, cx| window.set_content_zoom(zoom, cx));
                for _ in 0..3 {
                    visual.update(|window, cx| {
                        let _ = window.draw(cx);
                    });
                }
                let at = format!(
                    "{} zoom {zoom} column {column} push {push:?}",
                    language.code()
                );
                let col = visual.debug_bounds("probe-col").expect("column laid out");
                let inside = |b: gpui::Bounds<gpui::Pixels>, what: &str| {
                    assert!(
                        b.origin.x + px(0.5) >= col.origin.x && b.right() <= col.right() + px(0.5),
                        "{what} escaped the panel ({at})"
                    );
                };
                for sel in [
                    "station-cores-summary",
                    "station-cores-show-all",
                    "station-cores-chips",
                    "station-cores-table",
                ] {
                    inside(visual.debug_bounds(sel).expect(sel), sel);
                }
                for (index, row) in rows().iter().enumerate().take(8) {
                    let address = row.address.clone().unwrap_or_default();
                    let need =
                        visual.update(|_, cx| design::mono_body_text_width(cx, &address, 400.0));
                    let mut sels = vec![
                        format!("station-core-addr-{index}"),
                        format!("station-core-act-{index}"),
                    ];
                    if row.state != RowState::Same {
                        sels.extend([
                            format!("station-core-card-{index}"),
                            format!("station-core-card-addr-{index}"),
                            format!("station-core-card-act-{index}"),
                        ]);
                    }
                    for sel in sels {
                        let b = visual
                            .debug_bounds(leak(sel.clone()))
                            .unwrap_or_else(|| panic!("{sel} not laid out ({at})"));
                        inside(b, &sel);
                        if sel.contains("addr") {
                            assert!(
                                f32::from(b.size.width) + 0.5 >= need,
                                "{sel} narrower than {address} ({at})"
                            );
                        }
                    }
                }
                let table = visual
                    .debug_bounds("station-cores-table")
                    .expect("table laid out");
                let cap = visual.update(|_, cx| design::ui_value(cx, TABLE_MAX_H));
                assert!(
                    f32::from(table.size.height) <= cap + 0.5,
                    "the full list grew past its cap ({at})"
                );
                let hint = visual
                    .debug_bounds("station-cores-sync-hint")
                    .expect("sync note laid out");
                inside(hint, "sync note");
                let gap = 10.0 * zoom + 1.0;
                match push {
                    None => {
                        assert!(
                            visual.debug_bounds("station-cores-send-all").is_none(),
                            "a push button with nothing to push ({at})"
                        );
                        assert!(
                            f32::from(hint.origin.y) - f32::from(table.bottom()) <= gap,
                            "a gap opened above the sync note ({at})"
                        );
                    }
                    Some(_) => {
                        let button = visual
                            .debug_bounds("station-cores-send-all")
                            .expect("push button laid out");
                        inside(button, "push button");
                        assert!(
                            f32::from(button.size.width) < f32::from(col.size.width) - 24.0,
                            "the push button stretched to the panel ({at})"
                        );
                        assert!(
                            button.origin.y + px(0.5) >= table.bottom()
                                && hint.origin.y + px(0.5) >= button.bottom(),
                            "the push button overlaps its neighbours ({at})"
                        );
                    }
                }
            }
        }
    }
}

/// A differing core draws its actions open and again in the full list, and rows without a station
/// uid would all be keyed 0: shared ids make GPUI route a click to the wrong button.
#[test]
fn core_action_ids_are_unique_per_row_and_surface() {
    let ids = [
        action_id("send", ActionSurface::Card, 0),
        action_id("send", ActionSurface::Row, 0),
        action_id("send", ActionSurface::Card, 1),
        action_id("remove", ActionSurface::Card, 0),
    ];
    for (i, a) in ids.iter().enumerate() {
        for b in &ids[i + 1..] {
            assert_ne!(a, b);
        }
    }
}
