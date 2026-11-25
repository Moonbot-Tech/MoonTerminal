//! Unit tests for the synthetic feed: its still market, its announced build, its command drain.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::run_from;
use crate::config::ServerConfig;
use crate::feed::{
    ConnStatus, CoreCmd, CoreFeatures, FeedMsg, FeedTx, MarketDirtyFlags, QueuedCmd,
};

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
    sent_by_a_commanded_feed(still_at, Vec::new()).0
}

/// Everything a feed sends while it runs for [`RUN_FOR`], with `commands_midway` queued halfway.
///
/// Args:
///     still_at: The moment to hold the market at; `None` for the ordinary stream.
///     commands_midway: Commands queued for the feed after half of the run.
///
/// Returns:
///     The messages in order, and how many were sent before the commands were queued.
fn sent_by_a_commanded_feed(
    still_at: Option<f64>,
    commands_midway: Vec<CoreCmd>,
) -> (Vec<FeedMsg>, usize) {
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
    std::thread::sleep(RUN_FOR / 2);
    let mut messages: Vec<FeedMsg> = sent.try_iter().collect();
    let before = messages.len();
    for cmd in commands_midway {
        commands
            .send(QueuedCmd {
                at: Instant::now(),
                cmd,
            })
            .unwrap();
    }
    std::thread::sleep(RUN_FOR / 2);
    drop(commands);
    feed.join().unwrap().unwrap();
    messages.extend(sent.try_iter());
    (messages, before)
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

/// A synthetic core plays a current core, so it must announce both optional commands after `Ready`.
///
/// Breakage: without the announcement the store keeps `CoreFeatures::default()` and every control
/// gated on it — the order menu's forced cancellation, the exchange-wide entries cancel — stays
/// disabled on the fixture, the bench and the capture build. Sent before `Ready` it is lost too:
/// the store forgets the build on every non-Ready status.
#[test]
fn a_synthetic_core_announces_a_current_build_after_ready() {
    let sent = sent_by_a_feed(Some(MOMENT_MS));
    let ready = sent
        .iter()
        .position(|message| matches!(message, FeedMsg::Status(ConnStatus::Ready)))
        .expect("the feed reports Ready");
    let announced = sent
        .iter()
        .position(|message| {
            matches!(message, FeedMsg::CoreFeatures(features)
                if features.force_cancel && features.exchange_buys_cancel)
        })
        .expect("the feed announces a current build");
    assert!(ready < announced);
    assert!(!sent.iter().any(|message| matches!(message,
    FeedMsg::CoreFeatures(features) if *features != CoreFeatures {
        exchange_buys_cancel: true,
        force_cancel: true,
    })));
}

/// A forced cancellation sent to a synthetic core leaves the feed running: it keeps streaming after
/// the command and ends cleanly when the terminal goes away.
///
/// This proves survival only. The drain executes no command of any kind, so there is nothing about
/// the command's effect to observe here.
///
/// Breakage: a synth that handled the new command by panicking, ending its loop or blocking would
/// take the fixture's only core down at the first answered confirmation; the join in
/// [`sent_by_a_commanded_feed`] fails on a panic and the stream count on a stop or a stall.
#[test]
fn a_forced_cancellation_leaves_the_stream_running() {
    let (sent, before) = sent_by_a_commanded_feed(
        None,
        vec![
            CoreCmd::ForceCancelOrder { uid: 1 },
            CoreCmd::CancelOrder { uid: 1 },
        ],
    );
    assert!(reports_of(&sent[before..], MarketDirtyFlags::HISTORY) > 1);
}
