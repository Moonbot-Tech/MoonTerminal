//! Shared market picker for typed search and cached empty-field suggestions.
//!
//! The list is a two-level tree: exchange sections (drawn only when there is more than one), then
//! ONE ROW PER COIN, with the cores that offer it as child rows underneath. A coin row opens on the
//! core the host is addressing ([`pick_core`]); its `@server` names that core so the choice is
//! never hidden, and child rows expose a clipped core name in a tooltip. A group of more than
//! [`COIN_GROUP_AUTO_EXPAND`] cores starts collapsed — on a fifty-six-core config the flat form was
//! fifty-six identical rows of one coin. The widget does not define selection behavior; its owner
//! supplies `on_pick`, `on_toggle` and `on_expand`, and owns the expanded-row set the same way it
//! owns the multi-select one.
//!
//! Chart tabs open a market and may show Recent and Top 24h volatility sections, while the header
//! rate ticker and Report token filter remain query-only consumers.
//!
//! Consumers are the chart-tab strip and detached windows through the
//! [`crate::chart_tabs::coin_search`] shim, the header rate ticker in `shell/ticker.rs`, and the
//! Report token filter in `panels/report`.

use std::collections::{HashMap, HashSet};

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonDisclosure,
    MoonDisclosureDirection, MoonInputState, MoonPalette, h_flex,
};
use rust_i18n::t;

use crate::Backend;
use crate::core_order::ExchangeSection;
use crate::design;
use moon_core::config::ChartBucket;
use moon_core::market::MarketLabel;
use moon_core::session::CoreId;
use moon_core::venue::CoreVenue;

mod ranking;
mod tabs;

pub(crate) use tabs::{CoinBan, CoinTab, CoinTabsCfg, banned};

use ranking::{
    MOVER_VOL_REF, Mover, SUGGEST_ROW_CAP, merge_ranked_heads, mover_score,
    neutralize_blind_provider, turnover_usd,
};

/// Maximum number of MoonProto search results requested per core.
pub(crate) const COIN_SEARCH_LIMIT: usize = 8;

/// Results per core for "the same instrument on another exchange", where the list is filtered by
/// identity afterwards rather than shown.
///
/// Far wider than the popup's, and it has to be: a live Bybit core lists BTC under ten expiries
/// beside the perpetual, and at eight rows the perpetual can fall outside the answer entirely —
/// the comparison would then open a dated contract while claiming to show the coin.
pub(crate) const COIN_MATCH_LIMIT: usize = 32;

/// Maximum logical height before the coin list starts scrolling.
const COIN_LIST_RAW_CAP: f32 = 340.0;

/// Logical height of the continuation fade over an overflowing coin list.
const COIN_LIST_FADE_H: f32 = 12.0;

/// Returns the cores whose market universes feed this token field. None searches the full group,
/// the same as a shared bucket.
///
/// Returned in canonical order: the search popup groups its hits per core and renders them
/// in the order given, so this order is what the user reads as `COIN — Server` rows.
fn cores_for(b: &Backend, group: &str, bucket: Option<&ChartBucket>) -> Vec<CoreId> {
    let group_cores = || {
        b.session
            .sessions()
            .iter()
            .filter(|s| s.group == group)
            .filter(|s| b.core_displayed_in_group(group, s.id))
            .map(|s| s.id)
            .collect::<Vec<_>>()
    };
    let order = crate::core_order::CoreOrder::new(&b.config);
    let mut ids = match bucket {
        None | Some(ChartBucket::Shared) => group_cores(),
        // Already the caller's own resolved bucket — not an enumeration, so it stays unfiltered.
        Some(ChartBucket::Core(id)) => vec![*id],
        Some(ChartBucket::Bundle(name)) => {
            let split = b.config.charts_split_by_core;
            b.session
                .sessions()
                .iter()
                .filter(|s| s.group == group)
                .filter(|s| b.core_displayed_in_group(group, s.id))
                .filter(|s| {
                    b.config
                        .servers
                        .iter()
                        .find(|sv| sv.id == s.id)
                        .map(|sv| sv.chart_bucket(split) == ChartBucket::Bundle(name.clone()))
                        .unwrap_or(false)
                })
                .map(|s| s.id)
                .collect()
        }
    };
    order.sort_by(&mut ids, |id| *id);
    ids
}

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

/// Nominal width of the search popup, in font-scaled units.
///
/// Shared with the header ticker, which hand-positions its own box BEHIND this popup and cannot
/// measure it: the two are drawn by different code and only line up because they start from the
/// same figure.
pub(crate) const COIN_POPUP_W: f32 = 300.0;

/// Number of top-mover rows offered while the field is empty.
///
/// The movers are the section worth scrolling: they are the only place the dropdown ANSWERS a
/// question rather than repeating a choice the user already made.
pub(crate) const COIN_SUGGEST_LIMIT: usize = 12;

/// Number of recently opened markets offered above them.
///
/// Deliberately short. Recents are a shortcut back to what is already in hand, and a long tail of
/// them pushes the movers — the part the user cannot reconstruct from memory — off the screen.
pub(crate) const COIN_RECENT_LIMIT: usize = 3;

/// How long a built suggestion list stays usable before it is rebuilt on the next popup open.
///
/// Movement rankings drift slowly and the scan is expensive, so half a minute of staleness in a
/// list of SUGGESTIONS costs nothing — the user is picking a market, not reading a quote.
const COIN_SUGGEST_TTL: std::time::Duration = std::time::Duration::from_secs(30);

/// One cached suggestion list, with everything needed to decide whether it is still valid.
pub(crate) struct CoinSuggestEntry {
    /// When the list was built.
    pub(crate) at: std::time::Instant,
    /// The market universe it was built from; a change invalidates it regardless of age.
    pub(crate) sig: CoinUniverseSig,
    /// Bare `(core, market)` pairs. Deliberately NOT resolved rows: labels and server names are
    /// live state, so they are rebuilt at read time rather than frozen into the cache.
    pub(crate) markets: Vec<(CoreId, String)>,
}

impl CoinSuggestEntry {
    /// Whether this entry is young enough to serve. The cheap half of the freshness test, for the
    /// render path.
    pub(crate) fn is_recent(&self) -> bool {
        self.at.elapsed() < COIN_SUGGEST_TTL
    }

    /// Whether this entry may still be served for `sig` — age AND the universe it was built from.
    /// The full test, for the open path, which can afford to rebuild the signature.
    pub(crate) fn is_fresh(&self, sig: &CoinUniverseSig) -> bool {
        self.is_recent() && &self.sig == sig
    }
}

/// Identity of the market universe a suggestion list was built from.
///
/// A cached list outlives the state it was derived from — a core can disconnect, a provider can
/// change — so the cache is keyed by this rather than by a timestamp alone: a stale entry must be
/// discarded because the WORLD changed, not only because time passed.
pub(crate) type CoinUniverseSig = Vec<(CoreId, Option<CoreId>)>;

/// The `(core, provider)` pairs currently feeding this field, in canonical core order.
pub(crate) fn universe_sig(
    b: &Backend,
    group: &str,
    bucket: Option<&ChartBucket>,
) -> CoinUniverseSig {
    let ms = b.session.market_source();
    cores_for(b, group, bucket)
        .into_iter()
        .map(|core| (core, ms.provider_of(core)))
        .collect()
}

/// Resolves both empty-field sections for one coin field in a single pass over its core scope.
///
/// The persisted recents list is application-wide, but a coin field is not: an Add tab scoped to
/// one core, or a detached window scoped to its bucket, must never offer a market belonging to a
/// core outside that scope — picking one would drop a foreign core's chart into a bucket meant to
/// hold only its own. The history is therefore intersected with the very same `cores_for` scope the
/// typed search and the volatility ranking use, and only THEN trimmed to `limit`: trimming first
/// would let out-of-scope entries eat the visible slots.
///
/// Both sections resolve here rather than in two calls because this runs on the popup's render
/// path, and `cores_for` sorts the group's cores through `CoreOrder` on every call.
///
/// Args:
///     b: Backend holding the session, configuration and persisted history.
///     group: Window group whose cores feed this field.
///     bucket: Chart bucket narrowing those cores, or `None` for the whole group.
///     volatile: Cached top-movers markets, already ranked; see `Backend::coin_suggest_markets`.
///     limit: Maximum number of recent rows to return.
///
/// Returns:
///     `(recent, volatile)` hits, recents newest first.
pub(crate) fn suggestions(
    b: &Backend,
    group: &str,
    bucket: Option<&ChartBucket>,
    volatile: Vec<(CoreId, String)>,
) -> (Vec<CoinHit>, Vec<CoinHit>) {
    let scope = cores_for(b, group, bucket);
    let recent: Vec<(CoreId, String)> = b
        .recent_coins()
        .into_iter()
        .filter(|(core, _)| scope.contains(core))
        .take(COIN_RECENT_LIMIT)
        .collect();
    (hits_for(b, recent), hits_for(b, volatile))
}

/// Suggests the markets moving most over the last 24 hours across this field's cores.
///
/// Reuses the screener's own data rather than opening a feed of its own: `ScreenerRow::d_24h` is
/// the unsigned 24-hour range magnitude the Screener table already ranks by, weighed by turnover
/// through [`mover_score`] so a thin market's outsized percentage does not head the list. Ties
/// break on movement, then turnover, then the market name, so equal movers keep a stable order
/// instead of reshuffling per call.
///
/// `ScreenerRow::vol_24h` is denominated in each market's OWN quote currency, and one exchange
/// lists several — a BTC-quoted pair's turnover is numerically four orders below a USDT-quoted
/// one's at equal money. It is therefore converted through `quote_usd_rate` before comparison.
/// Rates are cached by quote during the scan; an unavailable rate uses the reference turnover so
/// missing conversion data does not classify a market as dead.
///
/// Rows are fetched ONCE PER PROVIDER but materialized against the CORES that consume that
/// provider: a provider is a market-data dedup key, not necessarily a core a chart may be opened
/// on. Each core therefore offers its own row for a shared market, exactly as a typed search does.
///
/// This walks every market of every provider — it is expensive and must never run at render; see
/// the suggestion cache in `Backend`.
///
/// Args:
///     b: Backend holding the session and market source.
///     group: Window group whose cores feed this field.
///     bucket: Chart bucket narrowing those cores, or `None` for the whole group.
///     limit: Maximum number of rows to return.
///
/// Returns:
///     Hits ordered by turnover-weighted 24-hour movement, descending, each carrying its resolved
///     label.
pub(crate) fn suggest_volatile(
    b: &Backend,
    group: &str,
    bucket: Option<&ChartBucket>,
    limit: usize,
) -> Vec<CoinHit> {
    let cores = cores_for(b, group, bucket);
    if cores.is_empty() || limit == 0 {
        return Vec::new();
    }
    let ms = b.session.market_source();
    // One fetch per distinct provider, then fan the rows back out to the cores that read it.
    // A core with no resolved provider is skipped rather than queried as its own provider, the
    // way the screener does it — asking a non-provider core for a market list answers nothing.
    let mut consumers: Vec<(CoreId, Vec<CoreId>)> = Vec::new();
    for core in &cores {
        let Some(provider) = ms.provider_of(*core) else {
            continue;
        };
        match consumers.iter_mut().find(|(p, _)| *p == provider) {
            Some((_, cs)) => cs.push(*core),
            None => consumers.push((provider, vec![*core])),
        }
    }

    // Rank each provider's markets and keep only its visible head, carrying the index of the
    // consumer list that head belongs to. Fanning every market out to its consuming cores BEFORE
    // ranking would multiply every market of every provider by its core count — thousands of cloned
    // market names on a many-core config — to produce the same handful of rows.
    let mut heads: Vec<Mover> = Vec::new();
    for (slot, (provider, members)) in consumers.iter().enumerate() {
        // The rate depends on the QUOTE, and one exchange lists only a handful of them, while this
        // loop walks every market it has. Resolving per market would take the market-source lock
        // and a snapshot hundreds of times to answer the same few questions.
        let mut rates: HashMap<String, Option<f64>> = HashMap::new();
        let mut ranked: Vec<Mover> = ms
            .screener_rows(*provider, members)
            .into_iter()
            .filter(|row| row.d_24h.is_finite() && row.d_24h > 0.0)
            .map(|row| {
                // No rate is UNKNOWN turnover, not zero turnover. Substituting 1.0 would value a
                // BTC-quoted market's turnover as though one BTC were one dollar and bury a market
                // that may be among the busiest on the board; scoring it at the reference figure
                // instead lets it compete on movement, which is all that is actually known.
                let quote = moon_core::symbol::resolve_quote(&row.market).to_string();
                let rate = *rates
                    .entry(quote)
                    .or_insert_with(|| ms.quote_usd_rate(*provider, &row.market));
                let turnover = turnover_usd(row.vol_24h, rate);
                Mover {
                    score: mover_score(row.d_24h, turnover.unwrap_or(MOVER_VOL_REF)),
                    movement: row.d_24h,
                    turnover,
                    market: row.market,
                    slot,
                }
            })
            .collect();
        // A provider with no convertible turnover is rescored so it still competes; see
        // `neutralize_blind_provider`. This runs before the head is cut because rescoring may
        // change which candidates belong in that head.
        neutralize_blind_provider(&mut ranked);
        heads.extend(merge_ranked_heads(ranked, limit));
    }
    // Merge those per-provider heads into the top movers of the WHOLE scope, rather than leaving
    // the first provider's head padded out with the next provider's.
    let heads = merge_ranked_heads(heads, limit);
    // ONE row per market, offered on the FIRST core that can open it. A mover is a property of the
    // market, not of the cores watching it, and on a config where forty cores share an exchange the
    // fan-out turned a top of eight into the same coin repeated down the whole list. `consumers`
    // was built by walking `cores_for`, which is already canonical order, so its head is the core
    // the user reads first everywhere else.
    let mut out: Vec<(CoreId, String)> = heads
        .into_iter()
        .filter_map(|mover| {
            let core = *consumers[mover.slot].1.first()?;
            Some((core, mover.market))
        })
        .collect();
    out.truncate(SUGGEST_ROW_CAP);
    hits_for(b, out)
}

/// Resolves `(core, market)` pairs into labelled hits, batching label lookups per core.
///
/// Shared by the volatility ranking and the recents list: both hold bare market keys and need the
/// same server name and [`MarketLabel`] a typed search resolves. Input order is preserved, because
/// for recents that order IS the information. A market whose core is no longer a live session is
/// dropped rather than rendered with an empty server name.
pub(crate) fn hits_for(
    b: &Backend,
    pairs: impl IntoIterator<Item = (CoreId, String)>,
) -> Vec<CoinHit> {
    let pairs: Vec<(CoreId, String)> = pairs.into_iter().collect();
    if pairs.is_empty() {
        return Vec::new();
    }
    let ms = b.session.market_source();
    let venues = b.session.core_venues();
    let mut out: Vec<Option<CoinHit>> = vec![None; pairs.len()];
    // Group the positions by core so each core resolves its labels under one lock and snapshot.
    let mut cores: Vec<CoreId> = Vec::new();
    for (core, _) in &pairs {
        if !cores.contains(core) {
            cores.push(*core);
        }
    }
    for core in cores {
        let Some(server) = b
            .session
            .sessions()
            .iter()
            .find(|s| s.id == core)
            .map(|s| s.name.clone())
        else {
            continue;
        };
        // Cloned once per CORE rather than per hit: one core contributes many rows to a query, and
        // the value is three small fields.
        let venue = venues.get(&core).cloned();
        let positions: Vec<usize> = pairs
            .iter()
            .enumerate()
            .filter(|(_, (c, _))| *c == core)
            .map(|(ix, _)| ix)
            .collect();
        let refs: Vec<&str> = positions.iter().map(|ix| pairs[*ix].1.as_str()).collect();
        let labels = ms.market_labels(core, &refs);
        for (ix, label) in positions.into_iter().zip(labels) {
            out[ix] = Some(CoinHit {
                core,
                market: pairs[ix].1.clone(),
                server: server.clone(),
                label,
                venue: venue.clone(),
            });
        }
    }
    out.into_iter().flatten().collect()
}

/// Identity of one coin row: the exchange it sits under, and the instrument it names.
///
/// The value a HOST retains between frames to remember which rows are open, so it borrows nothing.
/// The exchange belongs in the key because the same instrument on two exchanges is two different
/// choices — folding them together would offer `BTC-USDT` once and silently pick a venue.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CoinGroupKey {
    pub(crate) section: ExchangeSection,
    pub(crate) pair: SharedString,
}

/// One coin, and every core in this section that can open it.
pub(crate) struct CoinGroup {
    pub(crate) key: CoinGroupKey,
    /// The instrument label the coin row displays, formatted once by [`group_hits`].
    pub(crate) pair: SharedString,
    /// The cores offering it, in canonical core order. Never empty.
    pub(crate) members: Vec<CoinHit>,
}

/// One exchange's block of coin rows.
pub(crate) struct CoinSection {
    /// What the section stands for, or `None` for the cores no venue could be named for.
    pub(crate) venue: Option<CoreVenue>,
    pub(crate) groups: Vec<CoinGroup>,
}

/// Number of cores above which a coin row starts COLLAPSED.
///
/// Three cores fit under a coin without pushing the next coin off the screen; the user's own config
/// has fifty-six, where an expanded-by-default row IS the wall this grouping exists to remove.
pub(crate) const COIN_GROUP_AUTO_EXPAND: usize = 3;

/// Keeps small groups immediately usable without reopening a wall of cores on shared markets.
pub(crate) fn group_starts_expanded(members: usize) -> bool {
    members <= COIN_GROUP_AUTO_EXPAND
}

/// Fold hits into exchange sections, each holding one row per COIN with its cores underneath.
///
/// The list concatenates one page of hits per core, so a coin every core carries arrives as one row
/// per core — fifty-six identical `BTC-USDT` lines on the user's config. Folding them makes the
/// coin the row and the cores its children.
///
/// The key is the exchange section plus the FULL instrument label ([`MarketLabel::pair`]), never
/// the contract-stripped coin: `display_coin`/`match_key` fold `BTC_0925` into `BTC` on purpose, so
/// grouping by either would merge a perpetual with a dated contract — two different instruments the
/// user must be able to tell apart. Nothing is removed or deduplicated: the members ARE the cores,
/// each still openable on its own.
///
/// Sections come from [`crate::core_order::exchange_sections`], the same bucketing the left rail
/// and the Strategies tree use, so a coin list and a core list can never disagree about which
/// exchange a core belongs to. Ordering is stable in both directions — groups appear in the order
/// their first hit did (canonical core order), and members keep their arrival order inside a
/// group — so the list does not reshuffle as the query grows.
///
/// Args:
///     hits: Search or suggestion hits, in the order they were produced.
///
/// Returns:
///     Exchange sections, unidentified first, each holding its coin groups.
pub(crate) fn group_hits(hits: Vec<CoinHit>) -> Vec<CoinSection> {
    // Resolve the sections while the hits are still borrowable, and take OWNED keys out of that
    // borrow so the hits can be consumed below.
    let plan: Vec<(ExchangeSection, Option<CoreVenue>, Vec<usize>)> =
        crate::core_order::exchange_sections(
            hits.iter()
                .enumerate()
                .map(|(ix, hit)| (ix, hit.venue.as_ref())),
        )
        .into_iter()
        .map(|(venue, members)| {
            (
                crate::core_order::section_of(venue),
                venue.cloned(),
                members,
            )
        })
        .collect();
    // Moved out by index below: `exchange_sections` hands back POSITIONS, and a coin's members are
    // scattered across them, so the hits cannot simply be drained in order.
    let mut hits: Vec<Option<CoinHit>> = hits.into_iter().map(Some).collect();
    plan.into_iter()
        .map(|(section, venue, members)| {
            // First-appearance order of each pair inside this section, so groups read in canonical
            // core order rather than in hash order.
            let mut order: Vec<SharedString> = Vec::new();
            let mut buckets: HashMap<SharedString, Vec<CoinHit>> = HashMap::new();
            for ix in members {
                let Some(hit) = hits[ix].take() else {
                    continue;
                };
                let pair = SharedString::from(hit.label.pair());
                match buckets.get_mut(&pair) {
                    Some(bucket) => bucket.push(hit),
                    None => {
                        order.push(pair.clone());
                        buckets.insert(pair, vec![hit]);
                    }
                }
            }
            let groups = order
                .into_iter()
                .filter_map(|pair| {
                    let members = buckets.remove(&pair)?;
                    Some(CoinGroup {
                        key: CoinGroupKey {
                            section,
                            pair: pair.clone(),
                        },
                        pair,
                        members,
                    })
                })
                .collect();
            CoinSection { venue, groups }
        })
        .filter(|section| !section.groups.is_empty())
        .collect()
}

/// Choose the core a coin row opens on: the ACTIVE one when it carries the coin, else the first.
///
/// The coin row stands for the instrument rather than for any one core, so it needs a rule for
/// which core it hands to `on_pick`. The active core is what every other surface in the window is
/// already addressing; when it does not carry this instrument the first member is the core the user
/// reads first everywhere else, because `members` is in canonical [`cores_for`] order.
///
/// Args:
///     members: The cores offering one instrument, in canonical order. Never empty.
///     active: Core the window is currently addressing, or `None` for an unscoped list.
///
/// Returns:
///     The member to open, or `None` only for an empty slice, which [`group_hits`] never produces.
pub(crate) fn pick_core(members: &[CoinHit], active: Option<CoreId>) -> Option<&CoinHit> {
    active
        .and_then(|active| members.iter().find(|hit| hit.core == active))
        .or_else(|| members.first())
}

/// The market `Enter` opens: the first matching coin, on the active core.
///
/// Only a TYPED query has a "first match" — the empty-field list is Recent and Top movers, two
/// SUGGESTIONS the user has not asked for, so `Enter` there must open nothing rather than pick one
/// for them.
///
/// Args:
///     results: What the dropdown is currently showing.
///     active: Core the window is addressing, resolved exactly as the row click resolves it.
///
/// Returns:
///     The core and market to open, or `None` for an empty field, an empty query and a
///     suggestion list.
pub(crate) fn enter_target(
    results: CoinResults,
    active: Option<CoreId>,
) -> Option<(CoreId, String)> {
    let CoinResults::Query(hits) = results else {
        return None;
    };
    if hits.is_empty() {
        return None;
    }
    let sections = group_hits(hits);
    let group = sections.first()?.groups.first()?;
    let hit = pick_core(&group.members, active)?;
    Some((hit.core, hit.market.clone()))
}

/// What the dropdown is showing: matches for a typed query, or suggestions for an empty field.
///
/// Modelled as data rather than as a flag beside a single vector so a caller cannot render
/// suggestions under the "no matches" empty state, or label a query result "Recent".
pub(crate) enum CoinResults {
    /// Matches for a non-empty query. Empty means the query matched nothing.
    Query(Vec<CoinHit>),
    /// Shown while the field is empty: what was opened lately, and what is moving most.
    Suggest {
        /// Recently opened markets, newest first.
        recent: Vec<CoinHit>,
        /// Markets ranked by unsigned 24-hour movement weighted by USD turnover.
        volatile: Vec<CoinHit>,
    },
    /// The markets the user marked, on the Favorites tab. Always empty for now — nothing marks one
    /// yet — and drawn as its own empty state rather than as "no matches", which would read as a
    /// failed search.
    Favorites(Vec<CoinHit>),
    /// The temporary bans the cores in scope are holding, on their own tab.
    Banned(Vec<CoinBan>),
}

/// [`CoinResults`] after grouping, so the row arithmetic and the renderer read the SAME shape.
enum GroupedResults {
    /// One grouped list drawn without a heading — a typed query, or the markets the reader marked.
    /// The same rows and the same arithmetic; only the note an EMPTY one prints differs, so they
    /// are one variant carrying that note rather than two identical arms.
    Sections {
        /// Section identity, which keeps element ids unique across lists.
        section: &'static str,
        /// Locale key of the note shown when the list is empty.
        empty: &'static str,
        sections: Vec<CoinSection>,
    },
    Suggest {
        recent: Vec<CoinSection>,
        volatile: Vec<CoinSection>,
    },
    /// Not grouped: a ban belongs to ONE core, so there is nothing to fold. Carried through this
    /// enum anyway so the row arithmetic and the renderer keep reading the same value.
    Banned(Vec<CoinBan>),
}

/// Returns the fixed height shared by every direct child of the scrolling result list.
///
/// Args:
///     cx: Application context used to resolve the font-scaled design height.
///
/// Returns:
///     The row height in logical pixels.
fn coin_row_h(cx: &App) -> f32 {
    design::fit_h_value(cx, 20.0, 12.0, 4.0)
}

/// Returns the visible whole-row count for a raw viewport cap.
///
/// Args:
///     raw_cap: Maximum viewport height in logical pixels.
///     row_h: Fixed direct-child row height in logical pixels.
///
/// Returns:
///     The floored integral count when a row fits, or one slot to keep a scaled list visible; the
///     latter minimum can exceed `raw_cap`.
fn whole_row_slots(raw_cap: f32, row_h: f32) -> usize {
    (raw_cap / row_h).floor().max(1.0) as usize
}

/// Returns a viewport cap composed of complete result rows.
///
/// Args:
///     raw_cap: Maximum viewport height in logical pixels.
///     row_h: Fixed direct-child row height in logical pixels.
///
/// Returns:
///     The largest integral cap at or below `raw_cap` when one row fits, or one full row so the
///     list cannot collapse; that minimum can exceed `raw_cap`.
fn whole_row_cap(raw_cap: f32, row_h: f32) -> f32 {
    whole_row_slots(raw_cap, row_h) as f32 * row_h
}

/// Applies a recorded toggle as an inversion of the size-based default.
///
/// `toggled` holds the groups the user has FLIPPED away from their default, not the open ones, so a
/// freshly opened popup needs no seeding and a small group is open without ever being recorded.
///
/// Args:
///     key: Identity of the coin row.
///     members: How many cores it holds, which decides its default.
///     toggled: Groups the host has recorded a click on.
///
/// Returns:
///     Whether its child rows are rendered.
pub(crate) fn group_is_open(
    key: &CoinGroupKey,
    members: usize,
    toggled: &HashSet<CoinGroupKey>,
) -> bool {
    group_starts_expanded(members) != toggled.contains(key)
}

/// Avoids redundant exchange chrome when every row belongs to one venue.
///
/// One exchange needs no heading — every row is on it, and the caption would be a line of chrome
/// repeating what the scope already says.
pub(crate) fn shows_sections(sections: &[CoinSection]) -> bool {
    sections.len() > 1
}

/// Counts the fixed-height direct children one grouped list adds to the scrolling list.
///
/// The viewport cap and the overflow fade are both derived from a COUNT of fixed-height direct
/// children ([`whole_row_slots`], [`whole_row_cap`]), so every row type this list can draw has to
/// be counted here or the last visible row is cut and the fade lies about what is below it.
///
/// Args:
///     sections: The grouped list, as [`group_hits`] produced it.
///     toggled: Groups the host has recorded a click on.
///
/// Returns:
///     Exchange headings, coin rows and the child rows of OPEN groups.
pub(crate) fn direct_row_count(sections: &[CoinSection], toggled: &HashSet<CoinGroupKey>) -> usize {
    let headings = if shows_sections(sections) {
        sections.len()
    } else {
        0
    };
    headings
        + sections
            .iter()
            .flat_map(|section| section.groups.iter())
            .map(|group| {
                // The `members > 1` half is NOT redundant, and dropping it is the bug this guard
                // exists for: a one-core group has no caret (`push_section` builds it under the
                // same condition), so it can never enter `toggled`, so `group_is_open` is
                // unconditionally true for it -- while the renderer skips its child row anyway.
                // Counting 2 where 1 is drawn inflates the total on the COMMON case and paints the
                // continuation fade over a list with nothing below it.
                1 + if group.members.len() > 1
                    && group_is_open(&group.key, group.members.len(), toggled)
                {
                    group.members.len()
                } else {
                    0
                }
            })
            .sum::<usize>()
}

/// Renders one grouped list into `list`, preceded by `heading` when there is something to show.
///
/// Three row kinds, every one a FIXED-HEIGHT DIRECT CHILD so [`direct_row_count`] can size the
/// viewport: an exchange heading (only when the list spans more than one), a COIN row, and — while
/// that coin row is open — one child row per core offering it.
///
/// Args:
///     list: Stateful scrolling list that receives the rows.
///     id: Stable popup identity used to derive row and checkbox IDs.
///     section: Stable section identity that keeps IDs unique across suggestion groups.
///     heading: Optional localized heading, omitted for ordinary query results.
///     sections: The grouped list, as [`group_hits`] produced it.
///     selected: Markets currently accumulated for multi-select.
///     toggled: Coin rows the host has recorded a caret click on.
///     multi_select: Whether rows include selection checkboxes.
///     show_server_per_row: Whether a coin row names the core it would open on.
///     active_core: Core the window is addressing, which decides what a coin row opens.
///     p: Active palette used by row text and hover states.
///     cx: Application context used to resolve scaled design tokens.
///     on_pick: Callback for opening a row's market.
///     on_toggle: Callback for changing a checkbox selection.
///     on_expand: Callback for a caret click, carrying the coin row's identity.
///
/// Returns:
///     The same stateful list with this section appended, or unchanged when `sections` is empty.
#[allow(clippy::too_many_arguments)]
fn push_section<F, G, E>(
    mut list: Stateful<Div>,
    id: &'static str,
    section: &'static str,
    heading: Option<String>,
    sections: Vec<CoinSection>,
    selected: &HashSet<(CoreId, String)>,
    toggled: &HashSet<CoinGroupKey>,
    multi_select: bool,
    show_server_per_row: bool,
    active_core: Option<CoreId>,
    p: MoonPalette,
    cx: &App,
    on_pick: F,
    on_toggle: G,
    on_expand: E,
) -> Stateful<Div>
where
    F: Fn(CoreId, String, &mut Window, &mut App) + Clone + 'static,
    G: Fn(CoreId, String, &mut App) + Clone + 'static,
    E: Fn(CoinGroupKey, &mut App) + Clone + 'static,
{
    if sections.is_empty() {
        return list;
    }
    let row_h = coin_row_h(cx);
    let hover_bg = rgb(p.shell_high);
    let show_sections = shows_sections(&sections);
    if let Some(heading) = heading {
        list = list.child(
            div()
                .w_full()
                .h(px(row_h))
                .flex_none()
                .flex()
                .items_center()
                .px(design::ui_px(cx, 8.0))
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(heading),
        );
    }
    // One running index across every section, so an element id stays unique when a venue's caption
    // repeats or a coin appears under two exchanges.
    let mut i = 0usize;
    for venue_section in sections {
        if show_sections {
            list = list.child(
                div()
                    .w_full()
                    .h(px(row_h))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px(design::ui_px(cx, 8.0))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(p.text_muted))
                    .child(crate::controls::venue_section_label(
                        venue_section.venue.as_ref(),
                    )),
            );
        }
        for group in venue_section.groups {
            let members = group.members.len();
            let open = group_is_open(&group.key, members, toggled);
            // The coin row stands for the instrument; this is the core it would actually open, and
            // it is NAMED on the row so the choice is never hidden.
            let Some(picked) = pick_core(&group.members, active_core) else {
                continue;
            };
            let pick_core_id = picked.core;
            let pick_market = picked.market.clone();
            let pick_server = picked.server.clone();
            let checked = selected.contains(&(pick_core_id, pick_market.clone()));

            let on_pick_row = on_pick.clone();
            let market_pick = pick_market.clone();
            let on_toggle_row = on_toggle.clone();
            let market_toggle = pick_market.clone();
            let on_expand_row = on_expand.clone();
            let caret_key = group.key.clone();
            let pair = group.pair.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("{id}-{section}-row-{i}")))
                    .w_full()
                    .h(px(row_h))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px(design::ui_px(cx, 8.0))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover_bg))
                    .child(
                        h_flex()
                            .w_full()
                            .gap(design::ui_px(cx, 6.0))
                            .items_center()
                            // Clicking a multi-select checkbox does not open the market. The
                            // wrapper below needs no stop_propagation because MoonCheckbox does not
                            // trigger the row's on_pick handler.
                            .when(multi_select, |row| {
                                row.child(
                                    MoonCheckbox::new(SharedString::from(format!(
                                        "{id}-{section}-cb-{i}"
                                    )))
                                    .checked(checked)
                                    .size(MoonCheckboxSize::Compact)
                                    .on_change(
                                        move |_v: &bool, _w, app| {
                                            on_toggle_row(pick_core_id, market_toggle.clone(), app);
                                            app.stop_propagation();
                                        },
                                    ),
                                )
                            })
                            // A caret only where there is something under the row. A single-core
                            // coin renders exactly the row it always did.
                            .when(members > 1, |row| {
                                row.child(
                                    MoonDisclosure::button(
                                        SharedString::from(format!("{id}-{section}-caret-{i}")),
                                        open,
                                    )
                                    .direction(MoonDisclosureDirection::DownUp)
                                    .size(design::DISCLOSURE_GLYPH)
                                    .box_size(design::DISCLOSURE_BOX)
                                    .hover_color(p.text)
                                    .on_toggle(
                                        move |_v: &bool, _w, app| {
                                            on_expand_row(caret_key.clone(), app);
                                            app.stop_propagation();
                                        },
                                    ),
                                )
                            })
                            // Clicking the row text opens the coin on the picked core.
                            .child(
                                h_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(design::ui_px(cx, 6.0))
                                    .items_baseline()
                                    .on_mouse_down(MouseButton::Left, move |_, window, app| {
                                        on_pick_row(pick_core_id, market_pick.clone(), window, app);
                                        app.stop_propagation();
                                    })
                                    // The instrument never yields; the optional core name does.
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_size(design::t_body(cx))
                                            .text_color(rgb(p.text))
                                            .child(pair),
                                    )
                                    .when(members > 1, |row| {
                                        row.child(
                                            div()
                                                .flex_none()
                                                .text_size(design::t_caption(cx))
                                                .text_color(rgb(p.text_muted))
                                                .child(
                                                    t!("chart.coin.cores", n = members.to_string())
                                                        .to_string(),
                                                ),
                                        )
                                    })
                                    .when(show_server_per_row, |row| {
                                        row.child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .text_size(design::t_caption(cx))
                                                .text_color(rgb(p.text_muted))
                                                // `@core` distinguishes the server qualifier from
                                                // the instrument symbol.
                                                .child(format!("@{pick_server}")),
                                        )
                                    }),
                            ),
                    ),
            );
            i += 1;
            if !open || members <= 1 {
                continue;
            }
            for member in group.members {
                let CoinHit {
                    core,
                    market,
                    server,
                    ..
                } = member;
                let checked = selected.contains(&(core, market.clone()));
                let on_pick_child = on_pick.clone();
                let market_pick = market.clone();
                let on_toggle_child = on_toggle.clone();
                let market_toggle = market.clone();
                list = list.child(
                    div()
                        .id(SharedString::from(format!("{id}-{section}-row-{i}")))
                        .w_full()
                        .h(px(row_h))
                        .flex_none()
                        .flex()
                        .items_center()
                        .px(design::ui_px(cx, 8.0))
                        .pl(design::ui_px(cx, 22.0))
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover_bg))
                        // On the ROW, which is the stateful element: a plain `Div` carries no
                        // tooltip. A core name wider than the popup clips, and this is how the
                        // whole name — never a shortened one — stays reachable.
                        .tooltip(crate::panels::common::text_tooltip(server.clone()))
                        .child(
                            h_flex()
                                .w_full()
                                .gap(design::ui_px(cx, 6.0))
                                .items_center()
                                .when(multi_select, |row| {
                                    row.child(
                                        MoonCheckbox::new(SharedString::from(format!(
                                            "{id}-{section}-cb-{i}"
                                        )))
                                        .checked(checked)
                                        .size(MoonCheckboxSize::Compact)
                                        .on_change(
                                            move |_v: &bool, _w, app| {
                                                on_toggle_child(core, market_toggle.clone(), app);
                                                app.stop_propagation();
                                            },
                                        ),
                                    )
                                })
                                // The row-level tooltip keeps a clipped core name available without
                                // widening the popup for one unusually long configured name.
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .whitespace_nowrap()
                                        .overflow_hidden()
                                        .truncate()
                                        .text_size(design::t_caption(cx))
                                        .text_color(rgb(p.text_soft))
                                        .on_mouse_down(MouseButton::Left, move |_, window, app| {
                                            on_pick_child(core, market_pick.clone(), window, app);
                                            app.stop_propagation();
                                        })
                                        .child(server),
                                ),
                        ),
                );
                i += 1;
            }
        }
    }
    list
}

/// Hand the keyboard back to the window when the coin search is finished with.
///
/// The field keeps focus after a pick — nothing takes it away, and the terminal deliberately does
/// not blur an input just because something else was clicked. Visually the search is over and the
/// user is back on the chart; in fact every keystroke still belongs to a text field, which eats the
/// editing shortcuts outright: Ctrl+Z is Undo there, Ctrl+X is Cut, and both are perfectly ordinary
/// things to bind New Long and New Short to. The hotkey then does nothing with no symptom at all.
///
/// The mechanism is [`crate::hotkeys::release_field_focus`], which is also why this is conditional
/// on `field` actually holding the focus — not a nicety. Some exits are not clicks on the field at
/// all: the header ticker's list closes when the pointer merely leaves it, which happens perfectly
/// often while the user is typing somewhere else entirely. An unconditional blur there would reach
/// across the window and empty the caret out of whatever field they were in.
///
/// Kept as its own name rather than calling that one directly at every site: the contract test
/// `every_coin_search_exit_releases_the_keyboard` counts these calls per host, so a coin search
/// growing a new way out has to say so here.
pub(crate) fn release_focus(field: &Entity<MoonInputState>, window: &mut Window, cx: &App) {
    crate::hotkeys::release_field_focus(field, window, cx);
}

/// Renders the result dropdown: query matches, or whichever list the open tab asks for — the
/// empty-field suggestions, the marked markets, or the cores' temporary bans — with multi-selection
/// checkboxes and an Open in New Tab button where the rows can carry them. Clicking outside a checkbox calls the
/// owner-defined `on_pick`; a checkbox calls `on_toggle`; the footer button calls `on_open_new` for
/// the accumulated selection. `selected` contains the currently checked markets. In single-selection
/// mode, `on_toggle` and `on_open_new` are never called.
///
/// Args:
///     id: Stable popup identity used for the scroll container and child controls.
///     results: Query matches, or the list the open tab resolved.
///     selected: Markets currently accumulated for multi-select.
///     toggled: Coin rows the host has recorded a caret click on; see [`group_is_open`].
///     multi_select: Whether checkboxes and the Open in New Tab footer are enabled at all; a list
///         whose rows carry no checkbox turns them off whatever the host asked for.
///     active_core: Core the window is addressing, which decides what a coin row opens.
///     server_context: Sole server named once above the rows, or `None` to label every row.
///     tabs: The host's tab strip, or `None` for a query-only host (the header ticker, the Report
///         token filter) which shows no tabs and can produce none of their lists.
///     p: Active palette used by the dropdown.
///     cx: Application context used to resolve scaled design tokens.
///     on_pick: Callback for opening a row's market.
///     on_toggle: Callback for changing a checkbox selection.
///     on_expand: Callback for a caret click, carrying the coin row's identity.
///     on_open_new: Callback for opening the accumulated selection in a new tab.
///
/// Returns:
///     A stateful dropdown element whose list can retain scroll position.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_popup<F, G, H, E>(
    id: &'static str,
    results: CoinResults,
    selected: &HashSet<(CoreId, String)>,
    toggled: &HashSet<CoinGroupKey>,
    multi_select: bool,
    active_core: Option<CoreId>,
    server_context: Option<String>,
    tabs: Option<CoinTabsCfg>,
    p: MoonPalette,
    cx: &App,
    on_pick: F,
    on_toggle: G,
    on_expand: E,
    on_open_new: H,
) -> Stateful<Div>
where
    F: Fn(CoreId, String, &mut Window, &mut App) + Clone + 'static,
    G: Fn(CoreId, String, &mut App) + Clone + 'static,
    H: Fn(&mut Window, &mut App) + Clone + 'static,
    E: Fn(CoinGroupKey, &mut App) + Clone + 'static,
{
    let selected_count = selected.len();
    let show_server_per_row = server_context.is_none();
    let row_h = coin_row_h(cx);
    let visible_slots = whole_row_slots(COIN_LIST_RAW_CAP, row_h);
    let list_cap = whole_row_cap(COIN_LIST_RAW_CAP, row_h);
    // Grouped ONCE, here, and handed to both the arithmetic and the renderer: counting one shape
    // while drawing another is exactly how a viewport cap starts lying.
    let grouped = match results {
        CoinResults::Query(hits) => GroupedResults::Sections {
            section: "q",
            empty: "chart.coin.no_results",
            sections: group_hits(hits),
        },
        CoinResults::Suggest { recent, volatile } => GroupedResults::Suggest {
            recent: group_hits(recent),
            volatile: group_hits(volatile),
        },
        CoinResults::Favorites(hits) => GroupedResults::Sections {
            section: "fav",
            empty: "chart.coin.no_favorites",
            sections: group_hits(hits),
        },
        CoinResults::Banned(rows) => GroupedResults::Banned(rows),
    };
    // Whether a selection can be accumulated at all is a property of the ROWS on screen, not of the
    // tab that asked for them: the ban list draws lift buttons where the others draw checkboxes, so
    // its hint row and its footer would count markets none of its rows shows. Asked of the grouped
    // value, which is the one thing both the arithmetic and the renderer read.
    let multi_select = multi_select && !matches!(grouped, GroupedResults::Banned(_));
    let result_rows = match &grouped {
        GroupedResults::Sections { sections, .. } => {
            if sections.is_empty() {
                1
            } else {
                direct_row_count(sections, toggled)
            }
        }
        GroupedResults::Suggest { recent, volatile } => {
            if recent.is_empty() && volatile.is_empty() {
                1
            } else {
                direct_row_count(recent, toggled)
                    + usize::from(!recent.is_empty())
                    + direct_row_count(volatile, toggled)
                    + usize::from(!volatile.is_empty())
            }
        }
        // One fixed-height row per ban, or the one row the empty note occupies.
        GroupedResults::Banned(rows) => rows.len().max(1),
    };
    let direct_child_count =
        result_rows + usize::from(server_context.is_some()) + usize::from(multi_select);
    let list_overflows = direct_child_count > visible_slots;
    // `.id(..)` makes the container stateful so `overflow_y_scroll` can let GPUI track wheel
    // scrolling by ID. The integral cap keeps its final visible row whole at every font scale.
    let mut list = div()
        .id(SharedString::from(format!("{id}-list")))
        .flex()
        .flex_col()
        .w_full()
        .max_h(px(list_cap))
        .overflow_y_scroll();

    if let Some(server) = server_context {
        let context = t!("chart.coin.server_context", server = server).to_string();
        let tooltip = context.clone();
        list = list.child(
            div()
                .id(SharedString::from(format!("{id}-server-context")))
                .w_full()
                .h(px(row_h))
                .flex_none()
                .flex()
                .items_center()
                .px(design::ui_px(cx, 8.0))
                .whitespace_nowrap()
                .overflow_hidden()
                .truncate()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .tooltip(crate::panels::common::text_tooltip(tooltip))
                .child(context),
        );
    }

    // A one-line note on what the checkboxes accumulate toward, so the footer button is not the
    // first explanation of the mode — and only where checkboxes actually exist. It must stay ONE
    // line: wrapped, it doubles the popup's header and pushes the first result out of view. The
    // dictionary values are short enough to fit at the stock font scale; `whitespace_nowrap`
    // makes a future longer translation clip instead of silently folding.
    if multi_select {
        list = list.child(
            div()
                .w_full()
                .h(px(row_h))
                .flex_none()
                .flex()
                .items_center()
                .px(design::ui_px(cx, 8.0))
                .whitespace_nowrap()
                .overflow_hidden()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(t!("chart.coin.multi_hint").to_string()),
        );
    }

    let empty_note = |list: Stateful<Div>, text: String| {
        list.child(
            div()
                .w_full()
                .h(px(row_h))
                .flex_none()
                .flex()
                .items_center()
                .px(design::ui_px(cx, 8.0))
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(text),
        )
    };

    match grouped {
        GroupedResults::Sections {
            section,
            empty,
            sections,
        } => {
            if sections.is_empty() {
                list = empty_note(list, t!(empty).to_string());
            } else {
                list = push_section(
                    list,
                    id,
                    section,
                    None,
                    sections,
                    selected,
                    toggled,
                    multi_select,
                    show_server_per_row,
                    active_core,
                    p,
                    cx,
                    on_pick.clone(),
                    on_toggle.clone(),
                    on_expand.clone(),
                );
            }
        }
        GroupedResults::Suggest { recent, volatile } => {
            if recent.is_empty() && volatile.is_empty() {
                list = empty_note(list, t!("chart.coin.no_suggestions").to_string());
            } else {
                list = push_section(
                    list,
                    id,
                    "recent",
                    Some(t!("chart.coin.recent").to_string()),
                    recent,
                    selected,
                    toggled,
                    multi_select,
                    show_server_per_row,
                    active_core,
                    p,
                    cx,
                    on_pick.clone(),
                    on_toggle.clone(),
                    on_expand.clone(),
                );
                list = push_section(
                    list,
                    id,
                    "volatile",
                    Some(t!("chart.coin.top_volatile").to_string()),
                    volatile,
                    selected,
                    toggled,
                    multi_select,
                    show_server_per_row,
                    active_core,
                    p,
                    cx,
                    on_pick.clone(),
                    on_toggle.clone(),
                    on_expand.clone(),
                );
            }
        }
        GroupedResults::Banned(rows) => {
            if rows.is_empty() {
                list = empty_note(list, t!("chart.coin.no_banned").to_string());
            } else {
                list = tabs::push_ban_rows(
                    list,
                    id,
                    rows,
                    show_server_per_row,
                    p,
                    cx,
                    on_pick.clone(),
                    // Only a tabbed host can be showing this list at all, so the lift command is
                    // always in hand here; taken through the option anyway rather than unwrapped,
                    // which would turn a future caller's mistake into a panic in the frame loop.
                    tabs.as_ref().map(|cfg| cfg.on_unban.clone()),
                );
            }
        }
    }

    // The fade is anchored outside the scroll content and has no input handlers, so it signals
    // continuation without becoming another row or taking wheel/click interaction from the list.
    let list = div()
        .relative()
        .flex_none()
        .w_full()
        .child(list)
        .when(list_overflows, |wrapper| {
            wrapper.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(design::ui_px(cx, COIN_LIST_FADE_H))
                    .bg(linear_gradient(
                        180.0,
                        linear_color_stop(design::moon_alpha(p.panel_high, 0.0), 0.0),
                        linear_color_stop(design::moon_alpha(p.panel_high, 1.0), 1.0),
                    )),
            )
        });

    // Show the Open in New Tab footer only in multi-select mode and enable it for a nonempty
    // selection. Keep it outside the scroller so it remains visible, with the selected count in
    // its label.
    let footer = multi_select.then(|| {
        let label = if selected_count > 0 {
            format!("{} ({selected_count})", t!("chart.coin.open_new_tab"))
        } else {
            t!("chart.coin.open_new_tab").to_string()
        };
        div()
            .w_full()
            .px(design::ui_px(cx, 6.0))
            .py(design::ui_px(cx, 6.0))
            .border_t_1()
            .border_color(rgb(p.border))
            .child(
                MoonButton::new(SharedString::from(format!("{id}-open-new")))
                    .label(label)
                    .size(MoonButtonSize::Toolbar)
                    .variant(if selected_count > 0 {
                        MoonButtonVariant::Blue
                    } else {
                        MoonButtonVariant::Soft
                    })
                    .disabled(selected_count == 0)
                    .on_click(move |_, window, app| {
                        on_open_new(window, app);
                        app.stop_propagation();
                    })
                    .render(),
            )
    });

    div()
        .id(id)
        .flex()
        .flex_col()
        .w(design::font_w_px(cx, COIN_POPUP_W))
        .bg(rgb(p.panel_high))
        .border_1()
        .border_color(rgb(p.border))
        .rounded(design::r_button(cx))
        // Intercept mouse_down across the popup. A checkbox reacts on_change rather than
        // mouse_down, so the event would otherwise reach the dismiss layer underneath and close
        // the list. A pick row has its own earlier mouse-down handler with stop_propagation.
        .on_mouse_down(MouseButton::Left, |_, _window, app| app.stop_propagation())
        // Claim the wheel — and the pointer generally — for whatever sits UNDER this popup. The
        // chart reads the wheel through an ordinary gpui handler gated on its own hitbox, so
        // without this a scroll over the results list also rescaled the chart behind them. The
        // inner list keeps scrolling: its hitbox is pushed after this one and so is still hit
        // before the traversal stops here.
        .occlude()
        // Above the list and outside it: the viewport cap counts the list's own fixed-height rows.
        .children(
            tabs.as_ref()
                .map(|cfg| tabs::render_tab_strip(id, cfg, p, cx)),
        )
        .child(list)
        .children(footer)
}

#[cfg(test)]
mod tests;
