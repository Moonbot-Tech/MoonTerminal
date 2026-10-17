//! Automatic reports through the owner tick, with injected pages and a UTC host.

use chrono::TimeZone;
use moon_core::telegram::notify::{AutoReport, ChatNotify};

use super::{InjectedAuto, offset_label};
use crate::notify::test_host::{CHAT, TempRoot, TickHost};

/// A scheduled report must use the saved mode for both its table and its view-switch caption.
#[test]
fn scheduled_reports_follow_the_chat_pick_and_only_fall_back_when_unset() {
    use moon_core::config::{
        CoreGroup, ReportLayout,
        telegram_menu::{ReportBasis, ReportView},
    };
    use moon_core::telegram::api::ReplyMarkup;
    let _locale = crate::test_locale::force("en");
    let root = TempRoot::new("auto-chat-view");
    let mut host = host_with(&root, &[AutoReport::Hourly], Some(at(13, 0)));
    host.config.telegram.bot.report_view = ReportView::Exchanges;
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE orders_rep (core_uid INTEGER, core_name TEXT,
        newrecid INTEGER, closedate INTEGER, profitbtc REAL, spentbtc REAL, basecurrency INTEGER);
        INSERT INTO orders_rep VALUES (1,'CORE_A',1,100,5,100,1);",
    )
    .unwrap();
    let inputs = crate::report::AutoInputs {
        zone: chrono_tz::UTC,
        basis: ReportBasis::Close,
        layout: ReportLayout::default(),
        view: ReportView::Exchanges,
        order: moon_core::session::core_order::CoreOrder::new(&host.config),
        names: Default::default(),
        venues: Default::default(),
        groups: vec![CoreGroup {
            name: "GROUP_A".into(),
            cores: vec![1],
        }],
    };
    for (saved, expected, label) in [
        (
            Some(ReportView::Groups),
            ReportView::Groups,
            "View: Groups ▾",
        ),
        (None, ReportView::Exchanges, "View: Exchanges ▾"),
    ] {
        host.edit(|file| file.chats.get_mut(&CHAT).unwrap().report_view = saved);
        let mut due = super::due_reports(&host, &host.file(), at(14, 2));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].view, expected);
        // Retain the scheduler's actual request path but place its frozen slot over this fixture.
        due[0].window.from = 0;
        due[0].window.to = 200;
        let page = super::read_all_on(&conn, &due, &inputs)
            .pop()
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            page.html.contains("GROUP_A"),
            saved.is_some(),
            "the table follows the chat preference"
        );
        let ReplyMarkup::Inline(markup) = page.keyboard else {
            panic!("inline")
        };
        assert_eq!(markup.inline_keyboard[0][0].text, label);
    }
}

/// A preference changed during a database read must be retried, not queued in the stale mode.
#[test]
fn a_scheduled_read_with_a_stale_view_does_not_spend_the_slot() {
    use moon_core::config::telegram_menu::ReportView;
    let root = TempRoot::new("auto-view-race");
    let mut host = host_with(&root, &[AutoReport::Hourly], Some(at(13, 0)));
    let due = super::due_reports(&host, &host.file(), at(14, 2));
    let next = if due[0].view == ReportView::Groups {
        ReportView::Cores
    } else {
        ReportView::Groups
    };
    host.edit(|file| file.chats.get_mut(&CHAT).unwrap().report_view = Some(next));
    let store = host.state.notify_store_override.as_ref().unwrap();
    let store_ptr = std::sync::Arc::as_ptr(store).addr();
    let page = crate::report::AutoPage {
        html: "<p>stale</p>".into(),
        keyboard: moon_core::telegram::api::ReplyMarkup::Inline(
            moon_core::telegram::api::InlineKeyboardMarkup::from_rows(Vec::new()),
        ),
        cores: None,
    };
    super::finish(&mut host, store_ptr, due, vec![Ok(Some(page))], at(14, 2));
    assert!(host.outbox().is_empty());
    assert_eq!(
        host.file().chats[&CHAT].ledger.reports.hourly.slot_utc,
        Some(at(13, 0))
    );
}

/// UTC seconds of 2026-10-03 at `hour:minute`.
fn at(hour: u32, minute: u32) -> i64 {
    chrono_tz::UTC
        .with_ymd_and_hms(2026, 10, 3, hour, minute, 0)
        .single()
        .expect("utc instant")
        .timestamp()
}

/// A paired owner chat with `kinds` on and every slot recorded at `recorded`.
fn host_with(root: &TempRoot, kinds: &[AutoReport], recorded: Option<i64>) -> TickHost {
    let mut host = TickHost::open(root.notifications());
    host.admit(CHAT);
    // No live core at all: an owner's report names none, so the purge of a tick with rows queued
    // keeps it, and reads no session.
    host.state.visible_override = Some(Vec::new());
    host.state.injected_auto = Some(InjectedAuto {
        html: "<p>report</p>".into(),
        cores: None,
    });
    host.edit(|file| {
        let mut chat = ChatNotify::default();
        for &kind in kinds {
            chat.settings.reports.set(kind, true);
            chat.ledger.reports.slot_mut(kind).slot_utc = recorded;
        }
        file.chats.insert(CHAT, chat);
    });
    host
}

/// Let the next tick read again at once, as the interval would after a minute.
fn reopen(host: &mut TickHost) {
    host.state.last_auto_run = None;
}

/// A report whose slot came is queued once as a rich row and its slot recorded; the same slot
/// does not queue it again, the next one does.
#[test]
fn a_report_is_queued_once_per_slot() {
    let root = TempRoot::new("auto-once");
    let mut host = host_with(&root, &[AutoReport::Hourly], Some(at(13, 0)));
    host.tick(at(14, 2));
    let outbox = host.outbox();
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0].chat, CHAT);
    assert_eq!(outbox[0].cores, None);
    assert_eq!(
        outbox[0].auto.as_ref().map(|auto| auto.kind),
        Some(AutoReport::Hourly)
    );
    let slot = host.file().chats[&CHAT].ledger.reports.hourly.slot_utc;
    assert_eq!(slot, Some(at(14, 0)));
    reopen(&mut host);
    host.tick(at(14, 40));
    assert_eq!(host.outbox().len(), 1, "the same slot is not sent twice");
    reopen(&mut host);
    host.tick(at(15, 0));
    assert_eq!(host.outbox().len(), 2, "hourly reports stay, one per hour");
}

/// A report switched on records the slot it was switched on in, so nothing goes before the next.
#[test]
fn nothing_is_due_inside_the_slot_a_report_was_switched_on_in() {
    let root = TempRoot::new("auto-fresh");
    let mut host = host_with(&root, &[AutoReport::Today], Some(at(14, 0)));
    host.tick(at(14, 37));
    assert!(host.outbox().is_empty());
    assert_eq!(host.jobs, 0, "no read is spawned when nothing is due");
}

/// After a run that missed several slots, each kind sends its latest slot once, not a batch.
#[test]
fn a_missed_run_sends_the_latest_slot_of_each_kind_once() {
    let root = TempRoot::new("auto-missed");
    let kinds = [AutoReport::Hourly, AutoReport::Today, AutoReport::Month];
    let mut host = host_with(&root, &kinds, Some(at(0, 0) - 5 * 86_400));
    host.tick(at(14, 2));
    let kinds: Vec<_> = host
        .outbox()
        .iter()
        .filter_map(|row| row.auto.as_ref().map(|auto| auto.kind))
        .collect();
    assert_eq!(kinds.len(), 3);
    let ledger = host.file().chats[&CHAT].ledger.reports;
    assert_eq!(ledger.hourly.slot_utc, Some(at(14, 0)));
    assert_eq!(ledger.today.slot_utc, Some(at(14, 0)));
    assert_eq!(ledger.month.slot_utc, Some(at(0, 0)));
    reopen(&mut host);
    host.tick(at(14, 30));
    assert_eq!(host.outbox().len(), 3);
}

/// An unpaired chat and a chat with the report off get nothing and record nothing.
#[test]
fn unpaired_or_switched_off_chats_get_nothing() {
    let root = TempRoot::new("auto-off");
    let mut host = host_with(&root, &[], None);
    host.tick(at(14, 2));
    assert!(host.outbox().is_empty());
    let mut host = host_with(&root, &[AutoReport::Hourly], None);
    host.config.telegram.authorized_chat_ids.clear();
    host.config.telegram.owner_chat_id = None;
    host.tick(at(14, 2));
    assert!(host.outbox().is_empty());
    assert_eq!(
        host.file().chats[&CHAT].ledger.reports.hourly.slot_utc,
        None
    );
}

/// A read in flight does not start a second; the interval holds a retry back.
#[test]
fn one_read_at_a_time_and_none_inside_the_interval() {
    let root = TempRoot::new("auto-gate");
    let mut host = host_with(&root, &[AutoReport::Hourly], Some(at(13, 0)));
    host.state.auto_busy = true;
    host.tick(at(14, 2));
    assert_eq!(host.jobs, 0);
    host.state.auto_busy = false;
    host.state.last_auto_run = Some(std::time::Instant::now());
    host.tick(at(14, 2));
    assert_eq!(host.jobs, 0);
    reopen(&mut host);
    host.tick(at(14, 2));
    assert_eq!(host.jobs, 1);
    assert!(!host.state.auto_busy, "finish clears the flag");
}

/// The caption names the zone by its offset at the slot.
#[test]
fn the_offset_label_reads_like_a_clock() {
    assert_eq!(offset_label(at(14, 0), chrono_tz::UTC), "UTC");
    assert_eq!(offset_label(at(14, 0), chrono_tz::Europe::Moscow), "UTC+3");
    assert_eq!(
        offset_label(at(14, 0), chrono_tz::Asia::Kathmandu),
        "UTC+5:45"
    );
    assert_eq!(
        offset_label(at(14, 0), chrono_tz::America::New_York),
        "UTC\u{2212}4"
    );
}

/// Turning off Hourly must leave Today due every hour and Month due only at midnight;
/// coupling the switches or changing their slots would contradict the schedule labels.
#[test]
fn today_keeps_its_hourly_schedule_with_hourly_off() {
    let root = TempRoot::new("auto-today-hourly-off");
    let mut host = host_with(
        &root,
        &[AutoReport::Today, AutoReport::Month],
        Some(at(13, 0)),
    );
    host.tick(at(14, 0));
    let first = host.outbox()[0].id;
    reopen(&mut host);
    host.tick(at(14, 59));
    assert_eq!(host.outbox().len(), 1);
    assert_eq!(host.outbox()[0].id, first, "no update inside the same hour");
    reopen(&mut host);
    host.tick(at(15, 0));
    assert_eq!(
        host.outbox().len(),
        1,
        "Today replaces its queued predecessor"
    );
    assert_ne!(
        host.outbox()[0].id,
        first,
        "a new hour queues a fresh Today report"
    );
    assert_eq!(host.outbox()[0].created_utc, at(15, 0));
    assert_eq!(
        host.file().chats[&CHAT].ledger.reports.today.slot_utc,
        Some(at(15, 0))
    );
    assert!(
        host.outbox()
            .iter()
            .all(|row| row.auto.as_ref().unwrap().kind == AutoReport::Today)
    );
    reopen(&mut host);
    host.tick(at(0, 0) + 86_400);
    let kinds: Vec<_> = host
        .outbox()
        .iter()
        .map(|row| row.auto.as_ref().unwrap().kind)
        .collect();
    assert_eq!(kinds, vec![AutoReport::Today, AutoReport::Month]);
    assert_eq!(host.outbox()[0].created_utc, at(0, 0) + 86_400);
    let window = moon_core::telegram::report::auto_window(
        AutoReport::Today,
        at(0, 0) + 86_400,
        chrono_tz::UTC,
    )
    .unwrap();
    assert_eq!((window.from, window.to), (at(0, 0), at(0, 0) + 86_400 - 1));
    assert_eq!(
        window.period,
        moon_core::telegram::report::Period::Yesterday
    );
}

/// Reusing the compact toggle key in a delivered report turns its caption into a settings
/// instruction. Preserve the independently reviewed report title while the toggle names timing.
#[test]
fn hourly_caption_keeps_the_delivered_report_title() {
    let _locale = crate::test_locale::force("en");
    let window =
        moon_core::telegram::report::auto_window(AutoReport::Hourly, at(14, 0), chrono_tz::UTC)
            .unwrap();
    let caption = super::caption(AutoReport::Hourly, &window, chrono_tz::UTC);
    assert_eq!(caption.title, "\u{1f4ca} Hourly report");
    assert_eq!(caption.zone, "UTC");
}
