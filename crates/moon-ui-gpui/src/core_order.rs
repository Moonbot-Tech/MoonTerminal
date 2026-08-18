//! Canonical ordering for core (server) lists.
//!
//! [`CoreOrder`] ranks lists from the current [`CoreSortMode`] at render time. Pair lists can
//! carry the privately constructed [`OrderedCores`] marker; arbitrary row shapes use
//! [`CoreOrder::sort_by`].

use std::collections::HashMap;

use moon_core::config::{AppConfig, CoreSortMode};
use moon_core::feed::ExchangeId;
use moon_core::session::CoreId;
use moon_core::venue::CoreVenue;

/// Cores in canonical order, paired with the name to display.
///
/// Its private field prevents callers from presenting an unranked pair list as canonical.
pub(crate) struct OrderedCores(Vec<(CoreId, String)>);

// Expose the slice API while keeping construction inside this module.
impl std::ops::Deref for OrderedCores {
    type Target = [(CoreId, String)];

    /// Expose the ordered pairs as a slice without exposing construction.
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl IntoIterator for OrderedCores {
    type Item = (CoreId, String);
    type IntoIter = std::vec::IntoIter<(CoreId, String)>;

    /// Consume the canonical list in order.
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// Which exchange section one core belongs to.
///
/// The value a caller RETAINS when it has to remember a section across frames — a filter selection,
/// a chosen source — because it borrows nothing: current sections derive from the live venue map,
/// and a `&CoreVenue` cannot outlive that. Identity alone is enough to name the bucket, and
/// [`venue_id_label`] can caption it after every member has disconnected.
///
/// [`venue_id_label`]: crate::controls::venue_id_label
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ExchangeSection {
    /// Every core no venue could be named for, which share one section.
    Unidentified,
    /// One identified venue, keyed exactly as [`exchange_sections`] groups on.
    Venue(ExchangeId),
}

/// Return the section a core's venue belongs to.
///
/// THE bucketing rule, so a filter and the headings it filters cannot drift apart: both ask this
/// function rather than each re-deciding what "unidentified" means.
///
/// Args:
///     venue: What the core reported it is connected to, or `None` before that arrives.
///
/// Returns:
///     The core's section.
pub(crate) fn section_of(venue: Option<&CoreVenue>) -> ExchangeSection {
    match nameable(venue) {
        Some(venue) => ExchangeSection::Venue(venue.id),
        None => ExchangeSection::Unidentified,
    }
}

/// Keep a venue only while something can put a name on it.
///
/// A core whose venue nothing can name belongs in the unidentified group, not in a section of its
/// own captioned with that same wording.
///
/// Args:
///     venue: What the core reported it is connected to, or `None` before that arrives.
///
/// Returns:
///     The venue when it is nameable, else `None`.
fn nameable(venue: Option<&CoreVenue>) -> Option<&CoreVenue> {
    venue.filter(|venue| venue.is_nameable())
}

/// Partition ordered rows into unknown-first exchange sections ordered by brand.
///
/// Callers decide membership and row order before invoking this helper; the returned member indices
/// preserve that input order inside each section. Rows are bucketed by venue IDENTITY rather than by
/// the caption a core reported, so two cores on one venue always land in one section however their
/// core builds spell its name, and two Hyperliquid cores on different HIP-3 DEXes stay apart. An
/// unidentified core remains explicit rather than being guessed from a user-defined core name.
///
/// Section order comes from the directory ([`section_order`]): brands in alphabetical order, and
/// within one brand its spot venue before its derivatives — NOT the alphabetical order of the
/// rendered captions, which would put `Binance Futures` above `Binance Spot`.
///
/// Args:
///     rows: `(source index, core venue)` pairs in the desired member order.
///
/// Returns:
///     Venues and the source indices belonging to each section, the unidentified one first.
pub(crate) fn exchange_sections<'a>(
    rows: impl IntoIterator<Item = (usize, Option<&'a CoreVenue>)>,
) -> Vec<(Option<&'a CoreVenue>, Vec<usize>)> {
    let mut unknown = Vec::new();
    // Grouped by identity in insertion order, then ordered below. A linear scan beats a map here:
    // the key is a `Copy` pair of integers and there are at most a dozen sections.
    let mut known: Vec<(&CoreVenue, Vec<usize>)> = Vec::new();
    for (index, venue) in rows {
        // Through the shared predicate, so this partition and [`section_of`] cannot disagree about
        // which cores belong to the unidentified group.
        let Some(venue) = nameable(venue) else {
            unknown.push(index);
            continue;
        };
        match known.iter_mut().find(|(known, _)| known.id == venue.id) {
            Some((_, members)) => members.push(index),
            None => known.push((venue, vec![index])),
        }
    }
    // Ordered by the DIRECTORY, not by the rendered caption: sorting on the caption would make
    // section order depend on the interface language and on the wire-text sanitizer, and would
    // format every caption twice — once to compare, once to draw.
    known.sort_by(|(a, _), (b, _)| section_order(a).cmp(&section_order(b)));

    std::iter::once((None, unknown))
        .filter(|(_, members)| !members.is_empty())
        .chain(
            known
                .into_iter()
                .map(|(venue, members)| (Some(venue), members)),
        )
        .collect()
}

/// Stable ordering key for one exchange section.
///
/// Known venues sort by brand and then by market kind, which is the order their captions read in
/// since every caption is `"{Brand} {Kind}"`. An ordinal the directory cannot name has no brand to
/// sort by and takes its own ordinal instead — deliberately NOT the caption its core reported:
/// ordering by wire text would make the list depend on a string this build does not control, and
/// on which core happened to connect first.
///
/// Args:
///     venue: Venue the section stands for.
///
/// Returns:
///     Comparable key: known venues by brand and kind, then unknown ordinals by code.
fn section_order(venue: &CoreVenue) -> (u8, &str, u8, u8, &str) {
    match venue.resolved() {
        Some(known) => (
            0,
            known.brand.display(),
            known.kind as u8,
            0,
            venue.dex.as_str(),
        ),
        None => (1, "", 0, venue.id.code, venue.dex.as_str()),
    }
}

/// Insertion-order key, shared by both `Added*` modes so they cannot drift apart.
///
/// A server added in the Settings draft has no uid yet, so it ranks as the newest possible one —
/// last in oldest-first and first in newest-first, which is what both modes promise. `id` breaks
/// ties between several unsaved rows, which makes the key INJECTIVE: distinct servers always get
/// distinct keys, so `Reverse` produces an exact mirror rather than something that depends on
/// whether the sort happens to be stable.
fn insertion_key(s: &moon_core::config::ServerConfig) -> (u64, u64) {
    (if s.uid == 0 { u64::MAX } else { s.uid }, s.id)
}

/// A rank table built from the current config for one render pass.
///
/// Rebuilding is cheap for these short lists and prevents stale order.
pub(crate) struct CoreOrder {
    rank: HashMap<CoreId, u32>,
}

impl CoreOrder {
    /// Rank every configured core according to the user's chosen sort mode.
    ///
    /// Inactive cores are ranked too: they hold their place so that switching one off and
    /// back on returns it to the same position instead of the end of the list.
    pub(crate) fn new(cfg: &AppConfig) -> Self {
        Self::from_parts(&cfg.servers, cfg.core_sort)
    }

    /// Rank a server list and sort mode directly for focused ordering tests.
    ///
    /// Production callers use [`CoreOrder::new`]; this private seam accepts exactly the config
    /// data consumed by the ordering algorithm.
    fn from_parts(servers: &[moon_core::config::ServerConfig], sort: CoreSortMode) -> Self {
        let mut ordered: Vec<&moon_core::config::ServerConfig> = servers.iter().collect();
        match sort {
            // Lexicographic order of lowercase Unicode names, matching the group sort. Cache
            // each key on render; uid makes equal names independent of the servers Vec order.
            CoreSortMode::Name => {
                ordered.sort_by_cached_key(|s| (s.name.to_lowercase(), s.uid));
            }
            CoreSortMode::AddedOldest => ordered.sort_by_key(|s| insertion_key(s)),
            // Reversing is well-defined WITHOUT relying on sort stability because
            // `insertion_key` is injective — see its doc comment.
            CoreSortMode::AddedNewest => {
                ordered.sort_by_key(|s| std::cmp::Reverse(insertion_key(s)))
            }
        }
        let rank = ordered
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id, i as u32))
            .collect();
        Self { rank }
    }

    /// Rank of one core; cores the config does not know rank last.
    ///
    /// Private so callers order through this module rather than duplicating rank logic.
    fn rank(&self, id: CoreId) -> u32 {
        self.rank.get(&id).copied().unwrap_or(u32::MAX)
    }

    /// Order the live sessions kept by `keep` (group / scope filter).
    ///
    /// The predicate controls membership; this method only canonicalizes the retained rows.
    pub(crate) fn from_sessions<F>(
        &self,
        sessions: &[moon_core::session::CoreSession],
        keep: F,
    ) -> OrderedCores
    where
        F: Fn(&moon_core::session::CoreSession) -> bool,
    {
        let mut cores: Vec<(CoreId, String)> = sessions
            .iter()
            .filter(|s| keep(s))
            .map(|s| (s.id, s.name.clone()))
            .collect();
        cores.sort_by_key(|(id, _)| self.rank(*id));
        OrderedCores(cores)
    }

    /// Order rows that came from the reports database, whose names are the DB's own.
    ///
    /// Database-only cores whose server is absent from the current config share the `u32::MAX`
    /// rank `rank` hands out for anything unknown, so they land after every configured core and
    /// keep the query order — `sort_by_key` is a STABLE sort.
    pub(crate) fn from_db(&self, mut rows: Vec<(CoreId, String)>) -> OrderedCores {
        rows.sort_by_key(|(id, _)| self.rank(*id));
        OrderedCores(rows)
    }

    /// Sort any slice whose items carry a `CoreId` into canonical order.
    ///
    /// Use this for row shapes that cannot be represented as [`OrderedCores`].
    pub(crate) fn sort_by<T>(&self, rows: &mut [T], key: impl Fn(&T) -> CoreId) {
        rows.sort_by_key(|row| self.rank(key(row)));
    }
}

#[cfg(test)]
mod tests;
