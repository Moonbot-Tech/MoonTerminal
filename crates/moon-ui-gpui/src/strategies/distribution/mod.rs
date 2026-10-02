//! The "WL distribution" tab of the Strategies window: how the strategies selected in the tree
//! share one exchange's coins between their cores.
//!
//! One ROW per core (a "slot"): every selected strategy of that core belongs to it and is meant to
//! carry the same `CoinsWhiteList` / `CoinsBlackList` — a long/short pair, typically. The rows
//! split the market between them, so the screen answers four questions at a glance: which coins
//! each core trades, which coin is traded by two cores at once, which whitelisted coin its own
//! core blacklists as well (and so does not trade), and which listed coin the exchange no longer
//! trades at all. A row with an EMPTY whitelist also shows what that emptiness trades — its
//! catalog less its blacklist — since those coins are its share although no list names them.
//!
//! "Trades" follows Moonbot's own reading of the two fields, and nothing else here may re-decide
//! it: an EMPTY whitelist admits every coin, a non-empty one admits only its entries, and the
//! blacklist removes coins from either. That is what lets a core with no whitelist and a blacklist
//! of everybody else's coins stand for "the rest of the market" — and be counted as such.
//!
//! This file is the model: pure functions over plain inputs, so every rule above is testable
//! without a session. Rendering lives in [`view`]; the report reads behind the chips' colours and
//! the trades table live in `stats`, the table itself in `trades`.

use std::collections::{HashMap, HashSet};

use moon_core::session::CoreId;
use moon_core::session::core_order::ExchangeSection;
use moon_core::symbol::{coin_match_key, split_coin_list};

mod stats;
mod trades;
pub(super) mod view;

#[cfg(test)]
mod tests;

/// Strategy field holding the coins a strategy is limited to; empty means "every coin".
pub(super) const WHITE_FIELD: &str = "CoinsWhiteList";
/// Strategy field holding the coins a strategy never trades.
pub(super) const BLACK_FIELD: &str = "CoinsBlackList";

/// One selected strategy, reduced to what the distribution reads.
#[derive(Clone, Debug)]
pub(super) struct StrategyInput {
    pub(super) name: String,
    /// `CoinsWhiteList` as the core stores it; a field the core omitted is empty.
    pub(super) white: String,
    /// `CoinsBlackList` as the core stores it.
    pub(super) black: String,
}

/// Every selected strategy of one core, with what the core is connected to.
#[derive(Clone, Debug)]
pub(super) struct SlotInput {
    pub(super) core: CoreId,
    pub(super) core_name: String,
    pub(super) section: ExchangeSection,
    pub(super) strategies: Vec<StrategyInput>,
}

/// Why no distribution can be drawn for the current selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Unavailable {
    /// Nothing is selected in the tree.
    NoSelection,
    /// The selected strategies sit on cores of different exchanges: one market cannot be split
    /// between them.
    MixedVenues,
    /// A selected core has not reported its exchange yet, so "the same market" cannot be checked.
    UnknownVenue,
}

/// How one coin chip is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChipState {
    Normal,
    /// The core's catalog no longer has the coin trading: delisted, or moved to another quote.
    Gone,
    /// A whitelisted coin that another row trades as well.
    Duplicate,
    /// A whitelisted coin the same row also blacklists: the blacklist wins, so the row does not
    /// trade it although its whitelist names it.
    Blocked,
}

/// What a chip's colour says, most important first: a coin the exchange no longer trades, a
/// whitelisted coin its own row blacklists, a coin two rows trade, then how the coin did for the
/// selected strategies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Look {
    Gone,
    Blocked,
    Duplicate,
    Profit,
    Loss,
    /// No closed trade in the period, or a flat result.
    Flat,
}

/// The colour a chip is drawn in.
///
/// Args:
///     state: The chip's list-level state.
///     profit: The coin's profit over the period, `None` without a closed trade.
pub(super) fn look(state: ChipState, profit: Option<f64>) -> Look {
    match state {
        ChipState::Gone => Look::Gone,
        ChipState::Blocked => Look::Blocked,
        ChipState::Duplicate => Look::Duplicate,
        ChipState::Normal => match profit {
            Some(p) if p > 0.0 => Look::Profit,
            Some(p) if p < 0.0 => Look::Loss,
            _ => Look::Flat,
        },
    }
}

/// Chips ordered best result first; coins without a trade sit between winners and losers, and a
/// tie keeps the name order the list already has.
///
/// Args:
///     chips: One list, in name order.
///     profit: Profit per coin match key; a coin it lacks counts as zero.
pub(super) fn by_profit(chips: &[Chip], profit: impl Fn(&str) -> Option<f64>) -> Vec<&Chip> {
    // A non-finite or absent figure sorts as zero; `+ 0.0` folds -0.0 onto 0.0 so a flat result
    // does not sink below coins without trades.
    let key = |c: &Chip| profit(&c.coin).filter(|p| p.is_finite()).unwrap_or(0.0) + 0.0;
    let mut out: Vec<&Chip> = chips.iter().collect();
    out.sort_by(|a, b| key(b).total_cmp(&key(a)));
    out
}

/// One coin of one list, folded to its match key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Chip {
    pub(super) coin: String,
    pub(super) state: ChipState,
}

/// One core of the distribution, as drawn.
#[derive(Clone, Debug)]
pub(super) struct Slot {
    pub(super) core: CoreId,
    pub(super) core_name: String,
    /// Names of the selected strategies on this core, in tree order.
    pub(super) strategies: Vec<String>,
    /// Whether those strategies do not all hold the same two lists. The row then shows the union.
    pub(super) lists_differ: bool,
    /// Whitelist, sorted by coin. Empty means "every coin the blacklist leaves".
    pub(super) white: Vec<Chip>,
    /// What an EMPTY whitelist actually trades: the core's catalog less its blacklist, sorted.
    /// `None` when the whitelist is not empty (it names the coins itself) or the catalog has not
    /// arrived. These coins are written nowhere; they are drawn so the row's share has a face.
    pub(super) traded: Option<Vec<Chip>>,
    /// Blacklist, sorted by coin.
    pub(super) black: Vec<Chip>,
}

/// How much of the exchange the rows cover between them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Coverage {
    /// Coins the exchange trades, by the catalogs of the rows' cores.
    pub(super) total: usize,
    /// Of those, coins at least one row trades.
    pub(super) traded: usize,
}

/// A drawable distribution.
#[derive(Clone, Debug)]
pub(super) struct Board {
    pub(super) section: ExchangeSection,
    /// Rows in distribution order: the top one receives coins first.
    pub(super) slots: Vec<Slot>,
    /// `None` while no row's catalog has arrived — unknown, not zero.
    pub(super) coverage: Option<Coverage>,
}

/// One row's lists as match-key sets, the form every rule reads.
struct Lists<'a> {
    white: HashSet<String>,
    black: HashSet<String>,
    /// The row core's tradable coins, or `None` while its catalog has not arrived.
    catalog: Option<&'a HashSet<String>>,
}

impl Lists<'_> {
    /// Moonbot's admission rule (see the module doc), on top of the core's own catalog: a coin
    /// the core's exchange no longer trades is not traded by the row, whatever its lists say.
    fn trades(&self, coin: &str) -> bool {
        (self.white.is_empty() || self.white.contains(coin))
            && !self.black.contains(coin)
            && self.catalog.is_none_or(|c| c.contains(coin))
    }
}

/// Build the distribution for the selected strategies.
///
/// Args:
///     inputs: One entry per core holding a selected strategy, in tree order.
///     order: Saved distribution order, as core ids; cores it does not name follow in tree order.
///     catalogs: Each core's tradable coins as match keys; a core missing from the map, or mapped
///         to `None`, has no catalog yet and marks nothing as gone.
///
/// Returns:
///     The board, or why none can be drawn.
pub(super) fn build(
    inputs: Vec<SlotInput>,
    order: &[CoreId],
    catalogs: &HashMap<CoreId, Option<HashSet<String>>>,
) -> Result<Board, Unavailable> {
    let section = match inputs.first() {
        None => return Err(Unavailable::NoSelection),
        Some(first) => first.section,
    };
    if inputs
        .iter()
        .any(|s| s.section == ExchangeSection::Unidentified)
    {
        return Err(Unavailable::UnknownVenue);
    }
    if inputs.iter().any(|s| s.section != section) {
        return Err(Unavailable::MixedVenues);
    }
    let tree_order: Vec<CoreId> = inputs.iter().map(|s| s.core).collect();
    let mut by_core: HashMap<CoreId, SlotInput> = inputs.into_iter().map(|s| (s.core, s)).collect();
    let slots: Vec<SlotInput> = ordered(&tree_order, order)
        .into_iter()
        .filter_map(|core| by_core.remove(&core))
        .collect();

    let catalog_of = |core: CoreId| catalogs.get(&core).and_then(Option::as_ref);
    let lists: Vec<Lists> = slots
        .iter()
        .map(|slot| slot_lists(slot, catalog_of(slot.core)))
        .collect();
    let mut universe: Option<HashSet<&String>> = None;
    for slot in &slots {
        if let Some(coins) = catalog_of(slot.core) {
            universe.get_or_insert_with(HashSet::new).extend(coins);
        }
    }
    // How many rows trade each coin: the market's, for the coverage count, plus every whitelisted
    // one, so a duplicate shows before — or without — any catalog.
    let mut traders: HashMap<&str, usize> = HashMap::new();
    let whitelisted = lists.iter().flat_map(|l| &l.white);
    for coin in universe.iter().flatten().copied().chain(whitelisted) {
        traders
            .entry(coin.as_str())
            .or_insert_with(|| lists.iter().filter(|l| l.trades(coin)).count());
    }
    let coverage = universe.as_ref().map(|u| Coverage {
        total: u.len(),
        traded: u
            .iter()
            .filter(|c| traders.get(c.as_str()).is_some_and(|n| *n > 0))
            .count(),
    });

    let drawn = slots
        .into_iter()
        .zip(&lists)
        .map(|(slot, list)| {
            let state = |coin: &str, white: bool| match list.catalog {
                Some(c) if !c.contains(coin) => ChipState::Gone,
                _ if white && list.black.contains(coin) => ChipState::Blocked,
                _ if white && traders.get(coin).is_some_and(|n| *n > 1) => ChipState::Duplicate,
                _ => ChipState::Normal,
            };
            let chips = |set: &HashSet<String>, white: bool| {
                let mut coins: Vec<&String> = set.iter().collect();
                coins.sort_unstable();
                coins
                    .into_iter()
                    .map(|coin| Chip {
                        coin: coin.clone(),
                        state: state(coin, white),
                    })
                    .collect()
            };
            let traded = match list.catalog {
                Some(catalog) if list.white.is_empty() => {
                    let set: HashSet<String> = catalog
                        .iter()
                        .filter(|coin| list.trades(coin))
                        .cloned()
                        .collect();
                    // Judged as whitelist entries: a coin another row trades too is a duplicate.
                    Some(chips(&set, true))
                }
                _ => None,
            };
            Slot {
                core: slot.core,
                core_name: slot.core_name.clone(),
                strategies: slot.strategies.iter().map(|s| s.name.clone()).collect(),
                lists_differ: lists_differ(&slot),
                white: chips(&list.white, true),
                traded,
                black: chips(&list.black, false),
            }
        })
        .collect();
    Ok(Board {
        section,
        slots: drawn,
        coverage,
    })
}

/// Put the rows in distribution order: cores the saved order names, in its order, then the rest
/// in tree order.
pub(super) fn ordered(tree_order: &[CoreId], saved: &[CoreId]) -> Vec<CoreId> {
    let present: HashSet<CoreId> = tree_order.iter().copied().collect();
    let mut out: Vec<CoreId> = Vec::with_capacity(tree_order.len());
    for core in saved.iter().chain(tree_order) {
        if present.contains(core) && !out.contains(core) {
            out.push(*core);
        }
    }
    out
}

/// Move one row up or down and return the order to save.
///
/// The rows on screen come first, in their new order; cores the saved order remembered from other
/// selections keep their places after them, so arranging one group does not forget another.
///
/// Args:
///     shown: Rows as currently drawn.
///     saved: The order saved so far.
///     core: Row to move.
///     up: Direction.
///
/// Returns:
///     The new order, or `None` when the row is already at that edge or not shown.
pub(super) fn moved(
    shown: &[CoreId],
    saved: &[CoreId],
    core: CoreId,
    up: bool,
) -> Option<Vec<CoreId>> {
    let at = shown.iter().position(|c| *c == core)?;
    let to = match up {
        true => at.checked_sub(1)?,
        false => (at + 1 < shown.len()).then_some(at + 1)?,
    };
    let mut out = shown.to_vec();
    out.swap(at, to);
    out.extend(saved.iter().filter(|c| !shown.contains(c)));
    Some(out)
}

/// The union of a row's lists, as match keys.
fn slot_lists<'a>(slot: &SlotInput, catalog: Option<&'a HashSet<String>>) -> Lists<'a> {
    let keys = |text: &str| {
        split_coin_list(text)
            .map(coin_match_key)
            .collect::<Vec<_>>()
    };
    let mut white = HashSet::new();
    let mut black = HashSet::new();
    for s in &slot.strategies {
        white.extend(keys(&s.white));
        black.extend(keys(&s.black));
    }
    Lists {
        white,
        black,
        catalog,
    }
}

/// Whether a row's strategies disagree about either list.
fn lists_differ(slot: &SlotInput) -> bool {
    let sets = |s: &StrategyInput| {
        (
            moon_core::symbol::parse_coin_list(&s.white),
            moon_core::symbol::parse_coin_list(&s.black),
        )
    };
    let mut all = slot.strategies.iter().map(sets);
    let Some(first) = all.next() else {
        return false;
    };
    all.any(|other| other != first)
}
