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

/// One message per batch, laid out as the cores' own bot writes it: the core and a colon, then each
/// event with its coin as a tag and its strategy below; a run of one core's events shares its name.
/// Core names use the shared hashtag mapping, an emulator entry is marked, and the
/// message discloses exactly the cores it names.
#[test]
fn a_batch_is_one_message_under_its_cores() {
    let _locale = crate::test_locale::force("en");
    let names = CoreNames::from_pairs([(1u64, "Al<pha")]);
    let a = detect(1, 1, "BTC<USDT>", "price +3% in 1m");
    let b = opened(1, 2, false);
    let c = opened(2, 3, true);
    let (html, cores) = render(&[&a, &b, &c], &names, 0).expect("a message");
    assert_eq!(
        html,
        "#Al_pha:\n\
         \u{1f514} #BTC_USDT_ \u{2014} price +3% in 1m\n\
         <i>Pump &lt;1&gt;</i>\n\
         \u{1f7e6} #ACE \u{2014} Trade opened\n\
         <i>MS</i>\n\
         #core_2:\n\
         \u{1f7e6} #ACE \u{2014} Trade opened (emulator)\n\
         <i>MS</i>"
    );
    assert_eq!(cores, vec![1, 2]);
    assert!(render(&[], &names, 0).is_none());
    let (lost_only, lost_cores) = render(&[], &names, 3).expect("the loss is told");
    assert!(lost_only.contains('3'), "{lost_only}");
    assert!(lost_cores.is_empty());
}

/// A relay using its old 48-character cap would search a different tag from a trade card.
#[test]
fn event_names_share_the_card_tag_cap_and_fallback() {
    let _locale = crate::test_locale::force("en");
    let long = format!("{}-tail", "A".repeat(63));
    let names = CoreNames::from_pairs([(1u64, long.as_str()), (2, "123<&>")]);
    let a = detect(1, 1, &long, "");
    let b = opened(2, 2, false);
    let (html, _) = render(&[&a, &b], &names, 0).expect("a message");
    let expected = format!("#{}_", "A".repeat(63));
    assert_eq!(html.lines().next(), Some(format!("{expected}:").as_str()));
    assert_eq!(
        html.lines().nth(1),
        Some(format!("\u{1f514} {expected}").as_str())
    );
    assert!(html.contains("\n123&lt;&amp;&gt;:\n"));
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
