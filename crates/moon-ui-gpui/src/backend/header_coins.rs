//! Header ticker, recent coins, suggestions, and clock zone.

use crate::Backend;
use gpui::Context;
use moon_core::session::CoreId;
use moon_core::session::core_order::CoreOrder;
use std::time::Duration;
use std::time::Instant;

impl Backend {
    /// Refresh the cached fallback ticker from the first canonical live core.
    ///
    /// `force` recomputes immediately when sorting changes the first core.
    pub(crate) fn refresh_header_ticker_default(&mut self, force: bool) {
        if !force
            && let Some((core, _)) = &self.header_ticker_default
            && self.session.sessions().iter().any(|s| s.id == *core)
        {
            return;
        }
        let now = Instant::now();
        if !force
            && self
                .last_header_ticker_refresh
                .is_some_and(|last| now.duration_since(last) < Duration::from_secs(1))
        {
            return;
        }
        self.last_header_ticker_refresh = Some(now);
        // Match the first core shown by canonical selectors.
        let all = CoreOrder::new(&self.config).from_sessions(self.session.sessions(), |_| true);
        let Some(core) = all.first().map(|(id, _)| *id) else {
            self.header_ticker_default = None;
            return;
        };
        let ms = self.session.market_source();
        let market = ["BTCUSDT", "UBTCUSDC"]
            .iter()
            .find(|cand| ms.search_markets(core, cand, 2).iter().any(|m| m == *cand))
            .map(|c| c.to_string())
            .or_else(|| ms.search_markets(core, "BTC", 1).into_iter().next());
        self.header_ticker_default = market.map(|market| (core, market));
    }

    /// Return the header price ticker source.
    ///
    /// A layout selection keyed by stable core UID is used while a session for that core is present;
    /// otherwise the precomputed default cache is returned. Rendering neither searches markets nor
    /// mutates the backend.
    pub(crate) fn header_ticker(&self) -> Option<(CoreId, String)> {
        if let Some(sel) = &self.layout.header_ticker
            && let Some(core) = self.core_of_uid(sel.core_uid)
            && self.session.sessions().iter().any(|s| s.id == core)
        {
            return Some((core, sel.market.clone()));
        }
        self.header_ticker_default
            .as_ref()
            .filter(|(core, _)| self.session.sessions().iter().any(|s| s.id == *core))
            .cloned()
    }

    /// Store the header ticker selected in the search popup by core UID and mark layout persistence dirty.
    pub(crate) fn set_header_ticker(&mut self, core: CoreId, market: String) {
        let Some(uid) = self.uid_of(core) else {
            return;
        };
        let sel = moon_core::config::layout::HeaderTicker {
            core_uid: uid,
            market: market.clone(),
        };
        if self.layout.header_ticker.as_ref() != Some(&sel) {
            self.layout.header_ticker = Some(sel);
            self.layout_dirty = true;
        }
    }

    /// The stable UID of a configured core, or `None` when it has no config entry.
    ///
    /// Persisted UI state names cores by UID rather than by `CoreId` so it survives a configuration
    /// reorder; this is the one place that translation is written, in either direction, together
    /// with [`Self::core_of_uid`].
    ///
    /// Args:
    ///     core: Live session identifier to translate.
    ///
    /// Returns:
    ///     The configured stable UID, or `None` when the core has no configuration entry.
    fn uid_of(&self, core: CoreId) -> Option<u64> {
        self.config
            .servers
            .iter()
            .find(|s| s.id == core)
            .map(|s| s.uid)
    }

    /// The live `CoreId` a persisted UID refers to, or `None` when that core is gone.
    ///
    /// Args:
    ///     uid: Stable configured UID to resolve.
    ///
    /// Returns:
    ///     The current session identifier, or `None` when the configuration no longer contains it.
    fn core_of_uid(&self, uid: u64) -> Option<CoreId> {
        self.config
            .servers
            .iter()
            .find(|s| s.uid == uid)
            .map(|s| s.id)
    }

    /// Record a market as the most recently opened one in the coin-search history.
    ///
    /// Thin wrapper: the MRU policy (move-to-front, dedup, cap) lives on [`WindowLayout`], beside
    /// the field it persists. A core with no config entry has no stable UID to store, so the call
    /// is a no-op rather than writing a UID that cannot be resolved back.
    ///
    /// Args:
    ///     core: Live core on which the market was opened.
    ///     market: Canonical market name.
    pub(crate) fn push_recent_coin(&mut self, core: CoreId, market: &str) {
        let Some(uid) = self.uid_of(core) else {
            return;
        };
        if self.layout.push_recent_coin(uid, market) {
            self.layout_dirty = true;
        }
    }

    /// Recently opened markets, newest first, resolved from stable UIDs to `CoreId`s.
    ///
    /// An entry whose core is gone from the configuration is skipped, but one whose core is merely
    /// OFFLINE is kept: liveness is decided once, downstream, by the resolver that also needs the
    /// session's name (`controls::coin_search::hits_for`), so this does not scan sessions itself.
    /// Nothing is ever dropped from the file here — a core that is offline right now keeps its
    /// history.
    ///
    /// Returns:
    ///     Resolvable `(core, market)` entries in most-recent-first order.
    pub(crate) fn recent_coins(&self) -> Vec<(CoreId, String)> {
        self.layout
            .recent_coins
            .iter()
            .flatten()
            .filter_map(|entry| Some((self.core_of_uid(entry.core_uid)?, entry.market.clone())))
            .collect()
    }

    /// Rebuild this field's coin suggestions unless a fresh list is already cached.
    ///
    /// Called when a coin-search popup OPENS — never from a render pass. Building the list walks
    /// every market of every provider feeding the field, which is far too expensive to repeat at
    /// frame rate; the chart chrome must not do work at present frequency.
    ///
    /// Args:
    ///     group: Window group whose cores feed the search field.
    ///     bucket: Optional chart bucket narrowing that core scope.
    pub(crate) fn refresh_coin_suggest(
        &mut self,
        group: &str,
        bucket: Option<&moon_core::config::ChartBucket>,
    ) {
        use crate::controls::coin_search;

        let key = (group.to_string(), bucket.cloned());
        let sig = coin_search::universe_sig(self, group, bucket);
        if self
            .coin_suggest
            .get(&key)
            .is_some_and(|entry| entry.is_fresh(&sig))
        {
            return;
        }
        let markets = coin_search::suggest_volatile(
            self,
            group,
            bucket,
            crate::controls::coin_search::COIN_SUGGEST_LIMIT,
        )
        .into_iter()
        .map(|hit| (hit.core, hit.market))
        .collect::<Vec<_>>();
        // An empty answer is "not ready yet", not "nothing to suggest": at startup the providers'
        // market snapshots have not arrived, so caching that emptiness would keep the section blank
        // — and the popup reading "no connected cores" — for the whole TTL after data does arrive.
        // Drop the entry instead, and let the next open try again.
        if markets.is_empty() {
            self.coin_suggest.remove(&key);
            return;
        }
        self.coin_suggest.insert(
            key,
            coin_search::CoinSuggestEntry {
                at: std::time::Instant::now(),
                sig,
                markets,
            },
        );
    }

    /// The cached suggestion markets for this field, or nothing when none is valid right now.
    ///
    /// This runs on the popup's RENDER path, so it only checks the entry's age. Validating the core
    /// and provider set here instead would re-sort the group's cores and take one market-source
    /// lock per core on every frame the popup is open — precisely the work the chart chrome must
    /// not do at present frequency. That check belongs to [`Self::refresh_coin_suggest`], which
    /// runs when the popup opens; between two opens an entry can at worst outlive a core by the
    /// TTL, and a suggestion for a core that just dropped resolves to nothing downstream anyway.
    ///
    /// Args:
    ///     group: Window group whose cached entry should be read.
    ///     bucket: Optional chart bucket narrowing that cache key.
    ///
    /// Returns:
    ///     Cached `(core, market)` pairs, or an empty vector when the entry is absent or expired.
    pub(crate) fn coin_suggest_markets(
        &self,
        group: &str,
        bucket: Option<&moon_core::config::ChartBucket>,
    ) -> Vec<(CoreId, String)> {
        self.coin_suggest
            .get(&(group.to_string(), bucket.cloned()))
            .filter(|entry| entry.is_recent())
            .map(|entry| entry.markets.clone())
            .unwrap_or_default()
    }

    /// The exact IANA zone id used application-wide, or `None` for an untouched profile.
    ///
    /// Returns the raw id rather than a parsed zone: resolution lives in the chrome layer, which
    /// already depends on `Backend`, and returning its type from here would close that loop.
    ///
    /// Returns:
    ///     Persisted IANA id, or `None` only for an untouched profile.
    pub(crate) fn header_clock_zone(&self) -> Option<&str> {
        self.layout.header_clock_zone.as_deref()
    }

    /// Store the application-wide display zone and publish a dedicated zone revision.
    ///
    /// `offset_min` is that zone's current offset, mirrored into the compatibility field so readers
    /// that understand only fixed offsets still show the right clock. Such a reader can rewrite the
    /// layout without the zone field, so a stale mirror would also lose the selection on its next
    /// save. A mirror-only change marks the layout dirty because summer time can move the offset
    /// while the zone remains stable; `chrome::clock` derives both values from one IANA zone.
    ///
    /// Args:
    ///     zone: Valid IANA zone id used by every civil-time surface.
    ///     offset_min: Current offset mirror retained for older layout readers.
    ///     cx: Backend context used to notify civil-time consumers when the zone identity changes.
    ///
    /// Returns:
    ///     Nothing; changed fields are marked dirty and zone observers are notified in place.
    pub(crate) fn set_header_clock_zone(
        &mut self,
        zone: &str,
        offset_min: i32,
        cx: &mut Context<Self>,
    ) {
        crate::chartdx::axes::set_display_zone(moon_core::util::display_time::zone_or_utc(Some(
            zone,
        )));
        let zone_changed = self.layout.header_clock_zone.as_deref() != Some(zone);
        if zone_changed || self.layout.header_clock_offset_min != offset_min {
            self.layout.header_clock_zone = Some(zone.to_string());
            self.layout.header_clock_offset_min = offset_min;
            self.layout_dirty = true;
            if zone_changed {
                self.display_time_revision.update(cx, |_, cx| cx.notify());
                // A station's bot cuts its reports in this zone too.
                self.station_zone_sync(cx);
            }
        }
    }
}
