//! The bot's box, in two columns that wrap under each other in a narrow window. On the left the
//! menu: the reply keyboard as a tree of buttons, the view a report opens in, and the time its
//! period is counted by (`TelegramConfig::bot`); the Report section is fixed. On the right one
//! chat's notifications and automatic reports (`chat_notify.rs`), the chat picked above them.
//!
//! It edits whichever bot this terminal has: its own (the Settings draft, saved by Save) or the
//! station's (the station's draft, sent by "Apply on the server" with the chats). Rows are never
//! edited as such: each button says whether it starts a row, so a row can never be left empty.
//!
//! The tree's items are pushed into its `MoonTreeState` only when the order of the buttons
//! changed ([`SettingsView::bot_menu_sync`], from the window's render root); a row reads the
//! draft as it is at render, through closures that hold the view weakly — `MoonTreeState`, owned
//! by this view, retains them.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::config::telegram_menu::{
    BotMenu, BotSettings, MenuEntry, MenuItem, ReportBasis, ReportView,
};
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonDropdown, MoonGroupBox,
    MoonPalette, MoonTree, MoonTreeItem, MoonTreeState, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::SettingsView;
use super::access::ChatsOf;
use crate::design;

/// One side's tree and the button order last pushed into it.
pub(in crate::settings) struct BotMenuEd {
    tree: Entity<MoonTreeState>,
    shape: Option<u64>,
}

impl BotMenuEd {
    /// An empty tree; the first render pushes the menu into it.
    pub(in crate::settings) fn new<T: 'static>(cx: &mut Context<T>) -> Self {
        Self {
            tree: cx.new(|cx| MoonTreeState::new(cx)),
            shape: None,
        }
    }
}

/// The tree id of the keyboard's root.
const ROOT: &str = "kb";

/// Unscaled height of one tree row.
const ROW_H: f32 = 30.0;

/// Unscaled narrowest width of one column; below two of them the columns stack.
pub(super) const COLUMN_MIN_W: f32 = 320.0;

/// The button order and the language its labels are in: all the tree's items depend on.
fn shape_sig(menu: &BotMenu) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    rust_i18n::locale().hash(&mut hasher);
    for (entry, _) in menu.flat() {
        entry.item.hash(&mut hasher);
    }
    hasher.finish()
}

/// The tree's items: the keyboard's root, a child per button in order.
fn tree_items(menu: &BotMenu) -> Vec<MoonTreeItem> {
    vec![
        MoonTreeItem::new(ROOT, t!("telegram.menu_editor.keyboard").to_string())
            .expanded(true)
            .children(menu.flat().into_iter().map(|(entry, _)| {
                MoonTreeItem::new(
                    format!("{ROOT}:{}", entry.item.id()),
                    item_caption(entry.item),
                )
                .folder(false)
            })),
    ]
}

/// A button's caption as the bot shows it, without its glyph.
fn item_caption(item: MenuItem) -> String {
    let key = format!("telegram.button_{}", item.id());
    t!(&key).to_string()
}

/// One button as a row shows it.
#[derive(Clone, Copy)]
struct Row {
    entry: MenuEntry,
    starts_row: bool,
    /// Its row's number, from 1.
    row: usize,
    first: bool,
    last: bool,
}

/// Every button of `menu` by tree id.
fn rows_by_id(menu: &BotMenu) -> HashMap<String, Row> {
    let mut rows = HashMap::new();
    let flat = menu.flat();
    let count = flat.len();
    let mut row = 0;
    for (index, (entry, starts_row)) in flat.into_iter().enumerate() {
        row += usize::from(starts_row);
        rows.insert(
            format!("{ROOT}:{}", entry.item.id()),
            Row {
                entry,
                starts_row,
                row,
                first: index == 0,
                last: index + 1 == count,
            },
        );
    }
    rows
}

impl SettingsView {
    fn bot_menu_ed(&self, side: ChatsOf) -> &BotMenuEd {
        match side {
            ChatsOf::Terminal => &self.telegram.menu,
            ChatsOf::Station => &self.telegram.server.menu,
        }
    }

    fn bot_menu_ed_mut(&mut self, side: ChatsOf) -> &mut BotMenuEd {
        match side {
            ChatsOf::Terminal => &mut self.telegram.menu,
            ChatsOf::Station => &mut self.telegram.server.menu,
        }
    }

    /// Push each side's buttons into its tree when their order changed since the last push.
    pub(in crate::settings) fn bot_menu_sync(&mut self, cx: &mut Context<Self>) {
        for side in [ChatsOf::Terminal, ChatsOf::Station] {
            let Some(menu) = self.chats(side, cx).map(|t| t.bot.menu.clone()) else {
                continue;
            };
            let shape = shape_sig(&menu);
            let ed = self.bot_menu_ed_mut(side);
            if ed.shape != Some(shape) {
                ed.shape = Some(shape);
                let items = tree_items(&menu);
                ed.tree.update(cx, |state, cx| {
                    state.set_items(items, cx);
                    state.set_force_expanded(true, cx);
                });
            }
        }
    }

    /// Change `side`'s bot settings; `edit` says whether it changed anything.
    fn bot_settings_edit(
        &mut self,
        side: ChatsOf,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut BotSettings) -> bool,
    ) {
        self.chats_edit(side, cx, |telegram| edit(&mut telegram.bot));
    }

    /// The bot's box for `side`: the menu on the left, a chat's notifications on the right.
    pub(in crate::settings) fn bot_menu_box(
        &self,
        side: ChatsOf,
        cx: &Context<Self>,
    ) -> AnyElement {
        let id = |what: &str| -> SharedString {
            match side {
                ChatsOf::Terminal => format!("tgm-{what}").into(),
                ChatsOf::Station => format!("tgms-{what}").into(),
            }
        };
        let column = |what: &str, key: &str, body: AnyElement| {
            div().flex_1().min_w(design::ui_px(cx, COLUMN_MIN_W)).child(
                MoonGroupBox::new(id(what))
                    .title(t!(key).to_string())
                    .padding(12.0)
                    .gap(10.0)
                    .child(body),
            )
        };
        MoonGroupBox::new(id("box"))
            .title(t!("telegram.menu_editor.title").to_string())
            .padding(14.0)
            .gap(10.0)
            .child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .items_start()
                    .gap(design::ui_px(cx, 16.0))
                    .child(column(
                        "keys",
                        "telegram.settings.buttons",
                        self.bot_menu_keys(side, cx),
                    ))
                    .child(column(
                        "notify",
                        "telegram.notify_editor.title",
                        self.chat_notify_column(side, cx),
                    )),
            )
            .into_any_element()
    }

    /// The menu column for `side`, or a note when the station predates the menu.
    fn bot_menu_keys(&self, side: ChatsOf, cx: &Context<Self>) -> AnyElement {
        let p = MoonPalette::active(cx);
        let muted = rgba_from(p.text_muted, 1.0);
        let id = |what: &str| -> SharedString {
            match side {
                ChatsOf::Terminal => format!("tgm-{what}").into(),
                ChatsOf::Station => format!("tgms-{what}").into(),
            }
        };
        let section = v_flex().w_full().min_w_0().gap(design::ui_px(cx, 10.0));
        let station_knows = self
            .telegram
            .server
            .access_base
            .as_ref()
            .is_some_and(|base| base.bot.is_some());
        let Some(settings) = self.chats(side, cx).map(|t| t.bot.clone()) else {
            return section.into_any_element();
        };
        if side == ChatsOf::Station && !station_knows {
            return section
                .child(
                    div()
                        .text_color(muted)
                        .child(t!("telegram.menu_editor.station_too_old").to_string()),
                )
                .into_any_element();
        }
        let rows = rows_by_id(&settings.menu);
        let count = 1 + rows.len();
        let row_h = design::ui_px(cx, ROW_H);
        let weak = cx.entity().downgrade();
        let tree = MoonTree::custom(
            &self.bot_menu_ed(side).tree,
            move |entry, _meta, _w, app| {
                let id = entry.item().id().to_string();
                let p = MoonPalette::active(app);
                let Some(row) = rows.get(&id).copied() else {
                    return div()
                        .h(row_h)
                        .flex()
                        .items_center()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(entry.item().label().clone())
                        .into_any_element();
                };
                item_row(side, &id, row, row_h, weak.clone(), p, app).into_any_element()
            },
        )
        .w_full()
        .h(row_h * count as f32);
        let view_items = {
            let weak = cx.entity().downgrade();
            crate::panels::radio_items(
                ReportView::ALL.map(|view| {
                    (
                        view,
                        SharedString::from(format!("{}-{}", id("view"), view.id())),
                        SharedString::from(view_caption(view)),
                    )
                }),
                settings.report_view,
                crate::panels::RadioMark::Check,
                move |app, view| {
                    let _ = weak.update(app, |this, cx| {
                        this.bot_settings_edit(side, cx, |bot| {
                            std::mem::replace(&mut bot.report_view, view) != view
                        });
                    });
                },
            )
        };
        let basis_items = {
            let weak = cx.entity().downgrade();
            crate::panels::radio_items(
                ReportBasis::ALL.map(|basis| {
                    (
                        basis,
                        SharedString::from(format!("{}-{}", id("basis"), basis.id())),
                        SharedString::from(basis_caption(basis)),
                    )
                }),
                settings.period_basis,
                crate::panels::RadioMark::Check,
                move |app, basis| {
                    let _ = weak.update(app, |this, cx| {
                        this.bot_settings_edit(side, cx, |bot| {
                            std::mem::replace(&mut bot.period_basis, basis) != basis
                        });
                    });
                },
            )
        };
        let dropdown = |key: &str, label: String, items| {
            h_flex()
                .gap(design::ui_px(cx, 8.0))
                .items_center()
                .child(div().text_color(muted).child(t!(key).to_string()))
                .child(
                    MoonDropdown::new(id(key))
                        .label(label)
                        .trigger_caret(true)
                        .trigger_variant(MoonButtonVariant::Neutral)
                        .trigger_size(MoonButtonSize::density(cx))
                        .trigger_width_scaled(140.0)
                        .menu_width_scaled(160.0)
                        .items(items),
                )
        };
        section
            .child(
                div()
                    .text_color(muted)
                    .child(t!("telegram.menu_editor.hint").to_string()),
            )
            .child(tree)
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 16.0))
                    .child(dropdown(
                        "telegram.menu_editor.view",
                        view_caption(settings.report_view),
                        view_items,
                    ))
                    .child(dropdown(
                        "telegram.menu_editor.basis",
                        basis_caption(settings.period_basis),
                        basis_items,
                    )),
            )
            .child(
                div()
                    .text_color(muted)
                    .child(t!("telegram.menu_editor.delivery").to_string()),
            )
            .when(side == ChatsOf::Station, |section| {
                section
                    .child(
                        div()
                            .text_color(muted)
                            .child(t!("telegram.menu_editor.station_control_note").to_string()),
                    )
                    .children(self.station_groups_row(cx))
                    .child(self.server_access_actions("server-menu", cx))
            })
            .into_any_element()
    }
}

/// A report view's caption.
fn view_caption(view: ReportView) -> String {
    match view {
        ReportView::Exchanges => t!("telegram.menu_editor.view_exchanges"),
        ReportView::Cores => t!("telegram.menu_editor.view_cores"),
    }
    .to_string()
}

/// A period basis's caption, the terminal Report's own words.
fn basis_caption(basis: ReportBasis) -> String {
    match basis {
        ReportBasis::Close => t!("report.period_basis.close"),
        ReportBasis::Open => t!("report.period_basis.open"),
    }
    .to_string()
}

/// One button's row: shown, starts a row, up and down.
fn item_row(
    side: ChatsOf,
    id: &str,
    row: Row,
    row_h: Pixels,
    weak: WeakEntity<SettingsView>,
    p: MoonPalette,
    app: &App,
) -> impl IntoElement {
    let item = row.entry.item;
    let key = |what: &str| -> SharedString {
        let side = match side {
            ChatsOf::Terminal => "t",
            ChatsOf::Station => "s",
        };
        format!("tgm-{side}-{id}-{what}").into()
    };
    let mut caption = item_caption(item);
    if item == MenuItem::Status && side == ChatsOf::Terminal {
        caption = format!("{caption} — {}", t!("telegram.menu_editor.station_only"));
    }
    let edit = move |app: &mut App, edit: Box<dyn FnOnce(&mut BotMenu) -> bool>| {
        let _ = weak.update(app, |this, cx| {
            this.bot_settings_edit(side, cx, |bot| edit(&mut bot.menu));
        });
    };
    let shown = {
        let edit = edit.clone();
        move |show: &bool, _: &mut Window, app: &mut App| {
            let show = *show;
            edit(app, Box::new(move |menu| menu.set_shown(item, show)));
        }
    };
    let starts = {
        let edit = edit.clone();
        move |starts: &bool, _: &mut Window, app: &mut App| {
            let starts = *starts;
            edit(app, Box::new(move |menu| menu.set_row_start(item, starts)));
        }
    };
    let up = {
        let edit = edit.clone();
        move |_: &ClickEvent, _: &mut Window, app: &mut App| {
            edit(app, Box::new(move |menu| menu.move_item(item, true)));
        }
    };
    let down = move |_: &ClickEvent, _: &mut Window, app: &mut App| {
        edit(app, Box::new(move |menu| menu.move_item(item, false)));
    };
    h_flex()
        .h(row_h)
        .w_full()
        .gap(design::ui_px(app, 10.0))
        .items_center()
        .pl(design::ui_px(app, 16.0))
        .child(
            div()
                .w(design::ui_px(app, 18.0))
                .text_color(rgba_from(p.text_muted, 1.0))
                .child(row.row.to_string()),
        )
        .child(
            div().flex_1().child(
                MoonCheckbox::new(key("show"))
                    .checked(row.entry.show)
                    .label(caption)
                    .on_change(shown),
            ),
        )
        .child(
            MoonCheckbox::new(key("row"))
                .checked(row.starts_row)
                .disabled(row.first)
                .label(t!("telegram.menu_editor.new_row").to_string())
                .on_change(starts),
        )
        .child(
            MoonButton::new(key("up"))
                .ghost()
                .label("\u{2191}")
                .tooltip(t!("telegram.menu_editor.up").to_string())
                .disabled(row.first)
                .on_click(up)
                .render(),
        )
        .child(
            MoonButton::new(key("down"))
                .ghost()
                .label("\u{2193}")
                .tooltip(t!("telegram.menu_editor.down").to_string())
                .disabled(row.last)
                .on_click(down)
                .render(),
        )
}

#[cfg(test)]
mod tests;
