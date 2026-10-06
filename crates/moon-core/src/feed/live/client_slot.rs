//! Connection target resolution and per-client lifetime state.

use super::*;

pub(super) struct ClientSlotGuard {
    pub(super) slot: SharedMoonClient,
}

impl Drop for ClientSlotGuard {
    fn drop(&mut self) {
        self.slot.set(None);
    }
}

/// Resolve the connection target and transport for a parsed MoonBot key.
///
/// Both configured values outrank the key's. The transport: MoonBot's own V0/V1/V2 switch moves
/// without issuing a new key, so a core that was flipped after its export is reachable only by
/// following the local choice. The endpoint override: the key's address can be wrong, missing,
/// or the public address a terminal on the core's own LAN cannot reach (#616). The key still
/// answers whatever is not configured -- a legacy export carries no network block at all, and
/// that is what the localhost/3000/V0 fallbacks are for.
///
/// Args:
///     network: Optional network metadata from `moonproto::parse_key_info`.
///     transport: Configured transport mode for this core, or `None` to follow the key.
///     endpoint_override: The row's parsed endpoint override, or `None` to follow the key.
///
/// Returns:
///     The unresolved target plus transport.
pub(crate) fn connection_target(
    network: Option<&moonproto::ImportedNetworkConfig>,
    transport: Option<TransportVersion>,
    endpoint_override: Option<&crate::config::EndpointOverride>,
) -> (crate::config::CoreTarget, TransportMode) {
    let target = crate::config::target_from_network(network, endpoint_override);
    let transport = transport.map(TransportMode::from).unwrap_or_else(|| {
        network
            .map(|network| network.transport_mode)
            .unwrap_or(TransportMode::V0)
    });
    (target, transport)
}

/// Which run-state halves the MoonBot instance currently on the other end has reported.
///
/// Both are false until it speaks for itself, which is also the state a server restart returns to.
#[derive(Default)]
pub(super) struct RunStateSeen {
    /// The market runtime (`TRuntimeStateCommand`).
    pub(super) runtime: bool,
    /// The global strategy engine (`TStratRuntimeState`).
    pub(super) trading: bool,
}
