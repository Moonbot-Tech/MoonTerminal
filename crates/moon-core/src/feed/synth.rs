//! Deterministic SYNTHETIC feed for render benchmarking, selected by a core's `synthetic` flag
//! (`MOON_CONFIG_PLAINTEXT_SYNTHETIC=1` turns a plaintext config's core into one).
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

/// Whether one synthetic-feed switch is set to anything but `0`.
///
/// Args:
///     k: Variable name.
///
/// Returns:
///     Whether the branch it selects is on.
fn env_flag(k: &str) -> bool {
    std::env::var(k).is_ok_and(|v| v != "0")
}

fn env_usize(k: &str, d: usize) -> usize {
    std::env::var(k)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(d)
}
/// Explicit market names from `MOON_SYNTH_MARKET_NAMES`, comma-separated.
///
/// Returns `None` when the variable is absent or holds nothing but separators, so the caller keeps
/// its generated `SYNTH0..N` names rather than starting a feed with zero markets.
fn named_markets() -> Option<Vec<String>> {
    let raw = std::env::var("MOON_SYNTH_MARKET_NAMES").ok()?;
    let names: Vec<String> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    (!names.is_empty()).then_some(names)
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
    // Spacing between order-book levels, as a fraction of price. The default 0.01% over 50 levels
    // spans half a percent, which on a chart scaled to a whole trading day is a line one pixel
    // wide; a bench that wants a book with visible extent raises it.
    let book_step = env_f64("MOON_SYNTH_BOOK_STEP_PCT", 0.01).max(0.0001) / 100.0;
    let seed = std::env::var("MOON_SYNTH_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1u64);

    // `MOON_SYNTH_MARKET_NAMES` names the markets explicitly instead of generating `SYNTH0..N`.
    // The fixture bench needs it: its candles and trades belong to one real market name, and a
    // chart opened on `SYNTH0` would find neither.
    let markets: Vec<String> = named_markets().unwrap_or_else(|| {
        (0..n).map(|i| format!("SYNTH{i}")).collect()
    });
    let n = markets.len();
    log::info!(
        "synth-фид: {windows} окон × {charts} панелей, {n} рынков, {tps} тик/с, {bookhz} стак/с"
    );

    let _ = tx.send(FeedMsg::Status(ConnStatus::Ready));
    // Synthetic exchange (code 200): the coordinator elects this core as its sole provider.
    let _ = tx.send(FeedMsg::Identity {
        id: ExchangeId::new(200),
        dex: String::new(),
        // No caption: the synthetic feed is not a venue, and naming it one would give it a section
        // of its own in every core list instead of the shared unidentified group.
        reported: String::new(),
    });
    // Synthetic base currency is USDT, so group-local USD sizes convert at a one-to-one rate.
    let _ = tx.send(FeedMsg::CoreBase {
        base: "USDT".to_string(),
    });

    // Run state, so the terminal's core RUN controls have something to show on a synthetic core.
    // A real core reports these over `TRuntimeStateCommand` and `TStratRuntimeState`; without them
    // the whole control renders as "never reported" and the bench cannot exercise it at all.
    // `MOON_SYNTH_STOPPED` / `MOON_SYNTH_TRADING_OFF` select the other branch of each — a stopped
    // runtime (which is what puts the restart button on screen) and a stopped strategy engine.
    let _ = tx.send(FeedMsg::RuntimeState(crate::feed::RuntimeState {
        is_started: !env_flag("MOON_SYNTH_STOPPED"),
        auto_detect_active: true,
    }));
    let _ = tx.send(FeedMsg::StrategiesRunning(!env_flag("MOON_SYNTH_TRADING_OFF")));

    // AddToChart: window w (1..=WINDOWS) receives CHARTS markets in the Chart{w} container.
    //
    // KeepInChart defaults to 0 here — "keep forever", see `DetectRow::keep_in_chart_ttl_ms` — so
    // the bench holds its panes for the whole run. It used to be a year in seconds: the same
    // eternity, but as a FINITE TTL, which meant no run ever reached the infinite-TTL branch. The
    // environment keeps the other branch (a finite TTL, so pruning and its timer) reachable too,
    // or runs would simply stop covering that one instead.
    // Clamped, not cast: `as u32` wraps, so a number past the type asks for one TTL and gets an
    // unrelated one — and a multiple of 2^32 wraps to 0, which here means "forever", the exact
    // opposite of the long finite TTL such a number asks for.
    let keep_in_chart = env_usize("MOON_SYNTH_KEEP_IN_CHART", 0).min(u32::MAX as usize) as u32;
    let mut dets = Vec::new();
    // Numbered from ONE. The ingest side skips everything with `seq <= last` and starts `last` at
    // zero, so a detect numbered zero is never taken. With fifty detects that lost one of them and
    // nobody noticed; with one detect per core - which is what a bench wants - it lost all of them,
    // and the chart tabs simply never appeared.
    let mut seq = 1u64;
    for w in 1..=windows {
        for m in 0..charts {
            dets.push(DetectRow {
                seq,
                market: markets[m % n].clone(),
                time_ms: now_ms(),
                sound_alert: false,
                keep_alert_secs: 0,
                add_to_chart: w as u32,
                keep_in_chart_secs: keep_in_chart,
                sound_name: None,
                is_alert: false,
                kind: 0,
                is_short: false,
                // The synthetic feed drives AddToChart, not the detect captions: a fabricated line
                // on a fixture chart would be indistinguishable from a real one in a bench shot.
                msg: String::new(),
                strat_name: String::new(),
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

    // `MOON_SYNTH_START_PRICE` anchors the whole stream — ticks, and through them the order book,
    // the ticker and the chart's price labels — at a given price instead of the default 100 per
    // market. The fixture bench needs it: its candles sit near 0.2, and a tick stream three orders
    // of magnitude above them drags the automatic Y fit and the header ticker with it, which on
    // screen looks exactly like "the chart is empty".
    let start_price = env_f64("MOON_SYNTH_START_PRICE", 0.0);
    let mut price: Vec<f64> = if start_price > 0.0 {
        vec![start_price; n]
    } else {
        (0..n).map(|i| 100.0 * (i as f64 + 1.0)).collect()
    };
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
                    let off = (k as f64 + 1.0) * mid * book_step;
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
