//! Confirmation and layout invariants from the approved station cores mockup.
use super::{
    CoreCache, CoresFilter, CoresSummary, TABLE_MIN_H, action_id, card, compact_name_cell,
    compact_row, core_address_cell, cores_summary, diverging, filter_rows, group_label,
    list_controls_el, removable, removal_check, result_els, retained_arm, state_label, summary_el,
    table_el, table_fits, table_layout,
};
use crate::backend::station::cores_sync::{self, Counts, LocalCore, Row, RowState};
use crate::design;
use gpui::{
    AnyElement, AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, ScrollHandle, Styled, TextOverflow, WhiteSpace, Window, div, px,
};
use moon_core::config::{CoreGroup, Language};
use moon_core::station_api::ListedCore;
use moon_ui::{MoonButton, MoonInputState, MoonTone, h_flex};
use rust_i18n::t;
use std::rc::Rc;

/// The table truncates a long name to its column and keeps the whole name on hover.
#[test]
fn core_names_truncate_in_their_column() {
    let mut compact = compact_name_cell(0, &row(RowState::Same, 0));
    let style = compact.style();
    assert_eq!(style.text.white_space, Some(WhiteSpace::Nowrap));
    assert!(matches!(
        style.text.text_overflow,
        Some(TextOverflow::Truncate(_))
    ));
}

/// A core in two saved groups lists both; a station-only core or one in no group shows a dash.
#[test]
fn group_label_names_every_group_holding_the_core() {
    let groups = vec![
        CoreGroup {
            name: "main".into(),
            cores: vec![1, 2],
        },
        CoreGroup {
            name: "scalp".into(),
            cores: vec![2],
        },
    ];
    assert_eq!(group_label(&groups, Some(1)), "main");
    assert_eq!(group_label(&groups, Some(2)), "main, scalp");
    assert_eq!(group_label(&groups, Some(3)), "\u{2014}");
    assert_eq!(group_label(&groups, None), "\u{2014}");
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

/// A removal confirmed while the station is busy, or after its row vanished or changed identity,
/// never starts; an idle station with the same row runs it.
#[test]
fn a_confirmed_removal_rechecks_busy_and_the_row() {
    let listing = [3, 9].map(|uid| ListedCore {
        endpoint_override: None,
        uid,
        name: format!("Core {uid}"),
        address: Some(format!("198.51.100.{uid}:4510")),
        key_fp: None,
    });
    let rows = cores_sync::reconcile(&[], &listing, None);
    let arm = (9, listing[1].address.clone(), listing[1].name.clone());
    assert_eq!(removal_check(false, &arm, &rows), Ok(()));
    assert_eq!(
        removal_check(true, &arm, &rows),
        Err("telegram.mini_error_busy_station")
    );
    assert_eq!(
        removal_check(false, &arm, &rows[..1]),
        Err("telegram.server.cores_changed")
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
    assert!(table_fits(600.0, &[200.0, 100.0, 100.0, 150.0], 50.0));
    assert!(!table_fits(599.0, &[200.0, 100.0, 100.0, 150.0], 50.0));
    assert!(!table_fits(600.0, &[300.0, 100.0, 100.0, 150.0], 50.0));
    assert!(table_fits(700.0, &[300.0, 100.0, 100.0, 150.0], 50.0));
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

/// The production row actions' visual shape: the widest button each differing state draws.
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
        RowState::OnlyOnStation => actions.child(
            button(format!("rm-{index}"), "telegram.server.cores_remove")
                .danger()
                .render(),
        ),
        RowState::Same => actions,
    };
    actions.into_any_element()
}

/// Synthetic group labels: every third core in a group, the rest in none.
fn groups_of(rows: &[Row]) -> Vec<String> {
    let groups = vec![CoreGroup {
        name: "group-a".into(),
        cores: (1..=60).filter(|uid| uid % 3 == 0).collect(),
    }];
    rows.iter()
        .map(|row| group_label(&groups, row.terminal_uid))
        .collect()
}

/// The Station cores page, backend-free: a bounded body as `render` gives the page, the page
/// column, the cores card with its verdict, controls and table, and the groups card under it.
struct PageProbe {
    rows: Vec<Row>,
    filter: CoresFilter,
    width: f32,
    query: Entity<MoonInputState>,
    scroll: ScrollHandle,
}

impl Render for PageProbe {
    /// Lay the page out as `station_cores_block` and `st_cores_column` do.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = cores_summary(&self.rows);
        let counts = Counts::from_rows(&self.rows);
        let groups = groups_of(&self.rows);
        let layout = table_layout(
            &self.rows,
            &groups,
            self.width - design::ui_value(cx, 26.0),
            cx,
        );
        let shown = filter_rows(&self.rows, "", self.filter)
            .into_iter()
            .map(|(index, row)| {
                compact_row(
                    index,
                    row,
                    &groups[index],
                    probe_actions(row, index),
                    layout,
                    cx,
                )
            })
            .collect();
        let cores = card("probe-cores", cx)
            .flex_1()
            .min_h(px(0.0))
            .child(summary_el(summary, &counts, cx))
            .child(list_controls_el(
                &self.query,
                self.filter,
                summary,
                Rc::new(|_, _, _| {}),
                cx,
            ))
            .child(table_el(
                layout,
                shown,
                "telegram.server.cores_no_differ",
                &self.scroll,
                cx,
            ))
            .children(result_els(None, cx));
        let column = super::super::st_cores_column(
            self.width + design::ui_value(cx, 2.0 * crate::settings::render::BODY_PAD),
            cores.into_any_element(),
            Some(
                div()
                    .debug_selector(|| "probe-groups".into())
                    .child(t!("telegram.server.groups_same", names = "group-a").to_string())
                    .into_any_element(),
            ),
            cx,
        );
        div().size_full().flex().flex_col().child(
            div()
                .debug_selector(|| "probe-col".into())
                .flex_1()
                .min_h(px(0.0))
                .flex()
                .flex_col()
                .child(column),
        )
    }
}

/// Catches Station cores falling back to the scrolling page body, where the table could never
/// take the remaining height.
#[test]
fn station_cores_gets_a_bounded_body() {
    use crate::settings::Tab;
    assert!(crate::settings::render::bounded_body(Tab::StCores));
    assert!(!crate::settings::render::bounded_body(Tab::StTape));
}

/// `debug_bounds` takes a `'static` selector; the probe's selectors are built per run.
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// Lay out the page probe at `width` x `height` and zoom, drawn until the scroll settles.
fn open_page(
    cx: &mut gpui::TestAppContext,
    rows: Vec<Row>,
    filter: CoresFilter,
    (width, height, zoom): (f32, f32, f32),
) -> (gpui::VisualTestContext, ScrollHandle) {
    let scroll = ScrollHandle::new();
    let handle = scroll.clone();
    let window = cx.open_window(gpui::size(px(width), px(height)), |window, cx| PageProbe {
        rows,
        filter,
        width,
        query: cx.new(|cx| MoonInputState::new(window, cx)),
        scroll: handle,
    });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, cx| window.set_content_zoom(zoom, cx));
    for _ in 0..3 {
        visual.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
    (visual, scroll)
}

/// The table takes the page's remaining height and grows with the window, its sixty rows
/// scroll inside it, the footnote and the groups card stay on screen under it, and at a wide
/// column the cells of a row sit side by side without overlapping or clipping an address — in
/// every language, at both zooms, and at a narrow column where a row folds onto two lines.
#[gpui::test]
fn the_table_fills_the_page_and_scrolls_in_every_language(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    for language in Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        for zoom in [1.0, 1.25] {
            for column in [1100.0, 420.0] {
                let width = column * zoom;
                let at = format!("{} zoom {zoom} column {column}", language.code());
                let mut heights = Vec::new();
                for height in [640.0, 940.0] {
                    let (mut visual, scroll) =
                        open_page(cx, rows(), CoresFilter::All, (width, height * zoom, zoom));
                    let at = format!("{at} height {height}");
                    let col = visual
                        .debug_bounds("station-cores-column")
                        .expect("column laid out");
                    let inside = |b: gpui::Bounds<gpui::Pixels>, what: &str| {
                        assert!(
                            b.origin.x + px(0.5) >= col.origin.x
                                && b.right() <= col.right() + px(0.5),
                            "{what} escaped the page ({at})"
                        );
                    };
                    for sel in [
                        "station-cores-summary",
                        "station-cores-counts",
                        "station-cores-chips",
                        "station-cores-table-area",
                    ] {
                        inside(visual.debug_bounds(sel).expect(sel), sel);
                    }
                    let area = visual
                        .debug_bounds("station-cores-table-area")
                        .expect("table laid out");
                    heights.push(f32::from(area.size.height));
                    let min_h = visual.update(|_, cx| design::ui_value(cx, TABLE_MIN_H));
                    assert!(
                        f32::from(area.size.height) + 0.5 >= min_h,
                        "the table shrank under its floor ({at})"
                    );
                    assert!(
                        f32::from(scroll.max_offset().y) > 0.0,
                        "sixty rows did not scroll inside the table ({at})"
                    );
                    // The footnote and the groups card stay on the page, under the table.
                    let hint = visual
                        .debug_bounds("station-cores-sync-hint")
                        .expect("sync note laid out");
                    let groups = visual
                        .debug_bounds("probe-groups")
                        .expect("groups card laid out");
                    inside(hint, "sync note");
                    assert!(hint.origin.y + px(0.5) >= area.bottom(), "{at}");
                    assert!(
                        f32::from(groups.bottom()) <= height * zoom + 0.5,
                        "the groups card fell off the page ({at})"
                    );
                    let wide = visual.update(|_, cx| {
                        table_layout(&rows(), &groups_of(&rows()), width - 26.0 * zoom, cx).wide
                    });
                    if column > 1000.0 {
                        assert!(wide, "a wide page folded the table ({at})");
                        assert!(visual.debug_bounds("station-cores-head").is_some(), "{at}");
                    }
                    for (index, row) in rows().iter().enumerate().take(6) {
                        let address = row.address.clone().unwrap_or_default();
                        let need = visual
                            .update(|_, cx| design::mono_body_text_width(cx, &address, 400.0));
                        let cell = |kind: &str| {
                            let sel = format!("station-core-{kind}-{index}");
                            visual
                                .debug_bounds(leak(sel.clone()))
                                .unwrap_or_else(|| panic!("{sel} not laid out ({at})"))
                        };
                        let [name, addr, group, act] = ["name", "addr", "group", "act"].map(cell);
                        for (b, what) in [(name, "name"), (addr, "addr"), (group, "group")] {
                            inside(b, what);
                        }
                        if row.state != RowState::Same {
                            inside(act, "action");
                        }
                        assert!(
                            f32::from(addr.size.width) + 0.5 >= need,
                            "address {index} narrower than {address} ({at})"
                        );
                        if wide {
                            assert!(name.right() <= addr.origin.x + px(0.5), "{at}");
                            assert!(addr.right() <= group.origin.x + px(0.5), "{at}");
                            if row.state != RowState::Same {
                                assert!(group.right() <= act.origin.x + px(0.5), "{at}");
                            }
                        }
                    }
                }
                assert!(
                    heights[1] - heights[0] >= 290.0,
                    "the table did not grow with the window: {heights:?} ({at})"
                );
            }
        }
    }
}

/// "Расходятся 0": an empty filter result is a line inside a table that keeps the page's
/// height, never a collapsed table — in every language.
#[gpui::test]
fn an_empty_filter_lays_out_inside_the_table(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    let same: Vec<Row> = (4..20).map(|index| row(RowState::Same, index)).collect();
    for language in Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        for width in [1100.0, 420.0] {
            let (mut visual, _) =
                open_page(cx, same.clone(), CoresFilter::Differ, (width, 700.0, 1.0));
            let at = format!("{} column {width}", language.code());
            let area = visual
                .debug_bounds("station-cores-table-area")
                .expect("table laid out");
            let empty = visual
                .debug_bounds("station-cores-empty")
                .unwrap_or_else(|| panic!("empty line not laid out ({at})"));
            assert!(f32::from(empty.size.height) > 0.0, "{at}");
            assert!(
                empty.origin.x + px(0.5) >= area.origin.x
                    && empty.right() <= area.right() + px(0.5)
                    && empty.bottom() <= area.bottom() + px(0.5),
                "the empty line escaped the table ({at})"
            );
            assert!(
                f32::from(area.size.height) + 0.5
                    >= visual.update(|_, cx| design::ui_value(cx, TABLE_MIN_H)),
                "the table collapsed around the empty line ({at})"
            );
        }
    }
}

/// Rows without a station uid would all be keyed 0: shared ids make GPUI route a click to the
/// wrong button.
#[test]
fn core_action_ids_are_unique_per_row() {
    let ids = [
        action_id("send", 0),
        action_id("send", 1),
        action_id("remove", 0),
        action_id("remove", 1),
    ];
    for (i, a) in ids.iter().enumerate() {
        for b in &ids[i + 1..] {
            assert_ne!(a, b);
        }
    }
}

/// The page's count is the differing rows, and its one send bar never carries a station-only
/// core: those leave the station only through their own confirmed removal.
#[test]
fn the_send_bar_counts_differences_and_skips_station_only_cores() {
    let rows = rows();
    assert_eq!(diverging(&rows), 4);
    assert_eq!(diverging(&rows[4..]), 0);
    let station_only = rows[0].station_uid;
    assert!(
        cores_sync::bulk(&rows)
            .iter()
            .all(|upsert| Some(upsert.station_uid) != station_only)
    );
}

/// Catches the unread listing falling back to «nothing to show»: a running read is Loading,
/// a finished one without a listing is NotRead with the failure carried along.
#[test]
fn an_unread_listing_is_loading_while_a_job_runs() {
    use super::CoresNote;
    assert_eq!(CoresNote::unread(true, None), CoresNote::Loading);
    assert_eq!(
        CoresNote::unread(true, Some("x".into())),
        CoresNote::Loading
    );
    assert_eq!(CoresNote::unread(false, None), CoresNote::NotRead(None));
    assert_eq!(
        CoresNote::unread(false, Some("timeout".into())),
        CoresNote::NotRead(Some("timeout".into()))
    );
}

/// Catches the Station cores card going blank again (the live defect: a heading over nothing
/// while the station was online): the loading, the not-read and the empty notes each lay out a
/// visible text that fits its words, inside the body, in every language at every probe size.
#[gpui::test]
fn the_station_cores_card_is_never_blank(cx: &mut gpui::TestAppContext) {
    use super::{CoresNote, cores_note_el};
    use crate::settings::layout_probe::{ProbeSpec, sweep};
    use crate::settings::telegram::tests::Host;
    for note in [
        CoresNote::Loading,
        CoresNote::NotRead(Some("synthetic read failure".into())),
        CoresNote::Empty,
    ] {
        let retry_drawn = matches!(note, CoresNote::NotRead(_));
        sweep(
            cx,
            {
                let note = note.clone();
                move |_, _, (width, _)| {
                    let note = note.clone();
                    Host {
                        width,
                        scroll: gpui::ScrollHandle::new(),
                        build: std::rc::Rc::new(move |_, cx| {
                            cores_note_el(note.clone(), std::rc::Rc::new(|_, _| {}), cx)
                        }),
                    }
                }
            },
            move |_| ProbeSpec {
                container: "probe-body",
                inside: ["station-cores-note", "station-cores-note-text"]
                    .map(String::from)
                    .to_vec(),
                ..Default::default()
            },
        );
        // The probe only proves what is laid out fits; this proves something is laid out.
        let window = cx.open_window(gpui::size(gpui::px(900.0), gpui::px(700.0)), |_, _| Host {
            width: 860.0,
            scroll: gpui::ScrollHandle::new(),
            build: std::rc::Rc::new({
                let note = note.clone();
                move |_, cx| cores_note_el(note.clone(), std::rc::Rc::new(|_, _| {}), cx)
            }),
        });
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        visual.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let text = visual
            .debug_bounds("station-cores-note-text")
            .expect("note text laid out");
        assert!(
            f32::from(text.size.height) > 0.0 && f32::from(text.size.width) > 0.0,
            "{note:?}"
        );
        assert_eq!(
            visual.debug_bounds("station-cores-read").is_some(),
            retry_drawn,
            "{note:?}"
        );
    }
}

/// At a 1900 px window the Ядро column shows a 40-character name whole, in every language.
#[gpui::test]
fn a_long_core_name_fits_its_column_on_a_wide_page(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    let name = "CORE_A synthetic subaccount number 31 xx";
    assert_eq!(name.chars().count(), 40);
    let mut cores = rows();
    cores[4].name = name.to_string();
    for language in Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        let (mut visual, _) = open_page(cx, cores.clone(), CoresFilter::All, (1900.0, 900.0, 1.0));
        let cell = visual
            .debug_bounds("station-core-name-4")
            .expect("name cell laid out");
        let need = visual.update(|_, cx| design::mono_body_text_width(cx, name, 400.0));
        assert!(
            f32::from(cell.size.width) + 0.5 >= need,
            "a 40-character name truncated: {} < {need} ({})",
            f32::from(cell.size.width),
            language.code()
        );
    }
}
