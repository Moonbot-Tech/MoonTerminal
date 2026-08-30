//! Production wiring and GPUI drag-session regressions for strategy-tree confinement.
//! Explicit imports only: the parent re-exports `gpui::*`, whose `test` would shadow `#[test]`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::prelude::*;
use gpui::{
    Bounds, DragMoveEvent, MouseButton, Point, SharedString, TestAppContext, VisualTestContext,
    Window, WindowId, canvas, div, point, px, size,
};

use super::moon::{NodeData, drop_dest};
use super::ui::{DragChip, StratDrag, strat_drag_event_should_stop};

/// Compile-time source of the tree pane that owns `strat-tree-scroll`.
const SRC: &str = include_str!("mod.rs");

/// Removing the prepaint writer, the StratDrag move listener, or `stop_active_drag` would leave
/// the independent oracles green while a live drag continued across the Strategies window.
#[test]
fn tree_panel_confines_strat_drag_to_scroll_bounds() {
    let start = SRC
        .find(".id(\"strat-tree-scroll\")")
        .expect("the folders-and-strategies field must keep id strat-tree-scroll");
    let panel = &SRC[start..];
    assert!(
        panel.contains("bounds_cell.set(Some(bounds))"),
        "prepaint must write live strat-tree-scroll bounds into the shared cell"
    );
    assert!(
        panel.contains("DragMoveEvent<ui::StratDrag>"),
        "tree_panel must observe StratDrag moves, not only hide the chip"
    );
    assert!(
        panel.contains("strat_drag_event_should_stop("),
        "the move listener must consult the payload-origin cancel helper"
    );
    assert!(
        panel.contains("event.drag(cx)"),
        "event-time cancellation must read origin from the StratDrag payload"
    );
    assert!(
        panel.contains("stop_active_drag("),
        "leaving the live tree field must stop the GPUI drag session"
    );
}

/// Narrow GPUI harness that starts a StratDrag the same way production does: payload origin,
/// DragChip overlay, event-time cancel helper, and Core/Folder drop_dest routing.
struct StratDragHarness {
    tree_field: Rc<Cell<Option<Bounds<gpui::Pixels>>>>,
    seen_origin: Cell<Option<WindowId>>,
    seen_event: Cell<Option<WindowId>>,
    dropped: RefCell<Option<(u64, Vec<String>)>>,
}

impl StratDragHarness {
    /// Bind the harness to the shared live `strat-tree-scroll` bounds cell.
    fn new(tree_field: Rc<Cell<Option<Bounds<gpui::Pixels>>>>) -> Self {
        Self {
            tree_field,
            seen_origin: Cell::new(None),
            seen_event: Cell::new(None),
            dropped: RefCell::new(None),
        }
    }
}

/// Second native window with no StratDrag listener; a move here must clear process-global drag.
struct OutsideWindow;

impl Render for OutsideWindow {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

impl Render for StratDragHarness {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let origin_window = window.window_handle().window_id();
        let payload = StratDrag {
            core: 7,
            ids: vec![9],
            origin_window,
        };
        let tree_field = self.tree_field.clone();
        let core_dest = drop_dest(&NodeData::Core {
            core: 7,
            label: "core".into(),
            active: 0,
            total: 0,
            open_orders: 0,
            selected: false,
            checked: false,
        });
        let folder_dest = drop_dest(&NodeData::Folder {
            core: 7,
            path: vec!["desk".into(), "live".into()],
            label: "folder".into(),
            active: 0,
            total: 0,
            selected: false,
            checked: false,
        });
        div()
            .size_full()
            .child(
                div()
                    .id("strat-tree-scroll")
                    .absolute()
                    .w(px(200.0))
                    .h(px(200.0))
                    .on_drag(payload, move |drag, _pos, _window, app| {
                        let origin_window = drag.origin_window;
                        let tree_field = tree_field.clone();
                        app.new(move |_| DragChip {
                            label: SharedString::from("≡"),
                            step: 0.0,
                            origin_window,
                            tree_field,
                            stop_when_outside: true,
                        })
                    })
                    .on_drag_move(cx.listener(
                        |this, event: &DragMoveEvent<StratDrag>, window, cx| {
                            let drag = event.drag(cx);
                            this.seen_origin.set(Some(drag.origin_window));
                            this.seen_event
                                .set(Some(window.window_handle().window_id()));
                            if strat_drag_event_should_stop(
                                drag,
                                window.window_handle().window_id(),
                                event.event.position,
                                this.tree_field.get(),
                            ) {
                                cx.stop_active_drag(window);
                            }
                        },
                    )),
            )
            .child(
                div()
                    .id("drop-core")
                    .absolute()
                    .left(px(40.0))
                    .top(px(40.0))
                    .w(px(40.0))
                    .h(px(20.0))
                    .can_drop({
                        let core_dest = core_dest.clone();
                        move |drag, _, _| drag.is::<StratDrag>() && core_dest.is_some()
                    })
                    .on_drop::<StratDrag>(cx.listener(move |this, _drag, _, _| {
                        *this.dropped.borrow_mut() = core_dest.clone();
                    })),
            )
            .child(
                div()
                    .id("drop-folder")
                    .absolute()
                    .left(px(90.0))
                    .top(px(40.0))
                    .w(px(40.0))
                    .h(px(20.0))
                    .can_drop({
                        let folder_dest = folder_dest.clone();
                        move |drag, _, _| drag.is::<StratDrag>() && folder_dest.is_some()
                    })
                    .on_drop::<StratDrag>(cx.listener(move |this, _drag, _, _| {
                        *this.dropped.borrow_mut() = folder_dest.clone();
                    })),
            )
    }
}

/// Drive mouse-down plus threshold moves so capture-phase `on_drag_move` sees `App::active_drag`.
fn start_strat_drag(cx: &mut VisualTestContext) {
    let start = point(px(20.0), px(20.0));
    cx.simulate_mouse_down(start, MouseButton::Left, gpui::Modifiers::default());
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    // Capture-phase on_drag_move sees App::active_drag only after the bubble-phase
    // threshold move creates it, so a second interior sample is required.
    cx.simulate_mouse_move(
        point(px(35.0), px(20.0)),
        Some(MouseButton::Left),
        gpui::Modifiers::default(),
    );
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.simulate_mouse_move(
        point(px(50.0), px(50.0)),
        Some(MouseButton::Left),
        gpui::Modifiers::default(),
    );
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
}

/// Starts StratDrag in window A, keeps it through an interior move, then cancels it with one
/// move in a second native window that has no StratDrag listener (chart/outside).
#[gpui::test]
fn strat_drag_cancels_across_a_second_window(cx: &mut TestAppContext) {
    let tree_field = Rc::new(Cell::new(Some(Bounds::new(
        Point::default(),
        size(px(200.0), px(200.0)),
    ))));
    let window_a = cx.open_window(size(px(400.0), px(400.0)), {
        let tree_field = tree_field.clone();
        move |_, _cx| StratDragHarness::new(tree_field)
    });
    let window_b = cx.open_window(size(px(400.0), px(400.0)), |_, _cx| OutsideWindow);
    let mut cx_a = VisualTestContext::from_window(window_a.into(), cx);
    let mut cx_b = VisualTestContext::from_window(window_b.into(), cx);
    let origin_id = cx_a.update(|window, _| window.window_handle().window_id());
    let other_id = cx_b.update(|window, _| window.window_handle().window_id());
    assert_ne!(origin_id, other_id);

    start_strat_drag(&mut cx_a);
    assert!(
        cx_a.update(|_, cx| cx.has_active_drag()),
        "an interior move in the origin tree must keep StratDrag live"
    );
    let harness = window_a.root(&mut cx_a).expect("origin harness");
    harness.read_with(&cx_a, |view, _| {
        assert_eq!(
            view.seen_origin.get(),
            Some(origin_id),
            "event-time cancellation must observe the payload origin, not a dummy window"
        );
        assert_eq!(view.seen_event.get(), Some(origin_id));
    });

    cx_b.simulate_mouse_move(
        point(px(80.0), px(80.0)),
        Some(MouseButton::Left),
        gpui::Modifiers::default(),
    );
    assert!(
        !cx_a.update(|_, cx| cx.has_active_drag()),
        "one move in a second native window must clear the process-global StratDrag"
    );
}

/// Same-window interior survival plus a Core drop after a live StratDrag.
#[gpui::test]
fn strat_drag_survives_interior_and_drops_on_core(cx: &mut TestAppContext) {
    let tree_field = Rc::new(Cell::new(Some(Bounds::new(
        Point::default(),
        size(px(200.0), px(200.0)),
    ))));
    let window = cx.open_window(size(px(400.0), px(400.0)), {
        let tree_field = tree_field.clone();
        move |_, _cx| StratDragHarness::new(tree_field)
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    start_strat_drag(&mut cx);
    assert!(
        cx.update(|_, cx| cx.has_active_drag()),
        "same-window interior survival is required before a Core drop"
    );
    cx.simulate_mouse_up(
        point(px(50.0), px(50.0)),
        MouseButton::Left,
        gpui::Modifiers::default(),
    );
    let harness = window.root(&mut cx).expect("drop harness");
    harness.read_with(&cx, |view, _| {
        assert_eq!(
            view.dropped.borrow().clone(),
            Some((7, Vec::new())),
            "a live StratDrag must still route a Core drop"
        );
    });
}

/// Folder drop routing stays available on a live same-window StratDrag.
#[gpui::test]
fn strat_drag_survives_interior_and_drops_on_folder(cx: &mut TestAppContext) {
    let tree_field = Rc::new(Cell::new(Some(Bounds::new(
        Point::default(),
        size(px(200.0), px(200.0)),
    ))));
    let window = cx.open_window(size(px(400.0), px(400.0)), {
        let tree_field = tree_field.clone();
        move |_, _cx| StratDragHarness::new(tree_field)
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    start_strat_drag(&mut cx);
    cx.simulate_mouse_move(
        point(px(100.0), px(50.0)),
        Some(MouseButton::Left),
        gpui::Modifiers::default(),
    );
    assert!(
        cx.update(|_, cx| cx.has_active_drag()),
        "moving onto the folder target must not cancel the drag"
    );
    cx.simulate_mouse_up(
        point(px(100.0), px(50.0)),
        MouseButton::Left,
        gpui::Modifiers::default(),
    );
    let harness = window.root(&mut cx).expect("folder drop harness");
    harness.read_with(&cx, |view, _| {
        assert_eq!(
            view.dropped.borrow().clone(),
            Some((7, vec!["desk".into(), "live".into()])),
            "a live StratDrag must still route a Folder drop"
        );
    });
}

/// Fixed-size tree field containing one draggable folder row and the production DragChip preview.
struct FolderDragHarness {
    width: f32,
    height: f32,
    tree_field: Rc<Cell<Option<Bounds<gpui::Pixels>>>>,
}

impl Render for FolderDragHarness {
    /// Render the production boundary listener around a real typed GPUI drag source.
    fn render(&mut self, window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let origin_window = window.window_handle().window_id();
        let bounds_writer = self.tree_field.clone();
        let preview_bounds = self.tree_field.clone();
        super::constrain_folder_drag_to_tree(
            div()
                .w(px(self.width))
                .h(px(self.height))
                .relative()
                .debug_selector(|| "folder-drag-field".to_string()),
        )
        .child(
            canvas(
                move |bounds, _window, _app| bounds_writer.set(Some(bounds)),
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
        .child(
            div()
                .id("folder-drag-row")
                .w_full()
                .h(px(24.0))
                .debug_selector(|| "folder-drag-row".to_string())
                .on_drag(
                    super::ui::FolderDrag {
                        core: 7,
                        path: vec!["alpha".to_string()],
                    },
                    move |_drag, _offset, _window, cx| {
                        let tree_field = preview_bounds.clone();
                        cx.new(move |_| DragChip {
                            label: SharedString::from("≡"),
                            step: 0.0,
                            origin_window,
                            tree_field,
                            stop_when_outside: true,
                        })
                    },
                ),
        )
    }
}

/// Start a folder drag from the rendered row and prove it remains active inside the field.
fn start_folder_drag(cx: &mut VisualTestContext, row: Bounds<gpui::Pixels>) {
    let start = row.center();
    cx.simulate_mouse_down(start, MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_move(
        point(start.x + px(8.0), start.y),
        Some(MouseButton::Left),
        gpui::Modifiers::default(),
    );
    cx.update(|_, cx| {
        assert!(
            cx.has_active_drag(),
            "an over-threshold move inside the tree field must preserve the folder drag"
        );
    });
}

/// Removing the production `stop_active_drag` call must redden the first outside assertion because
/// the typed FolderDrag remains application-global after crossing the tree field boundary.
#[gpui::test]
fn folder_drag_stops_when_pointer_leaves_tree_field(cx: &mut TestAppContext) {
    for (width, height) in [(160.0, 120.0), (420.0, 260.0)] {
        let tree_field = Rc::new(Cell::new(None));
        let window = cx.add_window(move |_, _| FolderDragHarness {
            width,
            height,
            tree_field,
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.update(|window, _| window.refresh());
        visual.run_until_parked();

        let field = visual
            .debug_bounds("folder-drag-field")
            .expect("the rendered tree field must expose its bounds");
        let row = visual
            .debug_bounds("folder-drag-row")
            .expect("the rendered folder row must expose its bounds");
        let outside = [
            point(field.origin.x - px(1.0), field.center().y),
            point(field.right() + px(1.0), field.center().y),
            point(field.center().x, field.origin.y - px(1.0)),
            point(field.center().x, field.bottom() + px(1.0)),
        ];

        for position in outside {
            start_folder_drag(&mut visual, row);
            visual.simulate_mouse_move(
                position,
                Some(MouseButton::Left),
                gpui::Modifiers::default(),
            );
            visual.update(|_, cx| {
                assert!(
                    !cx.has_active_drag(),
                    "folder drag escaped field {field:?} at {position:?}"
                );
            });
            visual.simulate_mouse_up(position, MouseButton::Left, gpui::Modifiers::default());
        }
    }
}

/// A FolderDrag crossing into a second native window must hide its chip and clear active_drag.
#[gpui::test]
fn folder_drag_cancels_across_a_second_window(cx: &mut TestAppContext) {
    let tree_field = Rc::new(Cell::new(None));
    let window_a = cx.open_window(size(px(400.0), px(400.0)), {
        let tree_field = tree_field.clone();
        move |_, _| FolderDragHarness {
            width: 200.0,
            height: 200.0,
            tree_field,
        }
    });
    let window_b = cx.open_window(size(px(400.0), px(400.0)), |_, _| OutsideWindow);
    let mut cx_a = VisualTestContext::from_window(window_a.into(), cx);
    let mut cx_b = VisualTestContext::from_window(window_b.into(), cx);
    cx_a.update(|window, _| window.refresh());
    cx_a.run_until_parked();

    let row = cx_a
        .debug_bounds("folder-drag-row")
        .expect("the rendered folder row must expose its bounds");
    start_folder_drag(&mut cx_a, row);
    assert!(
        cx_a.update(|_, cx| cx.has_active_drag()),
        "an interior move in the origin tree must keep FolderDrag live"
    );

    cx_b.simulate_mouse_move(
        point(px(80.0), px(80.0)),
        Some(MouseButton::Left),
        gpui::Modifiers::default(),
    );
    assert!(
        !cx_a.update(|_, cx| cx.has_active_drag()),
        "one move in a second native window must clear the process-global FolderDrag"
    );
}
