//! Pins for the relay of the cores' own Telegram events.

use moon_core::db::{CoreNames, ReportStamp};
use moon_core::feed::CoreTgEvent;
use moon_core::session::TgEventRow;
use moon_core::telegram::notify::EventRule;

use super::{Fresh, render, wanted};

fn detect(core: u64, seq: u64, market: &str, msg: &str) -> Fresh {
    Fresh {
        core,
        row: TgEventRow {
            seq,
            at_utc_ms: seq as i64,
            event: CoreTgEvent::Detect {
                market: market.into(),
                msg: msg.into(),
                strat_name: "Pump <1>".into(),
                is_short: false,
            },
        },
    }
}

fn opened(core: u64, seq: u64, emulator: bool) -> Fresh {
    Fresh {
        core,
        row: TgEventRow {
            seq,
            at_utc_ms: seq as i64,
            event: CoreTgEvent::Opened {
                rec_id: 7,
                coin: "ACE".into(),
                strat_name: "MS".into(),
                emulator,
                buy: ReportStamp::Seconds(1),
            },
        },
    }
}

/// Each switch relays its own kind only: a chat that asked for entries must not get a detect
/// storm.
#[test]
fn each_switch_relays_its_own_kind() {
    let detects = EventRule {
        opened: false,
        detects: true,
    };
    assert!(wanted(detects, &detect(1, 1, "A", "").row.event));
    assert!(!wanted(detects, &opened(1, 2, false).row.event));
    let entries = EventRule {
        opened: true,
        detects: false,
    };
    assert!(wanted(entries, &opened(1, 2, false).row.event));
    assert!(!wanted(entries, &detect(1, 1, "A", "").row.event));
}

/// One message per batch: every event a line, names escaped, the configured core name used, an
/// emulator entry marked; and the message discloses exactly the cores it names.
#[test]
fn a_batch_is_one_message_of_lines() {
    let _locale = crate::test_locale::force("en");
    let names = CoreNames::from_pairs([(1u64, "Alpha")]);
    let a = detect(1, 1, "BTC<USDT>", "price +3% in 1m");
    let b = opened(2, 2, true);
    let (html, cores) = render(&[&a, &b], &names, 0).expect("a message");
    let lines: Vec<&str> = html.lines().collect();
    assert_eq!(lines.len(), 2, "{html}");
    assert!(lines[0].starts_with(
        "\u{1f514} <b>BTC&lt;USDT&gt;</b> \u{00b7} Alpha \u{00b7} <i>Pump &lt;1&gt;</i>"
    ));
    assert!(lines[0].ends_with("price +3% in 1m"));
    assert!(
        lines[1].contains("core 2"),
        "an unnamed core names itself: {html}"
    );
    assert!(lines[1].contains("Trade opened (emulator)"), "{html}");
    assert_eq!(cores, vec![1, 2]);
    assert!(render(&[], &names, 0).is_none());
    let (lost_only, lost_cores) = render(&[], &names, 3).expect("the loss is told");
    assert!(lost_only.contains('3'), "{lost_only}");
    assert!(lost_cores.is_empty());
}

/// A storm must not overflow Telegram's 4096 limit: past the budget the rest is counted on one
/// line, and the cores of uncounted lines are not disclosed.
#[test]
fn a_storm_is_cut_and_counted() {
    let _locale = crate::test_locale::force("en");
    let names = CoreNames::default();
    let long = "x".repeat(400);
    let storm: Vec<Fresh> = (0..200)
        .map(|i| detect(if i < 100 { 1 } else { 2 }, i, "COIN", &long))
        .collect();
    let refs: Vec<&Fresh> = storm.iter().collect();
    let (html, cores) = render(&refs, &names, 0).expect("a message");
    assert!(html.encode_utf16().count() < 4096);
    let last = html.lines().last().unwrap_or_default();
    assert!(last.contains("more"), "{last}");
    assert_eq!(cores, vec![1], "only printed lines disclose their core");
}

/// A group, which Telegram takes a message every three seconds, waits longer between batches
/// than a private chat, so a storm never queues faster than the chat is sent to.
#[test]
fn a_group_waits_longer_between_batches_than_a_private_chat() {
    assert_eq!(super::glue(42), super::BATCH);
    assert_eq!(super::glue(-1_001_234), super::GROUP_BATCH);
    assert!(super::GROUP_BATCH > std::time::Duration::from_secs(3));
    assert!(super::BATCH > std::time::Duration::from_secs(1));
}
