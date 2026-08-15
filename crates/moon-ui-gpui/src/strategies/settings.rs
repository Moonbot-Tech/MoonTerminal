//! Persisted Strategies display preferences and their settings popover.
//!
//! The popup owns only process-lifetime open state. Each checkbox reads resolved state from the
//! view and writes its own optional `layout.toml` key through the shared layout coordinator.

use gpui::*;
use moon_core::config::layout::WindowLayout;
use moon_ui::{
    MoonCheckbox, MoonCheckboxSize, MoonGroupBox, MoonPalette, MoonPopover, MoonPopoverPlacement,
    MoonTheme, h_flex, v_flex,
};
use rust_i18n::t;

use super::StrategiesView;
use crate::design;
use crate::panels::{
    popup_close_button, popup_gear_trigger, popup_group, popup_group_inset_px, popup_title,
};

#[cfg(test)]
mod tests;

/// Compact MoonCheckbox mark width in design units.
const COMPACT_CHECKBOX_MARK: f32 = 12.0;
/// Compact MoonCheckbox gap between its mark and label in design units.
const COMPACT_CHECKBOX_GAP: f32 = 6.0;
/// Compact MoonCheckbox label size before font scaling.
const COMPACT_CHECKBOX_FONT: f32 = 9.5;
/// MoonGroupBox caption size before font scaling.
const GROUP_CAPTION_FONT: f32 = 10.5;

/// Resolved display preferences of the Strategies window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct StrategiesPrefs {
    /// Whether core roots are grouped under exchange headings.
    pub(super) group_by_venue: bool,
    /// Whether unchecked live strategies are hidden from the tree.
    pub(super) active_only: bool,
}

impl Default for StrategiesPrefs {
    /// Return the defaults applied when neither optional layout key has been written.
    ///
    /// Grouping preserves the established exchange hierarchy, while active-only stays off so an
    /// upgrade does not silently hide strategies that were visible before the preference existed.
    fn default() -> Self {
        Self {
            group_by_venue: true,
            active_only: false,
        }
    }
}

impl StrategiesPrefs {
    /// Resolve saved optional values over the shipped defaults.
    ///
    /// Args:
    ///     layout: Persisted window layout.
    ///
    /// Returns:
    ///     Saved values, with each absent key retaining its own default.
    pub(super) fn restore(layout: &WindowLayout) -> Self {
        let mut prefs = Self::default();
        for row in &PREF_ROWS {
            if let Some(saved) = (row.saved)(layout) {
                (row.set)(&mut prefs, saved);
            }
        }
        prefs
    }
}

/// One editable preference and the single layout key that persists it.
struct PrefRow {
    /// Stable element-identity suffix within the popup.
    id: &'static str,
    /// Locale key of the group caption this row belongs under.
    group: &'static str,
    /// Locale key of the visible checkbox label.
    label: &'static str,
    /// Read the resolved value from the view-owned preferences.
    read: fn(&StrategiesPrefs) -> bool,
    /// Apply a value to the view-owned preferences.
    set: fn(&mut StrategiesPrefs, bool),
    /// Read this preference's optional persisted value.
    saved: fn(&WindowLayout) -> Option<bool>,
    /// Write only this preference's optional persisted value.
    store: fn(&mut WindowLayout, bool),
}

/// Caption shared by both Strategies display preferences.
const DISPLAY_GROUP: &str = "strat.settings.display";

/// Strategies preferences in the order rendered by the popup.
const PREF_ROWS: [PrefRow; 2] = [
    PrefRow {
        id: "group-by-venue",
        group: DISPLAY_GROUP,
        label: "strat.settings.group_by_venue",
        read: |prefs| prefs.group_by_venue,
        set: |prefs, value| prefs.group_by_venue = value,
        saved: |layout| layout.strategies_group_by_venue,
        store: |layout, value| layout.strategies_group_by_venue = Some(value),
    },
    PrefRow {
        id: "active-only",
        group: DISPLAY_GROUP,
        label: "strat.settings.active_only",
        read: |prefs| prefs.active_only,
        set: |prefs, value| prefs.active_only = value,
        saved: |layout| layout.strategies_active_only,
        store: |layout, value| layout.strategies_active_only = Some(value),
    },
];

impl StrategiesView {
    /// Apply and persist one Strategies display preference.
    ///
    /// Args:
    ///     row: Preference metadata that owns the resolved and persisted fields.
    ///     value: New resolved value.
    ///     cx: View context used to update shared layout state and repaint.
    ///
    /// Returns:
    ///     Nothing; an unchanged value exits without touching layout state.
    fn write_pref(&mut self, row: &PrefRow, value: bool, cx: &mut Context<Self>) {
        if (row.read)(&self.prefs) == value {
            return;
        }
        (row.set)(&mut self.prefs, value);
        self.filter.active_only = self.prefs.active_only;
        let store = row.store;
        self.backend.update(cx, |backend, _| {
            store(&mut backend.layout, value);
            backend.layout_dirty = true;
        });
        cx.notify();
    }

    /// Disable active-only through the same persisted setter used by the popup.
    ///
    /// Args:
    ///     cx: View context used to persist and repaint when the preference changes.
    ///
    /// Returns:
    ///     Nothing; the shared setter handles the already-disabled case.
    pub(super) fn disable_active_only_for_reveal(&mut self, cx: &mut Context<Self>) {
        self.write_pref(&PREF_ROWS[1], false, cx);
    }

    /// Close the settings popup when it is currently open.
    ///
    /// Args:
    ///     cx: View context used to repaint after the state change.
    ///
    /// Returns:
    ///     Nothing; an already-closed popup exits without repainting.
    fn close_settings(&mut self, cx: &mut Context<Self>) {
        if !self.settings_open {
            return;
        }
        self.settings_open = false;
        cx.notify();
    }

    /// Build the settings popover anchored to its gear trigger.
    ///
    /// Args:
    ///     trigger: Button the popover anchors to.
    ///     palette: Active MoonUI palette.
    ///     cx: View context used to read preferences and wire callbacks.
    ///
    /// Returns:
    ///     The trigger with its controlled popover attached.
    pub(super) fn settings_popover(
        &self,
        trigger: impl IntoElement,
        palette: MoonPalette,
        cx: &Context<Self>,
    ) -> MoonPopover {
        let view = cx.entity();
        let mut popover = MoonPopover::new("strategies-settings-popover")
            .placement(MoonPopoverPlacement::BottomEnd)
            .close_on_content_click(false)
            .open(self.settings_open)
            .on_open_change({
                let view = view.clone();
                move |open, _window, app| {
                    view.update(app, |this, cx| {
                        this.settings_open = open;
                        cx.notify();
                    });
                }
            })
            .trigger(trigger);
        if !self.settings_open {
            return popover;
        }
        popover = popover
            .content_width(settings_content_width(cx))
            .content(settings_content(self.prefs, view, palette, cx));
        popover
    }
}

/// Measure the current locale's popup content at the metrics used to render it.
///
/// MoonPopover adds its own padding and border to this content width. The group-frame inset and
/// compact checkbox mark/gap are included here because they sit inside that content box.
///
/// Args:
///     cx: Application context providing locale, typography, and UI scaling.
///
/// Returns:
///     Rendered content-box width that keeps every title, caption, and checkbox label visible.
fn settings_content_width(cx: &App) -> f32 {
    let tokens = MoonTheme::active_tokens(cx);
    let title = t!("strat.settings.title");
    let group = t!(DISPLAY_GROUP);
    let title_width =
        design::ui_text_width(cx, &title, tokens.typography.mono_font_size, 400.0, true);
    let group_width = design::ui_text_width(cx, &group, GROUP_CAPTION_FONT, 600.0, true);
    let checkbox_label_width = PREF_ROWS
        .iter()
        .map(|row| design::ui_text_width(cx, &t!(row.label), COMPACT_CHECKBOX_FONT, 400.0, false))
        .fold(0.0_f32, f32::max);
    let checkbox_leading = f32::from(design::ui_px(
        cx,
        COMPACT_CHECKBOX_MARK + COMPACT_CHECKBOX_GAP,
    ));
    let group_content = group_width.max(checkbox_leading + checkbox_label_width);
    title_width.max(group_content + popup_group_inset_px(cx))
}

/// Build the shared gear trigger for the Strategies settings popup.
///
/// Args:
///     open: Whether the popup is currently showing.
///
/// Returns:
///     A shared icon-only settings trigger with localized tooltip text.
pub(super) fn settings_trigger(open: bool) -> impl IntoElement {
    popup_gear_trigger(
        "strategies-settings",
        t!("strat.settings.title").to_string(),
        open,
    )
}

/// Render the popup body from the current resolved preferences.
///
/// Args:
///     prefs: Values displayed by the checkboxes.
///     view: Strategies entity receiving edits.
///     palette: Active MoonUI palette.
///     cx: Application context supplying scaled geometry.
///
/// Returns:
///     Title row and the display-preferences group.
fn settings_content(
    prefs: StrategiesPrefs,
    view: Entity<StrategiesView>,
    palette: MoonPalette,
    cx: &App,
) -> AnyElement {
    // MoonPopover owns the surface chrome; this root supplies only content and spacing.
    v_flex()
        .id("strategies-settings-popup")
        .w_full()
        .gap(design::ui_px(cx, 8.0))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .child(popup_title(t!("strat.settings.title"), palette, cx))
                .child(popup_close_button("strategies-settings-close", {
                    let view = view.clone();
                    move |_, _window, app: &mut App| {
                        view.update(app, |this, cx| this.close_settings(cx));
                    }
                })),
        )
        .child(pref_group(
            "strategies-display",
            DISPLAY_GROUP,
            prefs,
            &view,
        ))
        .into_any_element()
}

/// Build one captioned group from the matching rows in [`PREF_ROWS`].
///
/// Args:
///     id: Stable group-box identity.
///     caption: Locale key shared by the group's rows.
///     prefs: Values displayed by the checkboxes.
///     view: Strategies entity receiving edits.
///
/// Returns:
///     Compact checkbox rows in preference-table order.
fn pref_group(
    id: &'static str,
    caption: &'static str,
    prefs: StrategiesPrefs,
    view: &Entity<StrategiesView>,
) -> MoonGroupBox {
    popup_group(id, t!(caption).to_string()).children(
        PREF_ROWS
            .iter()
            .enumerate()
            .filter(|(_, row)| row.group == caption)
            .map(|(index, row)| {
                let target = view.clone();
                MoonCheckbox::new(SharedString::from(format!("strategies-pref-{}", row.id)))
                    .label(t!(row.label).to_string())
                    .checked((row.read)(&prefs))
                    .size(MoonCheckboxSize::Compact)
                    .on_change(move |checked: &bool, _window, app| {
                        let checked = *checked;
                        target.update(app, |this, cx| {
                            this.write_pref(&PREF_ROWS[index], checked, cx)
                        });
                    })
                    .into_any_element()
            }),
    )
}
