//! Signature-gated caches for the asset table and the wallet section.

use super::*;

impl AssetsView {
    /// Render-gate signature for asset, transfer, sale-marker, and balance-freshness inputs.
    pub(super) fn assets_sig(&self, b: &Backend) -> u64 {
        let store = b.session.store();
        self.scope_cores(b)
            .iter()
            // Include CoreId so canonical reordering invalidates the cache when state is unchanged.
            .map(|(id, _)| (*id, store.core(*id)))
            .fold(0u64, |a, (id, core)| {
                let a = a.wrapping_mul(31).wrapping_add(id);
                let Some(c) = core else {
                    return a;
                };
                a.wrapping_mul(31)
                    .wrapping_add(c.assets_rev)
                    .wrapping_mul(31)
                    .wrapping_add(c.transfer_rev)
                    .wrapping_mul(31)
                    .wrapping_add(c.orders_table_rev)
                    // Hash the rendered trust state rather than selected ingredients. Status
                    // transitions bump no data revision, but they can change `balance_state()`
                    // and must therefore invalidate the rendered balance immediately.
                    .wrapping_mul(31)
                    .wrapping_add(c.balance_state().code())
            })
    }

    /// Cache identity: every input `collect`/`per_core` read, so a change to any of them forces
    /// a rebuild. Kept in one place because it is built at two sites (the backend observer and
    /// `rebuild_cache`) that must not drift apart.
    pub(super) fn cache_key(&self, sig: u64) -> (u64, u64) {
        (sig, self.min_value_usd.to_bits())
    }

    /// Rebuild all render caches from one backend snapshot.
    pub(super) fn rebuild_cache(&mut self, b: &Backend) {
        let sig = self.assets_sig(b);
        // Cache membership data; the dropdown ranks again at render time.
        let cores: Vec<(CoreId, String)> = self.scope_cores(b).into_iter().collect();
        let selected_valid = self
            .selected_core
            .is_some_and(|core| cores.iter().any(|(id, _)| *id == core));
        if !selected_valid {
            self.selected_core = cores.first().map(|(id, _)| *id);
            self.cached_wallet_key = None;
        }
        self.cached_cores = cores;
        // Drop filter entries whose core is gone. Without this, deleting the one selected core
        // leaves a set that matches nothing, and "empty means all" never resumes — every
        // remaining core is filtered out and the panel reads as an empty account.
        if !self.sel_cores.is_empty() {
            self.sel_cores
                .retain(|id| self.cached_cores.iter().any(|(cid, _)| cid == id));
        }
        self.request_missing_transfers(b);
        self.sell_marked = Rc::new(self.collect_sell_marked(b));
        // Apply the header sort here, over the collector's default order: rows are rebuilt on data
        // changes (about 1 Hz while the panel is open), so it costs one pass per rebuild and
        // nothing per repaint. With no active sort this is a no-op and `collect`'s
        // descending-value order stands.
        let mut entries = self.collect(b);
        self.sort_entries(&mut entries);
        self.cached_entries = Rc::new(entries);
        self.cached_aggs = Rc::new(self.per_core(b));
        self.cached_all_futures = self.all_scope_cores_futures(b);
        self.rebuild_wallet_cache(b);
        // Skip non-finite row values so one bad price cannot turn the whole Σ into `NaN`, but
        // COUNT what was skipped — a silently shortened sum is indistinguishable from an honest
        // one, and the footer needs to say so.
        // Sum exactly what the rows DISPLAY, so Σ is the sum of the column above it.
        let (mut sum, mut excluded) = (0.0f64, 0usize);
        for e in self.cached_entries.iter() {
            if e.display_value.is_finite() {
                sum += e.display_value;
            } else {
                excluded += 1;
            }
        }
        self.cached_total_value = sum;
        self.cached_value_excluded = excluded;
        self.cache_sig = Some(self.cache_key(sig));
    }

    /// Requests transfer assets again for scoped cores that have not delivered a snapshot
    /// (`transfer_rev == 0`). The initial request in `new` may precede connection, so cache rebuilds
    /// retry until the first response. A positive revision stops retries even for an empty wallet.
    /// The upper table needs this because some exchanges expose purchased coins only via transfer
    /// assets.
    pub(super) fn request_missing_transfers(&self, b: &Backend) {
        let store = b.session.store();
        for (id, _) in &self.cached_cores {
            let rev = store.core(*id).map(|cd| cd.transfer_rev).unwrap_or(0);
            if rev == 0 {
                let _ = b.session.refresh_transfer_assets(*id);
            }
        }
    }

    pub(super) fn wallet_cache_key(&self, b: &Backend) -> (Option<CoreId>, u64, u64) {
        let transfer_rev = self
            .selected_core
            .and_then(|core| b.session.store().core(core).map(|cd| cd.transfer_rev))
            .unwrap_or(0);
        (
            self.selected_core,
            transfer_rev,
            self.min_value_usd.to_bits(),
        )
    }

    pub(super) fn rebuild_wallet_cache(&mut self, b: &Backend) {
        let key = self.wallet_cache_key(b);
        if self.cached_wallet_key == Some(key) {
            return;
        }
        let Some(core) = key.0 else {
            self.cached_wallets = Rc::new(Vec::new());
            self.cached_wallet_key = Some(key);
            return;
        };
        let Some(cd) = b.session.store().core(core) else {
            self.cached_wallets = Rc::new(Vec::new());
            self.cached_wallet_key = Some(key);
            return;
        };
        let mut snapshots = Vec::new();
        for kind in WalletKind::ALL {
            let all_items = cd.transfer_assets.wallet(kind).to_vec();
            let total_count = all_items.len();
            let thr = self.min_value_usd;
            let mut rows: Vec<TransferAssetRow> = all_items
                .into_iter()
                .filter(|a| thr <= 0.0 || a.value_usdt > thr)
                .collect();
            rows.sort_by(|a, b| {
                b.value_usdt
                    .partial_cmp(&a.value_usdt)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            snapshots.push(WalletColumnSnapshot {
                kind,
                total_count,
                rows,
            });
        }
        self.cached_wallets = Rc::new(snapshots);
        self.cached_wallet_key = Some(key);
    }
}
