//! Owner-thread notification tick, with synthetic trades and links.
//!
//! The host below never starts a bot and never opens the report database. A
//! missing override panics before it can touch a live session.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use moon_core::telegram::notify::{ChatNotify, NotifyFile, Pending};
use moon_core::telegram::runtime::NotifyStore;

use super::InjectedReads;
use crate::notify::down::Link;
use crate::notify::test_host::{CHAT, DuringJob, TempRoot, TickHost};
use crate::notify::trades::ClosedTrade;
/// No rows. Installed so a mistaken job does not open the report database.
fn empty_reads() -> InjectedReads {
    InjectedReads { trades: Vec::new() }
}

/// One closed trade on core 7.
///
/// Args:
///     rec_id: Report row id.
///     close_utc: Close time, UTC Unix seconds. The open time is ten seconds earlier.
///     coin: Market symbol the card must show.
///     profit_usd: Realised profit, or `None` when the row is unvalued.
///
/// Returns:
///     A trade whose name, strategy, and volume are fixed by this fixture.
fn closed_trade(rec_id: i64, close_utc: i64, coin: &str, profit_usd: Option<f64>) -> ClosedTrade {
    ClosedTrade {
        core: 7,
        rec_id,
        close_utc,
        coin: coin.to_string(),
        core_name: "CoreA".to_string(),
        strategy: String::new(),
        volume_usd: None,
        profit_usd,
        profit_pct: None,
        ..ClosedTrade::default()
    }
}

/// The two closes the trade tests inject: one after enable, one before it.
///
/// Returns:
///     Rec 11 closed at 1500, and rec 12 closed at 900.
fn closes_around_enable() -> InjectedReads {
    InjectedReads {
        trades: vec![
            closed_trade(11, 1_500, "NEWCOIN", None),
            closed_trade(12, 900, "OLDCOIN", None),
        ],
    }
}

/// Turn trade cards on at unix 1000 and record settings revision 1.
///
/// Args:
///     file: Document the host is about to save.
fn enable_trades(file: &mut NotifyFile) {
    let mut chat = ChatNotify {
        revision: 1,
        ..ChatNotify::default()
    };
    chat.settings.trades.on = true;
    chat.ledger.trades_enabled_utc = Some(1_000);
    file.chats.insert(CHAT, chat);
}

/// A chat with every switch off spawns nothing and writes nothing.
#[test]
fn all_off_chat_spawns_no_job_and_enqueues_nothing() {
    let root = TempRoot::new("all-off");
    let mut host = TickHost::open(root.notifications());
    host.admit(CHAT);
    host.edit(|file| {
        file.chats.insert(CHAT, ChatNotify::default());
    });
    host.state.injected_reads = Some(empty_reads());
    host.tick(2_000);
    assert_eq!(host.jobs, 0, "an all-off file must not spawn a read");
    assert!(host.outbox().is_empty(), "an all-off file must not enqueue");
}

/// A close after the enable moment is sent once. An older close is not, and the next tick adds nothing.
#[test]
fn enabled_chat_enqueues_closes_after_enable_once() {
    let _locale = crate::test_locale::force("en");
    let root = TempRoot::new("trades");
    let mut host = TickHost::open(root.notifications());
    host.admit(CHAT);
    host.edit(enable_trades);
    host.state.visible_override = Some(vec![7]);
    host.state.injected_reads = Some(closes_around_enable());
    host.tick(2_000);
    let rows = host.outbox();
    assert_eq!(rows.len(), 1, "only the close after enable is announced");
    assert_eq!(rows[0].chat, CHAT);
    assert_eq!(rows[0].created_utc, 2_000);
    assert_eq!(rows[0].cores, Some(vec![7]));
    assert!(rows[0].html.contains("NEWCOIN"), "{}", rows[0].html);
    assert!(
        !rows[0].html.contains("OLDCOIN"),
        "a close before enable must stay out of the card: {}",
        rows[0].html
    );
    assert_eq!(host.seen_ids(), BTreeSet::from([11]));
    assert_eq!(host.jobs, 1);
    host.tick(2_000);
    assert_eq!(host.jobs, 1, "the interval has not elapsed");
    assert_eq!(
        host.outbox().len(),
        1,
        "a second tick must not send the card again"
    );
    assert_eq!(host.seen_ids(), BTreeSet::from([11]));
}

/// A settings revision that moves while the job runs skips the chat.
#[test]
fn revision_changed_during_the_job_skips_the_chat() {
    let _locale = crate::test_locale::force("en");
    let root = TempRoot::new("revision");
    let mut host = TickHost::open(root.notifications());
    host.admit(CHAT);
    host.edit(enable_trades);
    host.state.visible_override = Some(vec![7]);
    host.state.injected_reads = Some(closes_around_enable());
    host.during = DuringJob::BumpRevision(CHAT);
    host.tick(2_000);
    assert_eq!(host.jobs, 1);
    assert!(
        host.outbox().is_empty(),
        "a moved revision must not enqueue"
    );
    assert!(
        host.seen_ids().is_empty(),
        "a skipped chat must not mark seen"
    );
    assert!(!host.state.notify_busy, "finish must clear the busy flag");
    assert_eq!(
        host.file().chats.get(&CHAT).map(|chat| chat.revision),
        Some(2),
        "the in-flight settings save is the revision finish compares against"
    );
}

/// A fresh process with the same file does not send a close that is already in the ledger.
#[test]
fn restart_does_not_resend_a_seen_close() {
    let _locale = crate::test_locale::force("en");
    let root = TempRoot::new("restart");
    let path = root.notifications();
    let first_html = {
        let mut host = TickHost::open(path.clone());
        host.admit(CHAT);
        host.edit(enable_trades);
        host.state.visible_override = Some(vec![7]);
        host.state.injected_reads = Some(closes_around_enable());
        host.tick(2_000);
        let html = host.htmls();
        assert_eq!(html.len(), 1);
        assert!(html[0].contains("NEWCOIN"));
        html
    };
    let mut host = TickHost::open(path);
    host.admit(CHAT);
    host.state.visible_override = Some(vec![7]);
    host.state.injected_reads = Some(closes_around_enable());
    host.tick(2_000);
    assert_eq!(host.jobs, 1, "the restarted tick still reads");
    assert_eq!(
        host.htmls(),
        first_html,
        "a seen close must not be queued again"
    );
    assert_eq!(host.seen_ids(), BTreeSet::from([11]));
}

/// One down notice after the delay, one back notice, and no repeats.
#[test]
fn down_after_the_delay_is_enqueued_once() {
    let _locale = crate::test_locale::force("en");
    let root = TempRoot::new("down");
    let mut host = TickHost::open(root.notifications());
    host.admit(CHAT);
    host.edit(|file| {
        let mut chat = ChatNotify::default();
        chat.settings.down.on = true;
        chat.settings.down.after_minutes = 1;
        file.chats.insert(CHAT, chat);
    });
    host.state.visible_override = Some(vec![1]);
    host.state.injected_reads = Some(empty_reads());
    host.state.down_links_override = Some(vec![(1, "AlphaCore".to_string(), Link::Lost)]);

    host.tick(0);
    assert!(
        host.outbox().is_empty(),
        "the loss is still inside the delay"
    );
    assert_eq!(host.jobs, 0);

    host.tick(60);
    let down = host.outbox();
    assert_eq!(down.len(), 1);
    assert_eq!(down[0].created_utc, 60);
    assert_eq!(down[0].chat, CHAT);
    assert_eq!(down[0].cores, Some(vec![1]));
    assert!(down[0].html.contains("AlphaCore"), "{}", down[0].html);
    assert!(
        down[0].html.contains("lost connection since"),
        "{}",
        down[0].html
    );

    host.tick(120);
    assert_eq!(
        host.outbox().len(),
        1,
        "a still-lost core is not announced twice"
    );

    host.state.down_links_override = Some(vec![(1, "AlphaCore".to_string(), Link::Up)]);
    host.tick(180);
    let both = host.outbox();
    assert_eq!(both.len(), 2);
    assert!(
        both[0].html.contains("lost connection since"),
        "{}",
        both[0].html
    );
    assert!(
        both[1].html.contains("connection restored"),
        "{}",
        both[1].html
    );
    assert_eq!(both[1].created_utc, 180);
    assert_eq!(both[1].chat, CHAT);
    assert_eq!(both[1].cores, Some(vec![1]));

    host.tick(240);
    assert_eq!(
        host.outbox().len(),
        2,
        "a second up observation adds nothing"
    );
    assert_eq!(host.jobs, 0, "a down-only chat does not read the replica");
}

/// Unpairing while the job runs skips the chat.
#[test]
fn unpaired_chat_is_skipped_when_the_job_finishes() {
    let _locale = crate::test_locale::force("en");
    let root = TempRoot::new("unpair");
    let mut host = TickHost::open(root.notifications());
    host.admit(CHAT);
    host.edit(enable_trades);
    host.state.visible_override = Some(vec![7]);
    host.state.injected_reads = Some(closes_around_enable());
    host.during = DuringJob::Unpair;
    host.tick(2_000);
    assert_eq!(host.jobs, 1);
    assert!(
        host.outbox().is_empty(),
        "an unpaired chat must not enqueue"
    );
    assert!(
        host.seen_ids().is_empty(),
        "an unpaired chat must not mark seen"
    );
    assert!(!host.state.notify_busy);
}

/// A store pointer that changed while the job ran skips every chat.
#[test]
fn replaced_store_skips_every_chat() {
    let _locale = crate::test_locale::force("en");
    let root = TempRoot::new("replace");
    let mut host = TickHost::open(root.notifications());
    host.admit(CHAT);
    host.edit(enable_trades);
    host.state.visible_override = Some(vec![7]);
    host.state.injected_reads = Some(closes_around_enable());
    host.during = DuringJob::ReplaceStore;
    host.tick(2_000);
    assert_eq!(host.jobs, 1);
    assert!(
        host.outbox().is_empty(),
        "a replaced store must not be written"
    );
    assert!(host.seen_ids().is_empty());
    assert!(!host.state.notify_busy);
}

/// One queued row for a viewer fixture.
///
/// Args:
///     id: Outbox id. Tests pick ids so the remaining rows are obvious.
///     cores: Cores the row discloses. `None` is an unknown legacy row.
///         `Some([])` discloses no core and is not unknown.
///
/// Returns:
///     A row addressed to [`CHAT`].
fn queued(id: u64, cores: Option<Vec<u64>>) -> Pending {
    Pending {
        id,
        chat: CHAT,
        html: format!("row {id}"),
        created_utc: 1,
        cores,
        auto: None,
        ..Pending::default()
    }
}

/// Seed three outbox rows and an all-off chat so the tick purges and does not send.
///
/// Args:
///     host: Host whose store will hold the rows.
fn seed_mixed_outbox(host: &TickHost) {
    host.edit(|file| {
        file.outbox = vec![
            queued(1, Some(vec![7])),
            queued(2, Some(vec![8])),
            queued(3, None),
        ];
        file.next_id = 4;
        file.chats.insert(CHAT, ChatNotify::default());
    });
}

/// A viewer drops a revoked core and an unknown row before anything is sent.
#[test]
fn viewer_outbox_drops_revoked_and_unknown_rows_before_send() {
    let root = TempRoot::new("viewer-purge");
    let mut host = TickHost::open(root.notifications());
    host.config.telegram.authorized_chat_ids = vec![CHAT];
    host.config.telegram.owner_chat_id = None;
    host.config.telegram.chat_profile_mut(CHAT).core_uids = vec![8];
    seed_mixed_outbox(&host);
    host.tick(2_000);
    let ids: Vec<u64> = host.outbox().into_iter().map(|row| row.id).collect();
    assert_eq!(ids, vec![2]);
    assert_eq!(host.jobs, 0, "an all-off chat must not send");
}

/// The owner keeps an unknown row and drops a hidden core before anything is sent.
#[test]
fn owner_outbox_keeps_unknown_rows_and_drops_a_hidden_core() {
    let root = TempRoot::new("owner-purge");
    let mut host = TickHost::open(root.notifications());
    host.admit(CHAT);
    host.state.visible_override = Some(vec![8]);
    seed_mixed_outbox(&host);
    host.tick(2_000);
    let ids: Vec<u64> = host.outbox().into_iter().map(|row| row.id).collect();
    assert_eq!(ids, vec![2, 3]);
    assert_eq!(host.jobs, 0, "an all-off chat must not send");
}

/// `seen` and `trades_enabled_utc` written after the down snapshot must survive the write.
#[test]
fn write_down_keeps_seen_and_the_trade_floor_set_after_the_snapshot() {
    let root = TempRoot::new("down-partial");
    let store = Mutex::new(NotifyStore::open(root.notifications()).expect("open"));
    {
        let mut guard = store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard
            .update(|file| {
                let mut chat = ChatNotify::default();
                chat.ledger.down_announced.insert(1);
                file.chats.insert(CHAT, chat);
            })
            .expect("seed");
    }
    let mut stepped = {
        let guard = store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.file.chats[&CHAT].ledger.clone()
    };
    {
        let mut guard = store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard
            .update(|file| {
                let entry = file.chats.get_mut(&CHAT).expect("chat");
                entry.ledger.seen.insert(9, BTreeMap::from([(3, 50)]));
                entry.ledger.trades_enabled_utc = Some(40);
            })
            .expect("live edit");
    }
    stepped.down_announced.insert(4);
    let wrote = super::write_down(&store, CHAT, stepped.clone(), Vec::new(), 80);
    assert!(matches!(wrote, Ok(true)), "the down write must land");
    let ledger = store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .file
        .chats[&CHAT]
        .ledger
        .clone();
    assert_eq!(ledger.down_announced, stepped.down_announced);
    assert_eq!(ledger.trades_enabled_utc, Some(40));
    assert_eq!(
        ledger.seen.get(&9).and_then(|rows| rows.get(&3)).copied(),
        Some(50)
    );
}

/// A BTC card sent before its valuation waits for it when the chat asks for the dollars: the
/// sender records its message, and the read that finds the trade valued queues an edit of that
/// message and stops waiting. A USDT card never waits — its amount already is dollars.
#[test]
fn a_waiting_card_gets_its_dollars_written_in() {
    let _locale = crate::test_locale::force("en");
    let root = TempRoot::new("followup");
    let mut host = TickHost::open(root.notifications());
    host.admit(CHAT);
    host.edit(|file| {
        enable_trades(file);
        file.chats
            .get_mut(&CHAT)
            .unwrap()
            .settings
            .trades
            .usd_followup = true;
    });
    host.state.visible_override = Some(vec![7]);
    let mut btc = closed_trade(11, 1_500, "ETHBTC", None);
    btc.quote = moon_core::db::QuoteCurrency::from_report_ordinal(0);
    btc.profit_native = Some(0.00012);
    let mut usdt = closed_trade(12, 1_501, "ACEUSDT", Some(1.0));
    usdt.quote = moon_core::db::QuoteCurrency::from_report_ordinal(1);
    usdt.profit_native = Some(1.0);
    host.state.injected_reads = Some(InjectedReads {
        trades: vec![btc.clone(), usdt],
    });
    host.tick(2_000);
    let rows = host.outbox();
    assert_eq!(rows.len(), 2);
    let key = moon_core::telegram::notify::CardKey {
        core: 7,
        rec_id: 11,
    };
    assert_eq!(rows[0].card, Some(key));
    assert!(rows[0].html.contains("+0.00012 BTC"), "{}", rows[0].html);
    assert!(!rows[0].html.contains('\u{2248}'));
    assert_eq!(rows[1].card, None, "a USDT card has nothing to wait for");
    // The sender's ack: the rows leave the outbox and the card's message is recorded.
    host.edit(|file| {
        file.outbox.clear();
        let wait = file
            .chats
            .get_mut(&CHAT)
            .unwrap()
            .ledger
            .cards
            .get_mut(&7)
            .unwrap();
        wait.get_mut(&11).unwrap().message = Some(555);
    });
    btc.profit_usd = Some(7.5);
    host.state.injected_reads = Some(InjectedReads { trades: vec![btc] });
    host.state.last_notify_run = None;
    host.tick(2_100);
    let rows = host.outbox();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].edit, Some(555));
    assert!(rows[0].html.contains("\u{2248} +7.50$"), "{}", rows[0].html);
    assert!(host.file().chats[&CHAT].ledger.cards.is_empty());
}
