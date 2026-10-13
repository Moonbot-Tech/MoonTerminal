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
