//! Key-to-viewport mapping of the Screener table's keyboard scrolling, and the keys driven
//! through a real controlled `MoonDataTable`.

use gpui::{
    AppContext, Context, Entity, InteractiveElement, IntoElement, Modifiers, ParentElement, Render,
    Styled, VisualTestContext, Window, div, point, px, size,
};
use moon_ui::{
    MoonDataCell, MoonDataRow, MoonDataTable, MoonDataTableColumn, MoonDataTableState,
    MoonVirtualListScrollHandle,
};

use super::{ScrollTarget, Viewport, scroll_by_key, scroll_target};
use crate::design;
use crate::test_support::fixtures;

/// A 100-row table, 20 px rows, a viewport of 10.5 rows (210 px).
fn viewport_at(scroll_top: f32) -> Viewport {
    Viewport {
        scroll_top,
        height: 210.0,
        row_h: 20.0,
        row_count: 100,
    }
}

fn target(key: &str, scroll_top: f32) -> Option<ScrollTarget> {
    scroll_target(key, Modifiers::default(), viewport_at(scroll_top))
}

#[test]
fn arrows_move_one_row_from_a_row_boundary() {
    assert_eq!(target("down", 40.0), Some(ScrollTarget::TopRow(3)));
    assert_eq!(target("up", 40.0), Some(ScrollTarget::TopRow(1)));
}

/// An offset a float hair short of a boundary still counts as that boundary; otherwise Down
/// would re-target the row already at the top and the key would do nothing.
#[test]
fn an_offset_a_hair_off_a_boundary_still_steps() {
    assert_eq!(target("down", 39.9999), Some(ScrollTarget::TopRow(3)));
    assert_eq!(target("up", 40.0001), Some(ScrollTarget::TopRow(1)));
}

/// Mid-row (after a wheel scroll) each arrow goes to the nearest boundary in its direction.
#[test]
fn mid_row_arrows_go_to_the_next_boundary_without_skipping() {
    assert_eq!(target("down", 50.0), Some(ScrollTarget::TopRow(3)));
    assert_eq!(target("up", 50.0), Some(ScrollTarget::TopRow(2)));
}

/// A page is the whole rows that fit, never the partial one at the bottom.
#[test]
fn page_keys_move_by_whole_visible_rows() {
    assert_eq!(target("pagedown", 0.0), Some(ScrollTarget::TopRow(10)));
    assert_eq!(target("page_down", 0.0), Some(ScrollTarget::TopRow(10)));
    assert_eq!(target("pageup", 400.0), Some(ScrollTarget::TopRow(10)));
    assert_eq!(target("page_up", 400.0), Some(ScrollTarget::TopRow(10)));
}

#[test]
fn steps_stop_at_both_ends() {
    assert_eq!(target("up", 0.0), Some(ScrollTarget::TopRow(0)));
    assert_eq!(target("pageup", 60.0), Some(ScrollTarget::TopRow(0)));
    assert_eq!(target("pagedown", 1_990.0), Some(ScrollTarget::TopRow(99)));
}

#[test]
fn home_and_end_go_to_the_first_and_last_row() {
    assert_eq!(target("home", 800.0), Some(ScrollTarget::TopRow(0)));
    assert_eq!(target("end", 0.0), Some(ScrollTarget::Bottom));
}

/// A viewport shorter than one row still pages, by one row.
#[test]
fn a_viewport_under_one_row_pages_by_one() {
    let tiny = Viewport {
        height: 12.0,
        ..viewport_at(0.0)
    };
    assert_eq!(
        scroll_target("pagedown", Modifiers::default(), tiny),
        Some(ScrollTarget::TopRow(1))
    );
}

/// Chords belong to whoever binds them; Shift and the macOS function flag on arrows do not.
#[test]
fn command_chords_pass_through_and_shift_or_fn_do_not_block() {
    for held in [Modifiers::control(), Modifiers::alt(), Modifiers::command()] {
        assert_eq!(scroll_target("down", held, viewport_at(0.0)), None);
    }
    for held in [Modifiers::shift(), Modifiers::function()] {
        assert_eq!(
            scroll_target("down", held, viewport_at(0.0)),
            Some(ScrollTarget::TopRow(1))
        );
    }
}

#[test]
fn other_keys_and_an_empty_table_are_left_alone() {
    assert_eq!(target("left", 0.0), None);
    assert_eq!(target("a", 0.0), None);
    let empty = Viewport {
        row_count: 0,
        ..viewport_at(0.0)
    };
    assert_eq!(scroll_target("down", Modifiers::default(), empty), None);
}

/// Rows of the probe table: far more than its 300 px window shows.
const PROBE_ROWS: usize = 100;

/// The Screener's table arrangement in miniature: a controlled table inside a wrapper whose key
/// listener calls the same `scroll_by_key` the Screener calls.
struct ProbeHost {
    scroll: MoonVirtualListScrollHandle,
    state: Entity<MoonDataTableState>,
}

impl Render for ProbeHost {
    /// Lay out the wrapper and the controlled table over the shared scroll handle.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .on_key_down(cx.listener(|this: &mut Self, event, window, cx| {
                if scroll_by_key(&this.scroll, PROBE_ROWS, event, window, cx) {
                    cx.notify();
                }
            }))
            .child(
                MoonDataTable::new("keys-probe", PROBE_ROWS, |ix, _, _| {
                    MoonDataRow::new([MoonDataCell::text(format!("row {ix}"))])
                })
                .columns([MoonDataTableColumn::new("c", "C", 120.0)])
                .state(&self.state)
                .track_scroll(&self.scroll)
                .controlled_row_selection(true),
            )
    }
}

/// Distance in pixels from the first row to the top of the viewport.
fn scroll_top(scroll: &MoonVirtualListScrollHandle) -> f32 {
    -f32::from(scroll.0.borrow().base_handle.offset().y)
}

/// Draw one frame, so a deferred scroll is laid out.
fn draw(visual: &mut VisualTestContext) {
    visual.update(|window, cx| {
        window.refresh();
        let _ = window.draw(cx);
    });
}

/// After a click on a row, the navigation keys reach the wrapper past the controlled table and
/// move its viewport, while the table's own selection stays empty: the regression #1424's
/// controlled mode caused was exactly these keys doing nothing.
#[test]
fn keys_scroll_a_controlled_table_after_a_row_click_and_select_nothing() {
    fixtures::on_visual_stack(|| {
        let mut app = fixtures::visual_app();
        let scroll = MoonVirtualListScrollHandle::new();
        let state = app.new(|_| MoonDataTableState::default());
        let host = ProbeHost {
            scroll: scroll.clone(),
            state: state.clone(),
        };
        let window = app.open_window(size(px(400.0), px(300.0)), move |_, _| host);
        let mut visual = VisualTestContext::from_window(window.into(), &app);
        draw(&mut visual);
        let (head_h, row_h) =
            visual.update(|_, cx| (design::table_head_h(cx), design::table_row_h(cx)));
        visual.simulate_click(point(px(40.0), px(head_h + row_h * 1.5)), Modifiers::none());
        draw(&mut visual);
        assert_eq!(scroll_top(&scroll), 0.0, "a click alone must not scroll");

        visual.simulate_keystrokes("pagedown");
        draw(&mut visual);
        let paged = scroll_top(&scroll);
        assert!(paged >= row_h, "PageDown did not scroll: {paged}");
        let rows = paged / row_h;
        assert!(
            (rows - rows.round()).abs() < 1e-3,
            "a page is whole rows: {rows}"
        );

        visual.simulate_keystrokes("down");
        draw(&mut visual);
        assert!((scroll_top(&scroll) - (paged + row_h)).abs() < 1e-3);

        visual.simulate_keystrokes("up");
        draw(&mut visual);
        assert!((scroll_top(&scroll) - paged).abs() < 1e-3);

        visual.simulate_keystrokes("end");
        draw(&mut visual);
        assert_eq!(scroll.is_scrolled_to_end(), Some(true));

        visual.simulate_keystrokes("home");
        draw(&mut visual);
        assert_eq!(scroll_top(&scroll), 0.0);

        visual.update(|_, cx| {
            let table = state.read(cx);
            assert_eq!(table.selected_row, None, "keys must not select a row");
        });
    });
}
