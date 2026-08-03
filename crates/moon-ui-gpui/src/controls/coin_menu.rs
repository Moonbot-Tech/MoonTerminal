//! Shared right-click context menu for a market token. It is used wherever a token is shown,
//! including chart order lines, token or strategy cells in Orders, and token cells in Assets and
//! Report. The menu selects applicable entries from the context available in [`CoinMenuCtx`]: a
//! strategy (`strat_id`), an order (`order_uid` and `side`), and the source panel filter's selected
//! cores.
//!
//! Every blacklist action reads the current list, appends the token with deduplication, and sends
//! the entire list. MoonProto has no add-one-token command for either the core-wide blacklist via
//! `set_blacklist` or the strategy blacklist stored in `CoinsBlackList` via `edit_strategies`.

use gpui::*;
use moon_ui::{MoonContextMenuWindowExt as _, MoonMenuItem, MoonTone, MoonWindowExt as _};
use rust_i18n::t;

use moon_core::session::CoreId;

use crate::Backend;

/// MoonProto strategy field containing the token blacklist. Its string value is a comma-separated
/// token list, matching the core-wide blacklist format.
const FIELD_COINS_BLACK_LIST: &str = "CoinsBlackList";

/// Popup menu width in pixels. It is wider than the 170-pixel order menu to accommodate core-wide
/// blacklist labels containing a core name.
pub(crate) const MENU_WIDTH: f32 = 220.0;

/// Side of the order line at the clicked point. The chart distinguishes Buy from Sell by line type;
/// Buy provides Cancel, while Sell provides Join All Sells and Split actions.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OrderSide {
    Buy,
    Sell,
}

/// Origin of the menu. It controls minor behavior such as omitting Open on Chart when already there.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CoinMenuOrigin {
    OrderTable,
    ChartLine,
}

/// Context for the clicked token. Each caller supplies the data available at its location, and the
/// menu uses field presence to determine which entries to show.
pub struct CoinMenuCtx {
    pub core: CoreId,
    /// Source core name used in labels and the core-blacklist entry.
    pub core_name: String,
    /// Order or token market, such as `ADAUSDT`, used for navigation, joining, and splitting.
    pub market: String,
    /// Base token, such as `ADA`, written to blacklist values.
    pub coin: String,
    /// Cores selected by the source panel's filter for the selected-cores blacklist action. That
    /// entry appears only for more than one core. Chart lines supply `[core]`, so it remains hidden.
    pub selected_cores: Vec<CoreId>,
    /// Order strategy when `strat_id != 0`, enabling the strategy section. None means manual or join.
    pub strat_id: Option<u64>,
    /// Strategy name used in the entry label. None falls back to the strategy ID.
    pub strat_name: Option<String>,
    /// Order UID enabling the order section. None represents a token point without an order.
    pub order_uid: Option<u64>,
    /// Side for chart lines. A table supplies None, producing Edit and Cancel order actions.
    pub side: Option<OrderSide>,
    /// Order position direction used by `join_sells`.
    pub short: bool,
    pub origin: CoinMenuOrigin,
}

/// Opens the shared token context menu at window-coordinate `pos`, usually `event.position`.
pub fn open_coin_menu(
    ctx: CoinMenuCtx,
    backend: Entity<Backend>,
    pos: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let items = build_items(&ctx, &backend, cx);
    window.open_moon_context_menu(cx, "coin-context-menu", pos, items, MENU_WIDTH);
}

/// Builds context-dependent entries. It reads core state through `cx` to mark existing blacklist
/// membership and gates the strategy entry on its schema.
fn build_items(ctx: &CoinMenuCtx, backend: &Entity<Backend>, cx: &App) -> Vec<MoonMenuItem> {
    let b = backend.read(cx);
    let core = ctx.core;
    let coin = ctx.coin.clone();
    let mut items: Vec<MoonMenuItem> = Vec::new();

    // Navigation requires a market. Do not duplicate Open when already on the chart, and skip
    // balance rows that have no market.
    let has_market = !ctx.market.is_empty();
    if has_market && ctx.origin != CoinMenuOrigin::ChartLine {
        let backend_open = backend.clone();
        let market = ctx.market.clone();
        items.push(
            MoonMenuItem::with_key("coin-open", t!("coin_menu.open").to_string()).on_click(
                move |_, window, app| {
                    window.close_context_menu(app);
                    backend_open.update(app, |b, bcx| {
                        b.open_on_main((core, market.clone()), false);
                        bcx.notify();
                    });
                },
            ),
        );
    }
    if has_market {
        let backend_cmp = backend.clone();
        let market = ctx.market.clone();
        items.push(
            MoonMenuItem::with_key("coin-compare", t!("coin_menu.compare").to_string()).on_click(
                move |_, window, app| {
                    window.close_context_menu(app);
                    backend_cmp.update(app, |b, bcx| {
                        b.open_compare_request = Some((core, market.clone()));
                        b.open_compare_request_rev = b.open_compare_request_rev.wrapping_add(1);
                        bcx.notify();
                    });
                },
            ),
        );
    }

    // Blacklist actions.
    if !items.is_empty() {
        items.push(MoonMenuItem::separator());
    }

    // Add to the current core's global blacklist.
    let (_, cur_text) = core_blacklist(b, core);
    let in_core = blacklist_contains(&cur_text, &coin);
    {
        let backend_bl = backend.clone();
        let coin_c = coin.clone();
        items.push(
            MoonMenuItem::with_key(
                "coin-bl-core",
                t!("coin_menu.bl_core", core = ctx.core_name.clone()).to_string(),
            )
            .checked(in_core)
            .on_click(move |_, window, app| {
                window.close_context_menu(app);
                backend_bl.update(app, |b, _| add_to_core_blacklist(b, core, &coin_c));
            }),
        );
    }

    // Add to every core selected in the panel filter, but only when more than one is selected.
    if ctx.selected_cores.len() > 1 {
        let cores = ctx.selected_cores.clone();
        let all_in = cores
            .iter()
            .all(|&c| blacklist_contains(&core_blacklist(b, c).1, &coin));
        let backend_m = backend.clone();
        let coin_m = coin.clone();
        items.push(
            MoonMenuItem::with_key(
                "coin-bl-cores",
                t!("coin_menu.bl_cores", n = cores.len()).to_string(),
            )
            .checked(all_in)
            .on_click(move |_, window, app| {
                window.close_context_menu(app);
                backend_m.update(app, |b, _| {
                    for &c in &cores {
                        add_to_core_blacklist(b, c, &coin_m);
                    }
                });
            }),
        );
    }

    // Add to the order strategy's blacklist only when the strategy is known and its schema contains
    // `CoinsBlackList`; otherwise the view editor would silently discard the field edit.
    if let Some(sid) = ctx.strat_id {
        if strategy_has_blacklist_field(b, core, sid) {
            let in_strat = blacklist_contains(&strategy_blacklist(b, core, sid), &coin);
            let label = match ctx.strat_name.as_deref().filter(|n| !n.is_empty()) {
                Some(name) => t!("coin_menu.bl_strategy", name = name.to_string()).to_string(),
                None => t!("coin_menu.bl_strategy", name = sid.to_string()).to_string(),
            };
            let backend_s = backend.clone();
            let coin_s = coin.clone();
            items.push(
                MoonMenuItem::with_key("coin-bl-strat", label)
                    .checked(in_strat)
                    .on_click(move |_, window, app| {
                        window.close_context_menu(app);
                        backend_s
                            .update(app, |b, _| add_to_strategy_blacklist(b, core, sid, &coin_s));
                    }),
            );
        }
    }

    // Strategy actions.
    if let Some(sid) = ctx.strat_id {
        items.push(MoonMenuItem::separator());
        let backend_g = backend.clone();
        items.push(
            MoonMenuItem::with_key("coin-strat-goto", t!("coin_menu.strategy_goto").to_string())
                .on_click(move |_, window, app| {
                    window.close_context_menu(app);
                    let owner_display = window.display(app).map(|d| d.id());
                    crate::strategies::open_goto(
                        backend_g.clone(),
                        core,
                        sid,
                        Some(window.window_handle()),
                        owner_display,
                        app,
                    );
                }),
        );
    }

    // Order actions.
    if let Some(uid) = ctx.order_uid {
        items.push(MoonMenuItem::separator());
        let backend_e = backend.clone();
        items.push(
            MoonMenuItem::with_key("coin-order-edit", t!("chart.order_menu.edit").to_string())
                .on_click(move |_, window, app| {
                    window.close_context_menu(app);
                    crate::panels::open_order_edit(backend_e.clone(), core, uid, window, app);
                }),
        );
        match ctx.side {
            Some(OrderSide::Sell) => {
                let backend_j = backend.clone();
                let market_j = ctx.market.clone();
                let short = ctx.short;
                items.push(
                    MoonMenuItem::with_key(
                        "coin-order-join",
                        t!("chart.order_menu.join_sells").to_string(),
                    )
                    .on_click(move |_, window, app| {
                        window.close_context_menu(app);
                        backend_j.update(app, |b, _| {
                            let _ = b.session.join_sells(core, market_j.clone(), short);
                        });
                    }),
                );
                let backend_sp = backend.clone();
                items.push(
                    MoonMenuItem::with_key(
                        "coin-order-split",
                        t!("chart.order_menu.split").to_string(),
                    )
                    .on_click(move |_, window, app| {
                        window.close_context_menu(app);
                        backend_sp.update(app, |b, _| {
                            let _ = b.session.split_order(core, uid, SPLIT_PARTS);
                        });
                    }),
                );
            }
            // A chart Buy line or a table row with side=None cancels the entire order by UID.
            Some(OrderSide::Buy) | None => {
                let backend_c = backend.clone();
                items.push(
                    MoonMenuItem::with_key(
                        "coin-order-cancel",
                        t!("chart.order_menu.cancel").to_string(),
                    )
                    .tone(MoonTone::Danger)
                    .on_click(move |_, window, app| {
                        window.close_context_menu(app);
                        backend_c.update(app, |b, _| {
                            let _ = b.session.cancel_order(core, uid);
                        });
                    }),
                );
            }
        }
    }

    items
}

/// Number of parts produced by Split Order, matching the chart-line action and Moonbot's default.
const SPLIT_PARTS: i32 = 2;

// Blacklist read and write helpers.

/// Returns the current core-wide blacklist as `(enabled, list text)`. A missing settings snapshot
/// yields `(false, "")`.
fn core_blacklist(b: &Backend, core: CoreId) -> (bool, String) {
    b.session
        .store()
        .core(core)
        .and_then(|cd| cd.client_settings.as_ref())
        .map(|cs| (cs.use_blacklist, cs.blacklist_text.clone()))
        .unwrap_or((false, String::new()))
}

/// Appends a token to the core-wide blacklist and enables it. Enabling is required for the addition
/// to affect trading because the command sends the flag and text together. This is idempotent: an
/// existing token keeps the same text, while the blacklist is still enabled.
fn add_to_core_blacklist(b: &Backend, core: CoreId, coin: &str) {
    let (_, text) = core_blacklist(b, core);
    let new = blacklist_add(&text, coin);
    if let Err(err) = b.session.set_blacklist(core, true, new) {
        log::warn!(
            "coin_menu: add {coin} to core {} blacklist failed: {err:#}",
            moon_core::feed::core_label(core)
        );
    }
}

/// Returns the strategy's read-only `CoinsBlackList` snapshot. The server omits fields equal to the
/// schema default, so a missing value is treated as empty and the first token starts a new list.
fn strategy_blacklist(b: &Backend, core: CoreId, sid: u64) -> String {
    b.session
        .store()
        .core(core)
        .and_then(|cd| cd.strategies.iter().find(|s| s.id == sid))
        .and_then(|s| {
            s.fields
                .iter()
                .find(|(k, _)| k == FIELD_COINS_BLACK_LIST)
                .map(|(_, v)| v.clone())
        })
        .unwrap_or_default()
}

/// Returns whether the strategy-kind schema identified by `kind_ordinal` contains
/// `CoinsBlackList`. Without it, the field edit would be silently ignored, so the entry is hidden.
fn strategy_has_blacklist_field(b: &Backend, core: CoreId, sid: u64) -> bool {
    let Some(cd) = b.session.store().core(core) else {
        return false;
    };
    let Some(row) = cd.strategies.iter().find(|s| s.id == sid) else {
        return false;
    };
    let Some(schema) = cd.schema.as_ref() else {
        return false;
    };
    schema
        .kinds
        .iter()
        .find(|k| k.ordinal == row.kind_ordinal)
        .is_some_and(|k| {
            k.sections
                .iter()
                .any(|s| s.fields.iter().any(|f| f.name == FIELD_COINS_BLACK_LIST))
        })
}

/// Appends a token to the strategy's `CoinsBlackList` through the shared field editor.
fn add_to_strategy_blacklist(b: &Backend, core: CoreId, sid: u64, coin: &str) {
    let cur = strategy_blacklist(b, core, sid);
    let new = blacklist_add(&cur, coin);
    let edits = vec![(sid, vec![(FIELD_COINS_BLACK_LIST.to_string(), new)])];
    if let Err(err) = b.session.edit_strategies(core, edits) {
        log::warn!("coin_menu: add {coin} to strategy {sid}@{core} blacklist failed: {err:#}");
    }
}

/// Checks a comma-separated list for a token, ignoring ASCII case and surrounding whitespace.
///
/// Deliberately a LITERAL comparison, not `symbol::coin_match_key` — and this is now MEASURED
/// rather than assumed. MoonProto's `rebuild_market_blacklisted_cfg` compares each list entry
/// against the market's own `market_currency` with `same_text_ascii`, an exact case-insensitive
/// match with no folding. So the token to write and to compare is the core's spelling
/// (`BTC_RP`, `1kBONKPERP`), which is what `MarketLabel::coin` carries and what every caller of
/// this menu now supplies. Folding here would claim "already listed" about an entry the core
/// does not associate with the market.
///
/// The Analytics coin axis DOES fold, and it writes too (`analytics::tuner::coins`), so the two
/// paths still disagree for contract-qualified coins. That one is now the side known to be
/// wrong; it is left alone here because changing what a working list writes belongs in its own
/// change, not smuggled into this one.
fn blacklist_contains(text: &str, coin: &str) -> bool {
    text.split(',').any(|s| s.trim().eq_ignore_ascii_case(coin))
}

/// Appends a token to a comma-separated list with case-insensitive deduplication. An existing token
/// leaves the list unchanged, while an empty list becomes the token alone.
fn blacklist_add(text: &str, coin: &str) -> String {
    if blacklist_contains(text, coin) {
        return text.to_string();
    }
    let base = text.trim().trim_end_matches(',').trim_end();
    if base.is_empty() {
        coin.to_string()
    } else {
        format!("{base},{coin}")
    }
}

#[cfg(test)]
mod tests;
