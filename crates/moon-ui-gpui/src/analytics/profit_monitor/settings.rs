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

use gpui::*;
use moon_core::config::layout::WindowLayout;
use moon_ui::{
    MoonCheckbox, MoonCheckboxSize, MoonPalette, MoonPopover, MoonPopoverPlacement, h_flex, v_flex,
};
use rust_i18n::t;

use super::ProfitMonitorView;
use crate::design;
use crate::panels::{popup_close_button, popup_group, popup_group_inset_px, popup_title};

/// Popup CONTENT width in design units, before the group frame's own inset.
///
/// Sized for the longest localized checkbox label rather than for the control: the three labels are
/// full sentences in ES, and a narrower popup wraps them into two lines each.
const CONTENT_WIDTH: f32 = 268.0;

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
}

impl Default for MonitorPrefs {
    /// Return the defaults applied to a profile that has never opened this popup.
    ///
    /// All three are ON: they are the reason the window was extended, and a feature nobody can see
    /// until they find a settings popup is a feature nobody finds.
    fn default() -> Self {
        Self {
            exchange_icons: true,
            last_trade: true,
            flash: true,
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
/// Every row is DATA, not a closure. Adding a fourth preference is one entry in [`PREF_ROWS`], and
/// the four mechanical parts — restore it, show it, set it, save it — cannot drift apart because
/// none of them is written more than once.
struct PrefRow {
    /// Element-identity suffix, unique within the popup.
    id: &'static str,
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

/// Every display preference, in the order the popup shows them.
const PREF_ROWS: [PrefRow; 3] = [
    PrefRow {
        id: "exchange-icons",
        label: "profit_monitor.settings.exchange_icons",
        read: |prefs| prefs.exchange_icons,
        set: |prefs, value| prefs.exchange_icons = value,
        saved: |layout| layout.profit_monitor_exchange_icons,
        store: |layout, value| layout.profit_monitor_exchange_icons = Some(value),
    },
    PrefRow {
        id: "last-trade",
        label: "profit_monitor.settings.last_trade",
        read: |prefs| prefs.last_trade,
        set: |prefs, value| prefs.last_trade = value,
        saved: |layout| layout.profit_monitor_last_trade,
        store: |layout, value| layout.profit_monitor_last_trade = Some(value),
    },
    PrefRow {
        id: "flash",
        label: "profit_monitor.settings.flash",
        read: |prefs| prefs.flash,
        set: |prefs, value| prefs.flash = value,
        saved: |layout| layout.profit_monitor_flash,
        store: |layout, value| layout.profit_monitor_flash = Some(value),
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
            // repaint chain reads `flash_live`, so an unrendered stamp would keep ticking.
            self.flash.clear();
        }
        let store = row.store;
        self.backend.update(cx, |backend, _| {
            store(&mut backend.layout, value);
            backend.layout_dirty = true;
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
///     Title row and the display-preference group.
fn settings_content(
    prefs: MonitorPrefs,
    view: Entity<ProfitMonitorView>,
    palette: MoonPalette,
    cx: &App,
) -> AnyElement {
    let mut group = popup_group(
        "profit-monitor-display",
        t!("profit_monitor.settings.display"),
    );
    for (index, row) in PREF_ROWS.iter().enumerate() {
        let target = view.clone();
        group = group.child(
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
            }),
        );
    }
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
        .child(group)
        .into_any_element()
}
