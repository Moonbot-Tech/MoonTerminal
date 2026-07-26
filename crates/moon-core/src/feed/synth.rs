//! Deterministic SYNTHETIC feed for render benchmarking (MOON_SYNTH environment variable).
//! It does NOT access the network: it sends Ready + Identity + AddToChart detects (which create
//! stress-window containers), plus fixed-frequency tick and order-book streams. The goal is an
//! identical reproducible load in native and Tauri builds for a fair CPU/GPU comparison.
//!
//! It also emits synthetic NEWS for its markets, which is the only way to drive the chart's news
//! marks (the gems on the plot's bottom edge and their Ctrl-hover card) without a live news
//! subscription: real news for a chosen coin inside the chart's visible window cannot be summoned
//! on demand. See `MOON_SYNTH_NEWS*` below.

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use super::news::{NewsItem, NewsSnapshot, NEWS_RING_CAP};
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

/// Deterministic LCG matching the Tauri synthesizer to produce the same data stream.
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

/// Runs with the `feed::spawn` signature, like live::run but without network access or reports.
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
    // Synthetic exchange (code 200): the coordinator elects this core as its sole provider.
    let _ = tx.send(FeedMsg::Identity(ExchangeId::new(200)));
    // Synthetic base currency is USDT, so group-local USD sizes convert at a one-to-one rate.
    let _ = tx.send(FeedMsg::CoreBase {
        base: "USDT".to_string(),
    });

    // AddToChart: window w (1..=WINDOWS) receives CHARTS markets in the Chart{w} container.
    // The TTL is approximately one year.
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
                sound_name: None,
                is_alert: false,
                kind: 0,
                is_short: false,
            });
            seq += 1;
        }
    }
    let _ = tx.send(FeedMsg::Detects(dets));

    // Synthetic news for the chart's news marks. `MOON_SYNTH_NEWS` is how many items exist at
    // startup (0 disables the whole news stream), spaced `MOON_SYNTH_NEWS_SPACING_SEC` apart going
    // back from now so several marks land inside the chart's default window; after that one new
    // item arrives every `MOON_SYNTH_NEWS_EVERY_SEC` so the live path (a mark appearing on a
    // running chart) is driveable too.
    let news_count = env_usize("MOON_SYNTH_NEWS", 4);
    let news_spacing_sec = env_f64("MOON_SYNTH_NEWS_SPACING_SEC", 90.0).max(1.0);
    let news_every = Duration::from_secs_f64(env_f64("MOON_SYNTH_NEWS_EVERY_SEC", 30.0).max(1.0));
    let mut news: Vec<NewsItem> = Vec::new();
    let mut news_seq = 0u64;
    if news_count > 0 {
        for k in (0..news_count).rev() {
            news.push(synth_news_item(
                &mut news_seq,
                &markets,
                now_ms() as i64 - (k as f64 * news_spacing_sec * 1000.0) as i64,
            ));
        }
        let _ = tx.send(news_msg(&news));
    }
    let mut last_news = Instant::now();

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
        if news_count > 0 && last_news.elapsed() >= news_every {
            last_news = Instant::now();
            news.push(synth_news_item(&mut news_seq, &markets, now_ms() as i64));
            // The wire carries a ring, not a delta: keep the same cap the core's ring uses.
            if news.len() > NEWS_RING_CAP {
                news.drain(..news.len() - NEWS_RING_CAP);
            }
            if tx.send(news_msg(&news)).is_err() {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// The whole synthetic news ring as one wire message, with a small tag catalog to exercise the
/// tag filter. Sent on startup and again for every added item, because news travels as a ring.
fn news_msg(news: &[NewsItem]) -> FeedMsg {
    FeedMsg::News(NewsSnapshot {
        items: news.to_vec(),
        catalog: vec!["synth".to_string(), "listing".to_string()],
    })
}

/// One synthetic news item at `time_ms`, tagged with EVERY synthetic market's coin so any open
/// synthetic chart shows its mark. Bodies are long enough to exercise the hover card's wrapping.
fn synth_news_item(seq: &mut u64, markets: &[String], time_ms: i64) -> NewsItem {
    let id = *seq;
    *seq += 1;
    NewsItem {
        id: format!("synth-{id}"),
        time_ms,
        recv_time_ms: Some(time_ms + 120),
        send_time_ms: Some(time_ms + 180),
        recv_terminal_ms: None,
        source: "synth".to_string(),
        author: None,
        coins: markets.to_vec(),
        en: format!(
            "Synthetic news #{id}: an exchange announced a listing, a halt and a fee change in one \
             paragraph, which is long on purpose so the chart's hover card has to wrap it."
        ),
        ru: format!(
            "Синтетическая новость №{id}: биржа объявила листинг, остановку торгов и изменение \
             комиссий одним абзацем — намеренно длинным, чтобы карточка на графике переносила текст."
        ),
        es: String::new(),
        tags: vec!["synth".to_string()],
        is_original: true,
    }
}
