//! Exact fleet dependencies shared by the rail and connection status models.

use moon_core::config::{AppConfig, CoreSortMode, Secret, ServerConfig, TransportVersion};
use moon_core::feed::ConnStatus;
use moon_core::session::{CoreId, CoreStore};

/// The inputs to canonical ordering and the fleet-wide transport advice.
///
/// Neither configuration nor connection state has a revision covering all of these writes.
/// Comparing borrowed values keeps cache hits allocation-free, including after a config reload.
pub(super) struct FleetModelKey {
    sort: CoreSortMode,
    servers: Vec<FleetCore>,
}

/// One configured entry, including unshown siblings and duplicate ids in source order.
struct FleetCore {
    id: CoreId,
    uid: u64,
    name: String,
    transport: Option<TransportVersion>,
    key: Secret,
    ready: bool,
}

impl FleetModelKey {
    /// Retain the order and transport inputs on a model miss.
    pub(super) fn capture(config: &AppConfig, store: &CoreStore) -> Self {
        Self {
            sort: config.core_sort,
            servers: config
                .servers
                .iter()
                .map(|server| FleetCore {
                    id: server.id,
                    uid: server.uid,
                    name: server.name.clone(),
                    transport: server.transport,
                    key: server.key.clone(),
                    ready: is_ready(store, server),
                })
                .collect(),
        }
    }

    /// Compare live inputs without sorting names or decoding any transport key.
    pub(super) fn matches(&self, config: &AppConfig, store: &CoreStore) -> bool {
        self.sort == config.core_sort
            && self.servers.len() == config.servers.len()
            && self
                .servers
                .iter()
                .zip(&config.servers)
                .all(|(old, server)| {
                    old.id == server.id
                        && old.uid == server.uid
                        && old.name == server.name
                        && old.transport == server.transport
                        && old.key.expose() == server.key.expose()
                        && old.ready == is_ready(store, server)
                })
    }
}

/// Transport advice includes evidence from every ready configured entry.
fn is_ready(store: &CoreStore, server: &ServerConfig) -> bool {
    store
        .core(server.id)
        .is_some_and(|core| core.status == ConnStatus::Ready)
}
