//! Shared market picker for typed search and cached empty-field suggestions.
//!
//! Rows group identical full instrument labels across cores. They show the core as secondary
//! `@server` context unless one popup-level server context covers the whole list; the full pairing
//! remains in each row's tooltip. The widget does not define selection behavior; its owner supplies
//! `on_pick`.
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
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonPalette,
    h_flex,
};
use rust_i18n::t;

use crate::Backend;
use crate::design;
use moon_core::config::ChartBucket;
use moon_core::market::MarketLabel;
use moon_core::session::CoreId;

mod ranking;

use ranking::{
    MOVER_VOL_REF, Mover, SUGGEST_ROW_CAP, merge_ranked_heads, mover_score,
    neutralize_blind_provider, turnover_usd,
};

/// Maximum number of MoonProto search results requested per core.
pub(crate) const COIN_SEARCH_LIMIT: usize = 8;

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
            .map(|s| s.id)
            .collect::<Vec<_>>()
    };
    let order = crate::core_order::CoreOrder::new(&b.config);
    let mut ids = match bucket {
        None | Some(ChartBucket::Shared) => group_cores(),
        Some(ChartBucket::Core(id)) => vec![*id],
        Some(ChartBucket::Bundle(name)) => {
            let split = b.config.charts_split_by_core;
            b.session
                .sessions()
                .iter()
                .filter(|s| s.group == group)
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
    /// Core name shown beside the coin or in the popup-level context and row tooltip.
    pub(crate) server: String,
    /// Coin token and quote as the CORE names them; see `MarketDataSource::market_label`.
    pub(crate) label: MarketLabel,
}

/// Returns token-search results, each carrying its resolved label.
pub(crate) fn search(
    b: &Backend,
    group: &str,
    bucket: Option<&ChartBucket>,
    query: &str,
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
            ms.search_markets(core, query, COIN_SEARCH_LIMIT)
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
            });
        }
    }
    out.into_iter().flatten().collect()
}

/// One rendered result row: a hit plus whether it OPENS a run of the same instrument.
///
/// See [`group_hits`] for what a run is and why the flag is presentation-only.
struct CoinRow {
    hit: CoinHit,
    /// The instrument label this row displays, resolved once by [`group_hits`] as its grouping key
    /// rather than formatted again per row at render.
    pair: SharedString,
    /// First row of a run of the same instrument offered by several cores.
    first_of_group: bool,
}

/// Order hits so the same instrument offered by several cores forms one adjacent run.
///
/// The list concatenates one page of hits per core, so a coin available on eight cores arrives as
/// eight rows scattered by core order and reads as noise. Grouping makes each instrument one block.
///
/// The key is the FULL instrument label ([`MarketLabel::pair`]), never the contract-stripped coin:
/// `display_coin`/`match_key` fold `BTC_0925` into `BTC` on purpose, so grouping by either would
/// merge a perpetual with a dated contract — two different instruments the user must be able to
/// tell apart. Nothing is removed or deduplicated: the same coin on two cores is two real choices.
///
/// Ordering is stable in both directions — groups appear in the order their first hit did (which is
/// canonical core order), and hits keep their original order inside a group — so the list does not
/// reshuffle as the query grows.
///
/// Args:
///     hits: Search or suggestion hits, in the order they were produced.
///
/// Returns:
///     The same hits, reordered into runs, each row flagged as opening its run or continuing it.
fn group_hits(hits: Vec<CoinHit>) -> Vec<CoinRow> {
    // One `pair()` per hit, kept and reused: it formats a fresh string, and it is both the grouping
    // key and what the row displays.
    let mut keyed: Vec<(SharedString, CoinHit)> = hits
        .into_iter()
        .map(|hit| (SharedString::from(hit.label.pair()), hit))
        .collect();
    // Stable sort by first appearance of each key: groups keep the canonical core order they
    // arrived in, and members keep their order inside a group.
    let mut order: HashMap<SharedString, usize> = HashMap::new();
    for (key, _) in &keyed {
        let next = order.len();
        order.entry(key.clone()).or_insert(next);
    }
    keyed.sort_by_key(|(key, _)| order.get(key).copied().unwrap_or(usize::MAX));
    let mut out: Vec<CoinRow> = Vec::with_capacity(keyed.len());
    let mut previous: Option<SharedString> = None;
    for (pair, hit) in keyed {
        let first_of_group = previous.as_ref() != Some(&pair);
        previous = Some(pair.clone());
        out.push(CoinRow {
            hit,
            pair,
            first_of_group,
        });
    }
    out
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
}

/// Renders one section of rows into `list`, preceded by `heading` when there is something to show.
///
/// Args:
///     list: Stateful scrolling list that receives the section.
///     id: Stable popup identity used to derive row and checkbox IDs.
///     section: Stable section identity that keeps IDs unique across suggestion groups.
///     heading: Optional localized heading, omitted for ordinary query results.
///     hits: Rows to group by instrument and append.
///     selected: Markets currently accumulated for multi-select.
///     multi_select: Whether rows include selection checkboxes.
///     show_server_per_row: Whether each row displays its server beside the instrument.
///     p: Active palette used by row text and hover states.
///     cx: Application context used to resolve scaled design tokens.
///     on_pick: Callback for opening a row's market.
///     on_toggle: Callback for changing a checkbox selection.
///
/// Returns:
///     The same stateful list with this section appended, or unchanged when `hits` is empty.
#[allow(clippy::too_many_arguments)]
fn push_section<F, G>(
    mut list: Stateful<Div>,
    id: &'static str,
    section: &'static str,
    heading: Option<String>,
    hits: Vec<CoinHit>,
    selected: &HashSet<(CoreId, String)>,
    multi_select: bool,
    show_server_per_row: bool,
    p: MoonPalette,
    cx: &App,
    on_pick: F,
    on_toggle: G,
) -> Stateful<Div>
where
    F: Fn(CoreId, String, &mut Window, &mut App) + Clone + 'static,
    G: Fn(CoreId, String, &mut App) + Clone + 'static,
{
    if hits.is_empty() {
        return list;
    }
    let hover_bg = rgb(p.shell_high);
    if let Some(heading) = heading {
        list = list.child(
            div()
                .w_full()
                .px(design::ui_px(cx, 8.0))
                .pt(design::ui_px(cx, 6.0))
                .pb(design::ui_px(cx, 2.0))
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(heading),
        );
    }
    for (i, row) in group_hits(hits).into_iter().enumerate() {
        let CoinRow {
            hit,
            pair,
            first_of_group,
        } = row;
        let CoinHit {
            core,
            market,
            server,
            label: _,
        } = hit;
        // The full instrument, expiry included, on EVERY row: a continuation row that showed only
        // its core would make a perpetual and a dated contract on one core indistinguishable. The
        // continuation is subordinated by colour instead, so the run still reads as one block.
        let pair_fg = if first_of_group {
            rgb(p.text)
        } else {
            rgb(p.text_soft)
        };
        let tip = SharedString::from(format!("{pair} @ {server}"));
        let on_pick = on_pick.clone();
        let market_pick = market.clone();
        let checked = selected.contains(&(core, market.clone()));
        let on_toggle = on_toggle.clone();
        let market_toggle = market.clone();
        list = list.child(
            div()
                .id(SharedString::from(format!("{id}-{section}-row-{i}")))
                .w_full()
                .px(design::ui_px(cx, 8.0))
                .py(design::ui_px(cx, 4.0))
                .cursor_pointer()
                .hover(move |s| s.bg(hover_bg))
                // Visible row attribution may truncate or move to the popup header, so the full
                // pairing is always available on hover.
                .tooltip(crate::panels::common::text_tooltip(tip))
                .child(
                    h_flex()
                        .w_full()
                        .gap(design::ui_px(cx, 6.0))
                        .items_center()
                        // Clicking a multi-select checkbox does not open the market. The wrapper
                        // below needs no stop_propagation because MoonCheckbox does not trigger the
                        // row's on_pick handler.
                        .when(multi_select, |row| {
                            row.child(
                                MoonCheckbox::new(SharedString::from(format!(
                                    "{id}-{section}-cb-{i}"
                                )))
                                .checked(checked)
                                .size(MoonCheckboxSize::Compact)
                                .on_change(
                                    move |_v: &bool, _w, app| {
                                        on_toggle(core, market_toggle.clone(), app);
                                        app.stop_propagation();
                                    },
                                ),
                            )
                        })
                        // Clicking the row text selects one market through on_pick.
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .gap(design::ui_px(cx, 6.0))
                                .items_baseline()
                                .on_mouse_down(MouseButton::Left, move |_, window, app| {
                                    on_pick(core, market_pick.clone(), window, app);
                                    app.stop_propagation();
                                })
                                // The instrument never yields; the optional core name does.
                                .child(
                                    div()
                                        .flex_none()
                                        .text_size(design::t_body(cx))
                                        .text_color(pair_fg)
                                        .child(pair),
                                )
                                .when(show_server_per_row, |row| {
                                    row.child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(design::t_caption(cx))
                                            .text_color(rgb(p.text_muted))
                                            // `@core` distinguishes the server qualifier from the
                                            // instrument symbol.
                                            .child(format!("@{server}")),
                                    )
                                }),
                        ),
                ),
        );
    }
    list
}

/// Renders the result dropdown: query matches, or the empty-field suggestions, with multi-selection
/// checkboxes and an Open in New Tab button when enabled. Clicking outside a checkbox calls the
/// owner-defined `on_pick`; a checkbox calls `on_toggle`; the footer button calls `on_open_new` for
/// the accumulated selection. `selected` contains the currently checked markets. In single-selection
/// mode, `on_toggle` and `on_open_new` are never called.
///
/// Args:
///     id: Stable popup identity used for the scroll container and child controls.
///     results: Query matches or the two empty-field suggestion sections.
///     selected: Markets currently accumulated for multi-select.
///     multi_select: Whether checkboxes and the Open in New Tab footer are enabled.
///     server_context: Sole server named once above the rows, or `None` to label every row.
///     p: Active palette used by the dropdown.
///     cx: Application context used to resolve scaled design tokens.
///     on_pick: Callback for opening a row's market.
///     on_toggle: Callback for changing a checkbox selection.
///     on_open_new: Callback for opening the accumulated selection in a new tab.
///
/// Returns:
///     A stateful dropdown element whose list can retain scroll position.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_popup<F, G, H>(
    id: &'static str,
    results: CoinResults,
    selected: &HashSet<(CoreId, String)>,
    multi_select: bool,
    server_context: Option<String>,
    p: MoonPalette,
    cx: &App,
    on_pick: F,
    on_toggle: G,
    on_open_new: H,
) -> Stateful<Div>
where
    F: Fn(CoreId, String, &mut Window, &mut App) + Clone + 'static,
    G: Fn(CoreId, String, &mut App) + Clone + 'static,
    H: Fn(&mut App) + Clone + 'static,
{
    let selected_count = selected.len();
    let show_server_per_row = server_context.is_none();
    // `.id(..)` makes the container stateful so `overflow_y_scroll` can let GPUI track wheel
    // scrolling by ID. Without it, max_h would simply clip a long list.
    let mut list = div()
        .id(SharedString::from(format!("{id}-list")))
        .flex()
        .flex_col()
        .w_full()
        // Roughly a dozen rows before it starts scrolling. Raw pixels, unlike the width beside it:
        // the rows inside are font-scaled, so at a larger Font setting this shows fewer of them —
        // it is a scroll threshold, not a layout the content has to fit.
        .max_h(px(340.0))
        .overflow_y_scroll()
        .py(design::ui_px(cx, 4.0));

    if let Some(server) = server_context {
        let context = t!("chart.coin.server_context", server = server).to_string();
        let tooltip = context.clone();
        list = list.child(
            div()
                .id(SharedString::from(format!("{id}-server-context")))
                .w_full()
                .px(design::ui_px(cx, 8.0))
                .pb(design::ui_px(cx, 4.0))
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
                .px(design::ui_px(cx, 8.0))
                .pb(design::ui_px(cx, 4.0))
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
                .px(design::ui_px(cx, 8.0))
                .py(design::ui_px(cx, 4.0))
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(text),
        )
    };

    match results {
        CoinResults::Query(hits) => {
            if hits.is_empty() {
                list = empty_note(list, t!("chart.coin.no_results").to_string());
            } else {
                list = push_section(
                    list,
                    id,
                    "q",
                    None,
                    hits,
                    selected,
                    multi_select,
                    show_server_per_row,
                    p,
                    cx,
                    on_pick.clone(),
                    on_toggle.clone(),
                );
            }
        }
        CoinResults::Suggest { recent, volatile } => {
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
                    multi_select,
                    show_server_per_row,
                    p,
                    cx,
                    on_pick.clone(),
                    on_toggle.clone(),
                );
                list = push_section(
                    list,
                    id,
                    "volatile",
                    Some(t!("chart.coin.top_volatile").to_string()),
                    volatile,
                    selected,
                    multi_select,
                    show_server_per_row,
                    p,
                    cx,
                    on_pick.clone(),
                    on_toggle.clone(),
                );
            }
        }
    }

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
                    .on_click(move |_, _w, app| {
                        on_open_new(app);
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
        .child(list)
        .children(footer)
}

#[cfg(test)]
mod tests;
