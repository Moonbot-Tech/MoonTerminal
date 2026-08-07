//! The figure store and its `figures.json` persistence.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::config::{paths, write_file_atomic};
use crate::session::CoreId;

use super::Figure;

/// Figure-set key identifying the chart for a specific core and market. A figure always belongs
/// to the core it was drawn on; [`Figure::shared`] only widens who SEES it.
pub type FigureKey = (CoreId, String);

/// Store for all chart figures and their persistence. It lives in the UI backend; interactive
/// edits go through store methods that increment `rev` to gate redraws and set `dirty` for a
/// debounced save by the coordination tick, like config and dock state.
#[derive(Debug, Default)]
pub struct FigureStore {
    /// All chart figures: persisted local figures (`from_server=false`) and server-managed
    /// Moonbot alerts (`from_server=true`). Both are selected, moved, and deleted identically.
    by_key: HashMap<FigureKey, Vec<Figure>>,
    /// Figures this build could not parse, kept VERBATIM and written back unchanged.
    ///
    /// A file written by a newer build can hold a tool this one has no arm for. Skipping such a
    /// figure on load is only half the promise: the next debounced save would rewrite the file
    /// without it and destroy the drawing for the build that CAN draw it. Keeping the raw JSON
    /// here is what makes "an unknown figure costs only itself" true across a save, not merely
    /// across a load.
    unknown: HashMap<FigureKey, Vec<serde_json::Value>>,
    next_id: u64,
    /// Incremented on every change so the chart data state knows to reload the set.
    rev: u64,
    /// Whether edits are awaiting a debounced save.
    pub dirty: bool,
}

impl FigureStore {
    pub fn rev(&self) -> u64 {
        self.rev
    }

    /// Figures OWNED by this chart: drawn on this core, plus this core's server alerts.
    pub fn figures(&self, core: CoreId, market: &str) -> &[Figure] {
        self.by_key
            .get(&(core, market.to_string()))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Figures VISIBLE on this chart: the ones it owns, plus figures shared by other cores on the
    /// same market.
    ///
    /// The shared half scans the key set instead of keeping an index: the store holds tens of
    /// entries, and an index would have to be rebuilt on every drag frame — `edit` hands out a
    /// `&mut Figure` and cannot know whether the closure touched `shared`.
    pub fn visible<'a>(
        &'a self,
        core: CoreId,
        market: &'a str,
    ) -> impl Iterator<Item = &'a Figure> + 'a {
        let shared = self
            .by_key
            .iter()
            .filter(move |((c, m), _)| *c != core && m == market)
            .flat_map(|(_, v)| v.iter())
            .filter(|f| f.shared);
        self.figures(core, market).iter().chain(shared)
    }

    /// Whether any figure is visible on this chart, without building the iterator's tail.
    pub fn has_visible(&self, core: CoreId, market: &str) -> bool {
        self.visible(core, market).next().is_some()
    }

    /// Whether this chart OWNS the figure, as opposed to merely seeing it because another core
    /// shares it onto this market. Callers say so before a destructive action: deleting a figure
    /// one only sees destroys another core's original.
    pub fn owns(&self, core: CoreId, market: &str, id: u64) -> bool {
        self.figures(core, market).iter().any(|f| f.id == id)
    }

    /// The figure `id` as seen from this chart: the chart's own set first, then a figure another
    /// core shares onto the same market.
    ///
    /// Editing a shared figure edits the ORIGINAL, which is what makes an edit on one core's chart
    /// show up on every other core's. Resolving straight to the figure — rather than to a key the
    /// caller then looks up again — keeps a drag frame at one hash lookup.
    fn find_mut(&mut self, core: CoreId, market: &str, id: u64) -> Option<&mut Figure> {
        let own = (core, market.to_string());
        // Two passes over disjoint halves, because a borrow taken in the first would outlive it.
        if self
            .by_key
            .get(&own)
            .is_some_and(|v| v.iter().any(|f| f.id == id))
        {
            return self.by_key.get_mut(&own)?.iter_mut().find(|f| f.id == id);
        }
        self.by_key
            .iter_mut()
            .filter(|((c, m), _)| *c != core && m == market)
            .find_map(|(_, v)| v.iter_mut().find(|f| f.id == id && f.shared))
    }

    /// Reconciles server alert figures created in the core or Moonbot into the shared set.
    /// `server` is a fresh complete set of `from_server` figures decoded from blobs. Local
    /// figures (`from_server=false`) are preserved, and an armed local figure is not duplicated
    /// when a server figure has the same ID. This is called only when the server set changes,
    /// as gated by its activity counter, so `rev` is incremented unconditionally.
    pub fn set_server_figures(&mut self, server: HashMap<FigureKey, Vec<Figure>>) {
        for figs in self.by_key.values_mut() {
            figs.retain(|f| !f.from_server);
        }
        for (key, figs) in server {
            let entry = self.by_key.entry(key).or_default();
            for f in figs {
                if entry.iter().any(|e| e.id == f.id && !e.from_server) {
                    continue;
                }
                entry.push(f);
            }
        }
        self.by_key.retain(|_, v| !v.is_empty());
        self.rev = self.rev.wrapping_add(1);
    }

    /// Every READABLE figure in the store, as `(core, market, &Figure)` tuples.
    ///
    /// Readable excludes the [`Self::unknown`] set — figures written by a newer build that this one
    /// cannot parse. They are kept and written back verbatim, but there is no `Figure` to yield and
    /// nothing a list could usefully do with one.
    ///
    /// The Alerts panel lists the whole drawing layer — local figures and the core's own alerts
    /// side by side — and decides for itself which of them to show. It is deliberately not an
    /// "alerts only" iterator: a figure's `alert` flag is one of the columns the panel offers to
    /// filter and sort by, and the flag is what its checkbox WRITES, so a figure has to be listed
    /// before it can be armed.
    ///
    /// Each figure is yielded once, under the core that OWNS it — a figure shared onto other cores'
    /// charts is not repeated per viewer.
    pub fn all(&self) -> impl Iterator<Item = (CoreId, &str, &Figure)> + '_ {
        self.by_key
            .iter()
            .flat_map(|((c, m), v)| v.iter().map(move |f| (*c, m.as_str(), f)))
    }

    /// The figure as seen from this chart: its own set first, then a figure another core shares
    /// onto the same market. Spelled out rather than `visible(..).find(..)` because that iterator
    /// ties the market string's lifetime to the store's.
    /// Grace given to an upsert before the core's silence is taken as an answer, in milliseconds.
    ///
    /// Between our upsert and the core's echo the core legitimately does not hold the figure yet,
    /// and a reconcile triggered by ANY core in that window would read that absence as "Moonbot
    /// dropped it". Measured round trip on a live core is ~1.3 s; this is roughly four times it.
    const ALERT_ECHO_GRACE_MS: f64 = 5_000.0;

    /// Brings LOCAL figures' `alert` flags back in line with what the cores actually hold.
    ///
    /// Moonbot removes an object when its alert fires — measured on a live core against
    /// `chart alert` log events, including the case where the terminal merely DRAGS the figure and
    /// the core answers by dropping it — and nothing else ever told the terminal. A figure DRAWN in
    /// Moonbot disappears with it, because the server set is rebuilt
    /// from scratch on every reconcile; a figure drawn HERE kept a ticked box over an alert that no
    /// longer existed anywhere. That box is what the panel and the chart badge read, so it was a
    /// lie with nothing to notice it by.
    ///
    /// `held` is every `(core, obj_uid)` the cores currently hold. `authoritative` names the cores
    /// whose answer may be trusted: connected, and having reported their chart-alert set at least
    /// once this session — the core is asked for a full snapshot on connect, so anything absent
    /// after that is genuinely absent. A core that has said nothing yet cannot disarm anything.
    ///
    /// Returns whether any flag changed.
    pub fn reconcile_local_alerts(
        &mut self,
        held: &std::collections::HashSet<(CoreId, u64)>,
        authoritative: &std::collections::HashSet<CoreId>,
        now_ms: f64,
    ) -> bool {
        let mut changed = false;
        for ((core, _), figs) in self.by_key.iter_mut() {
            if !authoritative.contains(core) {
                continue;
            }
            for f in figs.iter_mut().filter(|f| f.alert && !f.from_server) {
                if held.contains(&(*core, f.id)) {
                    // Seen on the core: the round trip is over, and a later absence will be real.
                    f.alert_sent_ms = 0.0;
                    continue;
                }
                if f.alert_sent_ms > 0.0 && now_ms - f.alert_sent_ms < Self::ALERT_ECHO_GRACE_MS {
                    continue;
                }
                f.alert = false;
                f.alert_sent_ms = 0.0;
                changed = true;
            }
        }
        if changed {
            self.bump();
        }
        changed
    }

    pub fn get(&self, core: CoreId, market: &str, id: u64) -> Option<&Figure> {
        self.figures(core, market)
            .iter()
            .find(|f| f.id == id)
            .or_else(|| {
                self.by_key
                    .iter()
                    .filter(|((c, m), _)| *c != core && m == market)
                    .find_map(|(_, v)| v.iter().find(|f| f.id == id && f.shared))
            })
    }

    /// Adds a LOCAL figure and returns its ID.
    pub fn add(&mut self, core: CoreId, market: &str, mut fig: Figure) -> u64 {
        self.next_id += 1;
        fig.id = self.next_id;
        fig.from_server = false;
        let id = fig.id;
        self.by_key
            .entry((core, market.to_string()))
            .or_default()
            .push(fig);
        self.bump();
        id
    }

    /// Edits a figure in place while dragging a node or body. `edit` returns true if it changed
    /// anything. A figure shared by another core is found and edited in its owner's set.
    pub fn edit(
        &mut self,
        core: CoreId,
        market: &str,
        id: u64,
        edit: impl FnOnce(&mut Figure) -> bool,
    ) -> bool {
        let Some(fig) = self.find_mut(core, market, id) else {
            return false;
        };
        if edit(fig) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// Shares a figure across every core on its market, or takes the sharing back.
    ///
    /// Refused for a figure that cannot be shared — an armed alert belongs to one core, and a
    /// server-owned figure is not ours to widen. Returns whether anything changed.
    pub fn set_shared(&mut self, core: CoreId, market: &str, id: u64, shared: bool) -> bool {
        self.edit(core, market, id, |f| {
            if f.shared == shared || (shared && !f.can_share()) {
                return false;
            }
            f.shared = shared;
            true
        })
    }

    pub fn remove(&mut self, core: CoreId, market: &str, id: u64) -> Option<Figure> {
        // The owning key, resolved the same way `find_mut` resolves the figure itself.
        let own = (core, market.to_string());
        let key = if self
            .by_key
            .get(&own)
            .is_some_and(|v| v.iter().any(|f| f.id == id))
        {
            own
        } else {
            self.by_key
                .iter()
                .find(|((c, m), v)| {
                    *c != core && m == market && v.iter().any(|f| f.id == id && f.shared)
                })
                .map(|(k, _)| k.clone())?
        };
        let list = self.by_key.get_mut(&key)?;
        let idx = list.iter().position(|f| f.id == id)?;
        let fig = list.remove(idx);
        if list.is_empty() {
            self.by_key.remove(&key);
        }
        self.bump();
        Some(fig)
    }

    fn bump(&mut self) {
        self.rev = self.rev.wrapping_add(1);
        self.dirty = true;
    }

    // Persistence

    /// Highest core uid this store still holds figures for.
    ///
    /// Feeds the durable uid high-water mark: figures are keyed by `(CoreId, market)`, so a
    /// reissued uid would surface a deleted core's figures on the new core — and any edit or
    /// alert toggle made there would then target the new core.
    pub fn max_core_uid(&self) -> Option<u64> {
        // Retained-but-unreadable figures pin the floor as much as readable ones: their core still
        // owns a drawing, and its uid must not be handed to a new core.
        self.by_key
            .keys()
            .chain(self.unknown.keys())
            .map(|(core, _)| *core)
            .max()
    }

    /// Loads from `figures.json`; a missing or malformed file produces an empty store.
    ///
    /// An I/O failure that is NOT "no such file" is logged: the store would otherwise start empty
    /// and the first debounced save would overwrite a file that was merely locked or unreadable.
    pub fn load() -> Self {
        let path = paths::figures_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                log::warn!("не прочитал figures.json ({e}) → без фигур");
                String::new()
            }
        };
        Self::from_json(&text)
    }

    /// Parses store contents figure by figure.
    ///
    /// The file stays the flat ARRAY it has always been, deliberately: a version wrapper would
    /// make every file this build writes unreadable by the SHIPPED one, which falls back to an
    /// empty store and then saves that emptiness over the user's drawings. Tolerance is what
    /// forward compatibility actually needed, and it needs no new shape — a figure this build
    /// cannot parse is kept verbatim in `unknown` and written back untouched.
    ///
    /// Split from [`Self::load`] so the format rules are testable without a filesystem.
    pub fn from_json(text: &str) -> Self {
        let mut store = Self::default();
        if text.trim().is_empty() {
            return store;
        }
        let entries: Vec<PersistEntry> = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("figures.json битый ({e}) → без фигур");
                return store;
            }
        };
        for e in entries {
            let key = (e.core, e.market);
            let mut figures = Vec::with_capacity(e.figures.len());
            let mut unknown = Vec::new();
            for raw in e.figures {
                match serde_json::from_value::<Figure>(raw.clone()) {
                    Ok(mut f) => {
                        // An armed figure belongs to ONE core, so it cannot also be shared with
                        // all of them. The runtime guards cannot produce that pair; a hand-edited
                        // or foreign file can, and the alert is the half with a core to act on.
                        if f.alert && f.shared {
                            f.shared = false;
                        }
                        figures.push(f);
                    }
                    Err(err) => {
                        log::warn!(
                            "figures.json: фигуру ядра {} на {} читает только более новый билд ({err}) — сохраняю как есть",
                            key.0,
                            key.1
                        );
                        unknown.push(raw);
                    }
                }
            }
            if figures.is_empty() && unknown.is_empty() {
                continue;
            }
            // Ids of RETAINED figures count too: handing a new figure the id of one this build
            // cannot read would put two figures with one id in the file, and the build that reads
            // both could not tell them apart.
            let max_id = figures
                .iter()
                .map(|f| f.id)
                .chain(unknown.iter().filter_map(|v| v.get("id")?.as_u64()))
                .max()
                .unwrap_or(0);
            store.next_id = store.next_id.max(max_id);
            if !unknown.is_empty() {
                store.unknown.insert(key.clone(), unknown);
            }
            store.by_key.insert(key, figures);
        }
        store
    }

    /// Saves to `figures.json` non-fatally and clears `dirty`. Server alerts (`from_server`)
    /// are NOT persisted because they are managed by the core.
    pub fn save(&mut self) {
        match serde_json::to_string_pretty(&self.to_persist()) {
            Ok(s) => {
                if let Err(e) =
                    write_file_atomic(&paths::figures_path(), s.as_bytes(), "figures.json")
                {
                    log::warn!("не записал figures.json: {e}");
                }
            }
            Err(e) => log::warn!("не сериализовал figures.json: {e}"),
        }
        self.dirty = false;
    }

    /// Builds the persisted form: local figures only, one entry per chart, plus every figure this
    /// build could not read, written back exactly as it arrived.
    ///
    /// Keys are sorted so the file does not churn with the hash order on every save.
    fn to_persist(&self) -> Vec<PersistEntry> {
        let mut keys: Vec<&FigureKey> = self.by_key.keys().chain(self.unknown.keys()).collect();
        keys.sort_unstable();
        keys.dedup();
        keys.into_iter()
            .filter_map(|key| {
                let mut figures: Vec<serde_json::Value> = Vec::new();
                for f in self.by_key.get(key).into_iter().flatten() {
                    if f.from_server {
                        continue;
                    }
                    match serde_json::to_value(f) {
                        Ok(v) => figures.push(v),
                        Err(e) => log::warn!("figures.json: фигура {} не сериализуется: {e}", f.id),
                    }
                }
                figures.extend(self.unknown.get(key).into_iter().flatten().cloned());
                (!figures.is_empty()).then(|| PersistEntry {
                    core: key.0,
                    market: key.1.clone(),
                    figures,
                })
            })
            .collect()
    }
}

/// Flat serialization entry used because JSON cannot represent a `HashMap` with tuple keys.
///
/// Figures stay as raw values until they are parsed one by one, so an unknown figure type is
/// kept instead of failing the entry.
#[derive(Serialize, Deserialize)]
struct PersistEntry {
    core: CoreId,
    market: String,
    figures: Vec<serde_json::Value>,
}

#[cfg(test)]
mod tests;
