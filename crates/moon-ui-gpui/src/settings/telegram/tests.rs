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
