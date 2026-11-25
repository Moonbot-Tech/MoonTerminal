//! Policy tests for the quit question on the last main window's close (#1170).

use super::{CloseRequest, closes_last_main_window, decide};

/// Catches the question escaping its one case. Off — the default — must never ask; a main window
/// that is not the last must not ask (its close does not quit); and the application's own teardown
/// must not stop on a dialog.
#[test]
fn asks_only_when_on_and_the_close_would_quit() {
    assert_eq!(decide(true, false, true), CloseRequest::Ask);

    assert_eq!(decide(false, false, true), CloseRequest::Close);
    assert_eq!(decide(true, false, false), CloseRequest::Close);
    assert_eq!(decide(true, true, true), CloseRequest::Close);
}

/// Catches the last-window rule drifting from the one `on_window_closed` quits on: only the sole
/// registered main window counts. With a second main window open the close does not quit, and a
/// window absent from the registry (a chart window, a window already unregistered) never quits.
#[test]
fn only_the_sole_registered_main_window_is_the_last() {
    assert!(closes_last_main_window([7_u64], 7));

    assert!(!closes_last_main_window([7_u64, 9], 7));
    assert!(!closes_last_main_window([9_u64, 7], 7));
    assert!(!closes_last_main_window([9_u64], 7));
    assert!(!closes_last_main_window(Vec::<u64>::new(), 7));
}

/// Catches the hook being dropped from the main window: the policy is worthless unless the window
/// `spawn_group_window_inner` opens actually installs it.
#[test]
fn every_main_window_installs_the_question() {
    let source = include_str!("../group_window.rs");
    assert!(
        source.contains("crate::window::quit_confirm::install(window, b.clone(), cx)"),
        "the group window must install the quit question as it opens"
    );
}
