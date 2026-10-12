//! A layout-probe host for the rebuilt pages' sweeps: draws one page body inside the body box,
//! scrolling, as `render.rs` places it.

use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, InteractiveElement, IntoElement, ParentElement, Render, ScrollHandle,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::settings::common::Action;
use crate::settings::layout_probe::Probed;

/// Draws whatever `build` returns inside the probe container `probe-body`.
pub(in crate::settings) struct Host {
    width: f32,
    height: f32,
    scroll: ScrollHandle,
    build: Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>,
}

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = (self.build)(window, cx);
        div()
            .w(px(self.width))
            .h(px(self.height))
            .debug_selector(|| "probe-body".into())
            .child(
                div()
                    .id("probe-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(div().w_full().flex_none().child(content)),
            )
    }
}

impl Probed for Host {
    fn scroll(&self) -> Option<ScrollHandle> {
        Some(self.scroll.clone())
    }
}

/// A sweep's `make` for a body built by `build`.
pub(in crate::settings) fn host(
    build: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
) -> impl Fn(&mut Window, &mut Context<Host>, (f32, f32)) -> Host {
    let build: Rc<dyn Fn(&mut Window, &mut App) -> AnyElement> = Rc::new(build);
    move |_, _, (width, height)| Host {
        width,
        height,
        scroll: ScrollHandle::new(),
        build: build.clone(),
    }
}

/// A fixed-size stand-in for a control, tagged `id` for the probe.
pub(in crate::settings) fn stub(id: &'static str, w: f32, h: f32) -> AnyElement {
    div()
        .w(px(w))
        .h(px(h))
        .flex_none()
        .debug_selector(move || id.to_string())
        .into_any_element()
}

/// A long-help toggle that does nothing, every row unfolded: the tallest layout.
pub(in crate::settings) fn all_open(_: &'static str) -> (bool, Action) {
    (true, Rc::new(|_, _| {}))
}
