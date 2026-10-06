//! Local strategy-edit origin tracking and kind resolution.

use super::*;

/// Maps a `SignalType` field value to a strategy-kind (`StrategyKind`) ordinal. In Moonbot, a
/// strategy's type (kind) is its SignalType, but the snapshot stores the kind in a separate `kind`
/// byte rather than a field. Editing the field alone therefore does not change the kind, so map
/// the string to an ordinal and rebuild the snapshot consistently. First match the authoritative
/// kind names from the core schema, then fall back to our hard-coded names. No match means `None`
/// (leave the kind unchanged).
pub(super) fn signaltype_to_kind_ordinal(
    schema: Option<&StrategySchema>,
    value: &str,
) -> Option<u8> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    if let Some(s) = schema {
        if let Some(k) = s.kinds.iter().find(|k| k.name.eq_ignore_ascii_case(v)) {
            return Some(k.ordinal());
        }
    }
    (0u8..=23).find(|o| strat_kind_name(*o).eq_ignore_ascii_case(v))
}

/// Tracks local strategy-command timestamps in a `HashMap` plus a wildcard so strat_db can
/// heuristically mark snapshot versions `origin=local`. The wildcard covers commands without a
/// known id (creation assigns the id inside `rebuild_sync`). The 30-second TTL can misclassify a
/// recent remote change as local or a delayed local echo as remote.
pub(in crate::feed::live) struct LocalStratEdits {
    ids: std::collections::HashMap<u64, std::time::Instant>,
    wildcard: Option<std::time::Instant>,
}

const LOCAL_EDIT_TTL: std::time::Duration = std::time::Duration::from_secs(30);

impl LocalStratEdits {
    pub(in crate::feed::live) fn new() -> Self {
        Self {
            ids: std::collections::HashMap::new(),
            wildcard: None,
        }
    }

    pub(super) fn mark(&mut self, id: u64) {
        self.ids.insert(id, std::time::Instant::now());
    }

    pub(super) fn mark_all(&mut self) {
        self.wildcard = Some(std::time::Instant::now());
    }

    /// Returns the 30-second local-origin heuristic for this id or the wildcard.
    pub(in crate::feed::live) fn is_local(&self, id: u64) -> bool {
        let fresh = |t: &std::time::Instant| t.elapsed() < LOCAL_EDIT_TTL;
        self.ids.get(&id).map(fresh).unwrap_or(false)
            || self.wildcard.as_ref().map(fresh).unwrap_or(false)
    }

    pub(in crate::feed::live) fn prune(&mut self) {
        self.ids.retain(|_, t| t.elapsed() < LOCAL_EDIT_TTL);
        if self
            .wildcard
            .map(|t| t.elapsed() >= LOCAL_EDIT_TTL)
            .unwrap_or(false)
        {
            self.wildcard = None;
        }
    }
}
