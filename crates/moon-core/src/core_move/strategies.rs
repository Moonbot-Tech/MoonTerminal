//! Carrying a core's strategy set over to its moved copy, through MoonProto.
//!
//! A settings snapshot normally carries the strategy file. When it does not, the new core starts
//! with a default set of its own — and every terminal that follows the core on its uid then reads
//! the real strategies as deleted and the default ones as new. So a move reads the old core's set
//! exactly as that core confirms it (rows in order, ids, folders, checked state) and writes it to
//! the new core. A core accepts a client's strategy under any unique id, the path a strategy
//! restore already takes, so the ids — and with them every terminal's strategy history and every
//! report row's `strategyid` — carry over. Strategies the new core holds beyond that set are
//! deleted.
//!
//! Checked strategies arrive checked: a moved core trades as the old one did.

use std::collections::HashSet;
use std::time::{Duration, Instant};

/// The client configuration every call in `core_move` takes, named here for callers that do not
/// depend on MoonProto themselves.
pub use moonproto::ClientConfig;
use moonproto::state::StratsState;
use moonproto::{
    ConnectConfig, InitConfig, InitialStrategies, MoonClient, StrategySnapshot, TransportMode,
};

/// How often a wait below looks at the client's state again.
const POLL: Duration = Duration::from_millis(200);
/// How long a write waits for the target's own list before going on without it.
const SETTLE: Duration = Duration::from_secs(10);

/// A core's complete strategy set, as the core confirmed it.
#[derive(Clone)]
pub struct StrategySet {
    /// Every strategy, in the core's order.
    pub strategies: Vec<StrategySnapshot>,
    /// The core's folder tree, empty folders included.
    pub folders: Vec<String>,
}

/// What [`write_set`] changed on the target core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Carried {
    /// Strategies the target holds from the set after the write.
    pub written: usize,
    /// Strategies the target held beyond the set, deleted.
    pub deleted: usize,
}

/// A MoonProto client configuration for a core, from its connection packet and the endpoint to
/// dial. `mode` overrides the transport mode the packet names.
pub fn client_config(
    packet: &str,
    host: &str,
    port: u16,
    mode: Option<TransportMode>,
) -> anyhow::Result<ClientConfig> {
    let info = moonproto::parse_key_info(packet)
        .ok_or_else(|| anyhow::anyhow!("not a MoonBot connection packet"))?;
    let mode = mode
        .or_else(|| info.network.map(|n| n.transport_mode))
        .unwrap_or(TransportMode::V0);
    Ok(ClientConfig::new(
        host.to_owned(),
        port,
        info.keys.master_key,
        info.keys.mac_key,
    )
    .with_transport_mode(mode))
}

/// Read the complete strategy set of the core at `cfg`.
pub fn read_set(cfg: ClientConfig, timeout: Duration) -> anyhow::Result<StrategySet> {
    let client = connect(cfg, timeout)?;
    let set = wait_for(&client, timeout, "the core's strategy list", |strats| {
        (strats.last_modified() != 0).then(|| StrategySet {
            strategies: strats.snapshots().cloned().collect(),
            folders: strats.folder_paths().map(str::to_owned).collect(),
        })
    });
    let _ = client.disconnect();
    set
}

/// Make the core at `cfg` hold exactly `set`: its strategies with their ids, order, folders and
/// checked state, and none besides.
pub fn write_set(
    cfg: ClientConfig,
    set: &StrategySet,
    timeout: Duration,
) -> anyhow::Result<Carried> {
    let client = connect(cfg, timeout)?;
    let result = write_on(&client, set, timeout);
    let _ = client.disconnect();
    result
}

/// [`write_set`] on an open client.
fn write_on(client: &MoonClient, set: &StrategySet, timeout: Duration) -> anyhow::Result<Carried> {
    // A core that has never versioned its list — a fresh one holding only the default set it made
    // itself — sends no list on connect at all (measured on a 7.72 core). Give a versioned list a
    // moment to arrive, then go on either way.
    let _ = wait_for(client, SETTLE, "the new core's strategy list", |strats| {
        (strats.last_modified() != 0 || strats.last_server_epoch != 0).then_some(())
    });
    let wanted: HashSet<u64> = set.strategies.iter().map(|s| s.strategy_id).collect();
    // The folder tree can be sent only to a core that versions one; without it every row still
    // carries its folder in `path`, and only folders holding no strategy are left out.
    let has_folder_tree = client
        .snapshot()
        .is_some_and(|state| state.strats().folders_last_modified() != 0);
    if has_folder_tree {
        client
            .strategies()
            .sync_local_strategies_with_folders(set.strategies.clone(), set.folders.clone())?;
    } else {
        client
            .strategies()
            .sync_local_strategies(set.strategies.clone())?;
    }
    // Checked state travels as its own delta, as the terminal sends it — a row's `checked` field
    // inside a snapshot is not the path the core takes it from.
    for strategy in &set.strategies {
        client
            .strategies()
            .set_checked(strategy.strategy_id, strategy.checked)?;
    }
    client.strategies().send_checked_delta()?;
    // Done when the core CONFIRMS every row as wanted, not when the ids merely exist: a core
    // started from the snapshot's own strategy file already holds them, perhaps differently.
    wait_for(
        client,
        timeout,
        "the new core to confirm the strategies as sent",
        |strats| {
            set.strategies
                .iter()
                .all(|want| {
                    strats.snapshot(want.strategy_id).is_some_and(|have| {
                        have.checked == want.checked && *have.path == *want.path
                    })
                })
                .then_some(())
        },
    )?;
    let extra: Vec<u64> = client
        .snapshot()
        .map(|state| {
            state
                .strats()
                .snapshots()
                .map(|s| s.strategy_id)
                .filter(|id| !wanted.contains(id))
                .collect()
        })
        .unwrap_or_default();
    // Every extra is asked for before any failure is reported: stopping at the first would leave
    // the rest on the core.
    let refused: Vec<String> = extra
        .iter()
        .filter_map(|id| {
            client
                .strategies()
                .delete(*id, "")
                .err()
                .map(|e| format!("{id}: {e}"))
        })
        .collect();
    anyhow::ensure!(
        refused.is_empty(),
        "the new core's own strategies were not all deleted ({})",
        refused.join("; ")
    );
    wait_for(
        client,
        timeout,
        "the new core to delete its own strategies",
        |strats| {
            extra
                .iter()
                .all(|id| strats.snapshot(*id).is_none())
                .then_some(())
        },
    )?;
    Ok(Carried {
        written: wanted.len(),
        deleted: extra.len(),
    })
}

/// Connect and wait until the client is ready, announcing no local strategies — what every
/// terminal announces, and a Full snapshot never deletes what it omits.
pub(super) fn connect(cfg: ClientConfig, timeout: Duration) -> anyhow::Result<MoonClient> {
    let init = InitConfig {
        initial_strategies: Some(InitialStrategies::new(0, Vec::new())),
        ..Default::default()
    };
    MoonClient::connect_blocking(cfg, ConnectConfig::new(init), timeout)
        .map_err(|e| anyhow::anyhow!("MoonProto connect: {e}"))
}

/// Poll the client's strategy state until `ready` yields a value, or fail after `timeout` naming
/// `what` was awaited.
fn wait_for<T>(
    client: &MoonClient,
    timeout: Duration,
    what: &str,
    mut ready: impl FnMut(&StratsState) -> Option<T>,
) -> anyhow::Result<T> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(state) = client.snapshot() {
            if let Some(value) = ready(state.strats()) {
                return Ok(value);
            }
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "timed out after {}s waiting for {what}",
            timeout.as_secs()
        );
        std::thread::sleep(POLL);
    }
}
