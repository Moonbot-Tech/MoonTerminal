//! Shared custom color history and RGB bindings for every terminal MoonUI picker.

use std::rc::Rc;

use gpui::*;
use moon_core::config::custom_colors::CustomColors;
use moon_ui::{MoonColorPicker, MoonColorPickerEvent, MoonColorPickerState};

use crate::design;

/// One history entity per application, shared across windows and independent of Settings drafts.
struct GlobalCustomColorHistory(Entity<CustomColorHistory>);

impl Global for GlobalCustomColorHistory {}

/// Live history with writes disabled when initialization could not safely read the profile.
struct CustomColorHistory {
    colors: CustomColors,
    persistence_enabled: bool,
}

/// Convert persisted RGB entries into MoonUI's most-recent-first seed list.
fn custom_colors_seed(colors: &CustomColors) -> Vec<Hsla> {
    colors
        .custom_colors
        .iter()
        .map(|color| design::rgb_bytes_to_hsla(*color))
        .collect()
}

/// Build a picker bound to app-global custom history without coupling its value to other fields.
/// Weak entity-context bindings update only swatches and leave the picker and HEX input intact.
pub(crate) fn shared_color_state(
    value: [u8; 3],
    window: &mut Window,
    cx: &mut App,
) -> Entity<MoonColorPickerState> {
    let history = if let Some(global) = cx.try_global::<GlobalCustomColorHistory>() {
        global.0.clone()
    } else {
        let (colors, persistence_enabled) = match CustomColors::load() {
            Ok(colors) => (colors, true),
            Err(error) => {
                log::warn!(
                    "custom_colors.json initialization failed; persistence disabled: {}",
                    format!("{error:#}").escape_default()
                );
                (CustomColors::default(), false)
            }
        };
        let history = cx.new(|_| CustomColorHistory {
            colors,
            persistence_enabled,
        });
        cx.set_global(GlobalCustomColorHistory(history.clone()));
        history
    };
    let seed = custom_colors_seed(&history.read(cx).colors);
    let picker = cx.new(|cx| {
        cx.observe(
            &history,
            |picker: &mut MoonColorPickerState, history, cx| {
                picker.set_custom_colors(custom_colors_seed(&history.read(cx).colors), cx);
            },
        )
        .detach();
        MoonColorPickerState::new(window, cx)
            .default_value(design::rgb_bytes_to_hsla(value))
            .custom_colors(seed)
    });
    history.update(cx, |_, cx| {
        cx.subscribe(&picker, |history, _, event: &MoonColorPickerEvent, cx| {
            let MoonColorPickerEvent::CustomAdded(color) = event else {
                return;
            };
            history
                .colors
                .remember_custom_color(design::hsla_to_rgb8(*color));
            // Retry persistence even when the committed color was already at the front.
            if history.persistence_enabled
                && let Err(error) = history.colors.save()
            {
                log::warn!(
                    "custom_colors.json save failed: {}",
                    format!("{error:#}").escape_default()
                );
            }
            cx.notify();
        })
        .detach();
    });
    picker
}

/// The host's existing write door, refreshed whenever its control is rendered.
type ColorChange = Rc<dyn Fn([u8; 3], &mut App)>;

/// Window-local picker state and its subscription; dropping the control releases both.
struct Binding {
    picker: Entity<MoonColorPickerState>,
    on_change: ColorChange,
    _subscription: Subscription,
}

impl Binding {
    /// Seed without emitting a write; external changes recreate the picker because MoonUI's
    /// value setter is private. Changes made by this picker retain its focus and custom colors.
    fn new(
        value: [u8; 3],
        on_change: ColorChange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let picker = shared_color_state(value, window, cx);
        let subscription = cx.subscribe(&picker, |this, _, event, cx| {
            if let MoonColorPickerEvent::Change(color) = event {
                let on_change = this.on_change.clone();
                on_change(design::hsla_to_rgb8(*color), cx);
                cx.notify();
            }
        });
        Self {
            picker,
            on_change,
            _subscription: subscription,
        }
    }
}

/// A MoonUI picker whose RGB value and write callback come from the current host snapshot.
#[derive(IntoElement)]
pub(crate) struct ColorPicker {
    id: SharedString,
    value: [u8; 3],
    on_change: ColorChange,
}

impl ColorPicker {
    /// Keep field meaning and persistence in the caller; this adapter only owns widget state.
    pub(crate) fn new(
        id: impl Into<SharedString>,
        value: [u8; 3],
        on_change: impl Fn([u8; 3], &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            value,
            on_change: Rc::new(on_change),
        }
    }
}

impl RenderOnce for ColorPicker {
    /// Refresh externally changed values and callbacks without rebuilding on ordinary repaints.
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let binding = window.use_keyed_state(self.id.clone(), cx, |window, cx| {
            Binding::new(self.value, self.on_change.clone(), window, cx)
        });
        binding.update(cx, |state, cx| {
            if design::hsla_to_rgb8(state.picker.read(cx).value()) != self.value {
                *state = Binding::new(self.value, self.on_change.clone(), window, cx);
            } else {
                state.on_change = self.on_change;
            }
        });
        MoonColorPicker::new(&binding.read(cx).picker)
            .id(self.id)
            .colors(design::picker_palette())
    }
}
