//! The editing half of the "WL distribution" tab: the menu a chip opens, "Distribute" with its
//! two switches, and "Reset".
//!
//! Nothing here writes to a core. Every action STAGES strategy field drafts — the same
//! `field_edits` the parameters pane keeps — so the chips show the change (struck through,
//! added) and the window's one Apply sends it with every other draft. A draft that brings a field
//! back to what the core stores is dropped rather than kept as a no-op.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonCheckbox, MoonContextMenuWindowExt as _, MoonMenuItem, MoonWindowExt as _,
    h_flex,
};
use rust_i18n::t;

use moon_core::session::CoreId;

use super::edit::{
    ChipAction, DistributeOptions, RowLists, apply_action, distribute, list_keys, restore,
    rewrite_list,
};
use super::view::{ListKind, effective, field};
use super::{BLACK_FIELD, Board, Edit, WHITE_FIELD};
use crate::design;
use crate::strategies::StrategiesView;

/// The longest value a string field can carry: the wire writes its length as 16 bits and wraps
/// a longer one (moonproto `strategy_serializer::writer::write_u16_len_bytes`), which would cut
/// the list mid-entry.
const MAX_FIELD_BYTES: usize = u16::MAX as usize;

/// What a menu entry does: one of the list actions, or putting the coin back as the core has it.
#[derive(Clone, Copy)]
enum MenuAction {
    Do(ChipAction),
    /// Put the coin back as the core has it, on the whitelist (`true`) or the blacklist.
    Restore(bool),
}

/// Width bounds of the chip menu, in theme units.
const MENU_MIN_WIDTH: f32 = 160.0;
const MENU_MAX_WIDTH: f32 = 320.0;

impl StrategiesView {
    /// The board the tab drew last, if it drew one.
    fn drawn_board(&self) -> Option<&Board> {
        self.dist
            .cache
            .as_ref()
            .and_then(|(_, m)| m.as_ref().as_ref().ok())
    }

    /// "Distribute", its two switches, and "Reset" when the board's lists hold drafts.
    pub(super) fn distribution_toolbar(&self, board: &Board, cx: &Context<Self>) -> AnyElement {
        let view = cx.entity();
        let first = self.dist.first_blacklists;
        let skip = self.dist.skip_common_black;
        let has_drafts = board.slots.iter().any(|slot| {
            slot.ids.iter().any(|id| {
                [WHITE_FIELD, BLACK_FIELD].iter().any(|f| {
                    self.field_edits
                        .contains_key(&(slot.core, *id, (*f).to_string()))
                })
            })
        });
        h_flex()
            .w_full()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap(design::ui_px(cx, 12.0))
            .px(design::ui_px(cx, 12.0))
            .py(design::ui_px(cx, 4.0))
            .child(
                MoonButton::new("strat-dist-distribute")
                    .primary()
                    .label(t!("strat.dist_distribute").to_string())
                    .tooltip(t!("strat.dist_distribute_tip").to_string())
                    .disabled(board.universe.is_none())
                    .on_click(cx.listener(|this, _, _, cx| this.distribute_board(cx)))
                    .render(),
            )
            .child(
                MoonCheckbox::new("strat-dist-first-bl")
                    .label(t!("strat.dist_first_bl").to_string())
                    .checked(first)
                    .on_change({
                        let view = view.clone();
                        move |value: &bool, _window, app| {
                            let value = *value;
                            view.update(app, |this, cx| {
                                this.dist.first_blacklists = value;
                                cx.notify();
                            });
                        }
                    }),
            )
            .child(
                // The checkbox has no tooltip of its own; its host carries one.
                div()
                    .id("strat-dist-skip-common-host")
                    .flex_none()
                    .tooltip(crate::panels::common::text_tooltip(
                        t!("strat.dist_skip_common_tip").to_string(),
                    ))
                    .child(
                        MoonCheckbox::new("strat-dist-skip-common")
                            .label(t!("strat.dist_skip_common").to_string())
                            .checked(skip)
                            .on_change(move |value: &bool, _window, app| {
                                let value = *value;
                                view.update(app, |this, cx| {
                                    this.dist.skip_common_black = value;
                                    cx.notify();
                                });
                            }),
                    ),
            )
            .when(has_drafts, |row| {
                row.child(
                    MoonButton::new("strat-dist-reset")
                        .ghost()
                        .label(t!("strat.dist_reset").to_string())
                        .tooltip(t!("strat.dist_reset_tip").to_string())
                        .on_click(cx.listener(|this, _, _, cx| this.reset_board_drafts(cx)))
                        .render(),
                )
            })
            .into_any_element()
    }

    /// Open the menu of one chip at the pointer — the chip's right-click.
    pub(super) fn open_chip_menu(
        &mut self,
        core: CoreId,
        kind: ListKind,
        coin: String,
        edit: Edit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A copy is offered only where it would add something.
        let slot = self
            .drawn_board()
            .and_then(|b| b.slots.iter().find(|s| s.core == core));
        // Already there for every strategy of the row? `lists` is their union, so a row whose
        // strategies disagree is never "already there" — the copy still adds to some of them.
        let in_list = |white: bool| {
            slot.is_some_and(|s| {
                let list = if white {
                    &s.lists.white
                } else {
                    &s.lists.black
                };
                !s.lists_differ && list.contains(&coin)
            })
        };
        // A row whose whitelist is empty trades the whole market less its blacklist; one copied
        // coin would make it trade that coin alone.
        let white_empty = slot.is_some_and(|s| s.lists.white.is_empty());
        let mut actions: Vec<(MenuAction, &str)> = Vec::new();
        match (kind, edit) {
            (ListKind::White, Edit::Removed) => {
                actions.push((MenuAction::Restore(true), "strat.dist_menu_restore_white"))
            }
            (ListKind::White, _) => {
                if !in_list(false) {
                    actions.push((
                        MenuAction::Do(ChipAction::AddBlack),
                        "strat.dist_menu_copy_to_black",
                    ));
                }
                actions.push((
                    MenuAction::Do(ChipAction::RemoveWhite),
                    "strat.dist_menu_remove_white",
                ));
            }
            (ListKind::Black, Edit::Removed) => {
                actions.push((MenuAction::Restore(false), "strat.dist_menu_restore_black"))
            }
            (ListKind::Black, _) => {
                if !in_list(true) && !white_empty {
                    actions.push((
                        MenuAction::Do(ChipAction::AddWhite),
                        "strat.dist_menu_copy_to_white",
                    ));
                }
                actions.push((
                    MenuAction::Do(ChipAction::RemoveBlack),
                    "strat.dist_menu_remove_black",
                ));
            }
            // A coin the drafts take out of trading went there through the blacklist; putting
            // it back is undoing that.
            (ListKind::Traded, Edit::Removed) => {
                actions.push((MenuAction::Restore(false), "strat.dist_menu_restore_traded"))
            }
            (ListKind::Traded, _) => actions.push((
                MenuAction::Do(ChipAction::AddBlack),
                "strat.dist_menu_add_black",
            )),
        }
        let view = cx.entity();
        let items: Vec<MoonMenuItem> = actions
            .iter()
            .map(|(action, key)| {
                let (view, coin, action) = (view.clone(), coin.clone(), *action);
                MoonMenuItem::with_key(*key, t!(*key, coin = coin).to_string()).on_click(
                    move |_, window, app| {
                        window.close_context_menu(app);
                        view.update(app, |this, cx| {
                            this.stage_row_edit(
                                core,
                                |lists, live| match action {
                                    MenuAction::Do(action) => apply_action(lists, &coin, action),
                                    MenuAction::Restore(white) => {
                                        restore(lists, live, &coin, white)
                                    }
                                },
                                cx,
                            )
                        });
                    },
                )
            })
            .collect();
        window.open_fitted_moon_context_menu(
            cx,
            "strat-dist-chip-menu",
            window.mouse_position(),
            items,
            MENU_MIN_WIDTH,
            MENU_MAX_WIDTH,
        );
    }

    /// Stage one row's edit into every selected strategy of that core, each from its OWN lists.
    ///
    /// Refused while a saved version is on view: its panes are read-only, and a draft staged
    /// behind them would surface in that version's banner and in Apply.
    ///
    /// Args:
    ///     core: The row.
    ///     edit: The new lists given a strategy's lists as they will be and as the core has them.
    ///     cx: View context.
    fn stage_row_edit(
        &mut self,
        core: CoreId,
        edit: impl Fn(&RowLists, &RowLists) -> RowLists,
        cx: &mut Context<Self>,
    ) {
        if self.viewing_version() {
            return;
        }
        let Some(ids) = self
            .drawn_board()
            .and_then(|b| b.slots.iter().find(|s| s.core == core))
            .map(|s| s.ids.clone())
        else {
            return;
        };
        for id in ids {
            self.stage_strategy_lists(core, id, &edit, cx);
        }
        cx.notify();
    }

    /// Write one strategy's new lists into its drafts, dropping a draft that matches the core.
    fn stage_strategy_lists(
        &mut self,
        core: CoreId,
        id: u64,
        edit: &impl Fn(&RowLists, &RowLists) -> RowLists,
        cx: &mut Context<Self>,
    ) {
        use crate::strategies::logic::{pending_field_value, schema_field_in_kind};
        let staged = {
            let store = self.backend.read(cx).session.store();
            let Some(row) = store
                .core(core)
                .and_then(|cd| cd.strategies.iter().find(|r| r.id == id))
            else {
                return;
            };
            // A kind whose schema has no such field — or no schema yet — gets no draft: Apply
            // would send a field the strategy does not have.
            let (Some(white_schema), Some(black_schema)) = (
                schema_field_in_kind(store, core, row.kind_ordinal, WHITE_FIELD),
                schema_field_in_kind(store, core, row.kind_ordinal, BLACK_FIELD),
            ) else {
                return;
            };
            let pending = store.core(core).and_then(|cd| cd.strategy_edit(id));
            // What the field holds with no draft of ours: an edit on its way, else the core's.
            let base = |schema| {
                pending_field_value(pending, row, schema)
                    .unwrap_or_else(|| field(&row.fields, &schema.name).to_string())
            };
            let current_white = effective(self, store, core, row, WHITE_FIELD);
            let current_black = effective(self, store, core, row, BLACK_FIELD);
            let current = RowLists {
                white: list_keys(&current_white),
                black: list_keys(&current_black),
            };
            let live = RowLists {
                white: list_keys(field(&row.fields, WHITE_FIELD)),
                black: list_keys(field(&row.fields, BLACK_FIELD)),
            };
            let next = edit(&current, &live);
            // A coin new to a list is spelled as the strategy writes it elsewhere — the other
            // list, or either field as the core has it — so a contract entry stays a contract.
            let live_white = field(&row.fields, WHITE_FIELD);
            let live_black = field(&row.fields, BLACK_FIELD);
            [
                (
                    WHITE_FIELD,
                    rewrite_list(
                        &current_white,
                        &next.white,
                        &[&current_black, live_white, live_black],
                    ),
                    base(white_schema),
                ),
                (
                    BLACK_FIELD,
                    rewrite_list(
                        &current_black,
                        &next.black,
                        &[&current_white, live_black, live_white],
                    ),
                    base(black_schema),
                ),
            ]
        };
        // Both fields or neither: an edit landing on one list only is worse than no edit.
        if let Some((name, value, _)) = staged.iter().find(|(_, v, _)| v.len() > MAX_FIELD_BYTES) {
            log::warn!(
                "distribution: {name} of strategy {id} on core {core} would be {} bytes, over the \
                 wire's {MAX_FIELD_BYTES}; the edit is not staged",
                value.len()
            );
            return;
        }
        for (name, value, base) in staged {
            let key = (core, id, name.to_string());
            // Back to what the field holds without us — the same entries, in any order — is no
            // edit: dropping the draft also keeps the field's own spellings and order.
            let entries = |text: &str| {
                let mut keys = list_keys(text);
                keys.sort_unstable();
                keys
            };
            if entries(&value) == entries(&base) {
                self.field_edits.remove(&key);
            } else {
                self.field_edits.insert(key, value);
            }
        }
    }

    /// Deal the market out between the board's rows and stage the result.
    fn distribute_board(&mut self, cx: &mut Context<Self>) {
        let Some(board) = self.drawn_board() else {
            return;
        };
        let Some(universe) = board.universe.clone() else {
            return;
        };
        let rows: Vec<RowLists> = board.slots.iter().map(|s| s.lists.clone()).collect();
        let cores: Vec<CoreId> = board.slots.iter().map(|s| s.core).collect();
        let options = DistributeOptions {
            first_blacklists: self.dist.first_blacklists,
            skip_common_black: self.dist.skip_common_black,
        };
        let targets = distribute(&rows, &universe, options);
        for (core, target) in cores.into_iter().zip(targets) {
            // Every strategy of the row receives the row's lists whole.
            self.stage_row_edit(core, |_, _| target.clone(), cx);
        }
    }

    /// Drop every whitelist / blacklist draft of the board's strategies.
    fn reset_board_drafts(&mut self, cx: &mut Context<Self>) {
        let Some(board) = self.drawn_board() else {
            return;
        };
        let keys: Vec<(CoreId, u64)> = board
            .slots
            .iter()
            .flat_map(|s| s.ids.iter().map(|id| (s.core, *id)))
            .collect();
        for (core, id) in keys {
            for name in [WHITE_FIELD, BLACK_FIELD] {
                self.field_edits.remove(&(core, id, name.to_string()));
            }
        }
        cx.notify();
    }
}
