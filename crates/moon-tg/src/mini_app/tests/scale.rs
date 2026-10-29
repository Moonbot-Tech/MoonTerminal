//! Ignored ten-core measurements for the Mini App owner-thread poll projections.

use std::collections::HashMap;
use std::hint::black_box;
use std::path::PathBuf;
use std::time::Instant;

use moon_core::config::AppConfig;
use moon_core::feed::StrategyRow;
use moon_core::session::{CoreStore, SessionManager};
use moon_core::venue::CoreVenue;

use super::resting_order;
use crate::{HostKind, TelegramState, TgHost};

const CORES: u64 = 10;
const ORDERS_PER_CORE: usize = 500;
const STRATEGIES_PER_CORE: u64 = 50;
const POLLS: usize = 20;

/// Build the specified fleet with fifty matching chart rows and thousands of nonmatching rows.
fn scale_store() -> CoreStore {
    let mut store = CoreStore::default();
    for core in 1..=CORES {
        store.ensure(core);
        let data = store.core_mut(core).expect("synthetic core");
        let orders: Vec<_> = (0..ORDERS_PER_CORE)
            .map(|index| {
                let mut row = resting_order(100.0 + index as f64, 101.0 + index as f32);
                row.uid = core * 10_000 + index as u64;
                row.strat_id = index as u64 % STRATEGIES_PER_CORE + 1;
                if index < 5 {
                    row.coin = "XALPHA".into();
                    row.market = "XALPHAUSDT".into();
                } else {
                    row.coin = format!("X{core}_{index}");
                    row.market = format!("X{core}_{index}USDT");
                }
                row.market_display = row.market.clone();
                row
            })
            .collect();
        data.apply(moon_core::feed::FeedMsg::Orders(orders.into()));
        data.strategies = (1..=STRATEGIES_PER_CORE)
            .map(|id| StrategyRow {
                id,
                name: format!("UNIT_{id:02}"),
                kind: String::new(),
                kind_ordinal: 0,
                folder_path: format!("UNIT_{:02}", id % 7),
                checked: true,
                is_short: false,
                fields: Vec::new(),
            })
            .collect();
    }
    store
}

/// List the fleet in a fixed order on one venue.
fn listed() -> Vec<(u64, String, String)> {
    (1..=CORES)
        .map(|core| (core, format!("CORE_{core:02}"), "EX-XX".into()))
        .collect()
}

/// Time one owner-thread projection and print its mean milliseconds per poll.
fn measure(label: &str, mut poll: impl FnMut()) {
    poll();
    let started = Instant::now();
    for _ in 0..POLLS {
        poll();
    }
    let millis = started.elapsed().as_secs_f64() * 1000.0 / POLLS as f64;
    println!("[scale] {label}: {millis:.3} ms/poll");
}

/// Chart admission must not walk every retained order on every 500 ms poll.
#[test]
#[ignore = "manual ten-core Mini App chart scale measurement"]
fn mini_app_chart_snapshot_scale() {
    let store = scale_store();
    let listed = listed();
    let venues: HashMap<_, _> = (1..=CORES)
        .map(|core| (core, CoreVenue::identify(1, "", None)))
        .collect();
    measure("chart snapshot", || {
        black_box(super::super::charts::coin_snapshot(
            &store,
            &listed,
            &venues,
            1,
            "XALPHAUSDT",
        ));
    });
}

/// Unchanged Strategies polls must reuse indexed coins and mapped folders through the response cache.
#[test]
#[ignore = "manual ten-core Mini App strategies scale measurement"]
fn mini_app_strategies_projection_scale() {
    let store = scale_store();
    let listed = listed();
    let wanted = HashMap::new();
    let access = moon_core::config::telegram_access::TelegramReportAccess::Owner;
    let config = moon_core::config::TelegramConfig::default();
    let mut state = TelegramState::new(&config, HostKind::Station);
    measure("strategies cached poll", || {
        let now = Instant::now();
        let inputs = super::super::reads::strategies_inputs(&store, &listed, &wanted, now);
        if let Some(dto) = state.mini_strategies_last.as_ref().and_then(|cached| {
            (cached.chat == 7 && cached.access == access && cached.inputs == inputs)
                .then(|| cached.dto.clone())
        }) {
            black_box(dto);
            return;
        }
        let dto = super::super::reads::build_strategies(&store, &listed, &wanted, true, now);
        state.mini_strategies_last = Some(super::super::cache::CachedStrategies {
            chat: 7,
            access: access.clone(),
            inputs,
            dto: dto.clone(),
        });
        black_box(dto);
    });
}

/// Empty synthetic host used to time the real order-row formatter without any live data source.
struct ScaleHost {
    config: AppConfig,
    session: SessionManager,
    state: TelegramState,
    root: PathBuf,
}

impl ScaleHost {
    /// Start only an empty session against a fresh temporary data root.
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "moon-tg-mini-scale-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        assert!(moon_core::config::paths::set_data_dir_override(
            root.clone()
        ));
        let mut config = AppConfig::headless(Vec::new());
        config.telegram.mini_app_enabled = true;
        config.telegram.owner_chat_id = Some(7);
        config.telegram.authorized_chat_ids = vec![7];
        let session = SessionManager::start(&config, 0.0, None, None);
        let state = TelegramState::new(&config.telegram, HostKind::Station);
        Self {
            config,
            session,
            state,
            root,
        }
    }
}

impl Drop for ScaleHost {
    /// Remove the empty fixture databases and configuration directory.
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl TgHost for ScaleHost {
    /// Identify the synthetic host as a station.
    fn kind(&self) -> HostKind {
        HostKind::Station
    }

    /// Return a path inside the temporary fixture root.
    fn notifications_path(&self) -> PathBuf {
        self.root.join("notifications.json")
    }

    /// Return the synthetic configuration.
    fn config(&self) -> &AppConfig {
        &self.config
    }

    /// Return the empty session's market source.
    fn session(&self) -> &SessionManager {
        &self.session
    }

    /// Return the empty session for prohibited command mutations.
    fn session_mut(&mut self) -> &mut SessionManager {
        &mut self.session
    }

    /// Return process-only snapshot state.
    fn state(&self) -> &TelegramState {
        &self.state
    }

    /// Return process-only snapshot state for order observation.
    fn state_mut(&mut self) -> &mut TelegramState {
        &mut self.state
    }

    /// Keep fixture timestamps in UTC.
    fn report_zone(&self) -> chrono_tz::Tz {
        chrono_tz::UTC
    }

    /// Pairing persistence is outside this probe.
    fn save_paired_chat(&mut self, _: i64) -> bool {
        unreachable!()
    }

    /// Pairing persistence is outside this probe.
    fn save_cleared_pairing(&mut self) -> bool {
        unreachable!()
    }

    /// Bot settings persistence is outside this probe.
    fn save_bot_settings(&mut self, _: moon_core::config::telegram_menu::BotSettings) -> bool {
        unreachable!()
    }

    /// No synthetic market is panic-armed.
    fn is_panic_armed(&self, _: u64, _: &str) -> bool {
        false
    }

    /// Money commands are outside this probe.
    fn toggle_panic_sell(&mut self, _: u64, _: String) -> bool {
        unreachable!()
    }

    /// Reconnects are outside this probe.
    fn request_reconnect(&mut self, _: u64) {
        unreachable!()
    }

    /// Background reads are outside this probe.
    fn spawn(&mut self, _: crate::Job) {
        unreachable!()
    }

    /// No UI exists in this fixture.
    fn repaint(&mut self) {}
}

/// Unchanged Orders polls must reuse the response instead of recloning and formatting the fleet.
#[test]
#[ignore = "manual ten-core Mini App orders scale measurement"]
fn mini_app_orders_projection_scale() {
    let mut host = ScaleHost::new();
    let store = scale_store();
    let listed = listed();
    let access = moon_core::config::telegram_access::TelegramReportAccess::Owner;
    measure("orders cached poll", || {
        let source = host.session().market_source();
        let inputs = super::super::reads::orders_inputs(
            &store,
            &listed,
            |core, quote| source.currency_usd_rate(core, quote),
            |core, market| host.is_panic_armed(core, market),
        );
        if let Some((dto, snapshots)) = host.state().mini_orders_last.as_ref().and_then(|cached| {
            (cached.chat == 7 && cached.access == access && cached.inputs == inputs)
                .then(|| (cached.dto.clone(), cached.snapshots.clone()))
        }) {
            super::super::charts::remember_order_snapshots(host.state_mut(), snapshots);
            black_box(dto);
            return;
        }
        let (dto, snapshots) = super::super::reads::build_orders(&host, &store, &listed, true);
        let cached_snapshots =
            super::super::reads::current_order_snapshots(host.state(), &snapshots);
        super::super::charts::remember_order_snapshots(host.state_mut(), snapshots);
        host.state_mut().mini_orders_last = Some(super::super::cache::CachedOrders {
            chat: 7,
            access: access.clone(),
            inputs,
            dto: dto.clone(),
            snapshots: cached_snapshots,
        });
        black_box(dto);
    });
    measure("orders cache key", || {
        let source = host.session().market_source();
        black_box(super::super::reads::orders_inputs(
            &store,
            &listed,
            |core, quote| source.currency_usd_rate(core, quote),
            |core, market| host.is_panic_armed(core, market),
        ));
    });
    measure("orders cached dto clone", || {
        black_box(
            host.state()
                .mini_orders_last
                .as_ref()
                .expect("warmed cache")
                .dto
                .clone(),
        );
    });
    measure("orders snapshot refresh", || {
        let snapshots = host
            .state()
            .mini_orders_last
            .as_ref()
            .expect("warmed cache")
            .snapshots
            .clone();
        super::super::charts::remember_order_snapshots(host.state_mut(), snapshots);
    });
}
