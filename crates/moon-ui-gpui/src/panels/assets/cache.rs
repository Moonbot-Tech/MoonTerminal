//! Signature-gated caches for the asset table and the wallet section.

use super::*;

/// Folds every configured id and total mode without allocating, including out-of-scope cores.
pub(super) fn total_modes_sig(servers: &[moon_core::config::ServerConfig]) -> u64 {
    servers.iter().fold(0u64, |sig, server| {
        sig.wrapping_mul(31)
            .wrapping_add(server.id)
            .wrapping_mul(31)
            .wrapping_add(server.total_mode as u64)
    })
}

#[cfg(test)]
mod tests;

/// Cache work selected from explicit full-row, sale-marker, and periodic inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AssetsRefresh {
    /// Replace every cache and retry missing transfer snapshots.
    Full,
    /// Replace sale markers while retaining rows, balances, and wallet caches.
    SaleOnly,
    /// Keep every cache and skip notification.
    None,
}

/// Gives full-key changes and admitted gate ticks priority over order-only marker refreshes.
pub(super) fn assets_refresh(
    prev_key: Option<(u64, u64)>,
    key: (u64, u64),
    prev_sale: Option<u64>,
    sale: u64,
    gate_due: bool,
) -> AssetsRefresh {
    if prev_key != Some(key) || gate_due {
        AssetsRefresh::Full
    } else if prev_sale != Some(sale) {
        AssetsRefresh::SaleOnly
    } else {
        AssetsRefresh::None
    }
}

/// Advance represented identities together with the work admitted by the existing render gate.
pub(super) fn assets_observe_step(
    prev_key: Option<(u64, u64)>,
    key: (u64, u64),
    prev_sale: Option<u64>,
    sale: u64,
    gate_due: bool,
) -> (AssetsRefresh, Option<(u64, u64)>, Option<u64>) {
    let refresh = assets_refresh(prev_key, key, prev_sale, sale, gate_due);
    match refresh {
        AssetsRefresh::Full => (refresh, Some(key), Some(sale)),
        AssetsRefresh::SaleOnly => (refresh, prev_key, Some(sale)),
        AssetsRefresh::None => (refresh, prev_key, prev_sale),
    }
}

/// Combines full-cache and order-only identities for the existing repaint gate.
pub(super) fn mix(assets: u64, sale: u64) -> u64 {
    assets.wrapping_mul(31).wrapping_add(sale)
}

/// Selects cores awaiting their first transfer snapshot, retaining input order and duplicate ids.
pub(super) fn missing_transfer_cores(
    cores: impl Iterator<Item = (CoreId, u64)>,
) -> impl Iterator<Item = CoreId> {
    cores.filter_map(|(id, rev)| (rev == 0).then_some(id))
}

/// Folds scoped order revisions in canonical core order for sale-marker invalidation.
pub(super) fn sale_revisions_sig(cores: impl Iterator<Item = (CoreId, u64)>) -> u64 {
    cores.fold(0u64, |sig, (id, rev)| {
        sig.wrapping_mul(31)
            .wrapping_add(id)
            .wrapping_mul(31)
            .wrapping_add(rev)
    })
}

/// Returns the order-only identity of the cores represented by the cached sale markers.
pub(super) fn sale_sig(b: &Backend, cores: &[(CoreId, String)]) -> u64 {
    let store = b.session.store();
    sale_revisions_sig(
        cores
            .iter()
            .map(|(id, _)| (*id, store.core(*id).map_or(0, |core| core.orders_table_rev))),
    )
}

impl AssetsView {
    /// Full-cache signature for assets, transfers, balance trust, and account-total settings.
    pub(super) fn assets_sig(&self, b: &Backend) -> u64 {
        use std::hash::{Hash, Hasher};
        let store = b.session.store();
        // The footer total folds cores by account and per-core setting; neither bumps a data
        // revision (a Settings change, or `AuthCheck` arriving after the balance), so both enter
        // the signature directly. Configured modes use one allocation-free fold, including
        // out-of-scope settings whose changes can conservatively trigger an extra rebuild.
        // One hasher for every merge key: the key holds strings, so it is hashed, but only once.
        let mut keys = std::collections::hash_map::DefaultHasher::new();
        let sig = self
            .query_cores(b)
            .iter()
            // Include CoreId so canonical reordering invalidates the cache when state is unchanged.
            .map(|(id, _)| (*id, store.core(*id)))
            .fold(0u64, |a, (id, core)| {
                b.session.account_merge_key(id).hash(&mut keys);
                let a = a.wrapping_mul(31).wrapping_add(id);
                let Some(c) = core else {
                    return a;
                };
                a.wrapping_mul(31)
                    .wrapping_add(c.assets_rev)
                    .wrapping_mul(31)
                    .wrapping_add(c.transfer_rev)
                    // Hash the rendered trust state rather than selected ingredients. Status
                    // transitions bump no data revision, but they can change `balance_state()`
                    // and must therefore invalidate the rendered balance immediately.
                    .wrapping_mul(31)
                    .wrapping_add(c.balance_state().code())
            });
        sig.wrapping_mul(31)
            .wrapping_add(keys.finish())
            .wrapping_mul(31)
            .wrapping_add(total_modes_sig(&b.config.servers))
    }

    /// Full-row cache identity from the asset/balance signature and dust threshold. Kept in one
    /// place because the backend observer and `rebuild_cache` must use the same identity.
    pub(super) fn cache_key(&self, sig: u64) -> (u64, u64) {
        (sig, self.min_value_usd.to_bits())
    }

    /// Rebuild all render caches from one backend snapshot.
    ///
    /// Args:
    ///     b: Backend snapshot providing full validity and effective query scopes.
    ///
    /// Returns:
    ///     Nothing; retained state is reconciled and every render cache is replaced in place.
    pub(super) fn rebuild_cache(&mut self, b: &Backend) {
        let sig = self.assets_sig(b);
        // Cache membership data; the dropdown ranks again at render time.
        let cores: Vec<(CoreId, String)> = self.scope_cores(b).into_iter().collect();
        let valid: Vec<CoreId> = cores.iter().map(|(core, _)| *core).collect();
        let query_cores = self.query_cores(b);
        let effective: Vec<CoreId> = query_cores.iter().map(|(core, _)| *core).collect();
        if super::reconcile_retained_assets_state(
            &valid,
            &effective,
            &mut self.sel_cores,
            &mut self.selected_core,
        ) {
            self.cached_wallet_key = None;
        }
        let effective_wallet_core = self.effective_wallet_core(b);
        self.invalidate_pending_transfer_for_wallet_core(effective_wallet_core);
        self.cached_cores = query_cores;
        // Retained filters were reconciled against `cores`, the full configured/live universe;
        // `cached_cores` is intentionally only the effective query scope.
        self.request_missing_transfers(b);
        self.sell_marked = Rc::new(self.collect_sell_marked(b));
        self.sale_sig = Some(sale_sig(b, &self.cached_cores));
        // Apply the header sort here, over the collector's default order: rows are rebuilt on data
        // changes (about 1 Hz while the panel is open), so it costs one pass per rebuild and
        // nothing per repaint. With no active sort this is a no-op and `collect`'s
        // descending-value order stands.
        let mut entries = self.collect(b);
        self.sort_entries(&mut entries);
        self.cached_entries = Rc::new(entries);
        self.cached_aggs = Rc::new(self.per_core(b));
        // The roster's measured width is a property of these aggregates, so it dies with them.
        // Clearing rather than recomputing: measuring needs an `App` this method does not have,
        // and the first `render` that actually shows the wallets section refills it.
        self.cached_roster_auto_w = None;
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
        self.cached_scope_marker = self.scope_marker(b);
        self.cache_sig = Some(self.cache_key(sig));
    }

    /// Requests transfer assets again for scoped cores that have not delivered a snapshot
    /// (`transfer_rev == 0`). The initial request in `new` may precede connection, so cache rebuilds
    /// retry until the first response. A positive revision stops retries even for an empty wallet.
    /// The upper table needs this because some exchanges expose purchased coins only via transfer
    /// assets.
    pub(super) fn request_missing_transfers(&self, b: &Backend) {
        let store = b.session.store();
        let cores = &self.cached_cores;
        let revisions = cores
            .iter()
            .map(|(id, _)| (*id, store.core(*id).map_or(0, |cd| cd.transfer_rev)));
        for id in missing_transfer_cores(revisions) {
            let _ = b.session.refresh_transfer_assets(id);
        }
    }

    /// Build the wallet-detail cache identity from effective scope and transfer data.
    ///
    /// Args:
    ///     b: Backend snapshot providing workspace ownership and transfer revisions.
    ///
    /// Returns:
    ///     Effective wallet core, its transfer revision, and the dust-threshold bit pattern.
    pub(super) fn wallet_cache_key(&self, b: &Backend) -> (Option<CoreId>, u64, u64) {
        let selected_core = self.effective_wallet_core(b);
        let transfer_rev = selected_core
            .and_then(|core| b.session.store().core(core).map(|cd| cd.transfer_rev))
            .unwrap_or(0);
        (selected_core, transfer_rev, self.min_value_usd.to_bits())
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
