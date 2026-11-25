//! Permanent and temporary actions in the coin menu's blacklist submenu.
//!
//! Why nested rather than flat, which is how the permanent list used to be listed: the temporary
//! list doubles every target and multiplies it by five durations. Flat, that is over a dozen rows
//! in a menu that also carries navigation, strategy and order actions — and the reader's first
//! question is which LIST, long before it is which core.
//!
//! The permanent list is a text this terminal rewrites in full — comma-separated on the core,
//! any strategy-list separator in a strategy's field ([`strategy_contains`]); the temporary one
//! is a delta the feed merges at send time, because the core writes that list too. See
//! [`moon_core::feed::CoreCmd::SetTempBlacklist`].
//!
//! A multi-core row sends every core ITS OWN spelling of the coin, resolved once per menu by
//! [`CoinNames`]; a core that does not carry the coin is not a target and is not counted.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use gpui::*;
use moon_ui::{MoonMenuItem, MoonTone, MoonWindowExt as _};
use rust_i18n::t;

use super::core_ref::core_session_name;
use super::multi::distinct_tickers;
use moon_core::config::TempBanSpan;
use moon_core::session::CoreId;
use moon_core::symbol::coin_list::strategy_contains;

use super::names::CoinNames;
use super::places;
use super::state::{BlState, HeldBan, bl_right_label, permanent_count};
use super::undo::{UndoRecord, UndoStep, show_undo_toast};
use super::{
    CoinMenuCtx, blacklist_contains, core_blacklist, strategy_blacklist, where_listed_item,
    with_hint, workspace_action_allows_cores, write_core_blacklist, write_strategy_blacklist,
};
use crate::Backend;
use crate::display_text::plural_key;

/// Sections of the blacklist submenu, in their visible order.
#[derive(Debug, PartialEq, Eq)]
enum Section {
    Permanent,
    Sep,
    Temporary,
    WhereListed,
}

/// Keep the same sections when a coin has no market for a temporary ban.
fn submenu_plan(temporary: bool) -> Vec<Section> {
    let mut sections = vec![Section::Permanent];
    if temporary {
        sections.extend([Section::Sep, Section::Temporary]);
    }
    sections.extend([Section::Sep, Section::WhereListed]);
    sections
}

/// Build the shared submenu and its muted permanent-count and ban label.
pub(super) fn blacklist_item(
    ctx: &CoinMenuCtx,
    b: &Backend,
    backend: &Entity<Backend>,
    names: &Rc<CoinNames>,
) -> MoonMenuItem {
    let places = places::listed_places(b, names);
    let group = temp_group(ctx, names);
    let held = held_ban(ctx.core, &group, b, names);
    let state = BlState {
        listed: permanent_count(&places),
        ban: held.here,
    };
    let mut rows = Vec::new();
    for section in submenu_plan(temp_ban_symbol(names, ctx.core).is_some()) {
        match section {
            Section::Permanent => rows.extend(permanent_rows(ctx, b, backend, &ctx.coin, names)),
            Section::Sep => rows.push(MoonMenuItem::separator()),
            Section::Temporary => rows.extend(temporary_rows(ctx, backend, names, &group, &held)),
            Section::WhereListed => rows.push(where_listed_item(backend, names, places.len())),
        }
    }
    let item =
        MoonMenuItem::with_key("coin-bl", t!("coin_menu.blacklist").to_string()).submenu(rows);
    with_hint(item, bl_right_label(&state))
}

/// Permanent actions over whichever of the three targets this context has.
fn permanent_rows(
    ctx: &CoinMenuCtx,
    b: &Backend,
    backend: &Entity<Backend>,
    coin: &str,
    names: &CoinNames,
) -> Vec<MoonMenuItem> {
    let core = ctx.core;
    let mut rows = vec![MoonMenuItem::label(
        t!("coin_menu.bl_permanent").to_string(),
    )];

    let (_, held) = core_blacklist(b, core);
    rows.push(target_row(
        "coin-bl-core",
        t!("coin_menu.target_core", core = ctx.core_name.clone()).to_string(),
        blacklist_contains(&held, coin),
        backend.clone(),
        ctx.workspace_group.clone(),
        vec![core],
        core_blacklist_writer(vec![(core, coin.to_string())], coin.to_string()),
    ));

    // The selected cores that carry the coin, each with the token ITS lists match. Fewer than two
    // is not a group: the row above already covers the clicked core.
    let targets: Vec<(CoreId, String)> = ctx
        .selected_cores
        .iter()
        .filter_map(|&core| names.on(core).map(|name| (core, name.coin.clone())))
        .collect();
    if targets.len() > 1 {
        let all_in = targets
            .iter()
            .all(|(core, token)| blacklist_contains(&core_blacklist(b, *core).1, token));
        rows.push(target_row(
            "coin-bl-cores",
            t!("coin_menu.target_cores", n = targets.len()).to_string(),
            all_in,
            backend.clone(),
            ctx.workspace_group.clone(),
            targets.iter().map(|(core, _)| *core).collect(),
            core_blacklist_writer(targets, coin.to_string()),
        ));
    }

    // Only when the strategy's own schema carries the field: without it the edit is discarded by
    // the view editor without a word, so the row would promise something that cannot happen.
    if let Some(sid) = ctx.strat_id
        && b.session.strategy_has_blacklist(core, sid)
    {
        let name = ctx
            .strat_name
            .clone()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| sid.to_string());
        let label = t!("coin_menu.target_strategy", name = name.as_str()).to_string();
        // A strategy list takes spaces between coins (`ANC GALA`); the comma-only
        // `blacklist_contains` above would show a listed coin unchecked and add it again.
        let in_strategy = strategy_contains(&strategy_blacklist(b, core, sid), coin);
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
                    let mut inserted = Vec::new();
                    for &core in cores {
                        // Re-checked inside the click for the same reason the workspace is:
                        // the schema can change between the menu opening and the press.
                        if b.session.strategy_has_blacklist(core, sid) {
                            let lift = strategy_contains(&strategy_blacklist(b, core, sid), &coin);
                            if lift {
                                let _ = write_strategy_blacklist(b, core, sid, &coin, lift);
                            } else if let Some(step) = add_step(b, core, Some(sid), &coin) {
                                b.watch_strategy_edit(core, sid, coin.clone());
                                inserted.push(step);
                            }
                        }
                    }
                    UndoRecord::new(inserted, |_| {
                        t!(
                            "coin_menu.undo.added_strategy",
                            coin = coin.as_str(),
                            name = name.as_str()
                        )
                        .to_string()
                    })
                }
            },
        ));
    }

    rows
}

/// Temporary-ban presets and the lift action for held targets.
fn temporary_rows(
    ctx: &CoinMenuCtx,
    backend: &Entity<Backend>,
    names: &Rc<CoinNames>,
    group: &[CoreId],
    held: &HeldBan,
) -> Vec<MoonMenuItem> {
    let core = ctx.core;
    let mut rows: Vec<MoonMenuItem> = Vec::new();

    rows.push(MoonMenuItem::label(
        t!("coin_menu.bl_temporary", core = ctx.core_name.clone()).to_string(),
    ));
    rows.extend(hour_rows(
        "tbl-core",
        backend,
        ctx.workspace_group.clone(),
        temp_targets(names, ctx.shown_coin(), &[core]),
    ));

    if !group.is_empty() {
        rows.push(MoonMenuItem::separator());
        rows.push(MoonMenuItem::label(
            t!("coin_menu.target_cores", n = group.len()).to_string(),
        ));
        rows.extend(hour_rows(
            "tbl-cores",
            backend,
            ctx.workspace_group.clone(),
            temp_targets(names, ctx.shown_coin(), group),
        ));
    }

    if let Some(label) = held.lift_label() {
        let backend = backend.clone();
        let workspace_group = ctx.workspace_group.clone();
        let held_cores = held.cores.clone();
        let targets = temp_targets(names, ctx.shown_coin(), &held_cores);
        rows.push(
            MoonMenuItem::with_key("tbl-clear", label)
                .tone(MoonTone::Danger)
                .on_click(move |_, window, app| {
                    window.close_context_menu(app);
                    backend.update(app, |b, _| {
                        if !workspace_action_allows_cores(
                            b,
                            workspace_group.as_deref(),
                            &held_cores,
                        ) {
                            return;
                        }
                        let _ = send_temp_ban(b, &targets, None);
                    });
                }),
        );
    }

    rows
}

/// Offer a separate selected-cores section only when at least two carry the market.
fn temp_group(ctx: &CoinMenuCtx, names: &CoinNames) -> Vec<CoreId> {
    let mut group: Vec<_> = ctx
        .selected_cores
        .iter()
        .copied()
        .filter(|&core| temp_ban_symbol(names, core).is_some())
        .collect();
    if group.len() < 2 {
        group.clear();
    }
    group
}

/// Read the countdown and lift targets from one snapshot and one clock reading.
fn held_ban(core: CoreId, group: &[CoreId], b: &Backend, names: &CoinNames) -> HeldBan {
    let now = moon_core::util::now_unix_ms_i64();
    let store = b.session.store();
    let mut held = HeldBan::default();
    for target in std::iter::once(core).chain(group.iter().copied()) {
        let Some(symbol) = temp_ban_symbol(names, target) else {
            continue;
        };
        let Some(data) = store.core(target) else {
            continue;
        };
        let Some(left) = data.temp_ban_left(&symbol, now) else {
            continue;
        };
        if target == core {
            let stale = data.client_settings_state() != moon_core::feed::CoreConfigState::Live;
            held.here = Some((u64::try_from(left.as_millis()).unwrap_or(u64::MAX), stale));
        }
        held.cores.push(target);
    }
    held.cores.sort_unstable();
    held.cores.dedup();
    held
}

/// The write both core-blacklist rows perform, differing only in the targets they are built with.
///
/// Each target carries the token ITS core matches: one row's cores can spell the coin three ways.
/// Direction is decided once for the whole target set: a fully listed set lifts the coin from
/// every core; otherwise every core takes the add path. A per-core toggle would mix add and lift
/// on a mixed selected-cores row.
///
/// The cores the click hands over are the same set, already checked against the workspace; the
/// tokens travel with the closure because the click has no other way to learn them.
fn core_blacklist_writer(
    targets: Vec<(CoreId, String)>,
    coin: String,
) -> impl Fn(&mut Backend, &[CoreId]) -> Option<UndoRecord> + 'static {
    move |b, _cores| {
        let lift = targets
            .iter()
            .all(|(core, token)| blacklist_contains(&core_blacklist(b, *core).1, token));
        UndoRecord::new(
            write_core_blacklist_targets(b, &targets, lift),
            |steps| match steps {
                [step] => {
                    let core = core_session_name(b, step.core);
                    t!(
                        "coin_menu.undo.added_core",
                        coin = coin.as_str(),
                        core = core
                    )
                    .to_string()
                }
                _ => t!(
                    plural_key("coin_menu.undo.added_cores", steps.len() as u64),
                    coin = coin.as_str(),
                    n = steps.len()
                )
                .to_string(),
            },
        )
    }
}

/// Submit adds or lifts, collecting an inverse only for each successful insertion.
pub(super) fn write_core_blacklist_targets(
    b: &Backend,
    targets: &[(CoreId, String)],
    lift: bool,
) -> Vec<UndoStep> {
    let mut inserted = Vec::new();
    for (core, token) in targets {
        if lift {
            let _ = write_core_blacklist(b, *core, token, lift);
        } else {
            inserted.extend(add_step(b, *core, None, token));
        }
    }
    inserted
}

/// Ask the session for an inverse tied to its effective blacklist text and flag.
fn add_step(b: &Backend, core: CoreId, strategy: Option<u64>, coin: &str) -> Option<UndoStep> {
    match b.session.add_to_blacklist(core, strategy, coin) {
        Ok(revert) => UndoStep::inserted(core, coin, strategy, revert),
        Err(err) => {
            log::warn!(
                "coin_menu: add {coin} to blacklist {strategy:?} on core {} failed: {err:#}",
                moon_core::feed::core_label(core)
            );
            None
        }
    }
}

/// Submit one permanent-list write and keep one inverse per affected core.
pub(super) fn add_core_blacklist_batch(b: &Backend, targets: &[(CoreId, String)]) -> Vec<UndoStep> {
    let mut by_core = BTreeMap::<_, Vec<_>>::new();
    for (core, token) in targets {
        by_core.entry(*core).or_default().push(token.as_str());
    }
    by_core
        .into_iter()
        .filter_map(
            |(core, coins)| match b.session.add_many_to_blacklist(core, &coins) {
                Ok(Some((inserted, revert))) => {
                    Some(UndoStep::inserted_many(core, inserted, revert))
                }
                Ok(None) => None,
                Err(err) => {
                    log::warn!(
                        "coin_menu: bulk blacklist add on core {} failed: {err:#}",
                        moon_core::feed::core_label(core)
                    );
                    None
                }
            },
        )
        .collect()
}

/// One preset row per duration, all sending to the same targets.
///
/// [`TempBanSpan::ALL`] rather than a list of its own: MoonBot's own menu offers the same four
/// spans, and the set matters — a trader who bans a coin for four hours from one client and looks
/// for that ban in the other must not have to translate between two sets of presets.
pub(super) fn hour_rows(
    key_prefix: &str,
    backend: &Entity<Backend>,
    workspace_group: Option<String>,
    targets: Vec<TempTarget>,
) -> Vec<MoonMenuItem> {
    let cores: Vec<_> = targets.iter().map(|target| target.core).collect();
    let targets = Rc::new(targets);
    TempBanSpan::ALL
        .iter()
        .map(|&span| {
            let backend = backend.clone();
            let workspace_group = workspace_group.clone();
            let cores = cores.clone();
            let targets = targets.clone();
            let hours = span.hours();
            // Spelled in DAYS only past a day, matching MoonBot's own menu.
            let caption_span = if hours > 24 {
                t!("coin_menu.tbl_days", n = hours / 24)
            } else {
                t!("coin_menu.tbl_hours", n = hours)
            }
            .to_string();
            MoonMenuItem::with_key(
                SharedString::from(format!("{key_prefix}-{hours}h")),
                caption_span.clone(),
            )
            .on_click(move |_, window, app| {
                window.close_context_menu(app);
                let record = backend.update(app, |b, _| {
                    // Re-validated against the LIVE workspace: the menu may have been open while
                    // an Auto workspace moved the core out of this group's scope.
                    if !workspace_action_allows_cores(b, workspace_group.as_deref(), &cores) {
                        return None;
                    }
                    UndoRecord::new(send_temp_ban(b, &targets, Some(span.duration())), |steps| {
                        temp_added_caption(steps, &caption_span, |core| core_session_name(b, core))
                    })
                });
                if let Some(record) = record {
                    show_undo_toast(
                        record,
                        backend.clone(),
                        workspace_group.clone(),
                        window,
                        app,
                    );
                }
            })
        })
        .collect()
}

/// One blacklist target row: a checkmark when the coin is already listed, and a click that
/// revalidates workspace authority then adds or lifts that token.
pub(super) fn target_row(
    key: &'static str,
    label: String,
    already_listed: bool,
    backend: Entity<Backend>,
    workspace_group: Option<String>,
    cores: Vec<CoreId>,
    write: impl Fn(&mut Backend, &[CoreId]) -> Option<UndoRecord> + 'static,
) -> MoonMenuItem {
    MoonMenuItem::with_key(key, label)
        .checked(already_listed)
        .on_click(move |_, window, app| {
            window.close_context_menu(app);
            let record = backend.update(app, |b, _| {
                if workspace_action_allows_cores(b, workspace_group.as_deref(), &cores) {
                    write(b, &cores)
                } else {
                    None
                }
            });
            if let Some(record) = record {
                show_undo_toast(
                    record,
                    backend.clone(),
                    workspace_group.clone(),
                    window,
                    app,
                );
            }
        })
}

/// Set or lift each temporary ban, retaining the session's inverse for successful adds.
fn send_temp_ban(b: &Backend, targets: &[TempTarget], ban: Option<Duration>) -> Vec<UndoStep> {
    let mut steps = Vec::new();
    for TempTarget {
        core,
        symbol,
        ticker,
    } in targets
    {
        let result = match ban {
            Some(span) => b
                .session
                .add_temp_ban_with_undo(*core, symbol.clone(), span)
                .map(Some),
            None => b
                .session
                .set_temp_ban(*core, symbol.clone(), None)
                .map(|()| None),
        };
        match result {
            Ok(revert) => steps.extend(UndoStep::temp_sent(*core, ticker, revert)),
            Err(err) => log::warn!(
                "coin_menu: temp blacklist {symbol} on core {} failed: {err:#}",
                moon_core::feed::core_label(*core)
            ),
        }
    }
    steps
}

/// Use printed tickers and the actual destination cores in the temporary-ban toast.
fn temp_added_caption(
    steps: &[UndoStep],
    span: &str,
    core_name: impl FnOnce(CoreId) -> String,
) -> String {
    match steps {
        [step] => t!(
            "coin_menu.undo.added_temp",
            coin = step.coins[0].as_str(),
            core = core_name(step.core),
            span = span,
        ),
        _ => t!(
            plural_key("coin_menu.undo.added_temp_bulk", steps.len() as u64),
            coins = toast_tickers(steps.iter().flat_map(|step| step.coins.iter().cloned())),
            target = toast_cores(steps, core_name),
            n = steps.len(),
            span = span,
        ),
    }
    .to_string()
}

/// Name the first three affected tickers, with a localized count of the remaining coins.
pub(super) fn toast_tickers(tickers: impl IntoIterator<Item = String>) -> String {
    let coins = distinct_tickers(tickers);
    let shown = coins[..coins.len().min(3)].join(", ");
    if coins.len() > 3 {
        t!(
            "coin_menu.undo.more_coins",
            coins = shown,
            n = coins.len() - 3
        )
        .to_string()
    } else {
        shown
    }
}

/// Name a single core and count distinct cores for a broader write.
pub(super) fn toast_cores(steps: &[UndoStep], core_name: impl FnOnce(CoreId) -> String) -> String {
    let cores: std::collections::BTreeSet<_> = steps.iter().map(|step| step.core).collect();
    if cores.len() == 1 {
        core_name(*cores.first().expect("one core"))
    } else {
        t!("coin_menu.undo.core_count", n = cores.len()).to_string()
    }
}

/// The shown ticker identifies the action, while the symbol addresses the core.
#[derive(Clone)]
pub(super) struct TempTarget {
    pub(super) core: CoreId,
    pub(super) symbol: String,
    pub(super) ticker: String,
}

/// Capture the printed ticker and the market each offered core can match.
pub(super) fn temp_targets(names: &CoinNames, shown: &str, cores: &[CoreId]) -> Vec<TempTarget> {
    let ticker = if shown.is_empty() {
        names.clicked()
    } else {
        shown
    };
    cores
        .iter()
        .filter_map(|&core| {
            let Some(symbol) = temp_ban_symbol(names, core) else {
                log::warn!(
                    "coin_menu: core {} has no market for {}, temp blacklist skipped",
                    moon_core::feed::core_label(core),
                    names.clicked()
                );
                return None;
            };
            Some(TempTarget {
                core,
                symbol,
                ticker: ticker.to_string(),
            })
        })
        .collect()
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
/// the other targets may be on venues that spell it differently. The clicked core keeps its own
/// exact market; every other core's is the one [`CoinNames`] found in its catalogue by the coin's
/// identity — the clicked market itself where that core lists it, otherwise the undated contract in
/// the quote that core trades in.
///
/// Args:
///     names: The clicked coin as every connected core spells it.
///     core: Core the ban will be sent to.
///
/// Returns:
///     The market to write, or `None` when this core has no market for that coin, or the click
///     named none.
pub(super) fn temp_ban_symbol(names: &CoinNames, core: CoreId) -> Option<String> {
    names
        .on(core)
        .map(|name| name.market.clone())
        .filter(|market| !market.is_empty())
}

#[cfg(test)]
mod tests;
