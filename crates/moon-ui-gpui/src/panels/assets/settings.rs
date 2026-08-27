//! Persisted display preferences of the Assets wallet section and the gear popup that edits them.
//!
//! The section header owns two controls: the refresh button, which asks the selected core for its
//! transfer assets again, and this gear, which changes how the core list beside it is laid out.
//! One preference keeps one authority — the persisted `layout.toml` key itself. The checkbox and
//! the roster both resolve it from there at paint time and one setter writes it back through the
//! shared layout coordinator, so the two Assets surfaces that can carry a wallet section cannot
//! disagree about the shape of the list.

use super::*;
use moon_ui::{
    MoonCheckbox, MoonCheckboxSize, MoonGroupBox, MoonPopover, MoonPopoverPlacement, MoonTheme,
};

use crate::panels::{
    COMPACT_CHECKBOX_FONT, COMPACT_CHECKBOX_GAP, COMPACT_CHECKBOX_MARK, POPUP_GROUP_CAPTION_FONT,
    popup_close_button, popup_gear_trigger, popup_group, popup_group_inset_px, popup_title,
};

/// Caption of the popup's display-preferences group.
const DISPLAY_GROUP: &str = "assets.settings.display";
/// Label of the exchange-grouping checkbox.
const GROUP_BY_VENUE_LABEL: &str = "assets.settings.group_by_venue";
/// Title of the popup and tooltip of its gear trigger.
const SETTINGS_TITLE: &str = "assets.settings.title";

/// Default applied when the optional layout key has never been written.
///
/// Grouping stays on, so an upgrade keeps the exchange hierarchy the section shipped with rather
/// than silently flattening a roster the user already reads by section.
const GROUP_BY_VENUE_DEFAULT: bool = true;

impl AssetsView {
    /// Resolve the exchange-grouping preference from the shared persisted layout.
    ///
    /// Read where it is drawn rather than copied into the view at construction: two Assets
    /// surfaces can be open at once — the global "⧉" window and an Auto dock tab, both of which
    /// carry a wallet section — and a per-view copy would leave the one that does not host the
    /// gear drawing the old shape, and showing the old checkbox, until the app restarted. The
    /// layout entity is already read on every repaint of this section for the venue map.
    ///
    /// Args:
    ///     cx: Application context used to read the shared layout.
    ///
    /// Returns:
    ///     The saved value, or [`GROUP_BY_VENUE_DEFAULT`] when the key is absent or malformed.
    pub(super) fn group_by_venue(&self, cx: &App) -> bool {
        self.backend
            .read(cx)
            .layout
            .assets_group_by_venue
            .unwrap_or(GROUP_BY_VENUE_DEFAULT)
    }

    /// Apply and persist the exchange-grouping preference.
    ///
    /// Only this view is woken. The other surface, if one is open, observes the same backend and
    /// repaints on its own once-per-second gate — waking the whole scene to hand one checkbox to a
    /// second wallet section would cost every panel a frame for it.
    ///
    /// Args:
    ///     value: New resolved value.
    ///     cx: View context used to update shared layout state and repaint.
    ///
    /// Returns:
    ///     Nothing; an unchanged value exits without touching layout state.
    fn write_group_by_venue(&mut self, value: bool, cx: &mut Context<Self>) {
        if self.group_by_venue(cx) == value {
            return;
        }
        self.backend.update(cx, |backend, _| {
            backend.layout.assets_group_by_venue = Some(value);
            backend.layout_dirty = true;
        });
        cx.notify();
    }

    /// Close the settings popup when it is currently open.
    ///
    /// Args:
    ///     cx: View context used to repaint after the state change.
    ///
    /// Returns:
    ///     Nothing; an already-closed popup exits without repainting.
    fn close_wallet_settings(&mut self, cx: &mut Context<Self>) {
        if !self.wallet_settings_open {
            return;
        }
        self.wallet_settings_open = false;
        cx.notify();
    }

    /// Build the wallet-section settings gear with its controlled popover attached.
    ///
    /// Args:
    ///     p: Active MoonUI palette, resolved once by the caller.
    ///     cx: View context used to read the preference and wire the callbacks.
    ///
    /// Returns:
    ///     The gear trigger carrying its popover; the body is built only while open.
    pub(super) fn wallet_settings_popover(
        &self,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> MoonPopover {
        let open = self.wallet_settings_open;
        let view = cx.entity();
        let mut popover = MoonPopover::new("assets-wallets-settings-popover")
            .placement(MoonPopoverPlacement::BottomEnd)
            .close_on_content_click(false)
            .open(open)
            .on_open_change({
                let view = view.clone();
                move |open, _window, app| {
                    view.update(app, |this, cx| {
                        this.wallet_settings_open = open;
                        cx.notify();
                    });
                }
            })
            .trigger(popup_gear_trigger(
                "assets-wallets-settings",
                t!(SETTINGS_TITLE).to_string(),
                open,
            ));
        // Built ONLY while open: `MoonPopover` takes its content eagerly, so a shut popover would
        // otherwise rebuild the group box and its checkbox on every repaint of the section.
        if !open {
            return popover;
        }
        popover = popover
            .content_width(settings_content_width(cx))
            .content(settings_content(self.group_by_venue(cx), view, p, cx));
        popover
    }
}

/// Measure the current locale's popup content at the metrics used to render it.
///
/// MoonPopover adds its own padding and border to this content width. The group-frame inset and
/// the compact checkbox mark and gap are included here because they sit inside that content box.
///
/// Args:
///     cx: Application context providing locale, typography, and UI scaling.
///
/// Returns:
///     Rendered content-box width that keeps the title, caption, and checkbox label visible.
fn settings_content_width(cx: &App) -> f32 {
    let tokens = MoonTheme::active_tokens(cx);
    let title_width = design::ui_text_width(
        cx,
        &t!(SETTINGS_TITLE),
        tokens.typography.mono_font_size,
        400.0,
        true,
    );
    let group_width = design::ui_text_width(
        cx,
        &t!(DISPLAY_GROUP),
        POPUP_GROUP_CAPTION_FONT,
        600.0,
        true,
    );
    let label_width = design::ui_text_width(
        cx,
        &t!(GROUP_BY_VENUE_LABEL),
        COMPACT_CHECKBOX_FONT,
        400.0,
        false,
    );
    let checkbox_leading = f32::from(design::ui_px(
        cx,
        COMPACT_CHECKBOX_MARK + COMPACT_CHECKBOX_GAP,
    ));
    let group_content = group_width.max(checkbox_leading + label_width);
    title_width.max(group_content + popup_group_inset_px(cx))
}

/// Render the popup body from the current resolved preference.
///
/// Args:
///     group_by_venue: Value displayed by the checkbox.
///     view: Assets entity receiving the edit.
///     p: Active MoonUI palette.
///     cx: Application context supplying scaled geometry.
///
/// Returns:
///     Title row and the display-preferences group.
fn settings_content(
    group_by_venue: bool,
    view: Entity<AssetsView>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    // MoonPopover owns the surface chrome; this root supplies only content and spacing.
    v_flex()
        .id("assets-wallets-settings-popup")
        .w_full()
        .gap(design::ui_px(cx, 8.0))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .child(popup_title(t!(SETTINGS_TITLE), p, cx))
                .child(popup_close_button("assets-wallets-settings-close", {
                    let view = view.clone();
                    move |_, _window, app: &mut App| {
                        view.update(app, |this, cx| this.close_wallet_settings(cx));
                    }
                })),
        )
        .child(display_group(group_by_venue, view))
        .into_any_element()
}

/// Build the captioned display group holding the exchange-grouping checkbox.
///
/// Args:
///     group_by_venue: Value displayed by the checkbox.
///     view: Assets entity receiving the edit.
///
/// Returns:
///     One compact checkbox row inside the shared popup group frame.
fn display_group(group_by_venue: bool, view: Entity<AssetsView>) -> MoonGroupBox {
    popup_group("assets-wallets-display", t!(DISPLAY_GROUP).to_string()).child(
        MoonCheckbox::new("assets-pref-group-by-venue")
            .label(t!(GROUP_BY_VENUE_LABEL).to_string())
            .checked(group_by_venue)
            .size(MoonCheckboxSize::Compact)
            .on_change(move |checked: &bool, _window, app| {
                let checked = *checked;
                view.update(app, |this, cx| this.write_group_by_venue(checked, cx));
            }),
    )
}
