//! Persisted Strategies display preferences and the controls that edit them.
//!
//! Most rows live in the settings popover, which owns only process-lifetime open state; active-only
//! is edited from the filter row instead, because it is a filter the user flips while reading the
//! tree. Both surfaces are built here so one preference keeps one authority: each checkbox reads
//! resolved state from the view and writes its own optional `layout.toml` key through the shared
//! layout coordinator.

use gpui::*;
use moon_core::config::layout::WindowLayout;
use moon_ui::{
    MoonCheckbox, MoonCheckboxSize, MoonGroupBox, MoonPalette, MoonPopover, MoonPopoverPlacement,
    MoonTheme, h_flex, v_flex,
};
use rust_i18n::t;

use super::StrategiesView;
use crate::design::{self, moon};
use crate::panels::{
    COMPACT_CHECKBOX_FONT, COMPACT_CHECKBOX_GAP, COMPACT_CHECKBOX_MARK, POPUP_GROUP_CAPTION_FONT,
    popup_close_button, popup_gear_trigger, popup_group, popup_group_inset_px, popup_title,
};

#[cfg(test)]
mod tests;

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
    /// Locale key of the descriptive checkbox label.
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

/// Caption of the popup's display-preferences group.
const DISPLAY_GROUP: &str = "strat.settings.display";

/// Group core roots under exchange headings.
const GROUP_BY_VENUE: PrefRow = PrefRow {
    id: "group-by-venue",
    group: DISPLAY_GROUP,
    label: "strat.settings.group_by_venue",
    read: |prefs| prefs.group_by_venue,
    set: |prefs, value| prefs.group_by_venue = value,
    saved: |layout| layout.strategies_group_by_venue,
    store: |layout, value| layout.strategies_group_by_venue = Some(value),
};

/// Hide unchecked live strategies from the tree.
///
/// Edited from the filter row rather than the popup — it is a filter the user flips while reading
/// the tree — and named rather than indexed, so the writers that reach it cannot be repointed at
/// another preference by reordering a table.
const ACTIVE_ONLY: PrefRow = PrefRow {
    id: "active-only",
    group: DISPLAY_GROUP,
    label: "strat.settings.active_only",
    read: |prefs| prefs.active_only,
    set: |prefs, value| prefs.active_only = value,
    saved: |layout| layout.strategies_active_only,
    store: |layout, value| layout.strategies_active_only = Some(value),
};

/// Every preference, in the order `restore` resolves them. Persistence covers all of them wherever
/// their control lives.
const PREF_ROWS: [&PrefRow; 2] = [&GROUP_BY_VENUE, &ACTIVE_ONLY];

/// The subset the settings popup renders, in display order.
const POPUP_ROWS: [&PrefRow; 1] = [&GROUP_BY_VENUE];

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

    /// Apply and persist active-only from the filter-row toggle.
    ///
    /// Args:
    ///     value: New resolved value.
    ///     cx: View context used to persist and repaint when the preference changes.
    ///
    /// Returns:
    ///     Nothing; the shared setter ignores an unchanged value.
    fn set_active_only(&mut self, value: bool, cx: &mut Context<Self>) {
        self.write_pref(&ACTIVE_ONLY, value, cx);
    }

    /// Build the filter-row active-only toggle: its label, then its compact checkbox.
    ///
    /// The label sits left of the mark because the row reads left to right into the settings gear
    /// that follows it — which is also why this is not a labelled `MoonCheckbox`, the one thing that
    /// component cannot place. Clicking either half writes the same preference, so the label is not
    /// a dead zone beside a live checkbox, and the cluster carries one tooltip spelling the
    /// preference out in full.
    ///
    /// Args:
    ///     palette: Active MoonUI palette, resolved once by the caller.
    ///     cx: View context used to read the resolved preference and wire the callbacks.
    ///
    /// Returns:
    ///     The label and checkbox as one flex-none cluster.
    pub(super) fn active_only_toggle(
        &self,
        palette: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let checked = (ACTIVE_ONLY.read)(&self.prefs);
        let view = cx.entity();
        h_flex()
            .id("strategies-active-only")
            .flex_none()
            .items_center()
            .gap(design::ui_px(cx, COMPACT_CHECKBOX_GAP))
            .tooltip(crate::panels::common::text_tooltip(
                t!(ACTIVE_ONLY.label).to_string(),
            ))
            .child(
                div()
                    .id("strategies-active-only-label")
                    .cursor_pointer()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(palette.text_soft))
                    .child(t!("strat.active_only").to_string())
                    // Reads the preference at click time rather than flipping the value this frame
                    // captured, so the label cannot write a stale flip.
                    .on_click({
                        let view = view.clone();
                        move |_, _window, app: &mut App| {
                            view.update(app, |this, cx| {
                                let next = !(ACTIVE_ONLY.read)(&this.prefs);
                                this.set_active_only(next, cx);
                            });
                        }
                    }),
            )
            .child(
                MoonCheckbox::new("strategies-active-only-mark")
                    .checked(checked)
                    .size(MoonCheckboxSize::Compact)
                    .on_change(move |value: &bool, _window, app| {
                        let value = *value;
                        view.update(app, |this, cx| this.set_active_only(value, cx));
                    }),
            )
            .into_any_element()
    }

    /// Disable active-only through the same persisted setter used by the popup.
    ///
    /// Args:
    ///     cx: View context used to persist and repaint when the preference changes.
    ///
    /// Returns:
    ///     Nothing; the shared setter handles the already-disabled case.
    pub(super) fn disable_active_only_for_reveal(&mut self, cx: &mut Context<Self>) {
        self.set_active_only(false, cx);
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
    let group_width = design::ui_text_width(cx, &group, POPUP_GROUP_CAPTION_FONT, 600.0, true);
    let checkbox_label_width = POPUP_ROWS
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

/// Build one captioned group from the rows in [`POPUP_ROWS`] that sit under this caption.
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
        POPUP_ROWS
            .iter()
            .filter(|row| row.group == caption)
            .map(|row| {
                let target = view.clone();
                // `row` is a `&'static PrefRow`, so the handler carries the row itself instead of
                // an index back into the table.
                let row = *row;
                MoonCheckbox::new(SharedString::from(format!("strategies-pref-{}", row.id)))
                    .label(t!(row.label).to_string())
                    .checked((row.read)(&prefs))
                    .size(MoonCheckboxSize::Compact)
                    .on_change(move |checked: &bool, _window, app| {
                        let checked = *checked;
                        target.update(app, |this, cx| this.write_pref(row, checked, cx));
                    })
                    .into_any_element()
            }),
    )
}
