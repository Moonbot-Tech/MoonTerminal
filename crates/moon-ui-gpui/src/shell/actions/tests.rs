//! Group ownership and real MoonUI replacement behavior for inline preset editors.

// NOT `use super::*`: the parent imports `gpui::*`, whose `test` macro shadows `#[test]`.
use super::{
    missing_sound_copy, select_hotkey_target, select_preset_editor_after_render, take_group_edit,
};
use gpui::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, Window, div, px,
};
use moon_ui::{MoonInput, MoonInputState};

/// Render the actual preset field only while its inline editor is open.
struct PresetEditorHost {
    input: Entity<MoonInputState>,
    open: bool,
    select_pending: bool,
}

impl Render for PresetEditorHost {
    /// Mount and unmount MoonUI's real field so opening cannot rely on an old dispatch tree.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.select_pending) {
            select_preset_editor_after_render(&self.input, window, cx);
        }
        div().w(px(100.0)).children(
            self.open
                .then(|| MoonInput::new("preset-editor-test").state(&self.input)),
        )
    }
}

/// Bridge each Shell producer to the render consumer exercised by the real-field tests below.
///
/// Constructing Shell would also construct Backend and its dock panels. These source checks cover
/// that wiring separately: removing either drain's request must fail its own editor regression.
fn assert_preset_selection_route(drain_name: &str, input_field: &str) {
    let drain = function_body(include_str!("../actions.rs"), drain_name);
    assert!(drain.contains(&format!("let input = self.{input_field}.clone();")));
    assert!(drain.contains("self.pending_preset_selection = Some(input.clone());"));

    let render = function_body(include_str!("../render.rs"), "render");
    assert!(render.contains("self.pending_preset_selection.take()"));
    assert!(render.contains("select_preset_editor_after_render(&input, window, cx)"));
}

/// Drive the shared selection path with real MoonUI text input, including a reopen.
fn assert_preset_editor_replaces_on_open(cx: &mut gpui::TestAppContext, values: [&str; 2]) {
    cx.update(|cx| {
        moon_ui::MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(
                moon_core::config::UiThemeMode::Light,
                1.0,
            ),
            cx,
        );
    });
    let mut host = None;
    let window = cx.open_window(gpui::size(px(300.0), px(100.0)), |window, cx| {
        let editor = cx.new(|cx| PresetEditorHost {
            input: cx.new(|cx| MoonInputState::new(window, cx)),
            open: false,
            select_pending: false,
        });
        host = Some(editor.clone());
        moon_ui::Root::new(editor, window, cx)
    });
    let host = host.unwrap();
    for value in values {
        cx.update_window(window.into(), |_, window, cx| {
            host.update(cx, |host, cx| {
                host.open = true;
                host.select_pending = true;
                host.input.update(cx, |state, cx| {
                    state.set_value(value.to_string(), window, cx);
                    state.focus(window, cx);
                });
                cx.notify();
            });
            let _ = window.draw(cx);
        })
        .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |_, _, cx| {
                assert_eq!(
                    host.read(cx).input.read(cx).selected_value().as_ref(),
                    value
                );
            })
            .unwrap();
        cx.simulate_input(window.into(), "2");
        window
            .update(cx, |_, _, cx| {
                assert_eq!(host.read(cx).input.read(cx).value().as_ref(), "2");
            })
            .unwrap();
        cx.simulate_input(window.into(), "3");
        cx.update_window(window.into(), |_, window, cx| {
            host.update(cx, |host, cx| {
                assert_eq!(host.input.read(cx).value().as_ref(), "23");
                host.open = false;
                cx.notify();
            });
            window.blur();
            let _ = window.draw(cx);
        })
        .unwrap();
        cx.run_until_parked();
    }
}

/// Guard the size drain's wiring by source and prove the shared field replaces 5000 with 2.
#[gpui::test]
fn size_preset_editor_replaces_the_number_on_every_open(cx: &mut gpui::TestAppContext) {
    assert_preset_selection_route("drain_order_size_edit_request", "size_input");
    assert_preset_editor_replaces_on_open(cx, ["5000", "125.5"]);
}

/// Guard the take drain's wiring by source and prove replacement of fractional and zero values.
#[gpui::test]
fn take_preset_editor_replaces_the_number_on_every_open(cx: &mut gpui::TestAppContext) {
    assert_preset_selection_route("drain_sell_edit_request", "sell_input");
    assert_preset_editor_replaces_on_open(cx, ["1.25", "0"]);
}

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
