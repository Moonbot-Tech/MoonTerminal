//! The volume block's own right-click menu: what period it covers, and what it prints.
//!
//! The reference terminal puts this menu on the block itself, and that placement is the feature —
//! the period is the one caption setting a reader changes while WATCHING, several times a session,
//! and walking to the tab strip's settings popup for it breaks the watching.
//!
//! # Where the change goes
//!
//! Captions are a PER-TAB setting, persisted into `charts.json` by whichever host owns the tab —
//! the strip for a docked one, the window's own host for a detached one. A panel is the wrong place
//! to persist from: it does not know which tab spec it belongs to, and inventing an answer here
//! would write another tab's settings.
//!
//! So the panel does the half it can: it applies the change to ITSELF immediately, which is what
//! makes the menu feel instant, and hands the same configuration up as [`ChartPanel::pending_labels`].
//! The stack that owns the panel picks it up in the observer it already has, and passes it to the
//! host, which persists it exactly as the settings popup would. Three short relays instead of a new
//! global queue, and every one of them is a link that already existed.

use gpui::*;
use moon_core::config::{
    ChartLabelField, ChartLabelRow, ChartLabelsCfg, LabelSpan, LabelWindow, VolumeUnits,
};
use moon_ui::{MoonContextMenuWindowExt as _, MoonMenuItem};
use rust_i18n::t;

use super::ChartPanel;

/// Widths of the menu, in design pixels: enough for the longest localized period line.
const MENU_MIN_WIDTH: f32 = 170.0;
const MENU_MAX_WIDTH: f32 = 320.0;

/// Periods the menu offers, shortest first — the reference terminal's own list, plus the two hours
/// the retained candles state outright.
///
/// All of them are FIXED windows: a window may be one the protocol maintains itself, and every one
/// here is a period the retained aggregates can answer without walking raw trades. Arbitrary minute
/// counts remain in the model (see [`LabelSpan::Minutes`]) for a future "set N", but the menu does
/// not offer them — a list a reader scans should hold the periods they actually switch between.
const QUICK_PERIODS: [LabelWindow; 7] = [
    LabelWindow::M1,
    LabelWindow::M3,
    LabelWindow::M5,
    LabelWindow::M15,
    LabelWindow::M30,
    LabelWindow::H1,
    LabelWindow::H2,
];

/// Trade counts the menu offers, for the reader who counts prints rather than minutes.
const QUICK_TRADES: [u32; 4] = [100, 250, 500, 1000];

/// Captions the menu can switch on and off, with the locale key naming each.
const TOGGLE_FIELDS: [ChartLabelField; 4] = [
    ChartLabelField::WindowSpanName,
    ChartLabelField::WindowVolume,
    ChartLabelField::WindowTrades,
    ChartLabelField::WindowBuyShare,
];

impl ChartPanel {
    /// Open the volume menu when a right-click landed on a volume block.
    ///
    /// Args:
    ///     local_pos: Press position in the chart's own device pixels.
    ///     menu_pos: The same press in window coordinates, where the menu is anchored.
    ///
    /// Returns:
    ///     Whether the press was consumed — `false` leaves right-button zoom and the other menus
    ///     their normal turn.
    pub(super) fn try_open_volume_menu(
        &mut self,
        local_pos: (f32, f32),
        menu_pos: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pane) = self.input.pane_at(local_pos.0, local_pos.1) else {
            return false;
        };
        // The caption pass places its blocks in the WINDOW's logical pixels while this point is in
        // the slot's device pixels — the same conversion the arbitrage click documents, and getting
        // it wrong opens the menu for whatever block sits under the mis-scaled point.
        let Some((origin, sf)) = self.chart_origin_logical() else {
            return false;
        };
        let (lx, ly) = (local_pos.0 / sf + origin.0, local_pos.1 / sf + origin.1);
        let Some(row_ix) = self.chart.volume_module_at(pane, lx, ly) else {
            return false;
        };
        // The hit rectangle came from a frame that has already been drawn, so the module it names
        // is the one the reader aimed at; this guards a configuration replaced in between.
        let cfg = self.effective_labels(cx);
        let Some(row) = cfg.rows.get(row_ix) else {
            return false;
        };
        // Built from the configuration ALREADY in hand. This runs inside the panel's own lease, so
        // reading the entity again here — which is what `open_menu` does for the rebuild path —
        // would be a double lease and take the app down on the first right-click.
        let items = build_items(cx.entity(), row, row_ix, menu_pos);
        if items.is_empty() {
            return false;
        }
        install_menu(items, menu_pos, window, cx);
        true
    }

    /// This panel's captions with every question answered: its own override, or the default its
    /// KIND of tab follows.
    ///
    /// The same resolution the settings popup performs, and for the same reason: an edit starts
    /// from this value, so reading a neighbour's configuration would persist it as this tab's own.
    pub(super) fn effective_labels(&self, cx: &App) -> ChartLabelsCfg {
        let mut cfg = self.chart_labels.clone().unwrap_or_else(|| {
            self.backend
                .read(cx)
                .layout
                .chart_labels_for(self.default_kind)
                .clone()
        });
        cfg.sanitize();
        cfg
    }

    /// Apply an edited caption set: to this panel now, and to its tab through the host.
    ///
    /// See the module docs for why the persist half is a relay rather than a write from here.
    pub(super) fn apply_volume_edit(&mut self, cfg: ChartLabelsCfg, cx: &mut Context<Self>) {
        let mut cfg = cfg;
        cfg.sanitize();
        self.set_chart_labels(Some(cfg.clone()), cx);
        self.pending_labels = Some(cfg);
        cx.notify();
    }

    /// Take the caption set this panel's own menu produced, if any.
    ///
    /// Called by the stack that owns the panel, from the observer it already runs.
    pub(crate) fn take_pending_labels(&mut self) -> Option<ChartLabelsCfg> {
        self.pending_labels.take()
    }
}

/// Build the menu for one module and install it.
///
/// Also the way a TOGGLE refreshes: a context menu holds a finished snapshot of its rows, so a
/// check switched on inside an open one would keep drawing its old mark. Rebuilding at the same
/// position shows the new state and leaves the menu standing, which is what a reader flipping two
/// captions in a row expects.
fn open_menu(
    panel: &Entity<ChartPanel>,
    row_ix: usize,
    pos: Point<Pixels>,
    window: &mut Window,
    app: &mut App,
) {
    // Safe to read here: a menu row's handler runs with no lease on the panel. The press path does
    // NOT — see `try_open_volume_menu`, which passes the configuration it already holds.
    let cfg = panel.read(app).effective_labels(app);
    let Some(row) = cfg.rows.get(row_ix) else {
        return;
    };
    let items = build_items(panel.clone(), row, row_ix, pos);
    if items.is_empty() {
        return;
    }
    install_menu(items, pos, window, app);
}

/// Put a finished row set on screen.
fn install_menu(
    items: Vec<MoonMenuItem>,
    pos: Point<Pixels>,
    window: &mut Window,
    app: &mut App,
) {
    window.open_fitted_moon_context_menu(
        app,
        "chart-volume-menu",
        pos,
        items,
        MENU_MIN_WIDTH,
        MENU_MAX_WIDTH,
    );
}

/// Rewrite one module and hand the result to the panel.
fn edit_row(
    panel: &Entity<ChartPanel>,
    row_ix: usize,
    app: &mut App,
    edit: impl FnOnce(&mut ChartLabelRow),
) {
    panel.update(app, |panel, cx| {
        let mut cfg = panel.effective_labels(cx);
        let Some(row) = cfg.rows.get_mut(row_ix) else {
            return;
        };
        edit(row);
        panel.apply_volume_edit(cfg, cx);
    });
}

/// The module's volume captions, which is what every setting here writes to.
fn volume_parts(row: &mut ChartLabelRow) -> impl Iterator<Item = &mut moon_core::config::ChartLabelPart> {
    row.parts
        .iter_mut()
        .filter(|part| part.field.in_volume_block())
}

/// Set the period on EVERY caption of the module.
///
/// One action for the whole block, which is what the menu means: the heading, the two sides and
/// their total all state the same period, and moving them one at a time would print a block whose
/// own lines disagree.
fn set_span(row: &mut ChartLabelRow, span: LabelSpan, window: LabelWindow) {
    // The VOLUME captions only. A movement figure sharing the module reads its window and knows
    // nothing about a custom span: giving it one would print a prefix naming five hundred trades
    // over a figure still measured across the hour.
    for part in volume_parts(row) {
        part.span = span;
        if span == LabelSpan::Window {
            part.window = window;
        }
    }
}

/// A period row: checked when it is the one in force.
fn span_item(
    panel: &Entity<ChartPanel>,
    row_ix: usize,
    label: String,
    key: String,
    current: (LabelSpan, LabelWindow),
    want: (LabelSpan, LabelWindow),
    pos: Point<Pixels>,
) -> MoonMenuItem {
    let panel = panel.clone();
    MoonMenuItem::with_key(key, label)
        .selected(current == want)
        .on_click(move |_, window: &mut Window, app: &mut App| {
            edit_row(&panel, row_ix, app, |row| set_span(row, want.0, want.1));
            // Rebuilt like every other row here: the menu holds a finished snapshot, so without
            // this the chart moves to the new period while the list still marks the old one.
            open_menu(&panel, row_ix, pos, window, app);
        })
}

/// Build the menu for one volume module.
fn build_items(
    panel: Entity<ChartPanel>,
    row: &ChartLabelRow,
    row_ix: usize,
    pos: Point<Pixels>,
) -> Vec<MoonMenuItem> {
    // What the block is set to now, read from the first caption that reads a period: every caption
    // in the module carries the same one, because `set_span` writes them together.
    let current = row
        .parts
        .iter()
        .find(|p| p.field.in_volume_block())
        .map(|p| (p.span, p.window))
        .unwrap_or((LabelSpan::Window, LabelWindow::default()));
    let mut items: Vec<MoonMenuItem> = Vec::new();
    items.push(MoonMenuItem::label(t!("chart_labels.menu.period").to_string()));
    // Sorted HERE rather than trusted from the constant: the list is picked by length, and the one
    // time it was assembled by hand it read `30м` then `2м`.
    let mut periods = QUICK_PERIODS;
    periods.sort_by_key(|window| window.millis());
    for window in periods {
        items.push(span_item(
            &panel,
            row_ix,
            t!(window.locale_key()).to_string(),
            format!("vol-w-{window:?}"),
            current,
            (LabelSpan::Window, window),
            pos,
        ));
    }
    items.push(MoonMenuItem::separator());
    items.push(MoonMenuItem::label(t!("chart_labels.menu.trades").to_string()));
    for trades in QUICK_TRADES {
        items.push(span_item(
            &panel,
            row_ix,
            t!("chart_labels.span.trades", n = trades).to_string(),
            format!("vol-t-{trades}"),
            current,
            (LabelSpan::Trades(trades), LabelWindow::default()),
            pos,
        ));
    }
    items.push(MoonMenuItem::separator());

    // The unit, as a pair of checks rather than a toggle: "in money" and "in coin" are both states
    // a reader looks for, and a single line saying one of them hides the other.
    let units = row
        .parts
        .iter()
        .find(|p| p.field.uses_volume_units())
        .map(|p| p.units)
        .unwrap_or_default();
    for unit in VolumeUnits::ALL {
        let panel = panel.clone();
        items.push(
            MoonMenuItem::with_key(format!("vol-u-{unit:?}"), t!(unit.locale_key()).to_string())
                .checked(units == unit)
                .on_click(move |_, window: &mut Window, app: &mut App| {
                    edit_row(&panel, row_ix, app, |row| {
                        for part in volume_parts(row) {
                            part.units = unit;
                        }
                    });
                    open_menu(&panel, row_ix, pos, window, app);
                }),
        );
    }
    items.push(MoonMenuItem::separator());

    // Which captions the block prints. `Bv` and `Sv` are not on this list: they are what the block
    // IS, and a menu that could remove both would leave a module with a heading and nothing under
    // it — the editor is where a module is taken apart.
    for field in TOGGLE_FIELDS {
        let present = row.parts.iter().any(|p| p.is_used() && p.field == field);
        let panel = panel.clone();
        items.push(
            MoonMenuItem::with_key(
                format!("vol-f-{field:?}"),
                t!(field.locale_key()).to_string(),
            )
            .checked(present)
            .on_click(move |_, window: &mut Window, app: &mut App| {
                edit_row(&panel, row_ix, app, |row| toggle_field(row, field));
                open_menu(&panel, row_ix, pos, window, app);
            }),
        );
    }

    // The bars, which belong to the two sides and are switched for the module as a whole.
    let bars = row
        .parts
        .iter()
        .any(|p| p.is_used() && p.field.uses_volume_bar() && p.bar);
    let panel_bars = panel.clone();
    items.push(
        MoonMenuItem::with_key("vol-bars", t!("chart_labels.menu.bars").to_string())
            .checked(bars)
            .on_click(move |_, window: &mut Window, app: &mut App| {
                edit_row(&panel_bars, row_ix, app, |row| {
                    let on = !row
                        .parts
                        .iter()
                        .any(|p| p.is_used() && p.field.uses_volume_bar() && p.bar);
                    for part in volume_parts(row) {
                        part.bar = on;
                    }
                });
                open_menu(&panel_bars, row_ix, pos, window, app);
            }),
    );
    items
}

/// Add a caption to the module, or remove the one it already has.
///
/// A newly added caption inherits the module's period, so it lands showing the same thing the block
/// beside it does rather than the default hour.
fn toggle_field(row: &mut ChartLabelRow, field: ChartLabelField) {
    if let Some(ix) = row
        .parts
        .iter()
        .position(|p| p.is_used() && p.field == field)
    {
        row.remove_part(ix);
        return;
    }
    let inherited = row
        .parts
        .iter()
        .find(|p| p.field.in_volume_block())
        .map(|p| (p.span, p.window, p.units));
    if !row.push_part(field) {
        return;
    }
    let Some(ix) = row.parts.iter().position(|p| p.field == field) else {
        return;
    };
    if let Some((span, window, units)) = inherited {
        row.parts[ix].span = span;
        row.parts[ix].window = window;
        row.parts[ix].units = units;
    }
}

#[cfg(test)]
mod tests;
