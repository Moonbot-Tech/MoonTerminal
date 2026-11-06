//! Unit tests for the synthetic feed's still market.

use std::sync::mpsc;
use std::time::Duration;

use super::run_from;
use crate::config::ServerConfig;
use crate::feed::{FeedMsg, FeedTx, MarketDirtyFlags, QueuedCmd};

const MOMENT_MS: f64 = 1_785_000_000_000.0;

/// How long a feed runs for here: at its 50 ticks and 20 books a second, a dozen and half a dozen.
const RUN_FOR: Duration = Duration::from_millis(300);

/// Everything a feed sends while it runs for [`RUN_FOR`].
///
/// Args:
///     still_at: The moment to hold the market at; `None` for the ordinary stream.
///
/// Returns:
///     The messages, in order.
fn sent_by_a_feed(still_at: Option<f64>) -> Vec<FeedMsg> {
    let server: ServerConfig =
        serde_json::from_str(r#"{"id": 91002, "name": "still fixture"}"#).unwrap();
    let (data, sent) = mpsc::channel();
    let (commands, command_rx) = mpsc::channel::<QueuedCmd>();
    let feed = std::thread::spawn(move || {
        run_from(
            &server,
            &FeedTx::new(data, None),
            &command_rx,
            None,
            still_at,
        )
    });
    std::thread::sleep(RUN_FOR);
    drop(commands);
    feed.join().unwrap().unwrap();
    sent.try_iter().collect()
}

/// How many times a feed reported a change of this kind of market data.
fn reports_of(sent: &[FeedMsg], flags: MarketDirtyFlags) -> usize {
    sent.iter()
        .filter(|message| {
            matches!(message, FeedMsg::MarketDataChanged(dirty)
                if dirty.iter().any(|one| one.flags == flags))
        })
        .count()
}

/// Breaks when a still market ticks, which moves a chart, or sends its book only once, which leaves
/// a chart opened later with no book at all: the store has a market only once a chart has opened it.
#[test]
fn a_still_market_has_no_ticks_and_keeps_sending_its_book() {
    let sent = sent_by_a_feed(Some(MOMENT_MS));
    assert_eq!(reports_of(&sent, MarketDirtyFlags::HISTORY), 0);
    assert!(reports_of(&sent, MarketDirtyFlags::ORDERBOOK) > 1);
}

/// Breaks when a stamp of a still market comes from the running clock, which differs in every run.
#[test]
fn a_still_market_stamps_everything_with_its_moment() {
    let sent = sent_by_a_feed(Some(MOMENT_MS));
    let detects: Vec<f64> = sent
        .iter()
        .filter_map(|message| match message {
            FeedMsg::Detects(rows) => Some(rows.iter().map(|row| row.time_ms)),
            _ => None,
        })
        .flatten()
        .collect();
    assert!(!detects.is_empty());
    assert!(detects.iter().all(|time_ms| *time_ms == MOMENT_MS));

    let news: Vec<i64> = sent
        .iter()
        .filter_map(|message| match message {
            FeedMsg::News(snapshot) => Some(snapshot.items.iter().map(|item| item.time_ms)),
            _ => None,
        })
        .flatten()
        .collect();
    // The default four, 90 seconds apart, and no fifth: a later item would change the News panel.
    let moment = MOMENT_MS as i64;
    assert_eq!(
        news,
        [moment - 270_000, moment - 180_000, moment - 90_000, moment]
    );
}

/// Breaks when the stream every bench and FireTest run measures stops running without being asked to.
#[test]
fn the_ordinary_stream_keeps_running() {
    let sent = sent_by_a_feed(None);
    assert!(reports_of(&sent, MarketDirtyFlags::HISTORY) > 1);
    assert!(reports_of(&sent, MarketDirtyFlags::ORDERBOOK) > 1);
}
