//! Детерминированный СИНТЕТИЧЕСКИЙ фид для бенчмарка рендера (env MOON_SYNTH).
//! НЕ ходит в сеть: шлёт Ready + Identity + AddToChart-детекты (создают контейнеры
//! для стресс-окон) + поток тиков/стакана с фиксированной частотой. Цель — одинаковая
//! воспроизводимая нагрузка в нативе и Tauri для честного сравнения CPU/GPU.

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use super::{
    ConnStatus, CoreCmd, DetectRow, ExchangeId, FeedMsg, FeedTx, Level, MarketDirty,
    MarketDirtyFlags, OrderBook, Side, Tick,
};
use crate::config::ServerConfig;
use crate::market::SharedMarketStore;
use crate::util::now_unix_ms as now_ms;

fn env_usize(k: &str, d: usize) -> usize {
    std::env::var(k)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(d)
}
fn env_f64(k: &str, d: f64) -> f64 {
    std::env::var(k)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(d)
}

/// Детерминированный LCG — тот же, что в Tauri-синте (одинаковый поток данных).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Сигнатура под `feed::spawn` (как live::run, но без сети/reports).
pub fn run(
    server: &ServerConfig,
    tx: &FeedTx,
    cmd_rx: &Receiver<CoreCmd>,
    market_store: Option<&SharedMarketStore>,
) -> anyhow::Result<()> {
    let windows = env_usize("MOON_STRESS_WINDOWS", 10).max(1);
    let charts = env_usize("MOON_STRESS_CHARTS", 5).max(1);
    let n = env_usize("MOON_SYNTH_MARKETS", charts).max(1);
    let tps = env_f64("MOON_SYNTH_TPS", 50.0).max(0.1);
    let bookhz = env_f64("MOON_SYNTH_BOOKHZ", 20.0).max(0.1);
    let depth = env_usize("MOON_SYNTH_DEPTH", 50).max(1);
    let seed = std::env::var("MOON_SYNTH_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1u64);

    let markets: Vec<String> = (0..n).map(|i| format!("SYNTH{i}")).collect();
    log::info!(
        "synth-фид: {windows} окон × {charts} панелей, {n} рынков, {tps} тик/с, {bookhz} стак/с"
    );

    let _ = tx.send(FeedMsg::Status(ConnStatus::Ready));
    // Синт-биржа (код 200) — координатор изберёт это ядро провайдером (одно на «биржу»).
    let _ = tx.send(FeedMsg::Identity(ExchangeId(200)));
    // Синт-база — USDT (для дефолтов размера ордера в UI).
    let _ = tx.send(FeedMsg::CoreBase {
        base: "USDT".to_string(),
    });

    // AddToChart: окно w (1..=WINDOWS) ← CHARTS рынков в контейнер Chart{w}. TTL ~год.
    let mut dets = Vec::new();
    let mut seq = 0u64;
    for w in 1..=windows {
        for m in 0..charts {
            dets.push(DetectRow {
                seq,
                market: markets[m % n].clone(),
                time_ms: now_ms(),
                sound_alert: false,
                keep_alert_secs: 0,
                add_to_chart: w as u32,
                keep_in_chart_secs: 31_536_000,
            });
            seq += 1;
        }
    }
    let _ = tx.send(FeedMsg::Detects(dets));

    let mut price: Vec<f64> = (0..n).map(|i| 100.0 * (i as f64 + 1.0)).collect();
    let mut rng = Lcg(seed);
    let tick_dt = Duration::from_secs_f64(1.0 / tps);
    let book_dt = Duration::from_secs_f64(1.0 / bookhz);
    let mut last_tick = Instant::now();
    let mut last_book = Instant::now();

    loop {
        loop {
            match cmd_rx.try_recv() {
                Ok(_) => continue,
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }
        if last_tick.elapsed() >= tick_dt {
            last_tick = Instant::now();
            for (i, m) in markets.iter().enumerate() {
                price[i] *= 1.0 + (rng.unit() - 0.5) * 0.0004;
                let side = if rng.unit() < 0.5 {
                    Side::Buy
                } else {
                    Side::Sell
                };
                let tick = Tick {
                    time_ms: now_ms(),
                    price: price[i] as f32,
                    qty: (rng.unit() * 10.0 + 0.25) as f32,
                    side,
                };
                if let Some(store) = market_store {
                    store
                        .write()
                        .expect("synthetic market store poisoned")
                        .apply_ticks(server.id, m, &[tick]);
                }
            }
            let dirty: Vec<MarketDirty> = markets
                .iter()
                .map(|m| MarketDirty::new(m.clone(), MarketDirtyFlags::HISTORY))
                .collect();
            if tx.send(FeedMsg::MarketDataChanged(dirty)).is_err() {
                return Ok(());
            }
        }
        if last_book.elapsed() >= book_dt {
            last_book = Instant::now();
            for (i, m) in markets.iter().enumerate() {
                let mid = price[i];
                let mut bids = Vec::with_capacity(depth);
                let mut asks = Vec::with_capacity(depth);
                for k in 0..depth {
                    let off = (k as f64 + 1.0) * mid * 0.0001;
                    bids.push(Level {
                        price: (mid - off) as f32,
                        qty: (rng.unit() * 10.0 + 1.0) as f32,
                    });
                    asks.push(Level {
                        price: (mid + off) as f32,
                        qty: (rng.unit() * 10.0 + 1.0) as f32,
                    });
                }
                if let Some(store) = market_store {
                    store
                        .write()
                        .expect("synthetic market store poisoned")
                        .apply_book(server.id, m, &OrderBook { bids, asks });
                }
            }
            let dirty: Vec<MarketDirty> = markets
                .iter()
                .map(|m| MarketDirty::new(m.clone(), MarketDirtyFlags::ORDERBOOK))
                .collect();
            if tx.send(FeedMsg::MarketDataChanged(dirty)).is_err() {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}
