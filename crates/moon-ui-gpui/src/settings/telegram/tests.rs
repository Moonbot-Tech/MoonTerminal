//! Which bot the Telegram pages edit and how each page saves.

use super::access::ChatsOf;
use super::{bot_host, host_words};
use crate::settings::{SaveMode, Tab};

/// One UI serves both hosts: a known station makes the station's bot the edited one, else the
/// terminal's.
#[test]
fn the_edited_bot_is_the_stations_once_one_is_known() {
    assert_eq!(bot_host(false), ChatsOf::Terminal);
    assert_eq!(bot_host(true), ChatsOf::Station);
}

/// Bot pages save with the window's Save on the terminal and on the station by its own bar;
/// notifications apply at once on the terminal; the core's and station's pages always remote.
#[test]
fn bot_pages_save_where_the_bot_runs() {
    for (tab, local, station) in [
        (Tab::TgBot, SaveMode::Draft, SaveMode::Remote),
        (Tab::TgMessages, SaveMode::Draft, SaveMode::Remote),
        (Tab::TgNotify, SaveMode::Live, SaveMode::Remote),
        (Tab::TgCore, SaveMode::Remote, SaveMode::Remote),
        (Tab::StOverview, SaveMode::Remote, SaveMode::Remote),
        (Tab::StCores, SaveMode::Remote, SaveMode::Remote),
        (Tab::StTape, SaveMode::Remote, SaveMode::Remote),
        (Tab::StMaint, SaveMode::Remote, SaveMode::Remote),
    ] {
        assert_eq!(tab.save_mode(false), local, "{tab:?} without a station");
        assert_eq!(tab.save_mode(true), station, "{tab:?} with a station");
    }
}

/// The host line names the host and counts the chats in the plural the number takes.
#[test]
fn the_host_line_names_the_host_and_counts_chats() {
    let _locale = crate::test_locale::force("ru");
    assert_eq!(
        host_words(ChatsOf::Terminal, 1),
        "Бот работает в этом терминале · 1 чат"
    );
    assert_eq!(
        host_words(ChatsOf::Station, 3),
        "Бот работает на станции · 3 чата"
    );
    assert_eq!(
        host_words(ChatsOf::Station, 5),
        "Бот работает на станции · 5 чатов"
    );
}

/// A probe host drawing whatever `build` returns inside the body box.
pub(super) struct Host {
    pub(super) width: f32,
    pub(super) scroll: gpui::ScrollHandle,
    pub(super) build: std::rc::Rc<dyn Fn(&mut gpui::Window, &mut gpui::App) -> gpui::AnyElement>,
}

impl gpui::Render for Host {
    fn render(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{InteractiveElement, ParentElement, StatefulInteractiveElement, Styled};
        let content = (self.build)(window, cx);
        gpui::div()
            .w(gpui::px(self.width))
            .h_full()
            .debug_selector(|| "probe-body".into())
            .child(
                gpui::div()
                    .id("probe-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(gpui::div().w_full().flex_none().child(content)),
            )
    }
}

impl crate::settings::layout_probe::Probed for Host {
    fn scroll(&self) -> Option<gpui::ScrollHandle> {
        Some(self.scroll.clone())
    }
}

/// Synthetic chats: an owner, a viewer, one that sees nothing yet.
const CHATS: [(i64, &str, bool); 3] = [
    (100001, "Synthetic owner", false),
    (100002, "Synthetic viewer group with a long name", false),
    (100003, "Synthetic new chat", true),
];

/// The Bot page's host line and chats table keep every role and button inside the body and
/// apart, in every language at the default and the minimum window, on both hosts.
#[gpui::test]
fn the_bot_page_table_fits_the_body(cx: &mut gpui::TestAppContext) {
    use crate::settings::layout_probe::{ProbeSpec, sweep};
    use gpui::{InteractiveElement, IntoElement, ParentElement, Styled};
    for side in [ChatsOf::Terminal, ChatsOf::Station] {
        sweep(
            cx,
            move |_, _, (width, _)| Host {
                width,
                scroll: gpui::ScrollHandle::new(),
                build: std::rc::Rc::new(move |_, cx| {
                    let mut table = super::access::chat_table(side, true, cx);
                    for (chat, title, red) in CHATS {
                        let summary = if red {
                            rust_i18n::t!("telegram.access_waiting").to_string()
                        } else {
                            rust_i18n::t!("telegram.access_owner_summary").to_string()
                        };
                        let toggle = moon_ui::MoonButton::new(("probe-edit", chat as u64))
                            .ghost()
                            .label(rust_i18n::t!("telegram.chats.configure").to_string())
                            .render()
                            .into_any_element();
                        table = table.child(super::access::chat_row(
                            chat,
                            title.to_string(),
                            (summary, red),
                            toggle,
                            width,
                            cx,
                        ));
                    }
                    gpui::div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(gpui::px(12.0))
                        .child(gpui::div().debug_selector(|| "probe-host".into()).child(
                            moon_ui::MoonAlert::success(
                                "probe-host-line",
                                host_words(side, CHATS.len()),
                            ),
                        ))
                        .child(table)
                        .into_any_element()
                }),
            },
            |_| {
                let mut inside = vec!["probe-host".to_string(), "tg-chat-table".to_string()];
                for (chat, _, _) in CHATS {
                    inside.push(format!("tg-chat-role-{chat}"));
                    inside.push(format!("tg-chat-act-{chat}"));
                }
                ProbeSpec {
                    container: "probe-body",
                    no_overlap: CHATS
                        .iter()
                        .map(|(chat, _, _)| {
                            vec![
                                format!("tg-chat-role-{chat}"),
                                format!("tg-chat-act-{chat}"),
                            ]
                        })
                        .collect(),
                    inside: inside.into_iter().collect(),
                    last: Some("tg-chat-table".into()),
                    ..Default::default()
                }
            },
        );
    }
}

/// A synthetic card: a group box under a title with `n` lines of long text.
fn synthetic_card(tag: &'static str, n: usize, cx: &gpui::App) -> gpui::AnyElement {
    use gpui::{InteractiveElement, IntoElement, ParentElement, Styled};
    let selector = format!("probe-card-{tag}");
    gpui::div()
        .w_full()
        .debug_selector(move || selector.clone())
        .child(
            moon_ui::MoonGroupBox::new(gpui::SharedString::from(format!("probe-card-{tag}")))
                .child(crate::settings::common::group_title(
                    rust_i18n::t!("telegram.section_bot").to_string(),
                    cx,
                ))
                .padding(14.0)
                .children((0..n).map(move |i| {
                    gpui::div().w_full().child(format!(
                        "{tag} synthetic line {i} with a long caption that wraps"
                    ))
                })),
        )
        .into_any_element()
}

/// Draw three synthetic cards through `card_grid_el` in a page column `content_w` wide, in a
/// window wide enough for it, and return the drawn window.
fn draw_grid(cx: &mut gpui::TestAppContext, content_w: f32) -> gpui::VisualTestContext {
    let window = cx.open_window(
        gpui::size(gpui::px(content_w + 200.0), gpui::px(1200.0)),
        |_, _| Host {
            width: content_w,
            scroll: gpui::ScrollHandle::new(),
            build: std::rc::Rc::new(move |_, cx| {
                super::card_grid_el(
                    content_w,
                    vec![
                        synthetic_card("bot", 4, cx),
                        synthetic_card("chats", 9, cx),
                        synthetic_card("mini", 2, cx),
                    ],
                    cx,
                )
            }),
        },
    );
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    for _ in 0..3 {
        visual.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
    visual
}

/// Bounds of `sel` in the drawn window, `None` when it was not laid out.
fn drawn(visual: &mut gpui::VisualTestContext, sel: &str) -> Option<gpui::Bounds<gpui::Pixels>> {
    visual.debug_bounds(crate::settings::layout_probe::leak(sel.to_string()))
}

/// A wide Settings window (1700 px of content) puts a page's cards in two equal, non-overlapping
/// columns that reach the content's right edge; a re-introduced width cap or a `flex_wrap` grid
/// would leave the right half empty or the columns uneven. Every language.
#[gpui::test]
fn cards_take_two_equal_columns_across_a_wide_page(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    for language in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        let mut visual = draw_grid(cx, 1700.0);
        let body = drawn(&mut visual, "probe-body").expect("body");
        let left = drawn(&mut visual, "card-col-0").expect("left column");
        let right = drawn(&mut visual, "card-col-1").expect("right column");
        let code = language.code();
        assert!(
            (f32::from(left.size.width) - f32::from(right.size.width)).abs() <= 0.5,
            "[FAIL] {code}: columns differ in width"
        );
        assert!(
            left.right() <= right.origin.x,
            "[FAIL] {code}: columns overlap"
        );
        assert!(
            f32::from(body.right() - right.right()).abs() <= 24.0,
            "[FAIL] {code}: right column ends {} px short of the content",
            f32::from(body.right() - right.right())
        );
        for card in ["bot", "chats", "mini"] {
            let b = drawn(&mut visual, &format!("probe-card-{card}")).expect("card");
            assert!(
                b.right() <= body.right() + gpui::px(0.5),
                "[FAIL] {code}: {card} escapes"
            );
        }
    }
}

/// A ~900 px content width stacks the cards in one column that spans the content: two columns
/// there would squeeze each card below its readable width. Every language.
#[gpui::test]
fn cards_stack_in_one_full_column_on_a_narrow_page(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    for language in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        let mut visual = draw_grid(cx, 900.0);
        let code = language.code();
        let body = drawn(&mut visual, "probe-body").expect("body");
        assert!(
            drawn(&mut visual, "card-col-1").is_none(),
            "[FAIL] {code}: a second column at 900 px"
        );
        let column = drawn(&mut visual, "card-col-0").expect("column");
        assert!(
            f32::from(body.right() - column.right()).abs() <= 24.0,
            "[FAIL] {code}: the column ends {} px short of the content",
            f32::from(body.right() - column.right())
        );
        let bot = drawn(&mut visual, "probe-card-bot").expect("bot");
        let chats = drawn(&mut visual, "probe-card-chats").expect("chats");
        let mini = drawn(&mut visual, "probe-card-mini").expect("mini");
        assert!(
            bot.bottom_left().y <= chats.origin.y && chats.bottom_left().y <= mini.origin.y,
            "[FAIL] {code}: stacked cards overlap or are out of order"
        );
    }
}

/// The breakpoint: two columns once both reach the minimum beside the gap, never below it.
#[test]
fn the_grid_splits_only_when_both_columns_fit() {
    assert_eq!(super::card_columns(936.0, 460.0, 16.0), 2);
    assert_eq!(super::card_columns(935.0, 460.0, 16.0), 1);
    assert_eq!(super::card_columns(900.0, 460.0, 16.0), 1);
    assert_eq!(super::card_columns(1700.0, 460.0, 16.0), 2);
}

/// Catches the Station cores page falling back to its old narrow column, or the groups line
/// leaving its card: on a 1700 px window the cores card and the groups card both reach the
/// content's right edge, in every language.
#[gpui::test]
fn station_cores_cards_span_the_content(cx: &mut gpui::TestAppContext) {
    use crate::settings::general::probe_host::{host, stub, window_text};
    use crate::settings::layout_probe::{ProbeSpec, WIDE, sweep_sizes};
    use gpui::{InteractiveElement, IntoElement, ParentElement};
    sweep_sizes(
        cx,
        &[WIDE],
        |window, view, size| {
            host(move |_, cx| {
                let page_w =
                    size.0 + 2.0 * crate::design::ui_value(cx, crate::settings::render::BODY_PAD);
                let cores = gpui::div()
                    .debug_selector(|| "probe-cores".into())
                    .child(moon_ui::MoonGroupBox::new("probe-cores-box").child(stub(
                        "probe-cores-body",
                        200.0,
                        40.0,
                    )))
                    .into_any_element();
                let groups = gpui::div()
                    .child(rust_i18n::t!("telegram.server.groups_send").to_string())
                    .into_any_element();
                window_text(super::st_cores_column(page_w, cores, Some(groups), cx), cx)
            })(window, view, size)
        },
        |_| ProbeSpec {
            container: "probe-body",
            inside: vec!["probe-cores".into(), "station-groups".into()],
            reaches_right: vec![
                ("probe-cores".into(), 24.0),
                ("station-groups".into(), 24.0),
            ],
            ..Default::default()
        },
    );
}

/// Catches explanatory prose running the whole width of a full-width card again (a ~1800 px line
/// on Notifications, Trade tape and Maintenance): on a 1700 px window a Notifications hint, the
/// Trade tape breakdown and the auto-update switch's caption each stay within the prose cap, in
/// every language.
#[gpui::test]
fn card_prose_keeps_its_line_length_when_wide(cx: &mut gpui::TestAppContext) {
    use crate::settings::common::{PROSE_MAX_W, cap_prose};
    use crate::settings::general::probe_host::{host, window_text};
    use crate::settings::layout_probe::{ProbeSpec, WIDE, sweep_sizes};
    use gpui::{InteractiveElement, IntoElement, ParentElement};
    sweep_sizes(
        cx,
        &[WIDE],
        host(|_, cx| {
            let long = format!(
                "{} {}",
                rust_i18n::t!("telegram.notify_editor.trades_hint", minutes = 5),
                rust_i18n::t!("telegram.auto.hint")
            );
            let card = moon_ui::MoonGroupBox::new("probe-card")
                .child(cap_prose(
                    gpui::div()
                        .debug_selector(|| "probe-notify-hint".into())
                        .child(long.clone()),
                    cx,
                ))
                .child(super::server_tape::tape_breakdown(long, cx))
                .child(super::server_auto_update::auto_update_el(
                    super::server_auto_update::AutoUpdateView::Known(true),
                    false,
                    Box::new(|_, _, _| {}),
                    cx,
                ));
            window_text(card.into_any_element(), cx)
        }),
        |_| ProbeSpec {
            container: "probe-body",
            inside: vec![
                "probe-notify-hint".into(),
                "server-auto-update-caption".into(),
            ],
            max_width: [
                "probe-notify-hint",
                "server-tape-breakdown",
                "server-auto-update-caption",
            ]
            .map(|sel| (sel.to_string(), PROSE_MAX_W))
            .to_vec(),
            ..Default::default()
        },
    );
}

/// Catches the Notifications page stacking every explanation under its controls on a wide window
/// (half the card empty) or clipping it on a narrow one: a row of the three thresholds and a row
/// of two switches keep their explanation right of the controls at a wide window and under them
/// at the default one, the thresholds on one line, the text uncut and inside the card, in every
/// language.
#[gpui::test]
fn notification_rows_put_the_explanation_beside_the_controls(cx: &mut gpui::TestAppContext) {
    use crate::settings::common::{PROSE_MAX_W, aside_row};
    use crate::settings::general::probe_host::{host, window_text};
    use crate::settings::layout_probe::{ProbeSpec, SIZES, WIDE, sweep_sizes};
    use gpui::{AppContext, IntoElement, ParentElement, SharedString, Styled};
    let amounts_hint =
        || rust_i18n::t!("telegram.notify_editor.trades_hint", minutes = 5).to_string();
    let events_hint = || rust_i18n::t!("telegram.notify_editor.events_hint").to_string();
    let make = || {
        host(move |window, cx| {
            let states: Vec<_> = (0..3)
                .map(|_| cx.new(|cx| moon_ui::MoonInputState::new(window, cx)))
                .collect();
            let (line, line_w) = super::chat_notify::amounts_line(
                vec![
                    (
                        &states[0],
                        "min-volume",
                        rust_i18n::t!("telegram.mini_settings_min_volume").to_string(),
                    ),
                    (
                        &states[1],
                        "profit",
                        rust_i18n::t!("telegram.mini_settings_profit_at_least").to_string(),
                    ),
                    (
                        &states[2],
                        "loss",
                        rust_i18n::t!("telegram.mini_settings_loss_at_least").to_string(),
                    ),
                ],
                |what| SharedString::from(format!("probe-amount-{what}")),
                cx,
            );
            let switches = gpui::div()
                .flex()
                .gap_4()
                .child(rust_i18n::t!("telegram.notify_editor.opened").to_string())
                .child(rust_i18n::t!("telegram.notify_editor.detects").to_string())
                .into_any_element();
            let card = moon_ui::MoonGroupBox::new("probe-notify-card")
                .child(aside_row(
                    "probe-amounts",
                    vec![line],
                    Some(amounts_hint()),
                    line_w,
                    false,
                    cx,
                ))
                .child(aside_row(
                    "probe-events",
                    vec![switches],
                    Some(events_hint()),
                    0.0,
                    true,
                    cx,
                ));
            window_text(card.into_any_element(), cx)
        })
    };
    let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
    let fields = || {
        ["min-volume", "profit", "loss"]
            .map(|what| format!("probe-amount-{what}-field"))
            .to_vec()
    };
    let common = move |right: Option<bool>| {
        let rows = ["probe-amounts", "probe-events"];
        let mut spec = ProbeSpec {
            container: "probe-body",
            inside: rows
                .iter()
                .flat_map(|row| [format!("{row}-controls"), format!("{row}-aside")])
                .chain(fields())
                .collect(),
            wraps: vec![
                ("probe-amounts-aside".into(), amounts_hint()),
                ("probe-events-aside".into(), events_hint()),
            ],
            max_width: rows
                .iter()
                .map(|row| (format!("{row}-aside"), PROSE_MAX_W))
                .collect(),
            ..Default::default()
        };
        let placed = rows
            .iter()
            .map(|row| pair(&format!("{row}-controls"), &format!("{row}-aside")))
            .collect();
        match right {
            Some(true) => {
                spec.right_of = placed;
                spec.same_top = vec![fields()];
            }
            Some(false) => spec.under = placed,
            // Beside or under by the language's text width: only fit and clipping are checked.
            None => {}
        }
        spec
    };
    // The wide window: the explanation beside, the thresholds on one line.
    sweep_sizes(cx, &[WIDE], make(), move |_| common(Some(true)));
    sweep_sizes(cx, &[SIZES[4], SIZES[3]], make(), move |_| common(None));
    // The default and the minimum window: too narrow for both, the explanation drops under.
    sweep_sizes(cx, &[SIZES[0], SIZES[1]], make(), move |_| {
        common(Some(false))
    });
}

/// Two synthetic chats for the chats-card width check: a short label and a long one.
const TWO_CHATS: [(i64, &str); 2] = [
    (200001, "Synthetic owner"),
    (
        200002,
        "Synthetic viewer group with a much longer label 208",
    ),
];

/// Catches the chats table's label cell going back to content sizing: a `min_w_0` label beside the
/// `flex_none` id stacked one glyph per line in the real window while this harness laid it out
/// fine, so the harness cannot show that collapse and this asserts the explicit-width contract
/// instead. In a two- or three-column page, in every language, each label cell is exactly
/// `chat_row_widths` wide, at least as wide as its text when the card has room, and one line tall.
#[gpui::test]
fn the_chats_table_label_takes_its_explicit_width(cx: &mut gpui::TestAppContext) {
    use crate::design;
    use gpui::{IntoElement, ParentElement};
    cx.update(moon_ui::init);
    for language in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        for content_w in [1700.0_f32, 900.0, 520.0] {
            let window = cx.open_window(
                gpui::size(gpui::px(content_w + 200.0), gpui::px(1200.0)),
                |_, _| Host {
                    width: content_w,
                    scroll: gpui::ScrollHandle::new(),
                    build: std::rc::Rc::new(move |_, cx| {
                        let card_w = super::card_grid_col_w(content_w, 3, cx);
                        let mut table = super::access::chat_table(ChatsOf::Terminal, true, cx);
                        for (chat, title) in TWO_CHATS {
                            let toggle = moon_ui::MoonButton::new(("probe-edit", chat as u64))
                                .ghost()
                                .label(rust_i18n::t!("telegram.chats.configure").to_string())
                                .render()
                                .into_any_element();
                            table = table.child(super::access::chat_row(
                                chat,
                                title.to_string(),
                                (
                                    rust_i18n::t!("telegram.access_owner_summary").to_string(),
                                    false,
                                ),
                                toggle,
                                card_w,
                                cx,
                            ));
                        }
                        let chats = moon_ui::MoonGroupBox::new("probe-chats")
                            .padding(14.0)
                            .child(table)
                            .into_any_element();
                        super::card_grid_el(
                            content_w,
                            vec![
                                synthetic_card("bot", 2, cx),
                                chats,
                                synthetic_card("mini", 2, cx),
                            ],
                            cx,
                        )
                    }),
                },
            );
            let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
            for _ in 0..3 {
                visual.update(|window, cx| {
                    let _ = window.draw(cx);
                });
            }
            let code = language.code();
            for (chat, title) in TWO_CHATS {
                let (widths, text_w) = visual.update(|_, cx| {
                    let card_w = super::card_grid_col_w(content_w, 3, cx);
                    let summary = rust_i18n::t!("telegram.access_owner_summary").to_string();
                    (
                        super::access::chat_row_widths(card_w, chat, title, &summary, cx),
                        design::ui_body_text_width(cx, title, 400.0),
                    )
                });
                let cell = drawn(&mut visual, &format!("tg-chat-title-{chat}"))
                    .expect("label cell laid out");
                let width = f32::from(cell.size.width);
                // The `#id` beside it is one line: the label must be no taller.
                let line_h = f32::from(
                    drawn(&mut visual, &format!("tg-chat-id-{chat}"))
                        .expect("id cell laid out")
                        .size
                        .height,
                );
                assert!(
                    (width - widths.title).abs() < 0.5,
                    "[FAIL] {code} at {content_w}: label {chat} is {width}, not {}",
                    widths.title
                );
                if widths.title > text_w {
                    assert!(
                        width >= text_w,
                        "[FAIL] {code} at {content_w}: label {chat} {width} < its text {text_w}"
                    );
                }
                assert!(
                    f32::from(cell.size.height) < 1.5 * line_h,
                    "[FAIL] {code} at {content_w}: label {chat} wraps ({} tall)",
                    f32::from(cell.size.height)
                );
            }
        }
    }
}

/// Catches the Notifications card's explanations starting at different x (a row's controls
/// widened by the per-currency block) and the empty gap under that block: at a wide window the
/// thresholds row, a per-currency row with its picker and the switches row share one controls
/// column, the currency's line fits in it, its hint sits in the explanation, and no controls
/// column is taller than what it holds — in every language.
#[gpui::test]
fn notification_rows_share_one_controls_column(cx: &mut gpui::TestAppContext) {
    use super::chat_notify_quotes::{QuoteList, quote_hint, quote_row_line, quote_row_w, remove_w};
    use crate::design;
    use crate::settings::common::aside_row;
    use crate::settings::layout_probe::leak;
    use gpui::{AppContext, InteractiveElement, IntoElement, ParentElement, SharedString, Styled};
    cx.update(moon_ui::init);
    for language in moon_core::config::Language::ALL {
        let code = language.code();
        let _locale = crate::test_locale::force(code);
        let width = 1700.0;
        let window = cx.open_window(gpui::size(gpui::px(1900.0), gpui::px(1200.0)), |_, _| {
            Host {
                width,
                scroll: gpui::ScrollHandle::new(),
                build: std::rc::Rc::new(move |window, cx| {
                    let mut state =
                        |cx: &mut gpui::App| cx.new(|cx| moon_ui::MoonInputState::new(window, cx));
                    let states: Vec<_> = (0..6).map(|_| state(cx)).collect();
                    let t = |key: &str| rust_i18n::t!(key).to_string();
                    let (line, line_w) = super::chat_notify::amounts_line(
                        vec![
                            (
                                &states[0],
                                "min-volume",
                                t("telegram.mini_settings_min_volume"),
                            ),
                            (
                                &states[1],
                                "profit",
                                t("telegram.mini_settings_profit_at_least"),
                            ),
                            (
                                &states[2],
                                "loss",
                                t("telegram.mini_settings_loss_at_least"),
                            ),
                        ],
                        |what| SharedString::from(format!("probe-amount-{what}")),
                        cx,
                    );
                    let fields = [
                        "telegram.quote_volume",
                        "telegram.quote_profit",
                        "telegram.quote_loss",
                    ]
                    .iter()
                    .zip(&states[3..])
                    .enumerate()
                    .map(|(n, (key, state))| {
                        (
                            t(key),
                            moon_ui::MoonInput::new(SharedString::from(format!("probe-q-{n}")))
                                .state(state)
                                .size(design::INPUT_SIZE)
                                .into_any_element(),
                        )
                    })
                    .collect();
                    let remove = moon_ui::MoonButton::new("probe-q-remove")
                        .ghost()
                        .padding_x(12.0)
                        .width(remove_w(cx))
                        .label(t("telegram.notify_editor.quote_remove"))
                        .render()
                        .into_any_element();
                    let quote = gpui::div()
                        .debug_selector(|| "probe-quote-line".into())
                        .child(quote_row_line("RUB", fields, remove, cx))
                        .into_any_element();
                    let add = gpui::div()
                        .debug_selector(|| "probe-quote-add".into())
                        .child(t("telegram.notify_editor.quote_add"))
                        .into_any_element();
                    let controls_w = line_w.max(quote_row_w(QuoteList::Trades, cx));
                    let switches = gpui::div()
                        .flex()
                        .gap_4()
                        .child(t("telegram.notify_editor.opened"))
                        .child(t("telegram.notify_editor.detects"))
                        .into_any_element();
                    let card = moon_ui::MoonGroupBox::new("probe-notify-card")
                        .child(aside_row(
                            "probe-trades",
                            vec![line, quote, add],
                            Some(format!(
                                "{}\n\n{}",
                                t("telegram.mini_settings_trades_hint"),
                                quote_hint()
                            )),
                            controls_w,
                            false,
                            cx,
                        ))
                        .child(aside_row(
                            "probe-usd",
                            vec![
                                gpui::div()
                                    .child(t("telegram.notify_editor.usd_followup"))
                                    .into_any_element(),
                            ],
                            Some(
                                rust_i18n::t!("telegram.notify_editor.trades_hint", minutes = 5)
                                    .to_string(),
                            ),
                            controls_w,
                            false,
                            cx,
                        ))
                        .child(aside_row(
                            "probe-events",
                            vec![switches],
                            Some(t("telegram.notify_editor.events_hint")),
                            controls_w,
                            true,
                            cx,
                        ));
                    crate::settings::general::probe_host::window_text(card.into_any_element(), cx)
                }),
            }
        });
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        for _ in 0..3 {
            visual.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
        let mut at = |sel: &str| {
            visual
                .debug_bounds(leak(sel.to_string()))
                .unwrap_or_else(|| panic!("{code}: {sel} not laid out"))
        };
        let rows = ["probe-trades", "probe-usd", "probe-events"];
        let asides: Vec<_> = rows.iter().map(|r| at(&format!("{r}-aside"))).collect();
        let controls: Vec<_> = rows.iter().map(|r| at(&format!("{r}-controls"))).collect();
        for (row, (aside, ctl)) in rows.iter().zip(asides.iter().zip(&controls)) {
            assert!(
                (f32::from(aside.origin.x) - f32::from(asides[0].origin.x)).abs() <= 0.5,
                "{code}: {row}'s explanation starts at {:?}, not {:?}",
                aside.origin.x,
                asides[0].origin.x
            );
            assert!(
                ctl.right() <= aside.origin.x,
                "{code}: {row} controls under its text"
            );
        }
        let line = at("probe-quote-line");
        let add = at("probe-quote-add");
        let amounts = at("probe-amount-loss-field");
        assert!(
            line.right() <= controls[0].right() + gpui::px(0.5),
            "{code}: the currency's line spills out of the controls column"
        );
        assert!(
            amounts.right() <= controls[0].right() + gpui::px(0.5),
            "{code}: the thresholds spill out of the controls column"
        );
        // Nothing under the picker: the controls column ends at it (its padding aside).
        assert!(
            f32::from(controls[0].bottom()) <= f32::from(add.bottom()) + 1.0,
            "{code}: an empty gap under the per-currency picker ({:?} > {:?})",
            controls[0].bottom(),
            add.bottom()
        );
        // The currency's line stays on one line: under two field heights, a wrap is more.
        let one_line = 2.0 * f32::from(at("probe-amount-min-volume-field").size.height);
        assert!(
            f32::from(line.size.height) <= one_line,
            "{code}: the currency's line wrapped ({:?})",
            line.size.height
        );
    }
}
