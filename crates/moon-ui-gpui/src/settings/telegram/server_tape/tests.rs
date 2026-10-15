//! The tape block's status line, its catch-up state and layout, and the Russian copy the user
//! approved.

use std::rc::Rc;

use chrono_tz::Tz;
use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, Styled, Window, div, px,
};
use moon_core::config::Language;
use moon_core::market::trade_replay::venue_caps;
use moon_core::station_api::{CatchupFailReason, CatchupSwitches, TapeCatchup};

use super::{
    CatchupState, TapeEd, TapeFacts, TapeStatus, TileValue, catchup_breakdown, catchup_sentence,
    catchup_state, catchup_switches, catchup_tiles, clock, follow, job_switches, plural_key,
    reported_switches, tape_breakdown, tape_status, tape_status_strip, tape_tiles,
    venue_reach_table,
};
use crate::design;

/// Preset prefixes typed inside custom numbers must not clear the field on the next render.
#[test]
fn custom_days_preserve_typed_preset_prefixes() {
    use moon_core::station_api::TapeKeep;
    let mut ed = TapeEd {
        draft_keep: Some(TapeKeep {
            max_age_days: 1825,
            record_emulator: true,
        }),
        ..TapeEd::default()
    };
    assert_eq!(ed.days_writeback().as_deref(), Some("1825"));
    for value in [3, 30, 300, 1, 18, 180, 1800, 3, 36, 365, 3650] {
        ed.draft_keep.as_mut().unwrap().max_age_days = value;
        ed.days_shown = Some(value);
        assert_eq!(ed.days_writeback(), None, "typed {value} was overwritten");
    }
    ed.draft_keep.as_mut().unwrap().max_age_days = 1825;
    assert_eq!(ed.days_writeback().as_deref(), Some("1825"));
}

/// Remaining on Trade Tape must keep polling after a post-write read sees a running purge.
#[test]
fn purge_polling_includes_the_trade_tape_page() {
    assert!(super::shows_tape_block(crate::settings::Tab::StTape));
    assert!(!super::shows_tape_block(crate::settings::Tab::TgBot));
}

/// Destructive writes that widen the tape window must disclose both deletion and requeue cost.
#[test]
fn combined_retention_and_window_edit_keeps_the_widen_warning() {
    let _locale = crate::test_locale::force("en");
    let mut probe = full_probe("draft", 860.0);
    probe.ed.draft.as_mut().unwrap().margin_s = 600;
    for confirm in [false, true] {
        probe.ed.confirm = confirm;
        let (text, warning) = super::keep::bar_message(&probe.ed, Some(&probe.c), false);
        assert!(
            warning && text.contains("75%") && text.contains("18920") && text.contains("about"),
            "{text}"
        );
        if !confirm {
            assert!(text.contains("delete"), "{text}");
        }
    }
}

/// A stored singular trade and Slavic counts ending in one must select the right noun forms.
#[test]
fn retention_counts_select_locale_plurals() {
    for (language, n, expected) in [
        ("en", 1, "tape for 1 trade"),
        ("ru", 21, "лента 21 сделки"),
        ("ru", 25, "лента 25 сделок"),
        ("uk", 21, "стрічка 21 угоди"),
    ] {
        let _locale = crate::test_locale::force(language);
        let mut c = catchup();
        c.tape_trades = Some(n);
        let tile = super::keep::db_tile(&c);
        let super::TileHint::Text(text) = tile.hint else {
            panic!("missing tape count");
        };
        assert_eq!(text, expected);
    }
}

/// Finishing retention while auto is off must not promise a pass that cannot start.
#[test]
fn completed_purge_with_auto_off_does_not_promise_catchup() {
    let _locale = crate::test_locale::force("en");
    let mut probe = full_probe("done", 860.0);
    probe.c.auto = Some(false);
    assert_eq!(catchup_state(Some(&probe.c)), CatchupState::Off);
    let text = super::keep::purge_sentence(&probe.c, chrono_tz::UTC).unwrap();
    assert!(text.contains("off") && text.contains("412"), "{text}");
    assert!(!text.contains("next pass"), "{text}");
}

/// Stale failures yield to newer passes, and a busy reader is a neutral compaction retry.
#[test]
fn deferred_compaction_is_pending_and_old_failures_expire() {
    use moon_core::station_api::{PurgeFail, TapePurge};
    let _locale = crate::test_locale::force("en");
    let mut probe = full_probe("done", 860.0);
    let at = probe.c.last_pass_ms.unwrap() + 1000;
    probe.c.purge = Some(TapePurge::Failed {
        at_ms: at,
        reason: PurgeFail::Deferred,
    });
    assert_ne!(catchup_state(Some(&probe.c)), CatchupState::PurgeFailed);
    assert!(
        super::keep::purge_sentence(&probe.c, chrono_tz::UTC)
            .unwrap()
            .contains("deferred")
    );
    probe.c.last_pass_ms = Some(at + 1);
    assert!(super::keep::purge_sentence(&probe.c, chrono_tz::UTC).is_none());
}

/// A later catch-up pass must not hide a failed deletion or a permanent compaction warning.
#[test]
fn review_purge_failures_remain_visible_after_catchup() {
    use super::keep::{KeepState, keep_state};
    use moon_core::station_api::{PurgeFail, TapePurge};
    let _locale = crate::test_locale::force("en");
    let mut probe = full_probe("done", 860.0);
    let at = probe.c.last_pass_ms.unwrap();
    for reason in [
        PurgeFail::DeleteFailed,
        PurgeFail::DeleteNoRoom,
        PurgeFail::NoRoom,
        PurgeFail::NotShrinkable,
        PurgeFail::Other,
    ] {
        probe.c.purge = Some(TapePurge::Failed { at_ms: at, reason });
        probe.c.last_pass_ms = Some(at + 1);
        assert_eq!(keep_state(Some(&probe.c)), KeepState::Failed { reason });
        assert!(super::keep::purge_sentence(&probe.c, chrono_tz::UTC).is_some());
    }
}

/// A full-disk rollback must never claim the tape was deleted or merely left uncompressed.
#[test]
fn disk_full_rollback_copy_reports_untouched_tape() {
    let _locale = crate::test_locale::force("ru");
    let probe = full_probe("delete_no_room", 860.0);
    assert_eq!(
        super::keep::purge_sentence(&probe.c, chrono_tz::UTC).unwrap(),
        "Чистка не удалась: нет места на диске, лента не тронута; станция повторит сама."
    );
}

/// A stale local Saved message would collapse the remote warning to one line and hide its cost.
#[gpui::test]
fn local_save_status_does_not_hide_remote_tape_warning(cx: &mut gpui::TestAppContext) {
    let _locale = crate::test_locale::force("en");
    cx.update(moon_ui::init);
    let window = cx.open_window(gpui::size(px(380.0), px(4000.0)), |_, _| {
        full_probe("widen", 380.0)
    });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    for _ in 0..3 {
        visual.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
    let text = visual.debug_bounds("server-tape-widen-warning").unwrap();
    assert!(
        f32::from(text.size.height) > 30.0,
        "warning was replaced with a short local status"
    );
}

/// Swapping the priority would name a job over dashes that have no window, or tell the user
/// an unsaved draft is the thing to do while the write button is already disabled.
#[test]
fn station_tape_status_picks_one_line() {
    let line = |known, dirty, busy, older_service| {
        tape_status(TapeFacts {
            known,
            dirty,
            busy,
            older_service,
        })
    };
    assert_eq!(line(true, false, false, false), TapeStatus::Clean);
    assert_eq!(line(true, true, false, false), TapeStatus::Draft);
    assert_eq!(line(true, true, true, false), TapeStatus::Busy);
    assert_eq!(line(false, true, true, false), TapeStatus::Down);
    assert_eq!(line(false, false, false, true), TapeStatus::Older);
    // A window that was read stays clean even if a stale older-service flag is still set.
    assert_eq!(line(true, false, false, true), TapeStatus::Clean);
}

/// Rewriting the approved Russian, or dropping a placeholder, puts the old "Set" button
/// or a raw `%{margin}` back in front of the user.
#[test]
fn station_tape_russian_copy_is_the_approved_wording() {
    let _locale = crate::test_locale::force("ru");
    let text = |key: &str| rust_i18n::t!(key).to_string();
    assert_eq!(text("telegram.server.keep_title"), "Что хранить");
    assert_eq!(
        text("telegram.server.keep_note_forever"),
        "Ничего не удаляется. Докачка в любом случае берёт не старше года: старше биржи и архивы не отдают."
    );
    assert_eq!(
        text("telegram.server.keep_bar_lengthen"),
        "Не сохранено. Ничего не удалится: срок стал длиннее."
    );
    assert_eq!(
        text("telegram.server.keep_delete_failed"),
        "Чистка не удалась, лента не тронута; станция повторит сама."
    );
    assert_eq!(
        text("telegram.server.keep_cpu_note"),
        "Докачка берёт не больше 75% процессора станции — бот, отчёты и запись вживую не тормозят."
    );
    assert_eq!(
        super::keep::keep_note(1825),
        "Лента сделок старше 5 лет удаляется, и дальше по мере старения. Докачка берёт не старше года."
    );
    assert_eq!(
        text("telegram.server.tape_hint"),
        "Станция круглосуточно записывает трейды рынка вокруг каждой сделки — даже когда терминал выключен. По этим записям тюнер потом проверяет варианты входа и выхода. Здесь задаётся, сколько записывать."
    );
    assert_eq!(
        text("telegram.server.tape_margin_hint"),
        "Сколько секунд рынка записать до входа и после выхода. Меньше 30 с нельзя — столько нужно тюнеру, чтобы судить выход."
    );
    assert_eq!(
        text("telegram.server.tape_long_hint"),
        "Сделка дольше этого пишется только по краям: вокруг входа и вокруг выхода. Середина не пишется — меньше места на диске, но тюнер не судит варианты, которые вышли бы в середине."
    );
    assert_eq!(text("telegram.server.tape_set"), "Записать на станцию");
    assert_eq!(
        text("telegram.server.tape_clean"),
        "Станция пишет с этими значениями."
    );
    assert_eq!(
        rust_i18n::t!(
            "telegram.server.tape_draft",
            margin = "30 с",
            long = "5 мин"
        )
        .as_ref(),
        "Не сохранено: сейчас станция пишет 30 с и 5 мин."
    );
    assert_eq!(
        rust_i18n::t!("telegram.server.tape_busy", job = "Обновить службу").as_ref(),
        "Идёт «Обновить службу» — кнопки вернутся, когда она закончится."
    );
    assert_eq!(
        rust_i18n::t!("telegram.server.tape_down", status = "Статус").as_ref(),
        "Станция не отвечает — её значения неизвестны. Проверьте, что она запущена («Статус»)."
    );
    assert_eq!(
        text("telegram.server.tape_footnote"),
        "У терминала свои такие же настройки для его собственной записи (Настройки → Хранилище → «Трейды сделок»). Станция взяла их один раз при установке, дальше меняется только здесь: к одной станции могут быть подключены несколько терминалов, и ни один не перепишет её молча."
    );
    assert_eq!(
        rust_i18n::t!("telegram.server.running", job = "Статус").as_ref(),
        "Выполняется: Статус…"
    );
    for (key, word) in [
        ("telegram.server.catchup_tag_done", "Всё докачано"),
        ("telegram.server.catchup_tag_running", "Идёт докачка"),
        ("telegram.server.catchup_tag_errors", "Есть ошибки"),
        ("telegram.server.catchup_tag_waiting", "Ждёт прохода"),
        ("telegram.server.catchup_tag_off", "Выключено"),
        ("telegram.server.catchup_tag_nodata", "Нет данных"),
    ] {
        assert_eq!(text(key), word);
    }
    assert_eq!(
        text("telegram.server.catchup_nodata"),
        "Эта станция не сообщает о докачке: она старой версии. Обновите станцию кнопкой выше."
    );
}

/// A synthetic catch-up report; every count distinct so a swapped tile shows.
fn catchup() -> TapeCatchup {
    TapeCatchup {
        running: false,
        auto: Some(true),
        archives: Some(true),
        pending: 0,
        filled: 41,
        no_route: 3,
        out_of_reach: 17,
        failed: 0,
        last_pass_ms: Some(1_700_000_000_000),
        archive_bytes: 0,
        failed_by: Vec::new(),
        ..TapeCatchup::default()
    }
}

/// Every reason at once: the longest breakdown a station can send.
/// Include each reported category so locale and wrapping fixtures exercise new failure labels.
fn every_reason() -> Vec<(CatchupFailReason, u32)> {
    [
        CatchupFailReason::Http,
        CatchupFailReason::RequestFault,
        CatchupFailReason::Network,
        CatchupFailReason::Throttled,
        CatchupFailReason::Corrupt,
        CatchupFailReason::ArchiveMissing,
        CatchupFailReason::NoRoom,
        CatchupFailReason::Other,
    ]
    .into_iter()
    .zip([90, 4, 12_345, 30, 7, 5, 2, 1])
    .collect()
}

/// The breakdown says why in plain words with each count, and says nothing when the station
/// sent no reasons (an older station) or nothing failed.
#[test]
fn station_catchup_breakdown_names_each_reason() {
    let _locale = crate::test_locale::force("ru");
    let failed = TapeCatchup {
        failed: 127,
        failed_by: vec![
            (CatchupFailReason::Http, 90),
            (CatchupFailReason::Throttled, 30),
            (CatchupFailReason::Corrupt, 7),
        ],
        ..catchup()
    };
    assert_eq!(
        catchup_breakdown(&failed).as_deref(),
        Some("Почему: биржа ответила ошибкой — 90 · лимит запросов — 30 · битые данные — 7")
    );
    let older = TapeCatchup {
        failed: 127,
        ..catchup()
    };
    assert_eq!(catchup_breakdown(&older), None);
    assert_eq!(catchup_breakdown(&catchup()), None);
    drop(_locale);
    for language in Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        let text = catchup_breakdown(&TapeCatchup {
            failed: 12,
            failed_by: every_reason(),
            ..catchup()
        })
        .unwrap();
        assert!(!text.contains("telegram."), "{text}");
    }
}

/// Reading a queue as "all caught up" or a running pass as an error tells the user the wrong
/// thing about trades the tuner will or will not judge.
#[test]
fn station_catchup_state_priority() {
    assert_eq!(catchup_state(None), CatchupState::NoData);
    let state = |edit: fn(&mut TapeCatchup)| {
        let mut c = catchup();
        edit(&mut c);
        catchup_state(Some(&c))
    };
    assert_eq!(state(|_| {}), CatchupState::Done);
    assert_eq!(state(|c| c.pending = 2), CatchupState::Waiting);
    assert_eq!(
        state(|c| {
            c.pending = 2;
            c.failed = 1;
        }),
        CatchupState::Errors
    );
    assert_eq!(
        state(|c| {
            c.failed = 1;
            c.running = true;
        }),
        CatchupState::Running
    );
    assert_eq!(
        state(|c| {
            c.running = true;
            c.auto = Some(false);
        }),
        CatchupState::Off
    );
    // A station predating the switches still catches up.
    assert_eq!(state(|c| c.auto = None), CatchupState::Done);
}

/// "Failed" replaces "No source" only while the last pass failed; off and unreported show none.
#[test]
fn station_catchup_tiles_swap_failed_for_no_route() {
    let c = catchup();
    let tiles = catchup_tiles(CatchupState::Done, &c);
    assert_eq!(
        tiles
            .iter()
            .map(|tile| tile.value.clone())
            .collect::<Vec<_>>(),
        [41, 0, 17, 3].map(TileValue::Count)
    );
    assert_eq!(tiles[3].label, "telegram.server.catchup_tile_no_route");
    let failed = TapeCatchup {
        failed: 5,
        ..c.clone()
    };
    let tiles = catchup_tiles(CatchupState::Errors, &failed);
    assert_eq!(tiles[3].value, TileValue::Count(5));
    assert_eq!(tiles[3].label, "telegram.server.catchup_tile_failed");
    assert!(catchup_tiles(CatchupState::Off, &c).is_empty());
    assert!(catchup_tiles(CatchupState::NoData, &c).is_empty());
}

/// The next check is the last pass plus an hour in the user's zone — never a rounded UTC hour —
/// and an error names no retry time it cannot keep.
#[test]
fn station_catchup_next_check_is_last_pass_plus_an_hour() {
    let _locale = crate::test_locale::force("ru");
    let zone: Tz = "Asia/Kolkata".parse().expect("zone");
    // 2023-11-14 22:13:20 UTC is 03:43 next day in UTC+05:30.
    let c = catchup();
    assert_eq!(clock(1_700_000_000_000, zone), "03:43");
    let done = catchup_sentence(CatchupState::Done, Some(&c), zone);
    assert!(done.contains("03:43"), "{done}");
    assert!(done.contains("04:43"), "{done}");
    let failed = TapeCatchup {
        failed: 2,
        ..c.clone()
    };
    let errors = catchup_sentence(CatchupState::Errors, Some(&failed), zone);
    assert_eq!(
        errors,
        "Не скачались 2 сделки за последний проход — что можно повторить, станция повторит сама, ничего делать не нужно."
    );
    let never = TapeCatchup {
        last_pass_ms: None,
        ..c
    };
    assert_eq!(
        catchup_sentence(CatchupState::Done, Some(&never), zone),
        rust_i18n::t!("telegram.server.catchup_done_never")
    );
}

/// An untouched switch follows the station, an edited one stays, and a station that predates the
/// switches is never sent them: its `[tape]` would refuse the file.
#[test]
fn station_catchup_switches_follow_and_reach_only_new_stations() {
    let on = CatchupSwitches {
        auto: true,
        archives: true,
    };
    let off = CatchupSwitches {
        auto: false,
        archives: true,
    };
    let (mut seen, mut draft) = (None, None);
    follow(&mut seen, &mut draft, Some(on));
    assert_eq!((seen, draft), (Some(on), Some(on)));
    follow(&mut seen, &mut draft, Some(off));
    assert_eq!(draft, Some(off), "an untouched draft follows");
    draft = Some(on);
    follow(
        &mut seen,
        &mut draft,
        Some(CatchupSwitches {
            archives: false,
            ..off
        }),
    );
    assert_eq!(draft, Some(on), "an edited draft stays");
    follow(&mut seen, &mut draft, None);
    assert_eq!(draft, Some(on), "a read without switches changes nothing");

    let old = TapeCatchup {
        auto: None,
        archives: None,
        ..catchup()
    };
    assert_eq!(reported_switches(&old), None);
    assert_eq!(job_switches(reported_switches(&old), Some(off)), None);
    assert_eq!(reported_switches(&catchup()), Some(on));
    assert_eq!(job_switches(Some(on), Some(off)), Some(off));
}

/// A station rolled back (or another, older station in the view) answers a status without
/// switches: the editor forgets the ones it saw, so "Write to the station" sends none it would
/// refuse. No status read at all keeps them.
#[test]
fn station_catchup_switches_forgotten_on_a_pre_switch_read() {
    let on = CatchupSwitches {
        auto: true,
        archives: true,
    };
    let mut ed = TapeEd::default();
    ed.take_switches(Some(Some(on)));
    ed.draft_switches = Some(CatchupSwitches { auto: false, ..on });
    ed.take_switches(None);
    assert!(job_switches(ed.seen_switches, ed.draft_switches).is_some());
    let old = TapeCatchup {
        auto: None,
        archives: None,
        ..catchup()
    };
    ed.take_switches(Some(reported_switches(&old)));
    assert_eq!(job_switches(ed.seen_switches, ed.draft_switches), None);
    assert_eq!(ed.draft_switches, None);
}

/// A count-dependent line agrees with its number: Slavic one/few/many, one/other elsewhere.
#[test]
fn station_counts_pick_their_plural_form() {
    let errors = |n: u32| {
        rust_i18n::t!(
            &plural_key("telegram.server.catchup_errors", n.into()),
            n = n
        )
        .to_string()
    };
    {
        let _locale = crate::test_locale::force("ru");
        assert!(
            errors(1).starts_with("Не скачалась 1 сделка "),
            "{}",
            errors(1)
        );
        assert!(
            errors(2).starts_with("Не скачались 2 сделки "),
            "{}",
            errors(2)
        );
        assert!(
            errors(5).starts_with("Не скачалось 5 сделок "),
            "{}",
            errors(5)
        );
        assert!(
            errors(11).starts_with("Не скачалось 11 сделок "),
            "{}",
            errors(11)
        );
        assert!(
            errors(21).starts_with("Не скачалась 21 сделка "),
            "{}",
            errors(21)
        );
    }
    let _locale = crate::test_locale::force("en");
    assert!(errors(1).starts_with("1 trade did "), "{}", errors(1));
    assert!(errors(2).starts_with("2 trades did "), "{}", errors(2));
}

/// The tape block's catch-up pieces, backend-free, the venue table open.
struct TapeProbe {
    catchup: Option<TapeCatchup>,
    width: f32,
}

impl Render for TapeProbe {
    /// Stack the strip, tiles, switches and venue table as the block does.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let catchup = self.catchup.as_ref();
        let state = catchup_state(catchup);
        let tiles = catchup.map(|c| catchup_tiles(state, c)).unwrap_or_default();
        let snapshot = rust_i18n::t!("telegram.server.catchup_snapshot", at = "23:59").to_string();
        let switches = CatchupSwitches {
            auto: state != CatchupState::Off,
            archives: true,
        };
        div()
            .w(px(self.width))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .debug_selector(|| "probe-col".into())
            .child(tape_status_strip(
                state,
                catchup_sentence(state, catchup, Tz::UTC),
                Some(snapshot),
                cx,
            ))
            .child(tape_tiles(&tiles, self.width, cx))
            .children(
                catchup
                    .filter(|_| !tiles.is_empty())
                    .and_then(catchup_breakdown)
                    .map(|text| tape_breakdown(text, cx)),
            )
            .child(catchup_switches(
                switches,
                true,
                |_, _, _| {},
                |_, _, _| {},
                cx,
            ))
            .child(venue_reach_table(
                true,
                &venue_caps::reach_table(),
                self.width,
                Rc::new(|_, _| {}),
                cx,
            ))
    }
}

/// `debug_bounds` takes a `'static` selector; the probe's selectors are built per run.
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// Every catch-up state stays inside the panel, no tile is narrower than its word (or the panel)
/// and the switch labels keep their width, in every language, at a wide and a narrow column and
/// both zooms, and at the narrowest Settings window with the venue table open.
#[gpui::test]
fn the_tape_catchup_block_fits_every_language_width_and_zoom(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    let states: [Option<TapeCatchup>; 6] = [
        None,
        Some(TapeCatchup {
            auto: Some(false),
            ..catchup()
        }),
        Some(TapeCatchup {
            running: true,
            pending: 123_456,
            filled: 987_654,
            ..catchup()
        }),
        Some(TapeCatchup {
            failed: 12,
            failed_by: every_reason(),
            ..catchup()
        }),
        Some(TapeCatchup {
            pending: 9,
            ..catchup()
        }),
        Some(catchup()),
    ];
    for language in Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        for state in &states {
            // The last case is the narrowest Settings window: 620 px at 200% zoom.
            for (zoom, column) in [
                (1.0, 860.0),
                (1.0, 420.0),
                (1.25, 860.0),
                (1.25, 420.0),
                (2.0, 244.0),
            ] {
                {
                    let width = column * zoom;
                    let window =
                        cx.open_window(gpui::size(px(width), px(4000.0)), |_, _| TapeProbe {
                            catchup: state.clone(),
                            width,
                        });
                    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                    visual.update(|window, cx| window.set_content_zoom(zoom, cx));
                    for _ in 0..3 {
                        visual.update(|window, cx| {
                            let _ = window.draw(cx);
                        });
                    }
                    let kind = catchup_state(state.as_ref());
                    let at = format!("{} {kind:?} zoom {zoom} column {column}", language.code());
                    let col = visual.debug_bounds("probe-col").expect("column laid out");
                    let inside = |b: gpui::Bounds<gpui::Pixels>, what: &str| {
                        assert!(
                            b.origin.x + px(0.5) >= col.origin.x
                                && b.right() <= col.right() + px(0.5),
                            "{what} escaped the panel ({at})"
                        );
                    };
                    for sel in [
                        "server-tape-strip",
                        "server-tape-sentence",
                        "server-tape-switches",
                        "server-tape-venues",
                        "server-tape-venue-table",
                    ] {
                        inside(visual.debug_bounds(sel).expect(sel), sel);
                    }
                    let tiles = state
                        .as_ref()
                        .map(|c| catchup_tiles(kind, c))
                        .unwrap_or_default();
                    for (index, tile) in tiles.iter().enumerate() {
                        let sel = format!("server-tape-tile-{index}");
                        let b = visual
                            .debug_bounds(leak(sel.clone()))
                            .unwrap_or_else(|| panic!("{sel} not laid out ({at})"));
                        inside(b, &sel);
                        let label = rust_i18n::t!(tile.label).to_string();
                        let need =
                            visual.update(|_, cx| design::ui_body_text_width(cx, &label, 400.0));
                        let word = visual
                            .debug_bounds(leak(format!("server-tape-tile-label-{index}")))
                            .expect("tile label laid out");
                        inside(word, &label);
                        assert!(
                            f32::from(b.size.width) + 0.5 >= need.min(f32::from(col.size.width)),
                            "tile {label} narrower than its word ({at})"
                        );
                    }
                    if let Some(text) = state.as_ref().and_then(catchup_breakdown) {
                        let b = visual
                            .debug_bounds("server-tape-breakdown")
                            .unwrap_or_else(|| panic!("breakdown not laid out ({at})"));
                        inside(b, "breakdown");
                        let row = visual.debug_bounds("server-tape-tiles").expect("tiles");
                        let next = visual
                            .debug_bounds("server-tape-switches")
                            .expect("switches");
                        assert!(
                            b.origin.y + px(0.5) >= row.bottom()
                                && next.origin.y + px(0.5) >= b.bottom(),
                            "the breakdown overlaps the tiles or the switches ({at})"
                        );
                        // Wrapped, never clipped: the box holds at least one full line per
                        // panel width the text needs.
                        let need = visual.update(|_, cx| {
                            design::ui_body_text_width(cx, &text, 400.0)
                                * f32::from(design::t_caption(cx))
                                / f32::from(design::t_body(cx))
                        });
                        let lines = (need / f32::from(b.size.width)).floor();
                        let line = visual.update(|_, cx| f32::from(design::t_caption(cx)));
                        assert!(
                            f32::from(b.size.height) + 0.5 >= lines.max(1.0) * line,
                            "the breakdown is clipped ({at})"
                        );
                    }
                    if !tiles.is_empty() {
                        let row = visual
                            .debug_bounds("server-tape-tiles")
                            .expect("tiles row laid out");
                        let next = visual
                            .debug_bounds("server-tape-switches")
                            .expect("switches laid out");
                        assert!(
                            next.origin.y + px(0.5) >= row.bottom(),
                            "the row below overlaps the tiles ({at})"
                        );
                        let boxes: Vec<_> = (0..tiles.len())
                            .map(|index| {
                                visual
                                    .debug_bounds(leak(format!("server-tape-tile-{index}")))
                                    .expect("tile laid out")
                            })
                            .collect();
                        for (index, b) in boxes.iter().enumerate() {
                            assert!(
                                b.origin.y + px(0.5) >= row.origin.y
                                    && b.bottom() <= row.bottom() + px(0.5),
                                "tile {index} spills out of its row ({at})"
                            );
                            for other in &boxes {
                                if (f32::from(other.origin.y) - f32::from(b.origin.y)).abs() < 0.5 {
                                    continue;
                                }
                                let overlap = other.origin.y < b.bottom()
                                    && b.origin.y < other.bottom()
                                    && other.origin.x < b.right()
                                    && b.origin.x < other.right();
                                assert!(!overlap, "tiles overlap ({at})");
                            }
                        }
                        let first = boxes[0];
                        for b in &boxes {
                            assert!(
                                (f32::from(b.size.width) - f32::from(first.size.width)).abs() < 1.0,
                                "tiles differ in width ({at})"
                            );
                        }
                        for (index, b) in boxes.iter().enumerate() {
                            for other in &boxes {
                                let same_row =
                                    other.origin.y < b.bottom() && b.origin.y < other.bottom();
                                if same_row {
                                    assert!(
                                        (f32::from(other.origin.y) - f32::from(b.origin.y)).abs()
                                            < 0.5
                                            && (f32::from(other.size.height)
                                                - f32::from(b.size.height))
                                            .abs()
                                                < 0.5,
                                        "tile {index} is staggered in its row ({at})"
                                    );
                                }
                            }
                        }
                        if column >= 860.0 {
                            let per_row = boxes
                                .iter()
                                .filter(|b| {
                                    (f32::from(b.origin.y) - f32::from(first.origin.y)).abs() < 0.5
                                })
                                .count();
                            assert!(per_row >= 2, "a wide column stacks the tiles ({at})");
                        }
                    }
                    for index in 0..venue_caps::reach_table().len() {
                        if let Some(b) =
                            visual.debug_bounds(leak(format!("server-tape-venue-{index}")))
                        {
                            inside(b, "stacked venue row");
                        }
                    }
                    let switches = visual
                        .debug_bounds("server-tape-switches")
                        .expect("switches laid out");
                    let floor = visual.update(|_, cx| f32::from(design::font_w_px(cx, 200.0)));
                    assert!(
                        f32::from(switches.size.width) + 0.5 >= floor.min(width - 1.0),
                        "switch labels collapsed ({at})"
                    );
                }
            }
        }
    }
}

/// The tape block lives on the Station page, so its open state alone keeps a running catch-up
/// re-read; the Telegram page no longer shows the numbers it would refresh.
#[test]
fn running_catchup_rereads_while_the_station_page_is_open() {
    use crate::settings::Tab;
    assert!(super::shows_tape_block(Tab::StOverview));
    assert!(!super::shows_tape_block(Tab::TgBot));
}

/// A typed long-position threshold inside the range stays as typed; one outside it is clamped
/// and the field is marked for a write-back of the clamped number.
#[test]
fn an_out_of_range_long_position_is_written_back_clamped() {
    use super::long_shown_after;
    use moon_core::config::storage::clamp_long_position_min;
    assert_eq!(long_shown_after(15, clamp_long_position_min(15)), Some(15));
    assert_eq!(long_shown_after(500, clamp_long_position_min(500)), None);
    assert_eq!(long_shown_after(0, clamp_long_position_min(0)), None);
}

/// A legacy capability read must erase edits and target-bound confirmation instead of sending
/// unsupported TOML keys to a station that rejects them.
#[test]
fn keep_reaches_only_new_stations() {
    use super::keep::{job_keep, reported_keep};
    use moon_core::station_api::TapeKeep;
    let keep = TapeKeep {
        max_age_days: 1825,
        record_emulator: true,
    };
    let mut ed = TapeEd::default();
    ed.take_keep(Some(Some(keep)));
    ed.draft_keep = Some(TapeKeep {
        max_age_days: 30,
        ..keep
    });
    ed.confirm = true;
    ed.take_keep(None);
    assert_eq!(
        job_keep(ed.seen_keep, ed.draft_keep).unwrap().max_age_days,
        30
    );
    let old = catchup();
    assert_eq!(reported_keep(&old), None);
    ed.take_keep(Some(reported_keep(&old)));
    assert_eq!(job_keep(ed.seen_keep, ed.draft_keep), None);
    assert!(!ed.confirm);
    ed.take_keep(Some(Some(keep)));
    let narrowed = TapeKeep {
        max_age_days: 30,
        ..keep
    };
    ed.draft_keep = Some(narrowed);
    ed.confirm = true;
    ed.take_keep(Some(Some(narrowed)));
    assert!(
        !ed.confirm,
        "another terminal already acknowledged the same policy"
    );
    assert_eq!(
        reported_keep(&TapeCatchup {
            max_age_days: Some(1825),
            record_emulator: None,
            ..old
        }),
        None
    );
}

/// Emulator trades have one category even when old; unknown histograms never read as zero.
#[test]
fn purge_counts_from_station_ages() {
    use super::keep::{PurgeCounts, purge_counts};
    use moon_core::station_api::{TapeAge, TapeKeep};
    let c = TapeCatchup {
        tape_trades: Some(30),
        tape_ages: vec![
            TapeAge {
                days: 10,
                plain: 3,
                emulator: 4,
            },
            TapeAge {
                days: 31,
                plain: 10,
                emulator: 13,
            },
        ],
        ..catchup()
    };
    let keep = TapeKeep {
        max_age_days: 30,
        record_emulator: false,
    };
    assert_eq!(
        purge_counts(&c, keep),
        Some(PurgeCounts {
            older: 10,
            emulator: 17,
            total: 27
        })
    );
    assert_eq!(
        purge_counts(
            &c,
            TapeKeep {
                max_age_days: 0,
                ..keep
            }
        )
        .unwrap()
        .total,
        17
    );
    assert_eq!(
        purge_counts(
            &TapeCatchup {
                tape_ages: Vec::new(),
                ..c.clone()
            },
            keep
        ),
        None
    );
    assert_eq!(
        purge_counts(
            &TapeCatchup {
                tape_trades: None,
                ..c
            },
            keep
        ),
        None
    );
}

/// Extending a period, including forever, must dispatch directly; emulator removal still asks.
#[test]
fn lengthening_writes_without_confirmation() {
    use crate::backend::station::job::Job;
    use moon_core::station_api::{TapeKeep, TapeWindow};
    use moon_remote::ssh::Target;
    let target = Target {
        host: "CORE_A".into(),
        port: 22,
    };
    for days in [365, 1825, 0] {
        let mut ed = TapeEd {
            seen_keep: Some(TapeKeep {
                max_age_days: 180,
                record_emulator: true,
            }),
            draft_keep: Some(TapeKeep {
                max_age_days: days,
                record_emulator: true,
            }),
            draft: Some(TapeWindow {
                margin_s: 60,
                long_position_min: 120,
            }),
            ..TapeEd::default()
        };
        let Some(Job::Tape {
            keep: Some(keep), ..
        }) = ed.prepare_write(target.clone(), false, None)
        else {
            panic!("lengthening must write");
        };
        assert_eq!(keep.max_age_days, days);
        assert!(!ed.confirm);
        assert!(ed.awaiting_keep.is_none());
    }
}

/// A larger margin or long threshold warns, a smaller window does not, and an unknown rate
/// omits the timing clause rather than presenting a made-up completion time.
#[test]
fn widen_warning_only_when_the_window_grows() {
    use super::keep::widen_warning;
    use moon_core::station_api::TapeWindow;
    let _locale = crate::test_locale::force("en");
    let seen = TapeWindow {
        margin_s: 60,
        long_position_min: 120,
    };
    assert!(
        widen_warning(
            seen,
            TapeWindow {
                margin_s: 30,
                long_position_min: 60
            },
            Some(100),
            Some(10)
        )
        .is_none()
    );
    let widened = TapeWindow {
        margin_s: 600,
        ..seen
    };
    let no_rate = widen_warning(seen, widened, Some(100), None).unwrap();
    assert!(
        no_rate.contains("up to 100") && !no_rate.contains("about"),
        "{no_rate}"
    );
    let rate = widen_warning(seen, widened, Some(100), Some(10)).unwrap();
    assert!(rate.contains("about 10h"), "{rate}");
    assert!(
        widen_warning(
            seen,
            TapeWindow {
                long_position_min: 240,
                ..seen
            },
            Some(100),
            Some(0)
        )
        .is_some()
    );
}

/// The fixture renders the production builder, including its real keep, confirmation, disk,
/// switch and reach controls; no Backend, service or persisted data is constructed.
struct FullTapeProbe {
    ed: TapeEd,
    c: TapeCatchup,
    width: f32,
    sent: Vec<crate::backend::station::job::Job>,
}

impl Render for FullTapeProbe {
    /// Feed the exact production composition with synthetic reports and capture writes locally.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use super::block::{Actions, Controls, Snapshot, server_tape_block};
        use crate::settings::frame::{RemoteBar, SaveBar, save_bar};
        use moon_ui::{MoonButtonSize, MoonButtonVariant, MoonDropdown};
        if self.ed.days_input.is_none() {
            self.ed.days_input = Some(crate::settings::common::number_state(1825, window, cx));
            self.ed.long_input = Some(crate::settings::common::number_state(120, window, cx));
        }
        let days = cx.entity();
        let emu = days.clone();
        let yes = days.clone();
        let cancel = days.clone();
        let apply = days.clone();
        let actions = Actions {
            days: Rc::new(move |n, _, app| {
                days.update(app, |this, cx| {
                    if let Some(keep) = this.ed.draft_keep.as_mut() {
                        keep.max_age_days = n;
                    }
                    cx.notify();
                })
            }),
            emulator: Rc::new(move |on, _, app| {
                emu.update(app, |this, cx| {
                    if let Some(keep) = this.ed.draft_keep.as_mut() {
                        keep.record_emulator = on;
                    }
                    cx.notify();
                })
            }),
            confirm: Rc::new(move |_, app| {
                yes.update(app, |this, cx| {
                    if let Some(job) = this.ed.prepare_write(
                        moon_remote::ssh::Target {
                            host: "CORE_A".into(),
                            port: 22,
                        },
                        true,
                        this.c.purge,
                    ) {
                        this.sent.push(job);
                    }
                    cx.notify();
                })
            }),
            cancel: Rc::new(move |_, app| {
                cancel.update(app, |this, cx| {
                    this.ed.confirm = false;
                    cx.notify();
                })
            }),
        };
        let locked = self.ed.confirm
            || self.ed.awaiting_keep.is_some()
            || matches!(
                self.c.purge,
                Some(moon_core::station_api::TapePurge::Running { .. })
            );
        let margin = MoonDropdown::new("server-tape-margin")
            .label(crate::settings::SettingsView::trades_margin_label(
                self.ed.draft.map_or(60, |d| d.margin_s),
            ))
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::density(cx))
            .trigger_width(design::font_w(cx, 120.0))
            .disabled(locked)
            .into_any_element();
        let long = super::block::long_control(self.ed.long_input.as_ref().unwrap(), !locked, cx);
        let switches = catchup_switches(
            self.ed.draft_switches.unwrap(),
            !locked,
            |_, _, _| {},
            |_, _, _| {},
            cx,
        );
        let venues = venue_reach_table(
            true,
            &venue_caps::reach_table(),
            self.width,
            Rc::new(|_, _| {}),
            cx,
        );
        let block = server_tape_block(
            &self.ed,
            Snapshot {
                catchup: Some(&self.c),
                busy: false,
                width: self.width,
                read_at: None,
            },
            Controls {
                margin,
                long,
                switches: Some(switches),
                venues,
            },
            actions,
            cx,
        );
        let changed = self.ed.draft != self.ed.seen
            || self.ed.draft_keep != self.ed.seen_keep
            || self.ed.draft_switches != self.ed.seen_switches;
        let (msg, warning) = super::keep::bar_message(&self.ed, Some(&self.c), false);
        let bar = save_bar(
            SaveBar::Remote(RemoteBar {
                tone: warning.then_some(moon_ui::MoonTone::Warning),
                msg,
                dirty: changed && !locked,
                apply: Rc::new(move |_, app| {
                    apply.update(app, |this, cx| {
                        if let Some(job) = this.ed.prepare_write(
                            moon_remote::ssh::Target {
                                host: "CORE_A".into(),
                                port: 22,
                            },
                            false,
                            this.c.purge,
                        ) {
                            this.sent.push(job);
                        }
                        cx.notify();
                    })
                }),
                revert: None,
                label: rust_i18n::t!("telegram.server.tape_set").to_string(),
            }),
            Some(("Saved".into(), false)),
            Rc::new(|_, _| {}),
            Rc::new(|_, _| {}),
            cx,
        );
        div()
            .w(px(self.width))
            .flex()
            .flex_col()
            .debug_selector(|| "probe-col".into())
            .child(block)
            .children(bar)
    }
}

/// Reports independently define all acceptance states, including every maintenance failure.
fn full_probe(state: &str, width: f32) -> FullTapeProbe {
    use moon_core::station_api::{PurgeFail, TapeAge, TapeKeep, TapePurge, TapeWindow};
    let window = TapeWindow {
        margin_s: 60,
        long_position_min: 120,
    };
    let keep = TapeKeep {
        max_age_days: 1825,
        record_emulator: true,
    };
    let mut ed = TapeEd {
        seen: Some(window),
        draft: Some(window),
        seen_keep: Some(keep),
        draft_keep: Some(keep),
        seen_switches: Some(CatchupSwitches {
            auto: true,
            archives: true,
        }),
        draft_switches: Some(CatchupSwitches {
            auto: true,
            archives: true,
        }),
        ..TapeEd::default()
    };
    let mut c = TapeCatchup {
        max_age_days: Some(1825),
        record_emulator: Some(true),
        db_bytes: Some(696_254_464),
        tape_trades: Some(18_920),
        tape_ages: vec![TapeAge {
            days: 200,
            plain: 4880,
            emulator: 1330,
        }],
        catchup_rate_per_hour: Some(800),
        ..catchup()
    };
    match state {
        "draft" | "confirm" | "confirm_unknown" => {
            ed.draft_keep = Some(TapeKeep {
                max_age_days: 180,
                record_emulator: false,
            });
            ed.confirm = state != "draft";
            if state == "confirm_unknown" {
                c.tape_ages.clear();
            }
        }
        "forever" => {
            ed.draft_keep = Some(TapeKeep {
                max_age_days: 0,
                ..keep
            })
        }
        "custom" => {
            ed.draft_keep = Some(TapeKeep {
                max_age_days: 1234,
                ..keep
            })
        }
        "purging" => {
            c.purge = Some(TapePurge::Running {
                done: 42,
                total: 100,
            })
        }
        "done" => {
            c.purge = Some(TapePurge::Done {
                at_ms: c.last_pass_ms.unwrap() + 1000,
                freed_bytes: 433_061_888,
            });
            c.pending = 412;
        }
        "failed" | "delete_failed" | "delete_no_room" | "deferred" | "not_shrinkable" | "other" => {
            c.purge = Some(TapePurge::Failed {
                at_ms: c.last_pass_ms.unwrap() + 1000,
                reason: match state {
                    "delete_failed" => PurgeFail::DeleteFailed,
                    "delete_no_room" => PurgeFail::DeleteNoRoom,
                    "deferred" => PurgeFail::Deferred,
                    "not_shrinkable" => PurgeFail::NotShrinkable,
                    "other" => PurgeFail::Other,
                    _ => PurgeFail::NoRoom,
                },
            })
        }
        "old" => {
            c.max_age_days = None;
            c.record_emulator = None;
            c.db_bytes = None;
            c.tape_trades = None;
            ed.forget_keep();
        }
        "widen" | "widen_unknown_rate" => {
            ed.draft = Some(TapeWindow {
                margin_s: 600,
                ..window
            });
            if state == "widen_unknown_rate" {
                c.catchup_rate_per_hour = None;
            }
        }
        _ => {}
    }
    FullTapeProbe {
        ed,
        c,
        width,
        sent: Vec::new(),
    }
}

/// Dropping hint spacing, compact sizing or prose wrapping breaks the real page at narrow zooms.
#[gpui::test]
fn the_tape_keep_block_fits_every_language_width_and_zoom(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    for language in Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        for state in [
            "clean",
            "draft",
            "confirm",
            "confirm_unknown",
            "forever",
            "custom",
            "purging",
            "done",
            "failed",
            "delete_failed",
            "delete_no_room",
            "deferred",
            "not_shrinkable",
            "other",
            "old",
            "widen",
            "widen_unknown_rate",
        ] {
            for (zoom, column) in [
                (1.0, 860.0),
                (1.0, 420.0),
                (1.25, 860.0),
                (1.25, 420.0),
                (2.0, 244.0),
            ] {
                let width = zoom * column;
                let window = cx.open_window(gpui::size(px(width), px(12000.0)), |_, _| {
                    full_probe(state, width)
                });
                let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                visual.update(|window, cx| window.set_content_zoom(zoom, cx));
                for _ in 0..3 {
                    visual.update(|window, cx| {
                        let _ = window.draw(cx);
                    });
                }
                let col = visual.debug_bounds("probe-col").unwrap();
                let at = format!("{} {state} {zoom} {column}", language.code());
                let hint = visual
                    .debug_bounds("server-tape-hint")
                    .expect("header hint");
                let margin = visual
                    .debug_bounds("server-tape-margin-row")
                    .expect("first row");
                // The page's 10-unit row gap plus the hint's normal 8-unit block spacing.
                let gap = visual.update(|_, cx| design::ui_value(cx, 18.0));
                assert!(
                    margin.origin.y >= hint.bottom() + px(gap - 0.5),
                    "header hint touches the first row {at}"
                );
                let long = visual
                    .debug_bounds("server-tape-long")
                    .expect("minutes field");
                let compact = visual
                    .debug_bounds("server-tape-long-compact")
                    .expect("compact class");
                let (limit, height) = visual.update(|_, cx| {
                    let range = moon_core::config::storage::LONG_POSITION_MIN_RANGE;
                    let range = f64::from(*range.start())..=f64::from(*range.end());
                    (
                        crate::settings::common::num_field_min_w(
                            &range,
                            Some(&rust_i18n::t!("storage.unit_min")),
                            cx,
                        ),
                        // MoonNumberInput's outline adds one border pixel on each side.
                        design::ui_value(cx, design::CONTROL_TIER.control_metrics().height) + 2.0,
                    )
                });
                assert!(
                    long.size.width <= px(limit + 0.5) && long.size.height <= px(height + 0.5),
                    "minutes field {:?} exceeds compact {limit} x {height} {at}",
                    long.size
                );
                assert!(
                    long.right() <= compact.right() + px(0.5),
                    "minutes field escaped {at}"
                );
                for sel in [
                    "server-tape-block",
                    "server-tape-keep",
                    "server-tape-days-chips",
                    "server-tape-days-note",
                    "server-tape-strip",
                    "server-tape-sentence",
                    "server-tape-tile-db",
                    "server-tape-switches",
                    "server-tape-auto-caption",
                    "server-tape-emu-caption",
                ] {
                    let b = visual
                        .debug_bounds(sel)
                        .unwrap_or_else(|| panic!("missing {sel}: {at}"));
                    assert!(
                        b.origin.x + px(0.5) >= col.origin.x && b.right() <= col.right() + px(0.5),
                        "{sel} escaped {at}"
                    );
                    assert!(
                        b.origin.y >= col.origin.y && b.bottom() <= col.bottom() + px(0.5),
                        "{sel} clipped {at}"
                    );
                }
                for sel in [
                    "server-tape-confirm",
                    "server-tape-purge-progress",
                    "server-tape-keep-old",
                    "server-tape-widen-warning",
                ] {
                    let b = visual.debug_bounds(sel);
                    let expected = match sel {
                        "server-tape-confirm" => matches!(state, "confirm" | "confirm_unknown"),
                        "server-tape-purge-progress" => state == "purging",
                        "server-tape-keep-old" => state == "old",
                        "server-tape-widen-warning" => {
                            matches!(state, "widen" | "widen_unknown_rate")
                        }
                        _ => unreachable!(),
                    };
                    assert_eq!(b.is_some(), expected, "{sel} state visibility {at}");
                    if let Some(b) = b {
                        assert!(b.right() <= col.right() + px(0.5), "{sel} escaped {at}");
                    }
                }
                for (sel, key) in [
                    ("server-tape-hint", "telegram.server.tape_hint"),
                    ("server-tape-auto-caption", "telegram.server.keep_cpu_note"),
                    (
                        "server-tape-emu-caption",
                        "telegram.server.keep_emu_caption",
                    ),
                ] {
                    let b = visual.debug_bounds(sel).expect("caption rendered");
                    let need = visual.update(|_, cx| {
                        design::ui_caption_text_width(cx, &rust_i18n::t!(key), 400.0)
                    });
                    let line = visual.update(|_, cx| f32::from(design::t_caption(cx)));
                    let lines = (need / f32::from(b.size.width)).floor().max(1.0);
                    assert!(
                        f32::from(b.size.height) + 0.5 >= lines * line,
                        "{sel} text clipped {at}"
                    );
                }
                if state == "old" {
                    let note = visual
                        .debug_bounds("server-tape-keep-old")
                        .expect("old station note");
                    let next = visual
                        .debug_bounds("server-tape-strip")
                        .expect("next section");
                    let (need, line) = visual.update(|_, cx| {
                        (
                            design::ui_caption_text_width(
                                cx,
                                &rust_i18n::t!("telegram.server.keep_old"),
                                400.0,
                            ),
                            f32::from(design::t_caption(cx)),
                        )
                    });
                    assert!(
                        note.origin.x >= col.origin.x && note.right() <= col.right() + px(0.5),
                        "old station note escaped {at}"
                    );
                    assert!(
                        note.bottom() <= next.origin.y,
                        "old station note overlaps the next section {at}"
                    );
                    assert!(
                        f32::from(note.size.height) + 0.5
                            >= (need / f32::from(note.size.width)).floor().max(1.0) * line,
                        "old station note clipped {at}"
                    );
                }
                let db = visual.debug_bounds("server-tape-tile-db").unwrap();
                let first = visual
                    .debug_bounds("server-tape-tile-0")
                    .expect("first count tile");
                if language == Language::En && column == 860.0 {
                    assert!(
                        (f32::from(db.origin.y) - f32::from(first.origin.y)).abs() < 0.5,
                        "five tiles should share the wide row {at}"
                    );
                }
                if column == 420.0 {
                    let tiles = visual.debug_bounds("server-tape-tiles").expect("tile grid");
                    assert!(
                        (f32::from(db.size.width) - f32::from(tiles.size.width)).abs() < 1.0,
                        "disk tile must span the narrow row {at}"
                    );
                }
                let label = visual.update(|_, cx| {
                    design::ui_caption_text_width(
                        cx,
                        &rust_i18n::t!("telegram.server.keep_db"),
                        400.0,
                    )
                });
                assert!(
                    f32::from(db.size.width) + 0.5 >= label.min(f32::from(col.size.width)),
                    "DB label clipped {at}"
                );
            }
        }
    }
}

/// The real buttons must ask first, preserve the draft on Cancel, then dispatch both keep keys.
#[gpui::test]
fn narrowing_write_opens_confirm_cancel_closes_confirm_sends_keep(cx: &mut gpui::TestAppContext) {
    use crate::backend::station::job::Job;
    let _locale = crate::test_locale::force("en");
    cx.update(moon_ui::init);
    let window = cx.open_window(gpui::size(px(860.0), px(4000.0)), |_, _| {
        full_probe("draft", 860.0)
    });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    for name in [
        "settings-remote-apply",
        "server-tape-confirm-cancel",
        "settings-remote-apply",
        "server-tape-confirm-yes",
    ] {
        for _ in 0..3 {
            visual.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
        let button = visual
            .debug_bounds(name)
            .unwrap_or_else(|| panic!("missing {name}"));
        visual.simulate_click(button.center(), gpui::Modifiers::none());
        window
            .update(&mut visual, |probe, _, _| {
                if name == "server-tape-confirm-yes" {
                    assert_eq!(probe.sent.len(), 1);
                    let Job::Tape {
                        keep: Some(keep),
                        target,
                        ..
                    } = &probe.sent[0]
                    else {
                        panic!("wrong job");
                    };
                    assert_eq!((keep.max_age_days, keep.record_emulator), (180, false));
                    assert_eq!(target.host, "CORE_A");
                    assert!(!probe.ed.confirm);
                    assert!(probe.ed.awaiting_keep.is_some());
                    probe.ed.forget_keep();
                    assert!(
                        probe.ed.days_input.is_none(),
                        "the old target's custom field must be discarded"
                    );
                    assert!(probe.ed.draft_keep.is_none() && probe.ed.awaiting_keep.is_none());
                } else {
                    assert!(probe.sent.is_empty());
                    assert_eq!(probe.ed.confirm, name == "settings-remote-apply");
                }
            })
            .unwrap();
    }
}
