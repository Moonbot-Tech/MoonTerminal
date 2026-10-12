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
    CatchupState, TapeEd, TapeFacts, TapeStatus, catchup_breakdown, catchup_sentence,
    catchup_state, catchup_switches, catchup_tiles, clock, follow, job_switches, plural_key,
    reported_switches, tape_breakdown, tape_status, tape_status_strip, tape_tiles,
    venue_reach_table,
};
use crate::design;

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
    }
}

/// Every reason at once: the longest breakdown a station can send.
fn every_reason() -> Vec<(CatchupFailReason, u32)> {
    [
        CatchupFailReason::Http,
        CatchupFailReason::Network,
        CatchupFailReason::Throttled,
        CatchupFailReason::Corrupt,
        CatchupFailReason::ArchiveMissing,
        CatchupFailReason::NoRoom,
        CatchupFailReason::Other,
    ]
    .into_iter()
    .zip([90, 12_345, 30, 7, 5, 2, 1])
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
        tiles.iter().map(|tile| tile.value).collect::<Vec<_>>(),
        [41, 0, 17, 3]
    );
    assert_eq!(tiles[3].label, "telegram.server.catchup_tile_no_route");
    let failed = TapeCatchup {
        failed: 5,
        ..c.clone()
    };
    let tiles = catchup_tiles(CatchupState::Errors, &failed);
    assert_eq!(tiles[3].value, 5);
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
