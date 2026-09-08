//! The two blacklist entries of the coin menu, each a submenu over the same targets.
//!
//! Why nested rather than flat, which is how the permanent list used to be listed: the temporary
//! list doubles every target and multiplies it by five durations. Flat, that is over a dozen rows
//! in a menu that also carries navigation, strategy and order actions — and the reader's first
//! question is which LIST, long before it is which core.
//!
//! The permanent list is a comma-separated text this terminal rewrites in full; the temporary one
//! is a delta the feed merges at send time, because the core writes that list too. See
//! [`moon_core::feed::CoreCmd::SetTempBlacklist`].

use std::time::Duration;

use gpui::*;
use moon_ui::{MoonMenuItem, MoonWindowExt as _};
use rust_i18n::t;

use moon_core::config::TempBanSpan;
use moon_core::session::CoreId;

use super::{
    CoinMenuBranch, CoinMenuCtx, add_to_core_blacklist, add_to_strategy_blacklist,
    blacklist_contains, core_blacklist, open_coin_menu_branch, strategy_blacklist,
    strategy_has_blacklist_field, workspace_action_allows_cores,
};
use crate::Backend;
use crate::display_text::fmt_duration_short;

/// The "add to a blacklist" submenu, over whichever of the three targets this context has.
pub(super) fn permanent_blacklist_item(
    ctx: &CoinMenuCtx,
    b: &Backend,
    backend: &Entity<Backend>,
    coin: &str,
    pos: Point<Pixels>,
    expanded: Option<CoinMenuBranch>,
) -> MoonMenuItem {
    let core = ctx.core;
    let mut rows: Vec<MoonMenuItem> = Vec::new();

    let (_, held) = core_blacklist(b, core);
    rows.push(target_row(
        "coin-bl-core",
        t!("coin_menu.target_core", core = ctx.core_name.clone()).to_string(),
        blacklist_contains(&held, coin),
        backend.clone(),
        ctx.workspace_group.clone(),
        vec![core],
        core_blacklist_writer(coin),
    ));

    if ctx.selected_cores.len() > 1 {
        let cores = ctx.selected_cores.clone();
        let all_in = cores
            .iter()
            .all(|&core| blacklist_contains(&core_blacklist(b, core).1, coin));
        rows.push(target_row(
            "coin-bl-cores",
            t!("coin_menu.target_cores", n = cores.len()).to_string(),
            all_in,
            backend.clone(),
            ctx.workspace_group.clone(),
            cores,
            core_blacklist_writer(coin),
        ));
    }

    // Only when the strategy's own schema carries the field: without it the edit is discarded by
    // the view editor without a word, so the row would promise something that cannot happen.
    if let Some(sid) = ctx.strat_id
        && strategy_has_blacklist_field(b, core, sid)
    {
        let label = match ctx.strat_name.as_deref().filter(|name| !name.is_empty()) {
            Some(name) => t!("coin_menu.target_strategy", name = name.to_string()).to_string(),
            None => t!("coin_menu.target_strategy", name = sid.to_string()).to_string(),
        };
        let in_strategy = blacklist_contains(&strategy_blacklist(b, core, sid), coin);
        rows.push(target_row(
            "coin-bl-strat",
            label,
            in_strategy,
            backend.clone(),
            ctx.workspace_group.clone(),
            vec![core],
            {
                let coin = coin.to_string();
                move |b, cores| {
                    for &core in cores {
                        // Re-checked inside the click for the same reason the workspace is:
                        // the schema can change between the menu opening and the press.
                        if strategy_has_blacklist_field(b, core, sid) {
                            add_to_strategy_blacklist(b, core, sid, &coin);
                        }
                    }
                }
            },
        ));
    }

    branch_row(
        "coin-bl",
        t!("coin_menu.bl_group").to_string(),
        CoinMenuBranch::Blacklist,
        rows,
        ctx,
        backend,
        pos,
        expanded,
    )
}

/// The "add to the temporary blacklist" submenu, or `None` when this context has no symbol to ban.
pub(super) fn temp_blacklist_item(
    ctx: &CoinMenuCtx,
    b: &Backend,
    backend: &Entity<Backend>,
    pos: Point<Pixels>,
    expanded: Option<CoinMenuBranch>,
) -> Option<MoonMenuItem> {
    let core = ctx.core;
    let symbol = temp_ban_symbol(b, core, ctx)?;
    let mut rows: Vec<MoonMenuItem> = Vec::new();

    rows.push(MoonMenuItem::label(
        t!("coin_menu.target_core", core = ctx.core_name.clone()).to_string(),
    ));
    rows.extend(hour_rows("tbl-core", backend, ctx, vec![core]));

    if ctx.selected_cores.len() > 1 {
        rows.push(MoonMenuItem::separator());
        rows.push(MoonMenuItem::label(
            t!("coin_menu.target_cores", n = ctx.selected_cores.len()).to_string(),
        ));
        rows.extend(hour_rows(
            "tbl-cores",
            backend,
            ctx,
            ctx.selected_cores.clone(),
        ));
    }

    // Lifting is offered wherever a ban is actually held, over the same targets it can be set on:
    // a ban placed on five cores that can only be lifted on one is a control that half works.
    // One pass over the targets, and one clock read for the whole menu: every row would otherwise
    // ask the OS again for the same instant.
    let now_ms = moon_core::util::now_unix_ms_i64();
    let mut held_cores: Vec<CoreId> = Vec::new();
    let mut left_here: Option<Duration> = None;
    // The same targets the preset rows above are offered for: the group is only listed when more
    // than one core is selected, and a lift that reached a core the menu never offered to ban would
    // act outside what the reader was shown.
    let group = (ctx.selected_cores.len() > 1).then_some(ctx.selected_cores.as_slice());
    for target in std::iter::once(core).chain(group.unwrap_or_default().iter().copied()) {
        let Some(target_symbol) = temp_ban_symbol(b, target, ctx) else {
            continue;
        };
        let Some(left) = b
            .session
            .store()
            .core(target)
            .and_then(|data| data.temp_ban_left(&target_symbol, now_ms))
        else {
            continue;
        };
        if target == core {
            left_here = Some(left);
        }
        held_cores.push(target);
    }
    held_cores.sort_unstable();
    held_cores.dedup();
    if !held_cores.is_empty() {
        rows.push(MoonMenuItem::separator());
        // The remaining time of the core the menu was opened on; the others may differ, and the
        // row says which one it is speaking about by naming the coin rather than a core.
        if let Some(left) = left_here {
            let stale = b
                .session
                .store()
                .core(core)
                .is_some_and(|data| data.client_settings_stale);
            let left = fmt_duration_short(left.as_secs_f64());
            rows.push(MoonMenuItem::label(
                t!(
                    if stale {
                        "coin_menu.tbl_left_stale"
                    } else {
                        "coin_menu.tbl_left"
                    },
                    symbol = symbol.clone(),
                    left = left
                )
                .to_string(),
            ));
        }
        let backend = backend.clone();
        let workspace_group = ctx.workspace_group.clone();
        let ctx = ctx.clone();
        rows.push(
            MoonMenuItem::with_key("tbl-clear", t!("coin_menu.tbl_clear").to_string()).on_click(
                move |_, window, app| {
                    window.close_context_menu(app);
                    backend.update(app, |b, _| {
                        if !workspace_action_allows_cores(
                            b,
                            workspace_group.as_deref(),
                            &held_cores,
                        ) {
                            return;
                        }
                        send_temp_ban(b, &held_cores, &ctx, None);
                    });
                },
            ),
        );
    }

    Some(branch_row(
        "coin-tbl",
        t!("coin_menu.tbl_group").to_string(),
        CoinMenuBranch::TempBlacklist,
        rows,
        ctx,
        backend,
        pos,
        expanded,
    ))
}

/// One collapsible branch of the coin menu.
///
/// The click reopens the whole menu with this branch marked, because that mark is the only thing
/// MoonUI renders a nested level from — see [`CoinMenuBranch`]. Clicking the open branch folds it
/// again. The row does not call `close_context_menu`: the reopen installs the rebuilt menu over
/// this one, and closing first would make the branch blink shut on every expand.
#[allow(clippy::too_many_arguments)]
fn branch_row(
    key: &'static str,
    label: String,
    branch: CoinMenuBranch,
    rows: Vec<MoonMenuItem>,
    ctx: &CoinMenuCtx,
    backend: &Entity<Backend>,
    pos: Point<Pixels>,
    expanded: Option<CoinMenuBranch>,
) -> MoonMenuItem {
    let open = expanded == Some(branch);
    let ctx = ctx.clone();
    let backend = backend.clone();
    MoonMenuItem::with_key(key, label)
        .selected(open)
        .submenu(rows)
        .on_click(move |_, window, app| {
            let next = (!open).then_some(branch);
            open_coin_menu_branch(ctx.clone(), backend.clone(), pos, next, window, app);
        })
}

/// The write both core-blacklist rows perform, differing only in the cores they are handed.
fn core_blacklist_writer(coin: &str) -> impl Fn(&mut Backend, &[CoreId]) + 'static {
    let coin = coin.to_string();
    move |b, cores| {
        for &core in cores {
            add_to_core_blacklist(b, core, &coin);
        }
    }
}

/// One preset row per duration, all sending to the same targets.
///
/// [`TempBanSpan::ALL`] rather than a list of its own: the chart's ban BUTTON offers the same four
/// spans, and the set matters — a trader who bans a coin for four hours from one client and looks
/// for that ban in the other must not have to translate between two sets of presets.
fn hour_rows(
    key_prefix: &str,
    backend: &Entity<Backend>,
    ctx: &CoinMenuCtx,
    cores: Vec<CoreId>,
) -> Vec<MoonMenuItem> {
    let workspace_group = ctx.workspace_group.clone();
    TempBanSpan::ALL
        .iter()
        .map(|&span| {
            let backend = backend.clone();
            let workspace_group = workspace_group.clone();
            let cores = cores.clone();
            let ctx = ctx.clone();
            let hours = span.hours();
            MoonMenuItem::with_key(
                SharedString::from(format!("{key_prefix}-{hours}h")),
                // Spelled in DAYS only past a day: MoonBot's own menu reads "24 hours" and
                // "3 days", and "72 часа" is a figure the reader has to convert back.
                match hours > 24 {
                    true => t!("coin_menu.tbl_days", n = hours / 24).to_string(),
                    false => t!("coin_menu.tbl_hours", n = hours).to_string(),
                },
            )
            .on_click(move |_, window, app| {
                window.close_context_menu(app);
                backend.update(app, |b, _| {
                    // Re-validated against the LIVE workspace: the menu may have been open while
                    // an Auto workspace moved the core out of this group's scope.
                    if !workspace_action_allows_cores(b, workspace_group.as_deref(), &cores) {
                        return;
                    }
                    send_temp_ban(b, &cores, &ctx, Some(span.duration()));
                });
            })
        })
        .collect()
}

/// One blacklist target row: a checkmark for what is already listed, and a click that revalidates.
fn target_row(
    key: &'static str,
    label: String,
    already_listed: bool,
    backend: Entity<Backend>,
    workspace_group: Option<String>,
    cores: Vec<CoreId>,
    write: impl Fn(&mut Backend, &[CoreId]) + 'static,
) -> MoonMenuItem {
    MoonMenuItem::with_key(key, label)
        .checked(already_listed)
        .on_click(move |_, window, app| {
            window.close_context_menu(app);
            backend.update(app, |b, _| {
                if workspace_action_allows_cores(b, workspace_group.as_deref(), &cores) {
                    write(b, &cores);
                }
            });
        })
}

/// Set or lift a temporary ban on every target, `None` lifting it.
///
/// Each core is sent ITS OWN market spelling — see [`temp_ban_symbol`]; a core whose catalogue
/// cannot name the coin is skipped rather than sent a market it does not have.
fn send_temp_ban(b: &Backend, cores: &[CoreId], ctx: &CoinMenuCtx, ban: Option<Duration>) {
    for &core in cores {
        let Some(symbol) = temp_ban_symbol(b, core, ctx) else {
            log::warn!(
                "coin_menu: core {} has no market for {}, temp blacklist skipped",
                moon_core::feed::core_label(core),
                ctx.coin
            );
            continue;
        };
        if let Err(err) = b.session.set_temp_ban(core, symbol.clone(), ban) {
            log::warn!(
                "coin_menu: temp blacklist {symbol} on core {} failed: {err:#}",
                moon_core::feed::core_label(core)
            );
        }
    }
}

/// The symbol ONE core's temporary blacklist is keyed by, or `None` when this context cannot name a
/// market for it.
///
/// The MARKET, not the coin — measured, not assumed. A ban placed inside MoonBot on core «BB1»
/// came back over the wire as `TempBLSymbols = ["DOTUSDT"]` (`channels.settings`, 2026-09-07),
/// while the same ban sent as `"PONS"` was echoed back with an empty list and dropped after the
/// queue's three attempts. The permanent list is the opposite — it matches the core's
/// `market_currency` — so the two rows of this menu deliberately write different spellings.
///
/// Resolved per CORE rather than reused across them: the row was clicked on one core's market, and
/// the other selected cores may be on venues that spell it differently. The clicked core keeps its
/// own exact market; the rest are looked up in their own catalogue, preferring the same spelling,
/// then the same quote currency, and finally the best search hit.
///
/// Args:
///     b: Terminal state holding the market catalogue.
///     core: Core the ban will be sent to.
///     ctx: Menu context, carrying the clicked core and its market.
///
/// Returns:
///     The market to write, or `None` when this core has no market for that coin.
fn temp_ban_symbol(b: &Backend, core: CoreId, ctx: &CoinMenuCtx) -> Option<String> {
    if ctx.market.is_empty() {
        return None;
    }
    if core == ctx.core {
        return Some(ctx.market.clone());
    }
    if ctx.coin.is_empty() {
        return None;
    }
    let candidates = b
        .session
        .market_source()
        .search_markets(core, &ctx.coin, MARKET_LOOKUP_LIMIT);
    if let Some(exact) = candidates
        .iter()
        .find(|name| name.eq_ignore_ascii_case(&ctx.market))
    {
        return Some(exact.clone());
    }
    // Same coin on another venue: keep the QUOTE the click was made against, so a USDT ban does not
    // land on a BTC-quoted market that happens to sort first.
    let quote = ctx
        .market
        .to_uppercase()
        .strip_prefix(&ctx.coin.to_uppercase())
        .map(str::to_string)
        .filter(|quote| !quote.is_empty());
    if let Some(quote) = quote
        && let Some(same_quote) = candidates
            .iter()
            .find(|name| name.to_uppercase().ends_with(&quote))
    {
        return Some(same_quote.clone());
    }
    candidates.into_iter().next()
}

/// How deep to look in one core's catalogue for the coin's market. The search is ranked
/// exact → prefix → contains, so the answer is at the top when it exists at all.
const MARKET_LOOKUP_LIMIT: usize = 8;
