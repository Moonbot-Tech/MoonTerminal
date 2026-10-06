//! Query matching: scope, hits and the server name of a search.

use super::*;

/// Return the server name when one live core owns the entire search scope.
///
/// Args:
///     b: Backend holding the live sessions and server configuration.
///     group: Window group whose market universe is being searched.
///     bucket: The same bucket used by search and suggestion lookup.
///
/// Returns:
///     The sole live core's current name, or `None` for an empty or multi-core scope.
pub(crate) fn single_server_context(
    b: &Backend,
    group: &str,
    bucket: Option<&ChartBucket>,
) -> Option<String> {
    let cores = cores_for(b, group, bucket);
    let [core] = cores.as_slice() else {
        return None;
    };
    b.session
        .sessions()
        .iter()
        .find(|session| session.id == *core)
        .map(|session| session.name.clone())
}

/// Maps a Russian-layout character to the Latin character on the same physical QWERTY key. Market
/// tickers use Latin characters, so Russian-layout input usually indicates an unchanged keyboard
/// layout. Converting it lets a physically typed `BTC` query find `BTC`. Case is preserved, and an
/// unknown character is returned unchanged.
fn ru_key_to_en(ch: char) -> char {
    let lower = ch.to_lowercase().next().unwrap_or(ch);
    let mapped = match lower {
        'й' => 'q',
        'ц' => 'w',
        'у' => 'e',
        'к' => 'r',
        'е' => 't',
        'н' => 'y',
        'г' => 'u',
        'ш' => 'i',
        'щ' => 'o',
        'з' => 'p',
        'х' => '[',
        'ъ' => ']',
        'ф' => 'a',
        'ы' => 's',
        'в' => 'd',
        'а' => 'f',
        'п' => 'g',
        'р' => 'h',
        'о' => 'j',
        'л' => 'k',
        'д' => 'l',
        'ж' => ';',
        'э' => '\'',
        'я' => 'z',
        'ч' => 'x',
        'с' => 'c',
        'м' => 'v',
        'и' => 'b',
        'т' => 'n',
        'ь' => 'm',
        'б' => ',',
        'ю' => '.',
        'ё' => '`',
        _ => return ch,
    };
    if ch.is_uppercase() {
        mapped.to_ascii_uppercase()
    } else {
        mapped
    }
}

/// Converts Russian-layout input to the corresponding English QWERTY keys when the query contains
/// Russian letters; otherwise borrows the original string to avoid allocating for Latin input.
pub(crate) fn normalize_layout(query: &str) -> std::borrow::Cow<'_, str> {
    if query
        .chars()
        .any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c) || c == 'ё' || c == 'Ё')
    {
        std::borrow::Cow::Owned(query.chars().map(ru_key_to_en).collect())
    } else {
        std::borrow::Cow::Borrowed(query)
    }
}

/// One search hit: the market to open, and how to LABEL it.
///
/// The label is resolved here, once per hit, rather than in the row renderer: the renderer has no
/// core, and without one a Hyperliquid spot market can only be shown as its index (`@156`) and
/// three COIN-M contracts of one coin all read `SOL`.
#[derive(Clone)]
pub(crate) struct CoinHit {
    pub(crate) core: CoreId,
    /// Market key, used to open and to match the selection. Never displayed.
    pub(crate) market: String,
    /// Core name shown beside the coin or in the popup-level context.
    pub(crate) server: String,
    /// Coin token and quote as the CORE names them; see `MarketDataSource::market_label`.
    pub(crate) label: MarketLabel,
    /// Carries the core's venue into grouping, before render has lost the core identity.
    ///
    /// Resolved beside the label rather than at render for the same reason: the renderer holds no
    /// core, and the venue is what decides whether two cores offering `BTC-USDT` are ONE choice on
    /// one exchange or two choices on two.
    pub(crate) venue: Option<CoreVenue>,
    /// Whether this core already holds an open order or position on `market`; see
    /// [`in_trade`]. Decoration only: it sorts and marks rows, never changes what a row opens.
    pub(crate) in_trade: bool,
}

/// Returns token-search results, each carrying its resolved label.
pub(crate) fn search(
    b: &Backend,
    group: &str,
    bucket: Option<&ChartBucket>,
    query: &str,
) -> Vec<CoinHit> {
    search_limited(b, group, bucket, query, COIN_SEARCH_LIMIT)
}

/// [`search`] with the per-core cap stated by the caller.
///
/// The popup wants a short list because it renders it; a caller looking for ONE instrument wants
/// every candidate, because it filters them by identity and shows one.
pub(crate) fn search_limited(
    b: &Backend,
    group: &str,
    bucket: Option<&ChartBucket>,
    query: &str,
    limit: usize,
) -> Vec<CoinHit> {
    let query = normalize_layout(query.trim());
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    let ms = b.session.market_source();
    let pairs: Vec<(CoreId, String)> = cores_for(b, group, bucket)
        .into_iter()
        .flat_map(|core| {
            ms.search_markets(core, query, limit)
                .into_iter()
                .map(move |market| (core, market))
        })
        .collect();
    // One resolver for every list in this module, so a query row and a suggestion row can never
    // disagree about a core's name or about what to do with a core that is no longer live.
    hits_for(b, pairs)
}
