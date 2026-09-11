//! Bind stored RGB values to MoonUI's stateful picker in otherwise stateless settings surfaces.

use std::rc::Rc;

use gpui::*;
use moon_ui::{MoonColorPicker, MoonColorPickerEvent, MoonColorPickerState};

use crate::design;

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
        let picker = cx.new(|cx| {
            MoonColorPickerState::new(window, cx).default_value(design::rgb_bytes_to_hsla(value))
        });
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
