//! The Auto rail's ⚙ settings popup and the run-control preferences it edits.
//!
//! Same shape as the Profit Monitor's popup (`analytics/profit_monitor/settings.rs`): a
//! `MoonPopover` anchored to a gear in the rail's summary bar, contents built only while it is
//! open, every checkbox stateless — it reads the preference back out of `layout.toml` on each
//! render. Each preference is stored as an OPTION so an absent key means "never chosen".

use gpui::*;
use moon_core::config::layout::WindowLayout;
use moon_ui::{
    MoonCheckbox, MoonCheckboxSize, MoonPalette, MoonPopover, MoonPopoverPlacement, h_flex, v_flex,
};
use rust_i18n::t;

use super::super::Shell;
use crate::Backend;
use crate::controls::core_run::RunSlots;
use crate::design;
use crate::panels::{
    popup_close_button, popup_gear_trigger, popup_group, popup_group_inset_px, popup_title,
};
use crate::workspace::WorkspaceRailDensity;

/// Popup CONTENT width in design units, before the group frame's own inset.
///
/// Sized for the longest localized checkbox label so no row wraps: the exchange-heading row
/// measures ~290 px in EN and ~375 px in ES at the default face, and `MoonCheckbox` wraps its
/// label column rather than truncating. The popup grows rightward into the dock, where width
/// costs nothing.
const CONTENT_WIDTH: f32 = 400.0;

/// Run-control preferences of the Auto workspace rail.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct RailPrefs {
    /// Whether a row leads with its core's run status, and a restart button when it is stopped.
    pub(super) core_status: bool,
    /// Whether the start/stop control for the strategy engine is shown at all.
    pub(super) trading_buttons: bool,
    /// Whether the AutoDetect on/off switch is shown at all.
    pub(super) auto_buttons: bool,
    /// Whether an exchange heading ALSO carries whichever of those controls are on, acting on every
    /// core the group names.
    pub(super) exchange_controls: bool,
}

impl Default for RailPrefs {
    /// Return the defaults applied to a profile that has never opened this popup.
    ///
    /// Every control is ON: the rail exists to command cores, and this feature replaced the rail's
    /// plain connection dot at the user's request — the popup is the way to turn a control off,
    /// not the way to discover it. (The Profit Monitor chooses the opposite default for its own
    /// reason, `MonitorPrefs::default`.)
    fn default() -> Self {
        Self {
            core_status: true,
            trading_buttons: true,
            auto_buttons: true,
            exchange_controls: true,
        }
    }
}

impl RailPrefs {
    /// Restore the preferences saved in `layout.toml`.
    ///
    /// Args:
    ///     layout: Persisted window layout.
    ///
    /// Returns:
    ///     Saved values, with each unset key taking its default.
    pub(super) fn restore(layout: &WindowLayout) -> Self {
        let mut prefs = Self::default();
        for row in &PREF_ROWS {
            if let Some(saved) = (row.saved)(layout) {
                (row.set)(&mut prefs, saved);
            }
        }
        prefs
    }

    /// The slots every rail line reserves at this density.
    ///
    /// Icon density is 52 px wide: only the status dot fits, so the buttons are withheld there
    /// regardless of preference.
    ///
    /// Args:
    ///     density: Current Full, Compact, or Icon rail rung.
    ///
    /// Returns:
    ///     Slots every line of the rail reserves at this density.
    pub(super) fn slots(self, density: WorkspaceRailDensity) -> RunSlots {
        match density {
            WorkspaceRailDensity::Icon => RunSlots {
                status: self.core_status,
                trading: false,
                auto: false,
            },
            WorkspaceRailDensity::Full | WorkspaceRailDensity::Compact => RunSlots {
                status: self.core_status,
                trading: self.trading_buttons,
                auto: self.auto_buttons,
            },
        }
    }
}

/// One editable preference: its label, the field it lives in, and the key it is saved under.
///
/// Every row is DATA, not a closure. Adding another preference is one entry in [`PREF_ROWS`], and
/// the four mechanical parts — restore it, show it, set it, save it — cannot drift apart because
/// none of them is written more than once.
struct PrefRow {
    /// Element-identity suffix, unique within the popup.
    id: &'static str,
    /// Locale key of the visible label.
    label: &'static str,
    /// Read the current value out of the preferences.
    read: fn(&RailPrefs) -> bool,
    /// Apply an edited value to the preferences.
    set: fn(&mut RailPrefs, bool),
    /// Read this preference's saved value, or `None` when it was never chosen.
    saved: fn(&WindowLayout) -> Option<bool>,
    /// Save an edited value under this preference's own `layout.toml` key.
    ///
    /// Only the EDITED key is ever written. Stamping the others would turn "never chosen" into an
    /// explicit value for preferences nobody touched, which is what the optional storage exists to
    /// avoid — a later change of default would then silently skip them.
    store: fn(&mut WindowLayout, bool),
}

/// Every preference, in the order the popup shows them.
///
/// The first three labels are the Profit Monitor's own — the same controls, the same wording; only
/// the exchange-scope row is the rail's.
const PREF_ROWS: [PrefRow; 4] = [
    PrefRow {
        id: "core-status",
        label: "profit_monitor.settings.core_status",
        read: |prefs| prefs.core_status,
        set: |prefs, value| prefs.core_status = value,
        saved: |layout| layout.workspace_rail_core_status,
        store: |layout, value| layout.workspace_rail_core_status = Some(value),
    },
    PrefRow {
        id: "trading-buttons",
        label: "profit_monitor.settings.trading_buttons",
        read: |prefs| prefs.trading_buttons,
        set: |prefs, value| prefs.trading_buttons = value,
        saved: |layout| layout.workspace_rail_trading_buttons,
        store: |layout, value| layout.workspace_rail_trading_buttons = Some(value),
    },
    PrefRow {
        id: "auto-buttons",
        label: "profit_monitor.settings.auto_buttons",
        read: |prefs| prefs.auto_buttons,
        set: |prefs, value| prefs.auto_buttons = value,
        saved: |layout| layout.workspace_rail_auto_buttons,
        store: |layout, value| layout.workspace_rail_auto_buttons = Some(value),
    },
    PrefRow {
        id: "exchange-controls",
        label: "workspace.settings.exchange_controls",
        read: |prefs| prefs.exchange_controls,
        set: |prefs, value| prefs.exchange_controls = value,
        saved: |layout| layout.workspace_rail_exchange_controls,
        store: |layout, value| layout.workspace_rail_exchange_controls = Some(value),
    },
];

/// Build the ⚙ popover anchored to its trigger button in the rail summary bar.
///
/// Args:
///     shell: Shell that owns the open flag.
///     backend: Shared terminal state, written when a checkbox changes a layout key.
///     open: Whether the popup is currently showing.
///     prefs: Values the checkboxes display.
///     palette: Active MoonUI palette.
///     cx: Application context used to scale geometry and wire the toggles.
///
/// Returns:
///     The trigger with its anchored settings popover.
pub(super) fn rail_settings_popover(
    shell: &Entity<Shell>,
    backend: &Entity<Backend>,
    open: bool,
    prefs: RailPrefs,
    palette: MoonPalette,
    cx: &App,
) -> MoonPopover {
    let trigger = popup_gear_trigger(
        "workspace-rail-settings",
        t!("workspace.settings.title").to_string(),
        open,
    );
    let popover = MoonPopover::new("workspace-rail-settings-popover")
        // BottomStart: the gear sits at the rail's right edge with the whole window to its right,
        // so the popup grows down and rightward and never leaves the window.
        .placement(MoonPopoverPlacement::BottomStart)
        .content_width(f32::from(design::ui_px(cx, CONTENT_WIDTH)) + popup_group_inset_px(cx))
        .close_on_content_click(false)
        .open(open)
        .on_open_change({
            let shell = shell.clone();
            move |open, _window, app| {
                shell.update(app, |this, cx| this.set_rail_settings_open(open, cx));
            }
        })
        .trigger(trigger);
    if !open {
        return popover;
    }
    popover.content(settings_content(prefs, shell, backend, palette, cx))
}

/// Render the popup body from the current preferences.
///
/// Args:
///     prefs: Values the checkboxes display.
///     shell: Shell receiving the close and the rail repaint.
///     backend: Shared terminal state receiving the layout writes.
///     palette: Active MoonUI palette.
///     cx: Application context supplying scaled geometry.
///
/// Returns:
///     Title row and the core-control group.
fn settings_content(
    prefs: RailPrefs,
    shell: &Entity<Shell>,
    backend: &Entity<Backend>,
    palette: MoonPalette,
    cx: &App,
) -> AnyElement {
    // Chrome belongs to MoonPopover; a second surface here would double the popup's background.
    // Every title, caption, and checkbox label here is prose, so the popup flips to the UI face
    // on its own root rather than inheriting the rail root's mono.
    v_flex()
        .id("workspace-rail-settings-popup")
        .w_full()
        .font_family(design::ui_font())
        .gap(design::ui_px(cx, 8.0))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .child(popup_title(t!("workspace.settings.title"), palette, cx))
                .child(popup_close_button("workspace-rail-settings-close", {
                    let shell = shell.clone();
                    move |_, _window, app: &mut App| {
                        shell.update(app, |this, cx| this.set_rail_settings_open(false, cx));
                    }
                })),
        )
        .child(
            popup_group(
                "workspace-rail-core-control",
                t!("profit_monitor.settings.core_control").to_string(),
            )
            .children(PREF_ROWS.iter().enumerate().map(|(index, row)| {
                let backend = backend.clone();
                let shell = shell.clone();
                MoonCheckbox::new(SharedString::from(format!(
                    "workspace-rail-pref-{}",
                    row.id
                )))
                .label(t!(row.label).to_string())
                .checked((row.read)(&prefs))
                .size(MoonCheckboxSize::Compact)
                .on_change(move |checked: &bool, _window, app| {
                    let checked = *checked;
                    backend.update(app, |backend, backend_cx| {
                        if (PREF_ROWS[index].saved)(&backend.layout) == Some(checked) {
                            return;
                        }
                        (PREF_ROWS[index].store)(&mut backend.layout, checked);
                        backend.layout_dirty = true;
                        backend_cx.notify();
                    });
                    shell.update(app, |_, cx| cx.notify());
                })
                .into_any_element()
            })),
        )
        .into_any_element()
}
