//! Regression tests for Core Status defaults and server-name ordering.
//!
//! The warning detectors (sustained CPU, memory growth) moved to `backend::core_warn`; their tests
//! live beside that engine now.

use super::ordering::natural_cmp;
use super::{CoreStatusMode, mode_ctx_id};
use crate::test_support::{bench, fixtures};

/// Pair the headless model inputs with their configured names and synthetic feed status.
fn bench_status_rows() -> Vec<super::CoreStatusRow> {
    let fixture = fixtures::PanelFixture::new(200, 100, 0, 0, 0);
    let sample = super::layout_fixture::cores().remove(4);
    fixture
        .sessions
        .iter()
        .map(|session| {
            let mut row = sample.clone();
            row.id = session.id;
            row.name.clone_from(&session.name);
            row.status = fixture.store.core(session.id).unwrap().status.clone();
            row.endpoint.as_mut().unwrap().address =
                std::net::IpAddr::from([10, 0, 0, ((session.id - 1) / 10 + 1) as u8]);
            row
        })
        .collect()
}

/// Measure cold grouping on 200 status rows across 20 machines.
#[test]
#[ignore = "bench: S-smooth"]
fn bench_s_core_status_rebuild_200() {
    let rows = bench_status_rows();
    let names = Default::default();
    bench::measure(
        "core_status_rebuild_200_cold_server_aggregation",
        24,
        || {
            let mut groups = super::model::aggregate_servers(&rows, Some(770));
            super::ordering::assign_server_names(&mut groups, &names);
            assert_eq!(groups.len(), 20);
            assert_eq!(
                groups.iter().map(|group| group.cores.len()).sum::<usize>(),
                200
            );
            groups
        },
    );
}

/// Headless production server-card grid for a populated Core Status draw.
struct BenchStatusGrid(Vec<super::ServerStatusGroup>);

impl gpui::Render for BenchStatusGrid {
    /// Build the same card grid as the Servers view, with no live view to receive clicks.
    fn render(
        &mut self,
        _: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{ParentElement, Styled};
        let ordered = super::servers_view::card_order(&self.0);
        assert_eq!(
            ordered.iter().map(|group| group.cores.len()).sum::<usize>(),
            200
        );
        moon_ui::v_flex()
            .size_full()
            .child(super::servers_view::ordered_server_grid(
                ordered,
                false,
                None,
                None,
                None,
                None,
                &Default::default(),
                &Default::default(),
                1640.0,
                gpui::WeakEntity::new_invalid(),
                cx,
            ))
    }
}

/// Measure a populated repaint of the 200-core Servers grid.
#[test]
#[ignore = "bench: S-smooth"]
fn bench_s_core_status_draw_200() {
    fixtures::on_visual_stack(|| {
        let mut app = fixtures::visual_app();
        let rows = bench_status_rows();
        let groups = super::model::aggregate_servers(&rows, Some(770));
        let mut visual = super::layout_fixture::open(&mut app, 1640.0, 900.0, move |_, _| {
            BenchStatusGrid(groups)
        });
        bench::measure("core_status_draw_200_populated_servers_grid", 24, || {
            visual.update(|window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            });
        });
    });
}

/// `mode.rs:CoreStatusMode::default` must stay Flat. A fresh panel that opened on Servers would
/// hide the one-row table the toolbar calls Ядра.
#[test]
fn new_core_status_panels_default_to_flat() {
    assert_eq!(CoreStatusMode::default(), CoreStatusMode::Flat);
}

/// `mod.rs:mode_ctx_id` must preserve the dock/window suffix chosen by its argument. Hard-coding
/// the detached flag would let a dock tab silently overwrite the mode a detached Core Status window
/// restores.
#[test]
fn core_status_mode_contexts_are_distinct_for_dock_and_window() {
    assert_eq!(mode_ctx_id(false), "core-status-mode:dock");
    assert_eq!(mode_ctx_id(true), "core-status-mode:win");
    assert_ne!(mode_ctx_id(false), mode_ctx_id(true));
}

/// `mode.rs:CoreStatusMode::code/from_code` must keep the persisted vocabulary. Renaming the
/// Servers variant's code, or treating `"by-ip"` as unknown, would reopen a stored server view
/// on the Flat table. An unknown code follows the fresh-panel default, Flat.
#[test]
fn core_status_mode_codes_round_trip_and_unknown_codes_fall_back() {
    for (mode, code) in [
        (CoreStatusMode::Servers, "by-ip"),
        (CoreStatusMode::Flat, "flat"),
        (CoreStatusMode::Problems, "problems"),
        (CoreStatusMode::Warnings, "warnings"),
        (CoreStatusMode::Updates, "updates"),
    ] {
        assert_eq!(mode.code(), code);
        assert_eq!(CoreStatusMode::from_code(code), mode);
    }

    assert_eq!(CoreStatusMode::from_code(" flat "), CoreStatusMode::Flat);
    assert_eq!(
        CoreStatusMode::from_code(" by-ip "),
        CoreStatusMode::Servers
    );
    for code in ["", "garbage", "tree"] {
        assert_eq!(CoreStatusMode::from_code(code), CoreStatusMode::Flat);
    }
}

/// `CoreStatusView::new` must restore without calling `set_core_status_mode`, while it and
/// `interactions.rs:set_mode` both derive the key from their real detached state. A constructor
/// write would mark fresh layouts dirty, and a literal context flag would merge dock and window tabs.
#[test]
fn core_status_mode_restore_is_read_only_and_uses_each_real_context() {
    let panel_source = include_str!("mod.rs");
    let constructor = panel_source
        .split_once("fn new(")
        .expect("Core Status constructor must remain present")
        .1;
    let restore_start = constructor
        .find("let mode =")
        .expect("constructor must restore the Core Status mode");
    let restore_end = constructor[restore_start..]
        .find("let saved_widths")
        .expect("mode restoration must precede table-state setup")
        + restore_start;
    let restore = &constructor[restore_start..restore_end];
    assert!(restore.contains("&mode_ctx_id(detached)"));
    assert!(!restore.contains("set_core_status_mode"));
    assert!(!restore.contains("mode_ctx_id(false)"));
    assert!(!restore.contains("mode_ctx_id(true)"));

    let interactions_source = include_str!("interactions.rs");
    let set_mode = interactions_source
        .split_once("fn set_mode(")
        .expect("Core Status mode writer must remain present")
        .1;
    assert!(set_mode.contains("&super::mode_ctx_id(self.detached)"));
    assert!(!set_mode.contains("mode_ctx_id(false)"));
    assert!(!set_mode.contains("mode_ctx_id(true)"));
}

/// `ordering.rs:natural_cmp` sorts server names as a human reads them: numbers by value (so `Server 2`
/// precedes `Server 10`, not the lexical reverse) and custom names alphabetically.
#[test]
fn server_names_sort_naturally() {
    let mut names = [
        "Server 10",
        "Server 2",
        "QQ",
        "F1",
        "Server 1",
        "HLFutures2",
    ];
    names.sort_by(|a, b| natural_cmp(a, b));
    assert_eq!(
        names,
        [
            "F1",
            "HLFutures2",
            "QQ",
            "Server 1",
            "Server 2",
            "Server 10"
        ]
    );
}
