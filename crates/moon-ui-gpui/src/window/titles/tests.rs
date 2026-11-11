//! Regressions for native window titles following a locale switch.

use gpui::{App, VisualTestContext};
use rust_i18n::t;

use super::{follow_locale, retitle, track};

/// A window to carry a title; it draws nothing.
struct Host;

impl gpui::Render for Host {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        gpui::div()
    }
}

/// Catches a locale switch leaving a native title in the language the window opened in, and the
/// switch being written at once: it is made from inside the switching window's own handler (Save
/// in Settings), where that window cannot be updated again, so only a deferred write renames it.
#[gpui::test]
fn a_locale_switch_renames_the_window_it_was_made_from(cx: &mut gpui::TestAppContext) {
    let _locale = crate::test_locale::force("ru");
    let title = |_: &App| t!("settings.window_title").to_string();
    let window = cx.add_window(|_, _| Host);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, cx| {
        window.set_window_title(&title(cx));
        track(window.window_handle(), false, title, cx);
    });
    let russian = visual.window_title();

    visual.update(|_, cx| {
        rust_i18n::set_locale("tr");
        follow_locale(cx);
    });
    let turkish = t!("settings.window_title").to_string();
    assert_ne!(
        russian.as_deref(),
        Some(turkish.as_str()),
        "needs two languages"
    );
    assert_eq!(visual.window_title().as_deref(), Some(turkish.as_str()));
}

/// Catches the registry keeping windows that have closed — opening and closing a tool window all
/// session would grow it — and a closed one stopping the switch for the windows left open.
#[gpui::test]
fn a_closed_window_leaves_the_registry(cx: &mut gpui::TestAppContext) {
    let _locale = crate::test_locale::force("ru");
    let title = |_: &App| t!("settings.window_title").to_string();
    let kept = cx.add_window(|_, _| Host);
    let closed = cx.add_window(|_, _| Host);
    // The closed one first, so a pass that stopped at it would leave `kept` unrenamed.
    cx.update(|cx| {
        track(closed.into(), false, title, cx);
        track(kept.into(), false, title, cx);
    });
    closed
        .update(cx, |_, window, _| window.remove_window())
        .expect("the window is open");

    cx.update(|cx| {
        rust_i18n::set_locale("tr");
        follow_locale(cx);
    });
    let tracked = cx.read(|cx| {
        cx.global::<super::Titles>()
            .0
            .iter()
            .map(|entry| entry.window)
            .collect::<Vec<_>>()
    });
    assert_eq!(tracked, vec![gpui::AnyWindowHandle::from(kept)]);
    let turkish = t!("settings.window_title").to_string();
    let mut visual = VisualTestContext::from_window(kept.into(), cx);
    assert_eq!(visual.window_title().as_deref(), Some(turkish.as_str()));
}

/// Retargeting a singleton must replace its title builder, or the next locale switch restores the
/// previous strategy's name.
#[gpui::test]
fn retitle_replaces_the_tracked_subject(cx: &mut gpui::TestAppContext) {
    let _locale = crate::test_locale::force("ru");
    let window = cx.add_window(|_, _| Host);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, cx| {
        track(
            window.window_handle(),
            false,
            |_: &App| t!("settings.window_title").to_string(),
            cx,
        );
        retitle(
            window.window_handle(),
            |_: &App| t!("strat.window_title").to_string(),
            cx,
        );
        rust_i18n::set_locale("tr");
        follow_locale(cx);
    });

    let expected = t!("strat.window_title").to_string();
    assert_eq!(visual.window_title().as_deref(), Some(expected.as_str()));
}
