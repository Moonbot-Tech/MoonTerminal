//! Grouping coin rows under their exchanges.

use super::*;

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
/// Sections come from [`core_order::exchange_sections`], the same bucketing the left rail
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
        core_order::exchange_sections(
            hits.iter()
                .enumerate()
                .map(|(ix, hit)| (ix, hit.venue.as_ref())),
        )
        .into_iter()
        .map(|(venue, members)| (core_order::section_of(venue), venue.cloned(), members))
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
    /// The markets the cores in scope have MARKED, on the Favorites tab.
    Favorites(Vec<CoinActRow>),
    /// The temporary bans the cores in scope are holding, on their own tab.
    Banned(Vec<CoinActRow>),
}

/// [`CoinResults`] after grouping, so the row arithmetic and the renderer read the SAME shape.
pub(super) enum GroupedResults {
    /// A typed query's matches, grouped by exchange and coin.
    Query(Vec<CoinSection>),
    /// The two empty-field suggestion sections.
    Suggest {
        recent: Vec<CoinSection>,
        volatile: Vec<CoinSection>,
    },
    /// An acting list — the marked markets or the running bans. Not grouped: each row is ONE
    /// core's statement about one of its markets, so there is nothing to fold. One variant for
    /// both, carrying which list it is, because they differ only in that.
    Marked {
        list: tabs::MarkedList,
        rows: Vec<CoinActRow>,
    },
}
