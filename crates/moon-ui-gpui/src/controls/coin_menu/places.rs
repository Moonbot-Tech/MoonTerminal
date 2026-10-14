//! Where one coin is blacklisted: the core list, a temporary ban, or a strategy field.
//!
//! Chart menus and the places dialog both ask this module. Matching is the core's own
//! comma-separated token compare. A `*` entry is a literal token, not a mask.

use std::collections::BTreeSet;

use moon_core::feed::{FIELD_COINS_BLACK_LIST, StrategyRow};
use moon_core::session::CoreId;
use moon_core::session::store::CoreData;
use moon_core::symbol::coin_list;

use crate::Backend;

/// One place a coin is listed on one core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CoinPlace {
    /// The core-wide blacklist text contains the coin.
    ///
    /// `list_enabled` is the core's checkbox. A coin still listed while the checkbox is off is a
    /// row, because the text is where the user has to look.
    Core { core: CoreId, list_enabled: bool },
    /// A temporary ban whose market resolves to this coin.
    Temp {
        core: CoreId,
        market: String,
        until_ms: i64,
    },
    /// A strategy `CoinsBlackList` field contains the coin.
    Strategy {
        core: CoreId,
        sid: u64,
        /// Strategy folder path, or an empty string when the row has none.
        folder: String,
        strategy: String,
    },
}

impl CoinPlace {
    /// Core this place belongs to.
    pub(super) fn core(&self) -> CoreId {
        match self {
            Self::Core { core, .. } | Self::Temp { core, .. } | Self::Strategy { core, .. } => {
                *core
            }
        }
    }
}

/// Every blacklist place for `coin` across `cores`.
///
/// Cores are emitted in `CoreId` order, whatever order they were passed in. Inside one core the
/// order is the core list, then temporary bans in the order the core listed them, then strategies
/// in stored order. A missing core-settings snapshot, a missing strategy field, an unresolvable
/// temporary market, and a token that only shares a prefix with `coin` produce no row.
///
/// Args:
///     cores: Connected cores. Each id is paired with that core's account snapshot.
///     coin: Coin token, never a pair.
///     base_of: Resolves a temporary-ban market to its coin, or `None` when the catalogue cannot
///         name that market.
///
/// Returns:
///     One entry per matching place. Empty when nothing lists `coin`.
pub(crate) fn find_coin_places<'a>(
    cores: impl IntoIterator<Item = (CoreId, &'a CoreData)>,
    coin: &str,
    base_of: impl Fn(CoreId, &str) -> Option<String>,
) -> Vec<CoinPlace> {
    let mut cores: Vec<(CoreId, &CoreData)> = cores.into_iter().collect();
    cores.sort_by_key(|(id, _)| *id);
    let mut out = Vec::new();
    for (core, data) in cores {
        if let Some(settings) = data.client_settings.as_ref()
            && coin_list::contains(&settings.blacklist_text, coin)
        {
            out.push(CoinPlace::Core {
                core,
                list_enabled: settings.use_blacklist,
            });
        }
        for (market, until_ms) in data.temp_bans() {
            if base_of(core, market).is_some_and(|base| base.eq_ignore_ascii_case(coin)) {
                out.push(CoinPlace::Temp {
                    core,
                    market: market.to_string(),
                    until_ms,
                });
            }
        }
        for row in &data.strategies {
            if strategy_lists(row, coin) {
                out.push(CoinPlace::Strategy {
                    core,
                    sid: row.id,
                    folder: row.folder_path.clone(),
                    strategy: row.name.clone(),
                });
            }
        }
    }
    out
}

/// Whether this strategy's `CoinsBlackList` text lists `coin`.
///
/// A missing field is the empty list: the server omits a field that still has its default.
/// `CoinsBlackList` text on one strategy row.
///
/// The server omits a field that still has its default, so a missing field is the empty list.
///
/// Args:
///     row: One stored strategy.
///
/// Returns:
///     The field text, or `""` when the row has no `CoinsBlackList` entry.
pub(super) fn coins_black_list(row: &StrategyRow) -> &str {
    row.fields
        .iter()
        .find(|(key, _)| key == FIELD_COINS_BLACK_LIST)
        .map(|(_, value)| value.as_str())
        .unwrap_or("")
}

fn strategy_lists(row: &StrategyRow, coin: &str) -> bool {
    coin_list::contains(coins_black_list(row), coin)
}

/// Blacklist places for `coin` on cores that currently have a session.
///
/// The market catalogue is cloned before the store is borrowed. Resolving a temporary-ban market
/// takes that catalogue's lock, and the place scan holds the store at the same time.
///
/// Args:
///     backend: Live session, store, and market catalogue.
///     coin: Coin token, never a pair.
///
/// Returns:
///     Places from [`find_coin_places`], core id order.
pub(super) fn listed_places(backend: &Backend, coin: &str) -> Vec<CoinPlace> {
    let source = backend.session.market_source();
    let ids: Vec<CoreId> = backend
        .session
        .sessions()
        .iter()
        .map(|session| session.id)
        .collect();
    let store = backend.session.store();
    let cores = ids
        .iter()
        .filter_map(|id| store.core(*id).map(|data| (*id, data)));
    find_coin_places(cores, coin, |core, market| {
        let label = source.market_label(core, market).coin;
        if label.is_empty() { None } else { Some(label) }
    })
}

/// How many places were found, and on how many distinct cores.
///
/// Args:
///     places: Rows from [`find_coin_places`], in any order.
///
/// Returns:
///     `(places, cores)`. Both are zero for an empty slice.
pub(crate) fn places_summary<'a>(
    places: impl IntoIterator<Item = &'a CoinPlace>,
) -> (usize, usize) {
    let mut count = 0usize;
    let mut cores = BTreeSet::new();
    for place in places {
        count += 1;
        cores.insert(place.core());
    }
    (count, cores.len())
}

#[cfg(test)]
mod tests;
