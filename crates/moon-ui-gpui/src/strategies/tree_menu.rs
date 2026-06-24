//! ПКМ-контекст-меню дерева стратегий: набор пунктов по цели (папка/стратегия) и открытие
//! меню в MoonUI Root. Сами действия делегируются методам модалок/буфера.

use super::tree_ops;
use super::tree_ui::{ContextMenu, MenuTarget};
use super::*;
use moon_ui::{MoonContextMenuWindowExt as _, MoonWindowExt as _};
use rust_i18n::t;

impl StrategiesView {
    pub(super) fn open_menu(
        &mut self,
        menu: ContextMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.op = None;
        self.op_input = None;
        let pos = menu.pos;
        let items = self.context_menu_items(&menu, cx);
        window.open_moon_context_menu(cx, "strategies-context-menu", pos, items, 190.0);
        cx.notify();
    }

    fn context_menu_items(&self, menu: &ContextMenu, cx: &Context<Self>) -> Vec<MoonMenuItem> {
        let core = menu.core;
        let can_paste = self.clipboard.is_some();
        let view = cx.entity();

        let mut items: Vec<MoonMenuItem> = Vec::new();
        match &menu.target {
            MenuTarget::Folder(path) => {
                let pp = path.clone();
                items.push(
                    MoonMenuItem::with_key("rename-folder", t!("strat.menu_rename").to_string())
                        .on_click({
                            let view = view.clone();
                            move |_, window, app| {
                                window.close_context_menu(app);
                                view.update(app, |this, cx| {
                                    this.open_rename_folder(core, pp.clone(), window, cx);
                                });
                            }
                        }),
                );
                let pp = path.clone();
                items.push(
                    MoonMenuItem::with_key("copy-folder", t!("strat.menu_copy").to_string())
                        .on_click({
                            let view = view.clone();
                            move |_, window, app| {
                                window.close_context_menu(app);
                                view.update(app, |this, cx| {
                                    this.copy_folder(core, pp.clone(), cx);
                                    cx.notify();
                                });
                            }
                        }),
                );
                if can_paste {
                    let t = tree_ops::join_path(path);
                    items.push(
                        MoonMenuItem::with_key(
                            "paste-here",
                            t!("strat.menu_paste_here").to_string(),
                        )
                        .on_click({
                            let view = view.clone();
                            move |_, window, app| {
                                window.close_context_menu(app);
                                view.update(app, |this, cx| {
                                    this.paste_into(core, t.clone(), cx);
                                    cx.notify();
                                });
                            }
                        }),
                    );
                }
                let t = tree_ops::join_path(path);
                items.push(
                    MoonMenuItem::with_key(
                        "new-strategy-here",
                        t!("strat.menu_new_strategy_here").to_string(),
                    )
                    .on_click({
                        let view = view.clone();
                        move |_, window, app| {
                            window.close_context_menu(app);
                            view.update(app, |this, cx| {
                                this.open_create_strategy(core, t.clone(), window, cx);
                            });
                        }
                    }),
                );
                let t = tree_ops::join_path(path);
                items.push(
                    MoonMenuItem::with_key(
                        "new-folder-here",
                        t!("strat.menu_new_folder_here").to_string(),
                    )
                    .on_click({
                        let view = view.clone();
                        move |_, window, app| {
                            window.close_context_menu(app);
                            view.update(app, |this, cx| {
                                this.open_create_folder(core, t.clone(), window, cx);
                            });
                        }
                    }),
                );
                let pp = path.clone();
                items.push(
                    MoonMenuItem::with_key(
                        "delete-folder",
                        t!("strat.menu_delete_folder").to_string(),
                    )
                    .tone(MoonTone::Danger)
                    .on_click({
                        let view = view.clone();
                        move |_, window, app| {
                            window.close_context_menu(app);
                            view.update(app, |this, cx| {
                                this.request_delete_folder(core, pp.clone(), window, cx);
                            });
                        }
                    }),
                );
            }
            MenuTarget::Strategy(_id) => {
                items.push(
                    MoonMenuItem::with_key("copy-strategy", t!("strat.menu_copy").to_string())
                        .on_click({
                            let view = view.clone();
                            move |_, window, app| {
                                window.close_context_menu(app);
                                view.update(app, |this, cx| {
                                    this.copy_selection(cx);
                                    cx.notify();
                                });
                            }
                        }),
                );
                items.push(
                    MoonMenuItem::with_key(
                        "delete-strategy",
                        t!("strat.menu_delete_strategy").to_string(),
                    )
                    .tone(MoonTone::Danger)
                    .on_click({
                        let view = view.clone();
                        move |_, window, app| {
                            window.close_context_menu(app);
                            view.update(app, |this, cx| {
                                this.request_delete_selection(window, cx);
                            });
                        }
                    }),
                );
            }
        }

        items
    }
}
