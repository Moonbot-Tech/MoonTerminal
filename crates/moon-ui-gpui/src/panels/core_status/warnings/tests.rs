//! Duration text for an alarms row, and the cost of drawing a long alarms list.
//!
//! Explicit imports (no `use super::*`): the parent glob-imports `gpui`.

use std::cell::Cell;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::rc::Rc;

use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, div, point, px,
};
use moon_ui::{MoonDataTableState, MoonPalette, MoonVirtualListScrollHandle};

use super::super::layout_fixture;
use super::{alarms_table, dur_text, warn_row};
use crate::backend::core_warn::{WarnAxis, WarnEpisode, WarnSnapshot};

/// An open episode is the locale word alone. The closed arm is a different string.
#[test]
fn dur_text_ongoing_is_the_word_idjot() {
    let _locale = crate::test_locale::force("ru");
    let ongoing = dur_text(1_700_000_000_000, None, 1_700_000_240_000);
    assert_eq!(ongoing, "идёт");
    let closed = dur_text(
        1_700_000_000_000,
        Some(1_700_000_240_000),
        1_700_000_240_000,
    );
    assert_ne!(closed, ongoing);
}

/// Episodes in the alarms fixture: the live fleet's ~500-row list.
const EPISODES: usize = 500;
/// Cores the fixture spreads its episodes over, matching the 63-core fleet.
const CORES: u64 = 63;

/// Synthetic closed ping episodes on `10.0.0.x` addresses, newest first.
fn episodes() -> Vec<WarnEpisode> {
    (0..EPISODES)
        .map(|i| {
            let core = i as u64 % CORES + 1;
            let start_ms = 1_700_000_000_000 - i as i64 * 60_000;
            WarnEpisode {
                id: i as u64,
                axis: WarnAxis::Ping,
                server_ip: Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, (core % 10) as u8 + 1))),
                core_id: Some(core),
                start_ms,
                end_ms: Some(start_ms + 30_000),
                peak: 900,
                snap: WarnSnapshot::default(),
            }
        })
        .collect()
}

/// Test host that draws the alarms table the way the panel does and counts the work.
struct AlarmsHost {
    episodes: Rc<Vec<WarnEpisode>>,
    state: Entity<MoonDataTableState>,
    scroll: MoonVirtualListScrollHandle,
    /// `render` calls of this view.
    renders: Rc<Cell<usize>>,
    /// Rows the table asked for, across every render.
    rows_built: Rc<Cell<usize>>,
}

impl Render for AlarmsHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        let episodes = self.episodes.clone();
        let built = self.rows_built.clone();
        let names = Rc::new(HashMap::new());
        let cores = Rc::new(HashMap::new());
        div().size_full().flex().flex_col().child(alarms_table(
            "alarms-test",
            episodes.len(),
            move |ix, _window, app| {
                built.set(built.get() + 1);
                warn_row(
                    &episodes[ix],
                    &names,
                    &cores,
                    chrono_tz::UTC,
                    0,
                    MoonPalette::active(app),
                    app,
                )
            },
            &self.state,
            &self.scroll,
            window,
            cx,
        ))
    }
}

/// Open the alarms host in a 900 x 600 window and settle its first frames.
fn open_host(
    cx: &mut TestAppContext,
) -> (
    VisualTestContext,
    Rc<Cell<usize>>,
    Rc<Cell<usize>>,
    MoonVirtualListScrollHandle,
) {
    layout_fixture::install_dark(cx);
    let renders = Rc::new(Cell::new(0));
    let rows_built = Rc::new(Cell::new(0));
    let scroll = MoonVirtualListScrollHandle::new();
    let (r, b, s) = (renders.clone(), rows_built.clone(), scroll.clone());
    let mut visual = layout_fixture::open(cx, 900.0, 600.0, move |_window, cx| AlarmsHost {
        episodes: Rc::new(episodes()),
        state: cx.new(|_| MoonDataTableState::new()),
        scroll: s,
        renders: r,
        rows_built: b,
    });
    layout_fixture::draw(&mut visual);
    (visual, renders, rows_built, scroll)
}

/// `warnings.rs:alarms_table` must stay a virtualized table: a frame of the 500-row alarms list
/// builds only the rows on screen. Mutation: render the episodes as eager children (or wrap the
/// table in an unbounded-height scroll box) and every repaint, which a row hover or a scroll tick
/// triggers, builds all 500 rows again.
#[gpui::test]
fn alarms_frame_builds_only_the_visible_rows(cx: &mut TestAppContext) {
    let (mut visual, renders, rows_built, _scroll) = open_host(cx);
    let frames = renders.get().max(1);
    let per_frame = rows_built.get() / frames;
    // 600 px of window holds well under 60 rows at any shipped row height.
    assert!(
        per_frame < 60,
        "one frame built {per_frame} of {EPISODES} rows ({} rows over {frames} renders)",
        rows_built.get()
    );

    // A pointer sweeping down the rows repaints the host at each row change (GPUI notifies the
    // view that built a `.hover()` row), and each of those repaints stays bounded the same way.
    renders.set(0);
    rows_built.set(0);
    for step in 0..20 {
        visual.simulate_mouse_move(
            point(px(300.0), px(60.0 + step as f32 * 12.0)),
            None,
            gpui::Modifiers::default(),
        );
        visual.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
    let hover_renders = renders.get();
    let hover_rows = rows_built.get();
    assert!(
        hover_rows <= hover_renders.max(1) * 60,
        "a hover repaint built {hover_rows} rows over {hover_renders} renders"
    );
    eprintln!(
        "[MEASURE] first frames: {frames} renders; hover sweep: {hover_renders} renders, \
         {hover_rows} rows built"
    );
}

/// `warnings.rs:vertical_bar` must draw the Alarms list's vertical scrollbar whenever the list
/// overflows. Mutation: return `None`, switch to `Hover` visibility, or stop sharing the table's
/// scroll handle; the 500-row log goes back to showing no scrollbar.
#[gpui::test]
fn alarms_list_shows_a_vertical_scrollbar(cx: &mut TestAppContext) {
    let (mut visual, _renders, _rows, scroll) = open_host(cx);
    let max_y = f32::from(scroll.0.borrow().base_handle.max_offset().y);
    assert!(max_y > 0.0, "500 rows must overflow a 600 px window");
    // The track is the right edge of the body: a press there jumps the rows, which only a
    // drawn bar sharing the table's scroll handle can do.
    visual.simulate_mouse_down(
        point(px(895.0), px(450.0)),
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    layout_fixture::draw(&mut visual);
    let offset = f32::from(scroll.0.borrow().base_handle.offset().y);
    assert!(
        offset < 0.0,
        "a press on the alarms scrollbar track must scroll the rows"
    );
}
