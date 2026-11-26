//! The Screener's "copy the coins" entries in the row's coin menu (#1164, copy half).
//!
//! Two lists of the rows the table currently lists, in its current sort and after every control
//! of the bottom bar and the core picker: all of them, and the ones from the top through the
//! clicked row — MoonBot's "copy markets above selected", where the sort is the selection. The
//! clipboard gets the coin as the Market cell prints it (the coin, not the pair), comma-separated
//! with no spaces, which is the form a strategy coin list takes.
//!
//! The list walks the displayed leads; a coin listed on multiple venues is written once, where
//! it first appears. The lists are taken when the menu opens, so the entry copies the rows the user
//! was looking at when he clicked, not a publication that arrived while the menu stood open.

use std::collections::HashSet;

use gpui::ClipboardItem;
use moon_ui::{MoonMenuItem, MoonNotification, MoonWindowExt as _};
use rust_i18n::t;

use moon_core::session::CoreId;

use super::table::Entry;

/// The two coin lists one menu offers, already de-duplicated.
pub(super) struct CoinLists {
    /// Every listed row's coin, in table order.
    pub(super) on_screen: Vec<String>,
    /// The coins from the first listed row through the clicked one; `None` when the clicked row
    /// is no longer listed — a publication that arrived after the right click moved it out.
    pub(super) through_clicked: Option<Vec<String>>,
}

/// Collect both lists from the rows the table lists.
///
/// The clicked row is found by its identity — core and market — rather than by the line it was
/// drawn on: the list is rebuilt about once a second, and a line number from the frame the user
/// clicked can name another coin in the current one.
///
/// Args:
///     entries: The publication's entries, indexed by `rows`.
///     rows: Lead indices of the listed rows, in table order.
///     clicked: Lead core and canonical market of the right-clicked row.
///
/// Returns:
///     The coin lists for the two menu entries.
pub(super) fn coin_lists(entries: &[Entry], rows: &[usize], clicked: (CoreId, &str)) -> CoinLists {
    let listed = || rows.iter().filter_map(|&index| entries.get(index));
    let position = listed().position(|e| e.row.core == clicked.0 && e.row.market == clicked.1);
    CoinLists {
        on_screen: unique_coins(listed().map(|e| e.row.coin.as_str())),
        through_clicked: position
            .map(|last| unique_coins(listed().take(last + 1).map(|e| e.row.coin.as_str()))),
    }
}

/// Trim each coin, drop the empty ones and keep the first occurrence of each spelling.
///
/// Spellings are compared exactly: `1000BONK` and `1kBONK` are what two exchanges call one coin,
/// and a core matches its strategy lists by exact text, so each keeps its own entry.
///
/// Args:
///     coins: Coin names in table order.
///
/// Returns:
///     The distinct coins, in the order they first appear.
pub(super) fn unique_coins<'a>(coins: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut seen = HashSet::new();
    coins
        .into_iter()
        .map(str::trim)
        .filter(|coin| !coin.is_empty() && seen.insert(*coin))
        .map(str::to_string)
        .collect()
}

/// Build the menu entries for both lists; an empty list gets no entry.
///
/// Args:
///     lists: The lists taken when the menu opened.
///
/// Returns:
///     Zero, one or two entries for the coin menu's trailing slot, each counting its coins.
pub(super) fn copy_items(lists: CoinLists) -> Vec<MoonMenuItem> {
    let mut items = Vec::new();
    if !lists.on_screen.is_empty() {
        items.push(copy_item(
            "screener-copy-on-screen",
            t!("screener.copy_coins_on_screen").to_string(),
            lists.on_screen,
        ));
    }
    if let Some(coins) = lists.through_clicked.filter(|coins| !coins.is_empty()) {
        items.push(copy_item(
            "screener-copy-through-row",
            t!("screener.copy_coins_through_row").to_string(),
            coins,
        ));
    }
    items
}

/// One entry that writes `coins` to the clipboard and says how many went.
///
/// Coin names are not secret, but a list of them is not logged either: the toast is the receipt.
///
/// Args:
///     key: The entry's stable menu key.
///     label: The translated caption.
///     coins: The distinct, non-empty coins in table order.
///
/// Returns:
///     The menu entry, its trailing slot showing the count.
fn copy_item(key: &'static str, label: String, coins: Vec<String>) -> MoonMenuItem {
    let count = coins.len();
    let text = coins.join(",");
    MoonMenuItem::with_key(key, label)
        .right_label(count.to_string())
        .on_click(move |_, window, app| {
            window.close_context_menu(app);
            app.write_to_clipboard(ClipboardItem::new_string(text.clone()));
            window.push_notification(
                MoonNotification::success(t!("screener.coins_copied", n = count).to_string()),
                app,
            );
        })
}

#[cfg(test)]
mod tests;
