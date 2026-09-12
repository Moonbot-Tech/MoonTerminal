//! Right-click context menu for strategy-tree folders and strategies. MoonUI Root owns the open
//! menu, while actions delegate to the modal and clipboard methods.

use super::super::*;
use super::ops;
use super::ui::{ContextMenu, MenuTarget};
use moon_ui::{MoonContextMenuWindowExt as _, MoonWindowExt as _};
use rust_i18n::t;

/// Minimum outer width at the configured font reference size, keeping the short entries from
/// rendering as a cramped stub. Unlike the retired fixed width, a fitted minimum is multiplied by
/// the theme's text scale, so the menu now follows the font slider like the rest of the UI.
const MENU_MIN_WIDTH: f32 = 190.0;
/// Maximum outer width at that same reference size. Every current label is a locale constant that
/// fits well inside it; the cap exists so a longer translation widens the menu instead of stretching
/// it across the pane.
const MENU_MAX_WIDTH: f32 = 460.0;

/// The "Paste here" row, shared by every branch that can receive a paste.
///
/// ALWAYS listed, never hidden: a menu that silently drops an entry when the clipboard is empty
/// leaves the operator wondering whether the feature exists at all. Disabled with the reason
/// instead, which is the rule every other unavailable action in this window follows.
fn paste_here_item(
    view: &Entity<StrategiesView>,
    core: CoreId,
    target: String,
    paste_ready: bool,
) -> MoonMenuItem {
    let mut item = MoonMenuItem::with_key("paste-here", t!("strat.menu_paste_here").to_string())
        .disabled(!paste_ready);
    if !paste_ready {
        item = item.right_label(t!("strat.reason_nothing_to_paste").to_string());
    }
    item.on_click({
        let view = view.clone();
        move |_, window, app| {
            window.close_context_menu(app);
            let target = target.clone();
            view.update(app, |this, cx| {
                // Reported, like every other paste door. Discarding the count here was how the one
                // gesture that most looks like it did something - right-click, "Paste here" - ended
                // up being the only one that said nothing.
                this.retire_replaced_cut(cx);
                let moving = this.cut.is_some();
                let mut split_folders = Vec::new();
                let landed = this.paste_into(core, target, &mut split_folders, cx);
                this.note_split_folders(split_folders, cx);
                let note = match (landed, moving) {
                    (0, _) => tree::ui::TreeNote::NothingToPaste,
                    (n, true) => tree::ui::TreeNote::Moved { strategies: n },
                    (n, false) => tree::ui::TreeNote::Pasted {
                        strategies: n,
                        cores: 1,
                    },
                };
                note.say(window, cx);
                cx.notify();
            });
        }
    })
}

/// The "Cut" row, shared by the strategy and folder branches so they cannot drift.
fn cut_item(view: &Entity<StrategiesView>) -> MoonMenuItem {
    MoonMenuItem::with_key("cut", t!("strat.menu_cut").to_string())
        .right_label(t!("strat.cut_chord").to_string())
        .on_click({
            let view = view.clone();
            move |_, window, app| {
                window.close_context_menu(app);
                view.update(app, |this, cx| this.cut_tree_target(window, cx));
            }
        })
}

/// The "Move to folder..." row.
///
/// Disabled with a reason when the selection spans several cores: a move rewrites `folder_path`
/// within ONE core, and moving between cores is the cut/paste path instead.
fn move_to_folder_item(
    view: &Entity<StrategiesView>,
    core: CoreId,
    spans_cores: bool,
) -> MoonMenuItem {
    let mut item = MoonMenuItem::with_key(
        "move-to-folder",
        t!("strat.menu_move_to_folder").to_string(),
    )
    .disabled(spans_cores);
    if spans_cores {
        item = item.right_label(t!("strat.reason_several_cores").to_string());
    }
    item.on_click({
        let view = view.clone();
        move |_, window, app| {
            window.close_context_menu(app);
            view.update(app, |this, cx| this.open_move_for_target(core, window, cx));
        }
    })
}

impl StrategiesView {
    /// Open the menu with paste availability resolved from the current text and destination schema.
    pub(super) fn open_menu(
        &mut self,
        menu: ContextMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.op = None;
        self.op_input = None;
        let pos = menu.pos;
        let paste_ready = self.clipboard_for_core(menu.core, cx).is_some();
        let items = self.context_menu_items(&menu, paste_ready, cx);
        // Fitted, not fixed: the longest entry did not fit the fixed 190 px level, and a row that
        // overruns its level no longer lays out like the rows that fit it.
        window.open_fitted_moon_context_menu(
            cx,
            "strategies-context-menu",
            pos,
            items,
            MENU_MIN_WIDTH,
            MENU_MAX_WIDTH,
        );
        cx.notify();
    }

    fn context_menu_items(
        &self,
        menu: &ContextMenu,
        paste_ready: bool,
        cx: &Context<Self>,
    ) -> Vec<MoonMenuItem> {
        let core = menu.core;
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
                items.push(
                    MoonMenuItem::with_key("copy-folder", t!("strat.menu_copy").to_string())
                        .right_label(t!("strat.copy_chord").to_string())
                        .on_click({
                            let view = view.clone();
                            move |_, window, app| {
                                window.close_context_menu(app);
                                view.update(app, |this, cx| this.copy_tree_target(cx));
                            }
                        }),
                );
                items.push(cut_item(&view));
                items.push(move_to_folder_item(&view, core, false));
                items.push(paste_here_item(
                    &view,
                    core,
                    ops::join_path(path),
                    paste_ready,
                ));
                let t = ops::join_path(path);
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
                let t = ops::join_path(path);
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
                let folder_block = {
                    let store = self.backend.read(cx).session.store();
                    store
                        .core(core)
                        .map(|cd| ops::delete_block(&ops::rows_under(&cd.strategies, path)))
                        .unwrap_or(None)
                };
                let mut delete_folder = MoonMenuItem::with_key(
                    "delete-folder",
                    t!("strat.menu_delete_folder").to_string(),
                )
                .tone(MoonTone::Danger)
                .disabled(folder_block.is_some());
                if let Some(block) = folder_block {
                    delete_folder = delete_folder
                        .right_label(t!("strat.reason_enabled", n = block.enabled).to_string());
                }
                items.push(delete_folder.on_click({
                    let view = view.clone();
                    move |_, window, app| {
                        window.close_context_menu(app);
                        view.update(app, |this, cx| {
                            this.request_delete_folder(core, pp.clone(), window, cx);
                        });
                    }
                }));
            }
            MenuTarget::Strategy(id) => {
                // Acts on the SELECTION, which already contains this row: opening the menu on an
                // unselected strategy focuses it first (`strategy_row`). So right-clicking one row
                // moves that row, and right-clicking inside a multi-selection moves the block.
                let (can_up, can_down) = {
                    let backend = self.backend.read(cx);
                    let store = backend.session.store();
                    self.move_availability(store, backend.session.core_venues())
                };
                for (step, enabled) in
                    [(ops::MoveStep::Up, can_up), (ops::MoveStep::Down, can_down)]
                {
                    let (key, label, chord) = match step {
                        ops::MoveStep::Up => (
                            "move-up",
                            t!("strat.menu_move_up"),
                            t!("strat.move_up_chord"),
                        ),
                        ops::MoveStep::Down => (
                            "move-down",
                            t!("strat.menu_move_down"),
                            t!("strat.move_down_chord"),
                        ),
                    };
                    items.push(
                        MoonMenuItem::with_key(key, label.to_string())
                            .right_label(chord.to_string())
                            .disabled(!enabled)
                            .on_click({
                                let view = view.clone();
                                move |_, window, app| {
                                    window.close_context_menu(app);
                                    view.update(app, |this, cx| this.move_selection(step, cx));
                                }
                            }),
                    );
                }
                let keys = selected_keys(self);
                let selected_count = keys.len();
                // A move rewrites folder_path within ONE core; a selection spanning several has no
                // single destination list to offer.
                let spans_cores = keys.iter().any(|(c, _)| *c != core);
                let mut rename =
                    MoonMenuItem::with_key("rename-strategy", t!("strat.menu_rename").to_string())
                        .right_label(t!("strat.rename_chord").to_string())
                        .disabled(selected_count != 1);
                if selected_count != 1 {
                    // A rename edits ONE name; saying which count blocked it is more use than a
                    // greyed row with no explanation.
                    rename = rename
                        .right_label(t!("strat.reason_multi", n = selected_count).to_string());
                }
                let strategy_id = *id;
                items.push(rename.on_click({
                    let view = view.clone();
                    move |_, window, app| {
                        window.close_context_menu(app);
                        view.update(app, |this, cx| {
                            this.open_rename_strategy(core, strategy_id, window, cx);
                        });
                    }
                }));
                items.push(
                    MoonMenuItem::with_key("copy-strategy", t!("strat.menu_copy").to_string())
                        .right_label(t!("strat.copy_chord").to_string())
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
                items.push(cut_item(&view));
                items.push(move_to_folder_item(&view, core, spans_cores));
                // Prefill the case-insensitive substring search with the clicked strategy's full
                // name; additional names containing it may also match.
                let store = self.backend.read(cx).session.store();
                if let Some(name) = row(store, core, *id).map(|r| r.name.clone()) {
                    items.push(
                        MoonMenuItem::with_key(
                            "find-by-name",
                            t!("strat.menu_find_by_name").to_string(),
                        )
                        .on_click({
                            let view = view.clone();
                            move |_, window, app| {
                                window.close_context_menu(app);
                                view.update(app, |this, cx| {
                                    this.search_by_name(name.clone(), window, cx);
                                });
                            }
                        }),
                    );
                }
                let sel_block = {
                    let store = self.backend.read(cx).session.store();
                    let rows = self.selection_rows(store);
                    let refs: Vec<&StrategyRow> = rows.iter().map(|(_, r)| r).collect();
                    ops::delete_block(&refs)
                };
                let mut delete_strategy = MoonMenuItem::with_key(
                    "delete-strategy",
                    t!("strat.menu_delete_strategy").to_string(),
                )
                .tone(MoonTone::Danger)
                .disabled(sel_block.is_some());
                if let Some(block) = sel_block {
                    delete_strategy = delete_strategy
                        .right_label(t!("strat.reason_enabled", n = block.enabled).to_string());
                }
                items.push(delete_strategy.on_click({
                    let view = view.clone();
                    move |_, window, app| {
                        window.close_context_menu(app);
                        view.update(app, |this, cx| {
                            this.request_delete_selection(window, cx);
                        });
                    }
                }));
            }
            MenuTarget::Core => {
                let visible = {
                    let backend = self.backend.read(cx);
                    visible_strategy_cores(self, backend).iter().count()
                };
                let empty_core = self
                    .backend
                    .read(cx)
                    .session
                    .store()
                    .core(core)
                    .is_none_or(|cd| cd.strategies.is_empty());

                items.push(paste_here_item(&view, core, String::new(), paste_ready));
                let mut all = MoonMenuItem::with_key(
                    "paste-all-cores",
                    t!("strat.menu_paste_all_cores", n = visible).to_string(),
                )
                .disabled(!paste_ready);
                if !paste_ready {
                    all = all.right_label(t!("strat.reason_nothing_to_paste").to_string());
                }
                items.push(all.on_click({
                    let view = view.clone();
                    move |_, window, app| {
                        window.close_context_menu(app);
                        view.update(app, |this, cx| {
                            this.paste_into_all_visible(window, cx);
                            cx.notify();
                        });
                    }
                }));
                items.push(MoonMenuItem::separator());
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
                                this.open_create_strategy(core, String::new(), window, cx);
                            });
                        }
                    }),
                );
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
                                this.open_create_folder(core, String::new(), window, cx);
                            });
                        }
                    }),
                );
                // An empty path is what `copy_folder` already spells as "this whole core", so the
                // core-wide copy needs no second implementation.
                let mut copy_core =
                    MoonMenuItem::with_key("copy-core", t!("strat.menu_copy_core").to_string())
                        .disabled(empty_core);
                if empty_core {
                    copy_core = copy_core.right_label(t!("strat.reason_empty").to_string());
                }
                items.push(copy_core.on_click({
                    let view = view.clone();
                    move |_, window, app| {
                        window.close_context_menu(app);
                        view.update(app, |this, cx| {
                            this.copy_folder(core, Vec::new(), cx);
                            cx.notify();
                        });
                    }
                }));
                items.push(MoonMenuItem::separator());
                if self.rail_expanded_core != Some(core) {
                    let open = self.expanded_cores.contains(&core);
                    let (key, label) = match open {
                        true => ("collapse-core", t!("strat.menu_collapse_core")),
                        false => ("expand-core", t!("strat.menu_expand_core")),
                    };
                    items.push(MoonMenuItem::with_key(key, label.to_string()).on_click({
                        let view = view.clone();
                        move |_, window, app| {
                            window.close_context_menu(app);
                            view.update(app, |this, cx| {
                                this.toggle_core_expanded(core);
                                this.persist_session(cx);
                                cx.notify();
                            });
                        }
                    }));
                }
            }
            MenuTarget::DeletedFolder => {
                let rows: Vec<(u64, String)> = self
                    .deleted
                    .get(&core)
                    .map(|v| {
                        v.iter()
                            .map(|h| (h.strategy_id as u64, h.name.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                let n = rows.len();
                let ids: Vec<u64> = rows.iter().map(|(id, _)| *id).collect();
                items.push(
                    MoonMenuItem::with_key(
                        "forget-all",
                        t!("strat.menu_forget_all", n = n).to_string(),
                    )
                    .tone(MoonTone::Danger)
                    .disabled(n == 0)
                    .on_click({
                        let view = view.clone();
                        move |_, window, app| {
                            window.close_context_menu(app);
                            let ids = ids.clone();
                            view.update(app, |this, cx| {
                                let label = t!("strat.count_strategies", n = ids.len()).to_string();
                                this.request_forget(core, ids, label, window, cx);
                            });
                        }
                    }),
                );
            }
            MenuTarget::DeletedStrategy(id) => {
                // Restore under the original ID so version history and order profit remain joined
                // through `strategyid`.
                let id = *id;
                items.push(
                    MoonMenuItem::with_key(
                        "restore-strategy",
                        t!("strat.menu_restore").to_string(),
                    )
                    .on_click({
                        let view = view.clone();
                        move |_, window, app| {
                            window.close_context_menu(app);
                            view.update(app, |this, cx| {
                                this.restore_deleted_strategy(core, id, cx);
                            });
                        }
                    }),
                );
                let name = self
                    .deleted
                    .get(&core)
                    .and_then(|v| v.iter().find(|h| h.strategy_id as u64 == id))
                    .map(|h| h.name.clone())
                    .unwrap_or_default();
                items.push(
                    MoonMenuItem::with_key("forget-strategy", t!("strat.menu_forget").to_string())
                        .tone(MoonTone::Danger)
                        .on_click({
                            let view = view.clone();
                            move |_, window, app| {
                                window.close_context_menu(app);
                                let name = name.clone();
                                view.update(app, |this, cx| {
                                    this.request_forget(core, vec![id], name, window, cx);
                                });
                            }
                        }),
                );
            }
        }

        items
    }
}
