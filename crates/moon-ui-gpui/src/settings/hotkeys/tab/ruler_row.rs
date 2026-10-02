//! The percent ruler's row: which modifier turns a left drag over the chart plot into the ruler.

use gpui::*;
use moon_core::config::{HotkeysConfig, RulerDrag};
use moon_ui::{MoonButtonVariant, MoonDropdown, MoonMenuItem};
use rust_i18n::t;

use super::TableRow;
use crate::hotkeys::meta;
use crate::settings::SettingsView;
use crate::settings::hotkeys::clash::Clashes;

impl SettingsView {
    /// The ruler row, its modifier picked in the mouse cell like the label-scroll wheel's, and
    /// captioned with whatever answers its press first.
    pub(super) fn ruler_drag_row(
        &self,
        hotkeys: &HotkeysConfig,
        clashes: &Clashes,
        scope_w: Pixels,
        cx: &Context<Self>,
    ) -> AnyElement {
        self.table_row(
            TableRow {
                id: "ruler-drag".to_string(),
                title: t!("hotkeys.ruler_drag").to_string(),
                hint: Some(t!("hotkeys.ruler_drag_hint").to_string()),
                meta: Some(meta::RULER_DRAG),
                mono_title: false,
                muted: false,
                notes: clashes.ruler(hotkeys),
                key: None,
                mouse: Some(self.ruler_dropdown(hotkeys, cx).into_any_element()),
                param: None,
            },
            scope_w,
            cx,
        )
    }

    /// The modifier editor of the ruler row.
    fn ruler_dropdown(&self, hotkeys: &HotkeysConfig, cx: &App) -> MoonDropdown {
        let current = hotkeys.ruler_drag;
        let backend = self.backend.clone();
        let items = RulerDrag::ALL.into_iter().map(move |drag| {
            let backend = backend.clone();
            MoonMenuItem::with_key(drag.config_value(), ruler_label(drag))
                .checked(drag == current)
                .on_click(move |_, _, cx| {
                    backend.update(cx, |b, bcx| {
                        if let Some(p) = b.preview.as_mut()
                            && p.hotkeys.ruler_drag != drag
                        {
                            p.hotkeys.ruler_drag = drag;
                            bcx.notify();
                        }
                    });
                })
        });
        Self::row_dropdown("ruler-drag".to_string(), ruler_label(current), cx)
            .trigger_variant(if current == RulerDrag::None {
                MoonButtonVariant::Neutral
            } else {
                MoonButtonVariant::Blue
            })
            .menu_width_scaled(228.0)
            .items(items)
    }
}

/// The localized name of a ruler binding.
fn ruler_label(drag: RulerDrag) -> String {
    let key = format!("hotkeys.ruler.{}", drag.config_value());
    t!(&key).to_string()
}
