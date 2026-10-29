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
const ORDERS_PER_CORE: usize = 5200;
const STRATEGIES_PER_CORE: u64 = 50;
const POLLS: usize = 20;

/// Build 200 live orders and 5000 retained closed orders per core, all with synthetic traces.
fn scale_store(matching: usize) -> CoreStore {
    let mut store = CoreStore::default();
    for core in 1..=CORES {
        store.ensure(core);
        let data = store.core_mut(core).expect("synthetic core");
        let orders: Vec<_> = (0..ORDERS_PER_CORE)
            .map(|index| {
                let mut row = resting_order(100.0 + index as f64, 101.0 + index as f32);
                row.uid = core * 10_000 + index as u64;
                row.strat_id = index as u64 % STRATEGIES_PER_CORE + 1;
                row.job_is_done = index >= 200;
                row.pending = false;
                row.filled = true;
                row.entry_fill_time_ms = 1_700_000_000_000.0 + index as f64 * 1000.0;
                row.create_time_ms = row.entry_fill_time_ms - 1000.0;
                row.sell_create_time_ms = row.entry_fill_time_ms + 1000.0;
                row.sell_trace = Some(moon_core::feed::OrderTrace {
                    points: (0..200)
                        .map(|point| moon_core::feed::OrderTracePoint {
                            time_ms: point as f64 * 1000.0,
                            price: 100.0,
                        })
                        .collect(),
                    ..Default::default()
                });
                if index < matching {
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
        let live: Vec<_> = orders
            .iter()
            .filter(|row| !row.job_is_done)
            .cloned()
            .collect();
        data.apply(moon_core::feed::FeedMsg::Orders(orders.into()));
        data.apply(moon_core::feed::FeedMsg::Orders(live.into()));
        assert_eq!(data.orders.len(), 200);
        assert!(
            (200..ORDERS_PER_CORE)
                .all(|index| data.order_lines.knows(core * 10_000 + index as u64))
        );
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

/// Time one owner-thread projection and print the median milliseconds per poll.
fn measure(label: &str, mut poll: impl FnMut()) {
    poll();
    let mut samples = Vec::new();
    for _ in 0..POLLS {
        let started = Instant::now();
        poll();
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    let millis = samples[POLLS / 2];
    println!("[scale] {label}: {millis:.3} ms/poll");
}

/// Chart admission must not walk every retained order on every 500 ms poll.
#[test]
#[ignore = "manual ten-core Mini App chart scale measurement"]
fn mini_app_chart_snapshot_scale() {
    let store = scale_store(200);
    let listed = listed();
    let venues: HashMap<_, _> = (1..=CORES)
        .map(|core| (core, CoreVenue::identify(1, "", None)))
        .collect();
    measure("chart frozen full-row snapshot", || {
        let old: Vec<_> = listed
            .iter()
            .flat_map(|(id, name, _)| {
                store
                    .core(*id)
                    .into_iter()
                    .flat_map(|data| data.orders.iter())
                    .filter(|row| !row.job_is_done && row.market == "XALPHAUSDT")
                    .map(|row| (*id, name.clone(), row.clone()))
            })
            .collect();
        assert_eq!(old.len(), 2000);
        black_box(old);
    });
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
    let store = scale_store(5);
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
    job: Option<crate::Job>,
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
            job: None,
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

    /// Retain work so regressions can deliberately finish an older service's response last.
    fn spawn(&mut self, job: crate::Job) {
        assert!(self.job.is_none());
        self.job = Some(job);
    }

    /// No UI exists in this fixture.
    fn repaint(&mut self) {}
}

/// Frozen pre-change mapping, including the original full-row clone for a mark calculation.
fn frozen_order_dto(
    host: &ScaleHost,
    row: &moon_core::feed::OrderRow,
) -> moon_core::telegram::web::dto::OrderDto {
    use super::super::dto::*;
    use moon_core::util::fmt;
    let mut marked = row.clone();
    marked.price = row.price;
    let pnl = moon_core::feed::order_math::order_pnl(&marked).filter(|value| value.is_finite());
    let change =
        moon_core::feed::order_math::order_pnl_pct(&marked).filter(|value| value.is_finite());
    let quote = order_quote(&row.quote, &row.market);
    let distance = order_to_entry_pct(row);
    let price = |value: f64| (value.is_finite() && value > 0.0).then(|| fmt::adaptive(value));
    moon_core::telegram::web::dto::OrderDto {
        core: 1,
        core_name: "CORE_A".into(),
        exchange: "EX-XX".into(),
        uid: row.uid,
        coin: row.coin.clone(),
        market: row.market.clone(),
        side: if row.is_short { "sell" } else { "buy" }.into(),
        volume_text: entry_volume_text(row.size, row.buy_price, 1.0, &row.quote),
        entry_text: price(row.buy_price),
        mark_text: price(f64::from(row.price)),
        pnl,
        pnl_text: pnl.and_then(|value| pnl_text(value, &quote)),
        pnl_sign: pnl.map_or(0, |value| pnl_sign(value, &quote)),
        pnl_usd: pnl.and_then(|value| {
            pnl_usd(value, &quote, |currency| {
                host.session.market_source().currency_usd_rate(1, currency)
            })
        }),
        emulator: row.emulator,
        change_pct: change,
        change_text: change.and_then(|value| fmt::signed_pct(value, 2).map(|(text, _)| text)),
        to_entry_pct: distance,
        to_entry_text: distance.and_then(distance_text),
        panic_armed: false,
    }
}

/// Frozen rendering must survive the worker split; stale and revoked jobs cannot populate a new
/// service's response cache or clear its pending admission.
#[test]
fn orders_worker_preserves_rows_and_rejects_old_service_completions() {
    use moon_core::telegram::web::MiniAppApiError;
    // paths uses a permanent OnceLock override; isolate it from the parallel test process.
    const CHILD: &str = "MOON_TG_ORDERS_FIXTURE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "mini_app::tests::scale::orders_worker_preserves_rows_and_rejects_old_service_completions",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .expect("isolated Orders fixture process");
        assert!(
            output.status.success(),
            "child failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"),
            "the exact child selector must execute the fixture"
        );
        return;
    }
    let mut host = ScaleHost::new();
    for quote in ["USDT", "BTC", ""] {
        for (entry, mark) in [
            (100.0, 101.0),
            (1000.4, 1001.5),
            (0.0, 0.0),
            (f64::NAN, f32::NAN),
        ] {
            for (short, filled) in [(false, false), (false, true), (true, true)] {
                let mut row = resting_order(entry, mark);
                row.quote = quote.into();
                row.is_short = short;
                row.filled = filled;
                row.pending = !filled;
                let expected = frozen_order_dto(&host, &row);
                let actual =
                    super::super::dto::order_draft(&host, 1, "CORE_A".into(), "EX-XX".into(), &row)
                        .render();
                assert_eq!(actual, expected);
            }
        }
    }
    let (reply, old) = std::sync::mpsc::sync_channel(1);
    super::super::reads::mini_orders(&mut host, 7, reply);
    let finish_old = host.job.take().unwrap()();
    host.state.mini_orders_generation += 1;
    host.state.mini_orders_pending = false;
    let (reply, new) = std::sync::mpsc::sync_channel(1);
    super::super::reads::mini_orders(&mut host, 7, reply);
    let finish_new = host.job.take().unwrap()();
    finish_old(&mut host);
    assert!(matches!(
        old.try_recv().unwrap(),
        Err(MiniAppApiError::Rejected)
    ));
    assert!(host.state.mini_orders_pending);
    assert!(host.state.mini_orders_last.is_none());
    finish_new(&mut host);
    assert!(new.try_recv().unwrap().is_ok());
    assert!(!host.state.mini_orders_pending);
    host.state.mini_orders_last = None;
    let (reply, revoked) = std::sync::mpsc::sync_channel(1);
    super::super::reads::mini_orders(&mut host, 7, reply);
    let finish = host.job.take().unwrap()();
    host.config.telegram.authorized_chat_ids.clear();
    finish(&mut host);
    assert!(matches!(
        revoked.try_recv().unwrap(),
        Err(MiniAppApiError::Rejected)
    ));
    assert!(host.state.mini_orders_last.is_none());
}

/// Unchanged Orders polls must reuse the response instead of recloning and formatting the fleet.
#[test]
#[ignore = "manual ten-core Mini App orders scale measurement"]
fn mini_app_orders_projection_scale() {
    let mut host = ScaleHost::new();
    let store = scale_store(5);
    let listed = listed();
    let access = moon_core::config::telegram_access::TelegramReportAccess::Owner;
    measure("orders frozen synchronous rebuild", || {
        let old: Vec<_> = listed
            .iter()
            .flat_map(|(id, _, _)| {
                store
                    .core(*id)
                    .into_iter()
                    .flat_map(|data| data.orders.iter())
            })
            .map(|row| frozen_order_dto(&host, row))
            .collect();
        assert_eq!(old.len(), 2000);
        black_box(old);
    });
    measure("orders cached poll", || {
        let source = host.session().market_source();
        let inputs = super::super::reads::orders_inputs(
            &store,
            &listed,
            |core, quote| source.currency_usd_rate(core, quote),
            |core, market| host.is_panic_armed(core, market),
        );
        if let Some(dto) = host.state().mini_orders_last.as_ref().and_then(|cached| {
            (cached.chat == 7 && cached.access == access && cached.inputs == inputs)
                .then(|| cached.dto.clone())
        }) {
            black_box(dto);
            return;
        }
        let (drafts, snapshots) = super::super::reads::capture_orders(&host, &store, &listed);
        let dto = moon_core::telegram::web::dto::OrdersDto {
            orders: drafts
                .into_iter()
                .map(super::super::dto::OrderDraft::render)
                .collect(),
            can_control: true,
        };
        super::super::charts::remember_order_snapshots(
            host.state_mut(),
            &(1..=CORES).collect::<Vec<_>>(),
            snapshots,
        );
        host.state_mut().mini_orders_last = Some(super::super::cache::CachedOrders {
            chat: 7,
            access: access.clone(),
            inputs,
            dto: dto.clone(),
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
    measure("orders owner capture", || {
        black_box(super::super::reads::capture_orders(&host, &store, &listed));
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
}
