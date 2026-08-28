//! The Profit Monitor's ⚙ settings popup and the display preferences it edits.
//!
//! Same shape as the chart's "Candles and Trades" popup: a `MoonPopover` anchored to a button in
//! the control row, contents built only while it is open, and every control stateless — it reads
//! the preference back out of the view on each render instead of holding a copy.
//!
//! The preferences live in `layout.toml` rather than in the view, so they survive a restart like
//! every other monitor choice. Each is stored as an OPTION: an absent key means "never chosen" and
//! takes the default below, which is what lets a default change later without silently overriding
//! someone who deliberately turned the feature off.

use std::collections::HashSet;

use gpui::*;
use moon_core::config::layout::WindowLayout;
use moon_ui::{
    MoonCheckbox, MoonCheckboxSize, MoonGroupBox, MoonPalette, MoonPopover, MoonPopoverPlacement,
    h_flex, v_flex,
};
use rust_i18n::t;

use super::ProfitMonitorView;
use crate::design;
use crate::panels::{popup_close_button, popup_group, popup_group_inset_px, popup_title};

/// Popup CONTENT width in design units, before the group frame's own inset.
///
/// Sized for the longest localized checkbox label rather than for the control: the labels are full
/// sentences in ES, and a narrower popup wraps them into two lines each. Raised with the grouping
/// preference, whose RU and ES wordings are longer than every label that came before them.
const CONTENT_WIDTH: f32 = 300.0;

/// Display preferences of the Profit Monitor window.
///
/// Every field is a feature that can be turned off, which is the rule this window follows: nothing
/// added to it renders unconditionally.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct MonitorPrefs {
    /// Whether a row draws its exchange logo before the name.
    pub(super) exchange_icons: bool,
    /// Whether the profit cell appends the newest closed trade in parentheses.
    pub(super) last_trade: bool,
    /// Whether a row lights up and fades when its core closes a new trade.
    pub(super) flash: bool,
    /// Whether the by-core table splits into the saved core groups, with a subtotal each.
    pub(super) group_sections: bool,
    /// Whether an active core that closed no trade in the period still gets its zero row.
    pub(super) idle_cores: bool,
    /// Whether clicking a row's core cell filters every main-window panel to that core.
    pub(super) core_filter: bool,
    /// Whether a row leads with its core's run status, and a restart button when it is stopped.
    pub(super) core_status: bool,
    /// Whether a row carries the start/stop control for its own core's trading.
    pub(super) trading_buttons: bool,
    /// Whether a group caption carries that control for every core the group names.
    pub(super) group_trading: bool,
}

impl Default for MonitorPrefs {
    /// Return the defaults applied to a profile that has never opened this popup.
    ///
    /// Everything that changes how the EXISTING rows read is ON: those are the reason the window
    /// was extended, and a feature nobody can see until they find a settings popup is a feature
    /// nobody finds. The core filter is reversible by the same click that applied it, so an
    /// unexpected first one costs one gesture. Grouping belongs there too: with no saved group it
    /// changes nothing at all, and with one it splits rows that were already on screen.
    ///
    /// [`Self::idle_cores`] is the one exception, and it is OFF. It is the only preference that
    /// ADDS rows rather than decorating the ones a query produced: a configuration with two hundred
    /// active cores and ten trading ones would turn a ten-row table into a two-hundred-row one on
    /// the first launch after an update, which is not a default anybody asked for.
    fn default() -> Self {
        Self {
            exchange_icons: true,
            last_trade: true,
            flash: true,
            group_sections: true,
            idle_cores: false,
            core_filter: true,
            // The three run controls are OFF by default, and for a stronger reason than
            // `idle_cores` is: they SEND COMMANDS to cores. A profit window that quietly grew a
            // Stop button after an update is a window whose next mis-click stops a fleet's
            // trading. Someone who wants them switches them on and knows they are there.
            core_status: false,
            trading_buttons: false,
            group_trading: false,
        }
    }
}

impl MonitorPrefs {
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
}

/// One editable preference: its label, the field it lives in, and the key it is saved under.
///
/// Every row is DATA, not a closure. Adding another preference is one entry in [`PREF_ROWS`], and
/// the four mechanical parts — restore it, show it, set it, save it — cannot drift apart because
/// none of them is written more than once.
struct PrefRow {
    /// Element-identity suffix, unique within the popup.
    id: &'static str,
    /// Locale key of the group caption this row belongs under.
    ///
    /// Rows are shown in table order within their group, so a new preference joins an existing
    /// group simply by naming it.
    group: &'static str,
    /// Whether switching this preference OFF must also release the broadcast core filter.
    ///
    /// A per-row fact rather than a read of the whole preferences: without it, editing any
    /// unrelated checkbox while the filter feature is off would republish the empty set, and only
    /// the equality guard in `set_core_filter` would keep that from waking five panels.
    releases_cores: bool,
    /// Locale key of the visible label.
    label: &'static str,
    /// Read the current value out of the preferences.
    read: fn(&MonitorPrefs) -> bool,
    /// Apply an edited value to the preferences.
    set: fn(&mut MonitorPrefs, bool),
    /// Read this preference's saved value, or `None` when it was never chosen.
    saved: fn(&WindowLayout) -> Option<bool>,
    /// Save an edited value under this preference's own `layout.toml` key.
    ///
    /// Only the EDITED key is ever written. Stamping the others would turn "never chosen" into an
    /// explicit value for preferences nobody touched, which is what the optional storage exists to
    /// avoid — a later change of default would then silently skip them.
    store: fn(&mut WindowLayout, bool),
}

/// Caption of the group holding everything that only changes what this window draws.
const DISPLAY_GROUP: &str = "profit_monitor.settings.display";

/// Caption of the group holding what a click in this window does to the rest of the terminal.
const INTERACTION_GROUP: &str = "profit_monitor.settings.interaction";

/// Caption of the group holding the controls that COMMAND cores rather than describe them.
///
/// Its own group, not a third block of "interaction": everything above changes what this window
/// shows or which cores other panels show, while everything here sends a command to a core.
const CORE_CONTROL_GROUP: &str = "profit_monitor.settings.core_control";

/// Every preference, in the order the popup shows them.
const PREF_ROWS: [PrefRow; 9] = [
    PrefRow {
        id: "exchange-icons",
        releases_cores: false,
        group: DISPLAY_GROUP,
        label: "profit_monitor.settings.exchange_icons",
        read: |prefs| prefs.exchange_icons,
        set: |prefs, value| prefs.exchange_icons = value,
        saved: |layout| layout.profit_monitor_exchange_icons,
        store: |layout, value| layout.profit_monitor_exchange_icons = Some(value),
    },
    PrefRow {
        id: "last-trade",
        releases_cores: false,
        group: DISPLAY_GROUP,
        label: "profit_monitor.settings.last_trade",
        read: |prefs| prefs.last_trade,
        set: |prefs, value| prefs.last_trade = value,
        saved: |layout| layout.profit_monitor_last_trade,
        store: |layout, value| layout.profit_monitor_last_trade = Some(value),
    },
    PrefRow {
        id: "flash",
        releases_cores: false,
        group: DISPLAY_GROUP,
        label: "profit_monitor.settings.flash",
        read: |prefs| prefs.flash,
        set: |prefs, value| prefs.flash = value,
        saved: |layout| layout.profit_monitor_flash,
        store: |layout, value| layout.profit_monitor_flash = Some(value),
    },
    PrefRow {
        id: "group-sections",
        releases_cores: false,
        group: DISPLAY_GROUP,
        label: "profit_monitor.settings.group_sections",
        read: |prefs| prefs.group_sections,
        set: |prefs, value| prefs.group_sections = value,
        saved: |layout| layout.profit_monitor_group_sections,
        store: |layout, value| layout.profit_monitor_group_sections = Some(value),
    },
    PrefRow {
        id: "idle-cores",
        releases_cores: false,
        group: DISPLAY_GROUP,
        label: "profit_monitor.settings.idle_cores",
        read: |prefs| prefs.idle_cores,
        set: |prefs, value| prefs.idle_cores = value,
        saved: |layout| layout.profit_monitor_idle_cores,
        store: |layout, value| layout.profit_monitor_idle_cores = Some(value),
    },
    PrefRow {
        id: "core-filter",
        releases_cores: true,
        group: INTERACTION_GROUP,
        label: "profit_monitor.settings.core_filter",
        read: |prefs| prefs.core_filter,
        set: |prefs, value| prefs.core_filter = value,
        saved: |layout| layout.profit_monitor_core_filter,
        store: |layout, value| layout.profit_monitor_core_filter = Some(value),
    },
    PrefRow {
        id: "core-status",
        releases_cores: false,
        group: CORE_CONTROL_GROUP,
        label: "profit_monitor.settings.core_status",
        read: |prefs| prefs.core_status,
        set: |prefs, value| prefs.core_status = value,
        saved: |layout| layout.profit_monitor_core_status,
        store: |layout, value| layout.profit_monitor_core_status = Some(value),
    },
    PrefRow {
        id: "trading-buttons",
        releases_cores: false,
        group: CORE_CONTROL_GROUP,
        label: "profit_monitor.settings.trading_buttons",
        read: |prefs| prefs.trading_buttons,
        set: |prefs, value| prefs.trading_buttons = value,
        saved: |layout| layout.profit_monitor_trading_buttons,
        store: |layout, value| layout.profit_monitor_trading_buttons = Some(value),
    },
    PrefRow {
        id: "group-trading",
        releases_cores: false,
        group: CORE_CONTROL_GROUP,
        label: "profit_monitor.settings.group_trading",
        read: |prefs| prefs.group_trading,
        set: |prefs, value| prefs.group_trading = value,
        saved: |layout| layout.profit_monitor_group_trading,
        store: |layout, value| layout.profit_monitor_group_trading = Some(value),
    },
];

impl ProfitMonitorView {
    /// Apply and persist one display preference.
    ///
    /// The body is invalidated explicitly because it is a cached sibling view: notifying only the
    /// parent would repaint the chrome around an unchanged table.
    ///
    /// Args:
    ///     row: The preference being edited; it knows both where it lives and how it is saved.
    ///     value: Its new value.
    ///     cx: View context used to persist and repaint.
    fn write_pref(&mut self, row: &PrefRow, value: bool, cx: &mut Context<Self>) {
        if (row.read)(&self.prefs) == value {
            return;
        }
        (row.set)(&mut self.prefs, value);
        if !self.prefs.flash {
            // Turning the highlight off must clear what is already glowing, not wait it out: the
            // repaint chain re-arms on `Arrivals::live`, so an unrendered stamp would keep ticking.
            self.flash.clear();
        }
        let store = row.store;
        // Turning the core filter off has to RELEASE what it filtered. The gesture that set it is
        // gone with the setting, so a panel left narrowed would carry a filter whose cause is no
        // longer visible anywhere. Scoped to the row that owns the filter, and to switching it OFF.
        let release_cores = row.releases_cores && !value;
        self.backend.update(cx, |backend, backend_cx| {
            store(&mut backend.layout, value);
            backend.layout_dirty = true;
            if release_cores {
                backend.set_core_filter(HashSet::new(), backend_cx);
            }
        });
        self.invalidate_content(cx);
        cx.notify();
    }

    /// Close the settings popup.
    ///
    /// The already-closed guard mirrors the candle popup's: clicking the trigger while the popup is
    /// open makes `Popover` report `false` twice, and the second report would reopen nothing while
    /// still costing a repaint.
    ///
    /// Args:
    ///     cx: View context used to repaint.
    fn close_settings(&mut self, cx: &mut Context<Self>) {
        if !self.settings_open {
            return;
        }
        self.settings_open = false;
        cx.notify();
    }

    /// Build the ⚙ popover anchored to its trigger button.
    ///
    /// Args:
    ///     trigger: Button the popover anchors to.
    ///     palette: Active MoonUI palette.
    ///     cx: View context used to read preferences and wire the toggles.
    ///
    /// Returns:
    ///     The trigger with its anchored settings popover.
    pub(super) fn settings_popover(
        &self,
        trigger: impl IntoElement,
        palette: MoonPalette,
        cx: &mut Context<Self>,
    ) -> MoonPopover {
        let view = cx.entity();
        let mut popover = MoonPopover::new("profit-monitor-settings-popover")
            // Bottom-end like the chart popups: the control row sits at the window's top edge, so
            // growing down and to the left keeps a wide popup inside a narrow monitor.
            .placement(MoonPopoverPlacement::BottomEnd)
            .content_width(f32::from(design::ui_px(cx, CONTENT_WIDTH)) + popup_group_inset_px(cx))
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
        popover = popover.content(settings_content(self.prefs, view, palette, cx));
        popover
    }
}

/// Render the popup body from the current preferences.
///
/// Args:
///     prefs: Values the checkboxes display.
///     view: Monitor entity receiving the edits.
///     palette: Active MoonUI palette.
///     cx: Application context supplying scaled geometry.
///
/// Returns:
///     Title row, the display group, and the interaction group.
fn settings_content(
    prefs: MonitorPrefs,
    view: Entity<ProfitMonitorView>,
    palette: MoonPalette,
    cx: &App,
) -> AnyElement {
    // Chrome belongs to MoonPopover; a second surface here would double the popup's background.
    v_flex()
        .id("profit-monitor-settings-popup")
        .w_full()
        .gap(design::ui_px(cx, 8.0))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .child(popup_title(
                    t!("profit_monitor.settings.title"),
                    palette,
                    cx,
                ))
                .child(popup_close_button("profit-monitor-settings-close", {
                    let view = view.clone();
                    move |_, _window, app: &mut App| {
                        view.update(app, |this, cx| this.close_settings(cx));
                    }
                })),
        )
        .child(pref_group(
            "profit-monitor-display",
            DISPLAY_GROUP,
            prefs,
            &view,
        ))
        .child(pref_group(
            "profit-monitor-interaction",
            INTERACTION_GROUP,
            prefs,
            &view,
        ))
        .child(pref_group(
            "profit-monitor-core-control",
            CORE_CONTROL_GROUP,
            prefs,
            &view,
        ))
        .into_any_element()
}

/// Build one caption's group box from the rows of [`PREF_ROWS`] that name it.
///
/// The membership stays a property of the table — a new preference joins a group by naming it —
/// while the group itself keeps a literal id, like every other settings popup in the terminal.
///
/// Args:
///     id: Stable element identity of the group box.
///     caption: Locale key shared by the rows that belong here, and the group's own title.
///     prefs: Values the checkboxes display.
///     view: Monitor entity receiving the edits.
///
/// Returns:
///     The group box with one compact checkbox per matching row, in table order.
fn pref_group(
    id: &'static str,
    caption: &'static str,
    prefs: MonitorPrefs,
    view: &Entity<ProfitMonitorView>,
) -> MoonGroupBox {
    popup_group(id, t!(caption).to_string()).children(
        PREF_ROWS
            .iter()
            .enumerate()
            .filter(|(_, row)| row.group == caption)
            .map(|(index, row)| {
                let target = view.clone();
                MoonCheckbox::new(SharedString::from(format!(
                    "profit-monitor-pref-{}",
                    row.id
                )))
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
