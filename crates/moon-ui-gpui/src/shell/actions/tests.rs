//! Group ownership contracts for shared inline-edit requests.

// NOT `use super::*`: the parent imports `gpui::*`, whose `test` macro shadows `#[test]`.
use super::{missing_sound_copy, select_hotkey_target, take_group_edit};

/// The body of one function, from its `fn` line up to the next item.
///
/// A sticky `.autohide(false)` elsewhere in the file (engine errors, strategy edits) must not
/// make this slice look sticky, and dropping the function name must fail the test rather than
/// scan the whole file.
fn function_body<'a>(source: &'a str, name: &str) -> &'a str {
    let marker = format!("fn {name}");
    let start = source
        .find(&marker)
        .unwrap_or_else(|| panic!("missing {name}"));
    let rest = &source[start..];
    let after = &rest[marker.len()..];
    let end = ["\nfn ", "\n    pub(super) fn ", "\n    /// "]
        .iter()
        .filter_map(|needle| after.find(needle))
        .min()
        .map(|offset| marker.len() + offset)
        .unwrap_or(rest.len());
    &rest[..end]
}

/// The missing-sound card used to stay until dismissed, so every custom Moonbot name stacked a
/// warning over the chart. Restoring `.autohide(false)` on the builder, or inlining a sticky
/// notification in the drain, brings that stack back. The words themselves are the locale
/// strings: a wrong key, or a body that drops the name, the folder or the fallback, leaves the
/// operator hearing ding1 with no way to tell which setting asked for it.
#[test]
fn the_missing_sound_toast_auto_hides_and_names_the_file() {
    let source = include_str!("../actions.rs");
    let drain = function_body(source, "drain_missing_sound_toasts");
    let built = function_body(source, "missing_sound_notification");
    assert!(
        drain.contains("missing_sound_notification("),
        "the drain must show the shared builder, not a second sticky card"
    );
    assert!(
        !drain.contains("autohide"),
        "the drain must not opt the toast out of auto-hide"
    );
    assert!(
        built.contains("MoonNotification::warning"),
        "a missing file is a warning, the same tone as before"
    );
    assert!(
        !built.contains("autohide"),
        "the builder must keep MoonNotification's default auto-hide"
    );

    let _locale = crate::test_locale::force("en");
    let (title, body) = missing_sound_copy(
        &crate::media::sound::MissingSound::Name("suetu-navesti-ohota".to_string()),
        "/sounds",
        "ding1",
    );
    assert_eq!(title, "Sound not found");
    assert_eq!(
        body,
        "Sound \"suetu-navesti-ohota\" was not found — ding1 plays instead. \
         Put suetu-navesti-ohota.wav into /sounds."
    );
    let (ordinal_title, ordinal_body) = missing_sound_copy(
        &crate::media::sound::MissingSound::Ordinal(23),
        "/sounds",
        "ding1",
    );
    assert_eq!(ordinal_title, "Sound not found");
    assert_eq!(
        ordinal_body,
        "Sound #23 does not exist — ding1 plays instead. \
         Put a file named 23_name.wav into /sounds, or pick the sound again in the core's settings."
    );
}

/// Regression target: replacing the group check with an unconditional `request.take()` lets the
/// first repainting window steal another group's editor, so the double-click appears to do nothing.
#[test]
fn another_group_cannot_consume_an_inline_edit_request() {
    let mut request = Some(("desk-a".to_string(), 3));

    assert_eq!(take_group_edit(&mut request, "desk-b"), None);
    assert_eq!(request, Some(("desk-a".to_string(), 3)));
    assert_eq!(
        take_group_edit(&mut request, "desk-a"),
        Some(("desk-a".to_string(), 3))
    );
    assert_eq!(request, None);
}

/// Regression target: removing the hovered preference routes a docked AddToChart hotkey to hidden
/// Main, so Cancel Buy or Panic Sell can execute on another core and market than the cursor shows.
#[test]
fn hovered_group_chart_wins_over_hidden_main_for_market_hotkeys() {
    let hovered = (7, "ETHUSDT".to_string());
    let main = (9, "BTCUSDT".to_string());

    assert_eq!(
        select_hotkey_target(Some(hovered.clone()), true, Some(main.clone())),
        Some(hovered)
    );
    assert_eq!(
        select_hotkey_target(Some((11, "SOLUSDT".to_string())), false, Some(main.clone())),
        Some(main)
    );
}
