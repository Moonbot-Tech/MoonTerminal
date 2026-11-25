//! Report multi-selection menu, with stable ordering and add-only blacklist targets.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::*;
use moon_core::session::CoreId;
use moon_ui::{MoonContextMenuWindowExt as _, MoonMenuItem, MoonNotification, MoonWindowExt as _};
use rust_i18n::t;

use super::blacklist::{hour_rows, target_row, temp_ban_symbol, write_core_blacklist_targets};
use super::names::CoinNames;
use super::undo::UndoRecord;
use super::{CoinMenuCtx, MENU_MAX_WIDTH, MENU_MIN_WIDTH};
use crate::Backend;
use crate::display_text::plural_key;
use crate::panels::{ReportScope, open_scoped_report};

/// Counts come from the selected rows and their distinct printed coins.
pub(crate) struct MultiFacts {
    pub(crate) rows: usize,
    pub(crate) coins: Vec<String>,
}

/// Keep one menu click or shortcut from opening more than twenty trade windows.
pub(crate) fn capped_trade_rows(rows: &[usize]) -> &[usize] {
    &rows[..rows.len().min(20)]
}

/// The slots in a compact Report multi-selection menu.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MultiSlot {
    TradeWindows(usize),
    Filter(usize),
    NewReport,
    Copy,
    Permanent(usize),
    Temporary(usize),
    Sep,
}

/// Order row commands, coin commands, then blacklist writes with one separator between groups.
fn plan_multi(facts: &MultiFacts) -> Vec<MultiSlot> {
    use MultiSlot::*;
    let coins = facts.coins.len();
    vec![
        TradeWindows(facts.rows),
        Sep,
        Filter(coins),
        NewReport,
        Copy,
        Sep,
        Permanent(coins),
        Temporary(coins),
    ]
}

impl MultiSlot {
    /// Emphasize the multi-selection equivalent of opening one trade window.
    fn emphasized(self) -> bool {
        matches!(self, Self::TradeWindows(_))
    }

    /// Localize a slot's caption using its row or coin count.
    fn label(self) -> String {
        match self {
            Self::TradeWindows(n) => multi_count("coin_menu.multi.windows", n),
            Self::Filter(n) => multi_count("coin_menu.multi.filter", n),
            Self::NewReport => t!("coin_menu.multi.report").to_string(),
            Self::Copy => t!("coin_menu.multi.copy").to_string(),
            Self::Permanent(n) => multi_count("coin_menu.multi.permanent", n),
            Self::Temporary(n) => multi_count("coin_menu.multi.temporary", n),
            Self::Sep => String::new(),
        }
    }
}

/// Translate a counted message using the shared locale plural rules.
pub(crate) fn multi_count(base: &str, n: usize) -> String {
    t!(plural_key(base, n as u64), n = n).to_string()
}

/// Preserve the first printed spelling in visible selection order, skipping blank tickers.
pub(crate) fn distinct_tickers(coins: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    coins
        .into_iter()
        .filter_map(|coin| {
            let coin = coin.trim();
            (!coin.is_empty() && seen.insert(coin.to_ascii_uppercase())).then(|| coin.to_string())
        })
        .collect()
}

/// Write one ticker per line and confirm the number copied in the originating window.
pub(crate) fn copy_tickers(coins: &[String], window: &mut Window, app: &mut App) {
    app.write_to_clipboard(ClipboardItem::new_string(tickers_text(coins)));
    window.push_notification(
        MoonNotification::success(multi_count("coin_menu.multi.copied", coins.len())),
        app,
    );
}

/// Format the order-preserving ticker set for the clipboard.
fn tickers_text(coins: &[String]) -> String {
    coins.join("\n")
}

/// A callback into the original Report, with stable inputs captured before the popup opens.
pub(crate) type ReportSelectionAction = Box<dyn Fn(&mut Window, &mut App)>;

/// Report-owned actions that need its exact query or one stable target per trade.
pub(crate) struct MultiReportActions {
    pub(crate) trade_windows: Option<ReportSelectionAction>,
    pub(crate) filter: ReportSelectionAction,
}

/// Open the compact Report menu using the same fitted width as a single row.
pub(crate) fn open_multi_coin_menu(
    facts: MultiFacts,
    rows: &[CoinMenuCtx],
    actions: MultiReportActions,
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut App,
) {
    let MultiReportActions {
        mut trade_windows,
        filter,
    } = actions;
    let mut filter = Some(filter);
    let scope = ReportScope::for_coins(
        rows.iter().map(|row| row.core).collect(),
        facts.coins.clone(),
    );
    let group = rows.first().and_then(|row| row.workspace_group.clone());
    let targets = BlacklistTargets::resolve(rows, backend.read(cx));
    let has_coins = !facts.coins.is_empty();
    let items = plan_multi(&facts)
        .into_iter()
        .map(|slot| {
            let label = slot.label();
            let item = match slot {
                MultiSlot::TradeWindows(_) => {
                    let item = MoonMenuItem::with_key("multi-trade-windows", label);
                    match trade_windows.take() {
                        Some(action) => item.on_click(move |_, window, app| {
                            window.close_context_menu(app);
                            action(window, app);
                        }),
                        None => item.disabled(true),
                    }
                }
                MultiSlot::Filter(_) => {
                    let item = MoonMenuItem::with_key("multi-filter", label);
                    match filter.take().filter(|_| has_coins) {
                        Some(action) => item.on_click(move |_, window, app| {
                            window.close_context_menu(app);
                            action(window, app);
                        }),
                        None => item.disabled(true),
                    }
                }
                MultiSlot::NewReport => new_report_item(label, scope.clone(), backend),
                MultiSlot::Copy => {
                    let coins = facts.coins.clone();
                    MoonMenuItem::with_key("multi-copy", label)
                        .disabled(!has_coins)
                        .on_click(move |_, window, app| {
                            window.close_context_menu(app);
                            copy_tickers(&coins, window, app);
                        })
                }
                MultiSlot::Permanent(_) => targets.permanent_item(label, backend, group.clone()),
                MultiSlot::Temporary(_) => MoonMenuItem::with_key("multi-temp", label)
                    .disabled(targets.temporary.is_empty())
                    .submenu(hour_rows(
                        "multi-temp",
                        backend,
                        group.clone(),
                        targets.temporary.clone(),
                    )),
                MultiSlot::Sep => MoonMenuItem::separator(),
            };
            if slot.emphasized() {
                item.emphasized(true)
            } else {
                item
            }
        })
        .collect();
    window.open_fitted_moon_context_menu(
        cx,
        "multi-coin-context-menu",
        window.mouse_position(),
        items,
        MENU_MIN_WIDTH,
        MENU_MAX_WIDTH,
    );
}

/// Defer one independent scoped window until the source menu's window lease has ended.
fn new_report_item(
    label: String,
    scope: Option<ReportScope>,
    backend: &Entity<Backend>,
) -> MoonMenuItem {
    let backend = backend.clone();
    let item = MoonMenuItem::with_key("multi-report", label)
        .right_label(t!("coin_menu.new_report").to_string());
    let Some(scope) = scope else {
        return item.disabled(true);
    };
    item.on_click(move |_, window, app| {
        window.close_context_menu(app);
        let owner = window.window_handle();
        let owner_display = window.display(app).map(|display| display.id());
        let backend = backend.clone();
        let scope = scope.clone();
        app.defer(move |app| open_scoped_report(backend, scope, Some(owner), owner_display, app));
    })
}

/// All blacklist targets are resolved before the menu opens, under each core's own spelling.
struct BlacklistTargets {
    own: Vec<(CoreId, String)>,
    selected: Vec<(CoreId, String)>,
    /// The Report's ticker for each permanent target, even when destination spellings differ.
    tickers: Rc<HashMap<(CoreId, String), String>>,
    temporary: Vec<(CoreId, String)>,
    source_name: String,
}

impl BlacklistTargets {
    /// Resolve the source row destinations and the optional selected-cores expansion.
    fn resolve(rows: &[CoinMenuCtx], backend: &Backend) -> Self {
        let mut own = Vec::new();
        let mut selected = Vec::new();
        let mut tickers = HashMap::new();
        let mut temporary = Vec::new();
        let mut source_name = String::new();
        for row in rows.iter().filter(|row| !row.coin.is_empty()) {
            if own.is_empty() {
                source_name.clone_from(&row.core_name);
            }
            let names = CoinNames::resolve_for(backend, row, Some(&row.selected_cores));
            let shown = row.shown_coin();
            let ticker = if shown.is_empty() { &row.coin } else { shown }.to_ascii_uppercase();
            if let Some(name) = names.on(row.core) {
                own.push((row.core, name.coin.clone()));
                tickers
                    .entry((row.core, name.coin.to_ascii_uppercase()))
                    .or_insert_with(|| ticker.clone());
            }
            for &core in &row.selected_cores {
                if let Some(name) = names.on(core) {
                    selected.push((core, name.coin.clone()));
                    tickers
                        .entry((core, name.coin.to_ascii_uppercase()))
                        .or_insert_with(|| ticker.clone());
                }
            }
            if let Some(symbol) = temp_ban_symbol(&names, row.core) {
                temporary.push((row.core, symbol));
            }
        }
        Self {
            own: distinct_targets(own),
            selected: distinct_targets(selected),
            tickers: Rc::new(tickers),
            temporary: distinct_targets(temporary),
            source_name,
        }
    }

    /// Offer the row cores and, where it expands the destinations, the selected cores.
    fn permanent_item(
        &self,
        label: String,
        backend: &Entity<Backend>,
        group: Option<String>,
    ) -> MoonMenuItem {
        let own_core_count = target_core_count(&self.own);
        let selected_core_count = target_core_count(&self.selected);
        let mut rows = Vec::new();
        if !self.own.is_empty() {
            let label = if own_core_count == 1 {
                t!("coin_menu.target_core", core = self.source_name.as_str()).to_string()
            } else {
                t!("coin_menu.multi.source_cores", n = own_core_count).to_string()
            };
            rows.push(add_row(
                "multi-bl-core",
                label,
                &self.own,
                self.tickers.clone(),
                backend,
                group.clone(),
            ));
        }
        if selected_core_count > 1
            && self
                .selected
                .iter()
                .any(|target| !self.own.contains(target))
        {
            let label = t!("coin_menu.target_cores", n = selected_core_count).to_string();
            rows.push(add_row(
                "multi-bl-cores",
                label,
                &self.selected,
                self.tickers.clone(),
                backend,
                group,
            ));
        }
        MoonMenuItem::with_key("multi-bl", label)
            .disabled(rows.is_empty())
            .submenu(rows)
    }
}

/// Deduplicate one core's equivalent list entries while retaining the first resolved spelling.
fn distinct_targets(targets: Vec<(CoreId, String)>) -> Vec<(CoreId, String)> {
    let mut seen = HashSet::new();
    targets
        .into_iter()
        .filter(|(core, token)| seen.insert((*core, token.to_ascii_uppercase())))
        .collect()
}

/// Count unique cores even if a target contains several tickers on the same core.
fn target_core_count(targets: &[(CoreId, String)]) -> usize {
    targets
        .iter()
        .map(|(core, _)| *core)
        .collect::<HashSet<_>>()
        .len()
}

/// Reuse the click-time workspace guard and always pass explicit add mode.
fn add_row(
    key: &'static str,
    label: String,
    targets: &[(CoreId, String)],
    tickers: Rc<HashMap<(CoreId, String), String>>,
    backend: &Entity<Backend>,
    group: Option<String>,
) -> MoonMenuItem {
    let cores = targets.iter().map(|(core, _)| *core).collect();
    let targets = targets.to_vec();
    target_row(
        key,
        label,
        false,
        backend.clone(),
        group,
        cores,
        move |b, _| {
            UndoRecord::new(write_core_blacklist_targets(b, &targets, false), |steps| {
                let coins: HashSet<_> = steps
                    .iter()
                    .map(|step| {
                        tickers
                            .get(&(step.core, step.coin.to_ascii_uppercase()))
                            .map_or(step.coin.as_str(), String::as_str)
                    })
                    .collect();
                multi_count("coin_menu.undo.added_multi", coins.len())
            })
        },
    )
}

#[cfg(test)]
mod tests;
