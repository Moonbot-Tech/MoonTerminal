//! Headless reachability regression for expanded content inside the bounded Settings body.

use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, ScrollHandle, Styled, Window,
    div, px,
};
use moon_ui::{MoonGroupBox, v_flex};

/// Minimal window host; the test draws only the production scroll container.
struct ScrollFixture {
    scroll: ScrollHandle,
    viewport: f32,
    height: f32,
}

impl Render for ScrollFixture {
    /// Draw the production scroll container in a real headless view context.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut content = v_flex().w_full().gap(px(16.0));
        for id in ["bot", "access", "chats", "mini"] {
            content = content.child(
                MoonGroupBox::new(id)
                    .padding(14.0)
                    .gap(10.0)
                    .child(div().h(px(60.0))),
            );
        }
        let mut details = v_flex().gap(px(10.0));
        for _ in 0..(self.height / 30.0) as usize {
            details = details.child(v_flex().gap(px(8.0)).child(div().h(px(30.0))));
        }
        content = content.child(
            MoonGroupBox::new("core")
                .padding(14.0)
                .gap(10.0)
                .child(div().h(px(30.0)))
                .child(details)
                .child(div().debug_selector(|| "last-control".into()).h(px(30.0))),
        );
        div()
            .w(px(620.0))
            .h(px(self.viewport))
            .child(super::scrollable_tab_content(content, &self.scroll, cx))
    }
}

/// Constraining the content wrapper's height can exclude its overflowing final control from scrolling.
#[gpui::test]
fn expanded_content_increases_scroll_extent(cx: &mut gpui::TestAppContext) {
    let scroll = ScrollHandle::new();
    let window = cx.add_window(|_, _| ScrollFixture {
        scroll: scroll.clone(),
        viewport: 180.0,
        height: 100.0,
    });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    for viewport in [180.0, 360.0, 720.0] {
        for height in [100.0, 1200.0, 100.0] {
            window
                .update(&mut visual, |fixture, window, cx| {
                    fixture.viewport = viewport;
                    fixture.height = height;
                    window.refresh();
                    cx.notify();
                })
                .expect("headless scroll window remains open");
            visual.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let tail = visual
                .debug_bounds("last-control")
                .expect("final control was laid out");
            let reachable = scroll.bounds().bottom_right().y + scroll.max_offset().y;
            assert!(
                reachable >= tail.bottom_right().y,
                "final control lies outside the scroll extent"
            );
        }
    }
}

/// A narrowed tab column with wrapped text, in a window much wider than the column.
struct NarrowColumnFixture {
    scroll: ScrollHandle,
}

impl Render for NarrowColumnFixture {
    /// Draw a column the way the Telegram tab narrows its own: an explicit width.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let long = "A hint long enough to wrap onto several lines in a narrow settings column, \
                    and again, and again, and again, and again.";
        let mut section = MoonGroupBox::new("section").padding(14.0).gap(10.0);
        for _ in 0..10 {
            section = section.child(div().child(long));
        }
        let column = v_flex()
            .w(px(400.0))
            .gap(px(16.0))
            .child(div().child(long))
            .child(section.child(div().debug_selector(|| "last-control".into()).h(px(30.0))));
        div()
            .w(px(1440.0))
            .h(px(300.0))
            .child(super::scrollable_tab_content(column, &self.scroll, cx))
    }
}

/// A column narrowed by `w_full().max_w(..)` is measured with its text wrapped at the viewport's
/// width and loses its end below the scroll extent (the Telegram tab's Install button, 2026-09-30);
/// an explicit width keeps the whole column reachable.
#[gpui::test]
fn a_narrow_column_with_wrapped_text_stays_reachable(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    let scroll = ScrollHandle::new();
    let window = cx.add_window(|_, _| NarrowColumnFixture {
        scroll: scroll.clone(),
    });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let tail = visual
        .debug_bounds("last-control")
        .expect("final control was laid out");
    let reachable = scroll.bounds().bottom_right().y + scroll.max_offset().y;
    assert!(
        reachable >= tail.bottom_right().y,
        "final control lies outside the scroll extent"
    );
}

/// The Telegram tab narrows its column by an explicit width: a `max_w` there brings back the
/// Install button below the scroll extent (see the test above).
#[test]
fn the_telegram_tab_narrows_by_an_explicit_width() {
    let source = include_str!("../telegram.rs");
    assert!(
        !source.contains(".max_w(design::font_w_px"),
        "the Telegram tab's column is narrowed with max_w again"
    );
    assert!(source.contains(".w(px(column_w))"));
}
