//! The Hotkeys page on `SettingsView`: fills [`super::page`]'s row model from the draft and
//! hands it the live editors — the key field, the gesture and move-kind pickers, the split-parts,
//! wheel and ruler pickers — each of which writes the draft. The header's "Взять раскладку с
//! ядра…" shows what a core's layout would change in a dialog and, on confirm, writes it into the
//! draft only.

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::config::moonbot_import::shortcut;
use moon_core::config::{
    GestureSlot, HotkeysConfig, KeySlot, MouseGestureBinding, MoveKind, MoveKindSlot, RulerDrag,
    SPLIT_PARTS_MAX, SPLIT_PARTS_MIN, WheelModifier,
};
use moon_core::session::CoreId;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonDropdown, MoonHotkeyInput,
    MoonInput, MoonMenuItem, MoonPalette, MoonText, MoonWindowExt, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::clash::{Clash, Clashes, Severity};
use super::page::{self, EDITOR_W, HkRow, RowText};
use super::pull::{PullRow, PullVerdict, apply_pull, preview_core_hotkeys};
use super::pull_gestures::{self, GesturePullRow, preview_core_gestures};
use super::registry::{self, HotkeyGroup, Row, SlotSpec};
use super::set_gesture_mirrored;
use crate::design;
use crate::hotkeys::meta;
use crate::settings::SettingsView;
use crate::settings::common::Action;

/// What the pull dialog lists: the key rows and gesture rows a core's layout would change.
struct PullDiff {
    keys: Vec<PullRow>,
    gestures: Vec<GesturePullRow>,
}

impl PullDiff {
    fn changes(&self) -> usize {
        self.keys
            .iter()
            .filter(|r| r.verdict == PullVerdict::WillApply)
            .count()
            + self
                .gestures
                .iter()
                .filter(|r| r.verdict == PullVerdict::WillApply)
                .count()
    }
}

impl SettingsView {
    /// The Hotkeys page: banner, search, categories, the command table, the mirror switch on the
    /// Mouse category, and the built-in combos; "Взять раскладку с ядра…", Copy and Paste in its
    /// header.
    pub(in crate::settings) fn hotkeys_page(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> crate::settings::frame::PageParts {
        let hotkeys = {
            let b = self.backend.read(cx);
            b.preview.as_ref().unwrap_or(&b.config).hotkeys.clone()
        };
        let clashes = Clashes::build(&hotkeys);
        let query = self.hotkeys_query.read(cx).value().to_string();
        let searching = !query.trim().is_empty();
        let group = self.hotkeys_group;
        let mouse = !searching && group == HotkeyGroup::Mouse;

        // Search runs over every category; otherwise the active one.
        let candidates: Vec<Row> = registry::rows()
            .iter()
            .filter(|row| !matches!(row, Row::SameForMove))
            .filter(|row| searching || row.group() == group)
            .copied()
            .collect();
        let problems: Vec<Vec<Clash>> = candidates
            .iter()
            .map(|row| row_problems(*row, &hotkeys, &clashes))
            .collect();
        let texts: Vec<RowText> = candidates
            .iter()
            .zip(&problems)
            .map(|(row, problems)| row_text(*row, &hotkeys, problems))
            .collect();
        let shown = page::filter_rows(&texts, &query, self.hotkeys_conflicts_only);
        let rows: Vec<HkRow> = shown
            .into_iter()
            .map(|ix| self.hk_row(candidates[ix], &hotkeys, &problems[ix], mouse, cx))
            .collect();

        let conflicts_per_group: Vec<u32> = HotkeyGroup::ALL
            .iter()
            .map(|g| {
                let n = registry::rows()
                    .iter()
                    .filter(|row| row.group() == *g && !matches!(row, Row::SameForMove))
                    .filter(|row| {
                        row_problems(**row, &hotkeys, &clashes)
                            .iter()
                            .any(|c| c.severity == Severity::Shadowed)
                    })
                    .count();
                u32::try_from(n).unwrap_or(u32::MAX)
            })
            .collect();

        let weak = cx.entity().downgrade();
        let toggle_conflicts: Action = {
            let weak = weak.clone();
            Rc::new(move |_, cx| {
                let _ = weak.update(cx, |this, cx| {
                    this.hotkeys_conflicts_only = !this.hotkeys_conflicts_only;
                    cx.notify();
                });
            })
        };
        let pick: Rc<dyn Fn(HotkeyGroup, &mut Window, &mut App)> = {
            let weak = weak.clone();
            Rc::new(move |g, _, cx| {
                let _ = weak.update(cx, |this, cx| {
                    if this.hotkeys_group != g {
                        this.hotkeys_group = g;
                        cx.notify();
                    }
                });
            })
        };
        let toggle_builtin: Action = {
            let weak = weak.clone();
            Rc::new(move |_, cx| {
                let _ = weak.update(cx, |this, cx| {
                    this.hotkeys_builtin_open = !this.hotkeys_builtin_open;
                    cx.notify();
                });
            })
        };

        let p = MoonPalette::active(cx);
        let search = div().w_full().max_w(design::ui_px(cx, 340.0)).child(
            MoonInput::new("hotkeys-search")
                .state(&self.hotkeys_query)
                .small()
                .cleanable(true)
                .prefix(
                    svg()
                        .path("icons/search.svg")
                        .size(design::ui_px(cx, 14.0))
                        .text_color(rgba_from(p.text_muted, 1.0)),
                ),
        );
        let empty = if self.hotkeys_conflicts_only {
            t!("hotkeys.conflicts.none")
        } else {
            t!("hotkeys.search.empty")
        }
        .to_string();
        let body = v_flex()
            .w_full()
            .gap(design::ui_px(cx, 12.0))
            .children(page::banner(
                super::conflict_count(&hotkeys),
                self.hotkeys_conflicts_only,
                toggle_conflicts,
                cx,
            ))
            .child(search)
            .children((!searching).then(|| {
                v_flex()
                    .w_full()
                    .gap(design::ui_px(cx, 6.0))
                    .child(page::categories(group, &conflicts_per_group, pick, cx))
                    .child(
                        div()
                            .text_size(design::t_caption(cx))
                            .text_color(rgba_from(p.text_muted, 1.0))
                            .child(group.hint()),
                    )
            }))
            .child(page::table(rows, mouse, empty, cx))
            .children(mouse.then(|| self.same_move_checkbox(&hotkeys, cx)))
            .child(page::builtin(self.hotkeys_builtin_open, toggle_builtin, cx))
            .into_any_element();

        let mut actions = vec![
            MoonButton::new("hotkeys-pull")
                .variant(moon_ui::MoonButtonVariant::Neutral)
                .label(t!("hotkeys.pull.open").to_string())
                .on_click(cx.listener(|this, _, window, cx| this.open_core_pull(window, cx)))
                .render()
                .into_any_element(),
        ];
        actions.extend(self.share_actions(cx));
        self.page_parts(crate::settings::Tab::Hotkeys, body, actions, cx)
    }

    /// One table row for `row`: name, MoonBot name, scope, problems and the editors.
    fn hk_row(
        &self,
        row: Row,
        hotkeys: &HotkeysConfig,
        problems: &[Clash],
        mouse_category: bool,
        cx: &Context<Self>,
    ) -> HkRow {
        let problems: Vec<(String, Severity)> = problems
            .iter()
            .map(|c| (c.text.clone(), c.severity))
            .collect();
        match row {
            Row::Slot(spec) => self.slot_row(spec, hotkeys, problems, mouse_category, cx),
            Row::LabelScroll => HkRow {
                id: "label-scroll".to_string(),
                name: t!("hotkeys.label_scroll").to_string(),
                moonbot: None,
                hint: Some(t!("hotkeys.label_scroll_hint").to_string()),
                scope: meta::LABEL_SCROLL.scope.resolved().label(),
                problems,
                muted: false,
                editors: vec![self.wheel_dropdown(hotkeys, cx).into_any_element()],
                add_gesture: None,
            },
            Row::RulerDrag => HkRow {
                id: "ruler-drag".to_string(),
                name: t!("hotkeys.ruler_drag").to_string(),
                moonbot: None,
                hint: Some(t!("hotkeys.ruler_drag_hint").to_string()),
                scope: meta::RULER_DRAG.scope.resolved().label(),
                problems,
                muted: false,
                editors: vec![self.ruler_dropdown(hotkeys, cx).into_any_element()],
                add_gesture: None,
            },
            Row::SameForMove => unreachable!("the mirror switch is drawn under the table"),
        }
    }

    /// A slot's row: its key field, and its gesture (with the move kind) once shown.
    fn slot_row(
        &self,
        spec: SlotSpec,
        hotkeys: &HotkeysConfig,
        problems: Vec<(String, Severity)>,
        mouse_category: bool,
        cx: &Context<Self>,
    ) -> HkRow {
        // A greyed short row follows its long twin while the mirror switch is on.
        let disabled = spec.follows_mirror() && hotkeys.same_hotkeys_for_move;
        let id = match (spec.key(), spec.mouse()) {
            (Some(key), _) => registry::key_id(key),
            (None, Some(mouse)) => registry::gesture_id(mouse),
            (None, None) => unreachable!("a row edits at least one slot"),
        };
        let mut editors: Vec<AnyElement> = Vec::new();
        if let Some(key) = spec.key() {
            editors.push(self.hotkey_input(key, hotkeys, cx).into_any_element());
            if key == KeySlot::SplitOrderX {
                editors.push(self.split_parts_dropdown(hotkeys, cx));
            }
        }
        let mut add_gesture = None;
        if let Some(mouse) = spec.mouse() {
            let set = hotkeys.gesture(mouse) != MouseGestureBinding::None;
            let shown = spec.key().is_none()
                || page::gesture_shown(
                    mouse_category,
                    set,
                    self.hotkeys_gesture_open.contains(&id),
                );
            if shown {
                editors.push(
                    self.gesture_dropdown(mouse, hotkeys, disabled, cx)
                        .into_any_element(),
                );
                if let Some(kind) = spec.kind() {
                    editors.push(self.move_kind_dropdown(kind, hotkeys, disabled, cx));
                }
            } else {
                let weak = cx.entity().downgrade();
                let open_id = id.clone();
                add_gesture = Some(Rc::new(move |_: &mut Window, cx: &mut App| {
                    let _ = weak.update(cx, |this, cx| {
                        this.hotkeys_gesture_open.insert(open_id.clone());
                        cx.notify();
                    });
                }) as Action);
            }
        }
        HkRow {
            id,
            name: spec.name(),
            moonbot: spec.moonbot_name(),
            hint: Some(spec.hint()),
            scope: spec.meta().scope.resolved().label(),
            problems,
            muted: disabled,
            editors,
            add_gesture,
        }
    }

    /// The keystroke editor of one row.
    fn hotkey_input(
        &self,
        slot: KeySlot,
        hotkeys: &HotkeysConfig,
        cx: &Context<Self>,
    ) -> MoonHotkeyInput {
        let raw = hotkeys.key(slot);
        let parsed = crate::hotkeys::parse_binding(raw);
        let invalid = !raw.trim().is_empty() && parsed.is_none();
        MoonHotkeyInput::new(format!("hotkey-{}", registry::key_id(slot)))
            .value(parsed)
            .placeholder(t!("hotkeys.unassigned").to_string())
            .recording_placeholder(t!("hotkeys.recording").to_string())
            .invalid(invalid)
            // No `.conflict()`: the component paints that frame AMBER and stamps an unlocalized
            // "conflict" badge inside the field; the Проблема column says it in words and in the
            // right colour.
            .compact()
            .width(EDITOR_W)
            .on_change(
                cx.processor(move |this, value: Option<Keystroke>, _window, cx| {
                    // Store the PHYSICAL key: a letter recorded under a Cyrillic layout would
                    // otherwise be saved as that layout's character.
                    let value = value
                        .map(|k| crate::hotkeys::recorded_keystroke(k).unparse())
                        .unwrap_or_default();
                    this.set_hotkey(slot, value, cx);
                }),
            )
    }

    /// The gesture editor of one row.
    fn gesture_dropdown(
        &self,
        slot: GestureSlot,
        hotkeys: &HotkeysConfig,
        disabled: bool,
        cx: &App,
    ) -> MoonDropdown {
        let current = hotkeys.gesture(slot);
        let backend = self.backend.clone();
        let items = MouseGestureBinding::ALL.into_iter().map(move |gesture| {
            let backend = backend.clone();
            MoonMenuItem::with_key(gesture.config_value(), gesture.menu_label())
                .checked(gesture == current)
                .on_click(move |_, _, cx| {
                    backend.update(cx, |b, bcx| {
                        if let Some(p) = b.preview.as_mut()
                            && set_gesture_mirrored(&mut p.hotkeys, slot, gesture)
                        {
                            bcx.notify();
                        }
                    });
                })
        });
        let label = if current == MouseGestureBinding::None {
            t!("hotkeys.unassigned").to_string()
        } else {
            current.label().to_string()
        };
        Self::row_dropdown(format!("mouse-{}", registry::gesture_id(slot)), label, cx)
            .trigger_variant(if current == MouseGestureBinding::None {
                MoonButtonVariant::Neutral
            } else {
                MoonButtonVariant::Blue
            })
            .menu_width_scaled(228.0)
            .disabled(disabled)
            .items(items)
    }

    /// The "Move kind" selector of one move row — Moonbot's column of the same name. The gesture
    /// says WHERE; this says which orders go there. `None` leaves the gesture recognised and inert.
    fn move_kind_dropdown(
        &self,
        slot: MoveKindSlot,
        hotkeys: &HotkeysConfig,
        disabled: bool,
        cx: &App,
    ) -> AnyElement {
        let current = hotkeys.move_kind(slot);
        let backend = self.backend.clone();
        let items = MoveKind::ALL.into_iter().map(move |kind| {
            let backend = backend.clone();
            let label_key = kind.locale_key();
            MoonMenuItem::with_key(kind.id(), t!(&label_key).to_string())
                .checked(kind == current)
                .on_click(move |_, _, cx| {
                    backend.update(cx, |b, bcx| {
                        if let Some(p) = b.preview.as_mut()
                            && p.hotkeys.set_move_kind(slot, kind)
                        {
                            bcx.notify();
                        }
                    });
                })
        });
        let current_key = current.locale_key();
        Self::row_dropdown(
            format!("move-kind-{}", registry::gesture_id(slot.half(false))),
            t!(&current_key).to_string(),
            cx,
        )
        // The trigger shows the chosen kind, so the menu carries the setting's name.
        .header(18.0, |_, cx| {
            let p = MoonPalette::active(cx);
            MoonText::new(t!("hotkeys.move_kind.title").to_string())
                .uppercase(false)
                .mono(false)
                .rendered_metrics(design::text_metrics(cx, -2.0, 12.0))
                .color(p.text_muted)
                .render()
                .into_any_element()
        })
        .trigger_variant(if current == MoveKind::None {
            MoonButtonVariant::Neutral
        } else {
            MoonButtonVariant::Blue
        })
        .menu_width_scaled(228.0)
        .disabled(disabled)
        .items(items)
        .into_any_element()
    }

    /// A row editor's dropdown at the key column's width.
    fn row_dropdown(id: String, label: impl Into<SharedString>, cx: &App) -> MoonDropdown {
        MoonDropdown::new(SharedString::from(id))
            .label(label)
            .trigger_caret(true)
            .trigger_size(MoonButtonSize::density(cx))
            .trigger_width(design::ui_value(cx, EDITOR_W))
    }

    /// The part count of `Split N` (Moonbot `Hotkeys.SplitParts`), under the `Split Order X` key.
    /// A picker over the allowed range: the value goes straight into a live split command.
    fn split_parts_dropdown(&self, hotkeys: &HotkeysConfig, cx: &Context<Self>) -> AnyElement {
        let current = hotkeys.split_n_parts();
        let items = (SPLIT_PARTS_MIN..=SPLIT_PARTS_MAX).map(|parts| {
            let backend = self.backend.clone();
            MoonMenuItem::with_key(format!("split-parts-{parts}"), parts.to_string())
                .checked(i32::from(parts) == current)
                .on_click(move |_, _, cx| {
                    backend.update(cx, |b, bcx| {
                        if let Some(preview) = b.preview.as_mut()
                            && preview.hotkeys.split_parts != parts
                        {
                            preview.hotkeys.split_parts = parts;
                            bcx.notify();
                        }
                    });
                })
        });
        Self::row_dropdown(
            "hotkey-split-parts".into(),
            t!("hotkeys.split_parts_value", n = current).to_string(),
            cx,
        )
        .trigger_variant(MoonButtonVariant::Blue)
        .menu_width_scaled(120.0)
        .items(items)
        .into_any_element()
    }

    /// The move-mirroring switch, under the Mouse category's table: it owns the four short move
    /// rows.
    fn same_move_checkbox(&self, hotkeys: &HotkeysConfig, cx: &App) -> AnyElement {
        let backend = self.backend.clone();
        let label = t!("hotkeys.mouse.same_move").to_string();
        let checkbox = MoonCheckbox::new("same-hotkeys-for-move")
            .label(label.clone())
            .checked(hotkeys.same_hotkeys_for_move)
            .on_change(move |value, _window, cx| {
                backend.update(cx, |b, bcx| {
                    if let Some(p) = b.preview.as_mut() {
                        let changed = p.hotkeys.same_hotkeys_for_move != *value;
                        p.hotkeys.same_hotkeys_for_move = *value;
                        if *value {
                            // Mirrors only on the way ON, as Moonbot's own dialog does.
                            for row in MoveKindSlot::ALL {
                                let long = p.hotkeys.gesture(row.half(false));
                                p.hotkeys.set_gesture(row.half(true), long);
                            }
                        }
                        if changed {
                            bcx.notify();
                        }
                    }
                });
            });
        crate::settings::common::floored_checkbox("same-hotkeys-for-move", &label, checkbox, cx)
    }

    /// The wheel-modifier editor of the label-scroll row.
    fn wheel_dropdown(&self, hotkeys: &HotkeysConfig, cx: &App) -> MoonDropdown {
        let current = hotkeys.label_scroll_wheel;
        let backend = self.backend.clone();
        let items = WheelModifier::ALL.into_iter().map(move |wheel| {
            let backend = backend.clone();
            MoonMenuItem::with_key(wheel.config_value(), wheel_label(wheel))
                .checked(wheel == current)
                .on_click(move |_, _, cx| {
                    backend.update(cx, |b, bcx| {
                        if let Some(p) = b.preview.as_mut()
                            && p.hotkeys.label_scroll_wheel != wheel
                        {
                            p.hotkeys.label_scroll_wheel = wheel;
                            bcx.notify();
                        }
                    });
                })
        });
        Self::row_dropdown("label-scroll-wheel".to_string(), wheel_label(current), cx)
            .trigger_variant(if current == WheelModifier::None {
                MoonButtonVariant::Neutral
            } else {
                MoonButtonVariant::Blue
            })
            .menu_width_scaled(228.0)
            .items(items)
    }

    /// The modifier editor of the percent-ruler row.
    fn ruler_dropdown(&self, hotkeys: &HotkeysConfig, cx: &App) -> MoonDropdown {
        let current = hotkeys.ruler_drag;
        let backend = self.backend.clone();
        let items = RulerDrag::ALL.into_iter().map(move |drag| {
            let backend = backend.clone();
            MoonMenuItem::with_key(drag.config_value(), ruler_label(drag))
                .checked(drag == current)
                .on_click(move |_, _, cx| {
                    backend.update(cx, |b, bcx| {
                        if let Some(p) = b.preview.as_mut()
                            && p.hotkeys.ruler_drag != drag
                        {
                            p.hotkeys.ruler_drag = drag;
                            bcx.notify();
                        }
                    });
                })
        });
        Self::row_dropdown("ruler-drag".to_string(), ruler_label(current), cx)
            .trigger_variant(if current == RulerDrag::None {
                MoonButtonVariant::Neutral
            } else {
                MoonButtonVariant::Blue
            })
            .menu_width_scaled(228.0)
            .items(items)
    }

    fn set_hotkey(&mut self, slot: KeySlot, value: String, cx: &mut Context<Self>) {
        let changed = self.backend.update(cx, |b, bcx| {
            let mut changed = false;
            if let Some(p) = b.preview.as_mut() {
                changed = p.hotkeys.set_key(slot, value);
                if changed {
                    bcx.notify();
                }
            }
            changed
        });
        if changed {
            cx.notify();
        }
    }

    /// Resolves the core whose layout "Взять раскладку с ядра…" addresses: the group's active
    /// trade core. With ONE group window open that group's active core is the answer in either
    /// workspace mode; with several, `Backend::singleton_workspace` resolves it (Auto-trading
    /// groups only, the same resolver the Strategies and Analytics windows use).
    fn core_pull_target(&self, cx: &Context<Self>) -> Option<CoreId> {
        let b = self.backend.read(cx);
        let mut groups = b.group_windows.keys();
        let group = match (groups.next(), groups.next()) {
            (Some(only), None) => only.clone(),
            _ => b.singleton_workspace()?.group,
        };
        b.active_trade_core(&group)
    }

    /// Asks `core` for a fresh copy of its configuration and arms the pending state, which clears
    /// once the store's `core_config_recv_rev` moves past the baseline (an identical echo bumps
    /// it too, unlike `core_config_rev`).
    fn request_core_pull(&mut self, core: CoreId, cx: &mut Context<Self>) {
        let baseline = self
            .backend
            .read(cx)
            .session
            .store()
            .core(core)
            .map(|d| d.core_config_recv_rev)
            .unwrap_or(0);
        if let Err(error) = self.backend.read(cx).session.refresh_shared_config(core) {
            self.status = Some((crate::settings::StatusMsg::Text(error.to_string()), true));
        }
        self.core_pull = Some((core, baseline));
        cx.notify();
    }

    /// The header button: resolve the core, refresh its layout, and show what it would change in
    /// a dialog. Without a layout yet the status line says so; the refresh is already on its way.
    fn open_core_pull(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(core) = self.core_pull_target(cx) else {
            self.status = Some((
                crate::settings::StatusMsg::Key("hotkeys.pull.no_core"),
                true,
            ));
            cx.notify();
            return;
        };
        let pending = self.core_pull.is_some_and(|(c, baseline)| {
            c == core
                && self
                    .backend
                    .read(cx)
                    .session
                    .store()
                    .core(core)
                    .map(|d| d.core_config_recv_rev)
                    == Some(baseline)
        });
        if !pending {
            self.request_core_pull(core, cx);
        }
        let Some(diff) = self.pull_diff(core, cx) else {
            self.status = Some((
                crate::settings::StatusMsg::Key(if pending {
                    "hotkeys.pull.pending"
                } else {
                    "hotkeys.pull.empty"
                }),
                false,
            ));
            cx.notify();
            return;
        };
        self.open_pull_dialog(diff, window, cx);
    }

    /// What `core`'s retained layout would change in the draft, or `None` before it arrived.
    fn pull_diff(&self, core: CoreId, cx: &App) -> Option<PullDiff> {
        let b = self.backend.read(cx);
        let c = b.session.store().core(core)?.core_config.as_ref()?;
        let draft = &b.preview.as_ref().unwrap_or(&b.config).hotkeys;
        Some(PullDiff {
            keys: preview_core_hotkeys(
                draft,
                &c.manual.core_hotkeys,
                &c.manual.strat_buttons.hot_keys,
            ),
            gestures: preview_core_gestures(draft, &c.gestures),
        })
    }

    /// The pull dialog: every change the core's layout makes, the conflicts it skips, and "Взять
    /// в черновик".
    fn open_pull_dialog(&mut self, diff: PullDiff, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        let changes = diff.changes();
        let diff = Rc::new(diff);
        window.open_unique_moon_dialog("hotkeys-pull", cx, move |dialog, _window, cx| {
            let p = MoonPalette::active(cx);
            let apply_weak = weak.clone();
            let shown = diff.clone();
            let footer = h_flex()
                .w_full()
                .justify_end()
                .gap_2()
                .child(
                    MoonButton::new("hotkeys-pull-cancel")
                        .ghost()
                        .label(t!("hotkeys.pull.cancel").to_string())
                        .on_click(|_, window, cx| window.close_dialog(cx))
                        .render(),
                )
                .child(
                    MoonButton::new("hotkeys-pull-apply")
                        .variant(MoonButtonVariant::Blue)
                        .disabled(changes == 0)
                        .label(t!("hotkeys.pull.apply").to_string())
                        .on_click(move |_, window, cx| {
                            window.close_dialog(cx);
                            let _ = apply_weak
                                .update(cx, |this, cx| this.confirm_core_pull(&shown, cx));
                        })
                        .render(),
                );
            let diff = diff.clone();
            dialog
                .w(design::ui_px(cx, 520.0))
                .close_button(true)
                .overlay(true)
                .overlay_closable(true)
                .bg(rgba_from(p.shell_high, 1.0))
                .border_color(rgba_from(p.border, 1.0))
                .rounded(design::r_container(cx))
                .text_color(rgba_from(p.text, 1.0))
                .header(
                    div()
                        .w_full()
                        .py_2()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t!("hotkeys.pull.title").to_string()),
                )
                .content(move |content, _window, cx| content.child(pull_body(&diff, cx)))
                .footer(footer.into_any_element())
        });
    }

    /// Writes the diff the dialog SHOWED into the draft — never a layout re-read at click time
    /// (a refresh may have replaced it while the dialog was open), never the saved configuration
    /// or `hotkeys.toml`. Save persists it like any other edit.
    fn confirm_core_pull(&mut self, diff: &PullDiff, cx: &mut Context<Self>) {
        let changed = self.backend.update(cx, |b, bcx| {
            let draft = b.preview.as_mut()?;
            let changed = apply_pull(&mut draft.hotkeys, &diff.keys, &diff.gestures);
            if changed {
                bcx.notify();
            }
            Some(changed)
        });
        self.status = match changed {
            Some(true) => Some((
                crate::settings::StatusMsg::Key("hotkeys.pull.applied"),
                false,
            )),
            Some(false) => Some((
                crate::settings::StatusMsg::Key("hotkeys.pull.nothing_to_apply"),
                false,
            )),
            None => None,
        };
        self.core_pull = None;
        cx.notify();
    }
}

/// The dialog's body: a summary line, each change as "name: current → incoming" with the keys in
/// mono, then the conflicts that will not apply.
fn pull_body(diff: &PullDiff, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    let changes = diff.changes();
    let mut will: Vec<AnyElement> = Vec::new();
    let mut skipped: Vec<AnyElement> = Vec::new();
    let unset = t!("hotkeys.unassigned").to_string();
    for row in &diff.keys {
        let current = if row.current.trim().is_empty() {
            unset.clone()
        } else {
            row.current.clone()
        };
        let line = pull_change_line(
            registry::key_name(row.slot),
            current,
            shortcut::display(row.core_decoded),
            cx,
        );
        match row.verdict {
            PullVerdict::WillApply => will.push(line),
            PullVerdict::Conflict => skipped.push(line),
            _ => {}
        }
    }
    for row in &diff.gestures {
        if row.verdict == PullVerdict::WillApply {
            will.push(pull_change_line(
                pull_gestures::target_label(row.target),
                row.current.clone(),
                row.incoming.clone(),
                cx,
            ));
        }
    }
    let caption = |text: String, color: u32| {
        div()
            .pt(design::ui_px(cx, 6.0))
            .text_size(design::t_caption(cx))
            .text_color(rgba_from(color, 1.0))
            .child(text)
    };
    v_flex()
        .id("hotkeys-pull-body")
        .w_full()
        .max_h(design::ui_px(cx, 360.0))
        .overflow_y_scroll()
        .gap(design::ui_px(cx, 4.0))
        .child(div().child(t!("hotkeys.pull.summary", n = changes).to_string()))
        .when(!will.is_empty(), |el| {
            el.child(caption(
                t!("hotkeys.pull.will_change").to_string(),
                p.text_muted,
            ))
            .children(will)
        })
        .when(!skipped.is_empty(), |el| {
            el.child(caption(
                t!("hotkeys.pull.conflict_skipped").to_string(),
                p.red_text,
            ))
            .children(skipped)
        })
        .into_any_element()
}

/// One change of the pull dialog: the command, then the old and the new binding in mono.
fn pull_change_line(name: String, current: String, incoming: String, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    h_flex()
        .w_full()
        .flex_wrap()
        .gap(design::ui_px(cx, 8.0))
        .child(div().flex_1().min_w(design::ui_px(cx, 160.0)).child(name))
        .child(
            div()
                .font_family(design::mono())
                .text_color(rgba_from(p.text_muted, 1.0))
                .child(format!("{current} -> {incoming}")),
        )
        .into_any_element()
}

/// The problems a row shows: the key's clashes and bare-key warning, the gesture's clashes (not
/// for a short row the mirror switch owns), or the ruler's.
fn row_problems(row: Row, hotkeys: &HotkeysConfig, clashes: &Clashes) -> Vec<Clash> {
    match row {
        Row::Slot(spec) => {
            let disabled = spec.follows_mirror() && hotkeys.same_hotkeys_for_move;
            let mut notes = Vec::new();
            if let Some(key) = spec.key() {
                notes.extend(clashes.key(hotkeys, key));
                notes.extend(super::clash::bare_key(hotkeys, key));
            }
            if let Some(mouse) = spec.mouse()
                && !disabled
            {
                notes.extend(clashes.mouse(hotkeys, mouse));
            }
            notes
        }
        Row::RulerDrag => clashes.ruler(hotkeys),
        Row::LabelScroll | Row::SameForMove => Vec::new(),
    }
}

/// What the search and the conflict filter read of a row.
fn row_text(row: Row, hotkeys: &HotkeysConfig, problems: &[Clash]) -> RowText {
    let problem = problems
        .iter()
        .any(|c| matches!(c.severity, Severity::Shadowed | Severity::Bare));
    match row {
        Row::Slot(spec) => {
            let mut keys = Vec::new();
            if let Some(key) = spec.key() {
                keys.push(hotkeys.key(key).to_string());
            }
            if let Some(mouse) = spec.mouse() {
                let g = hotkeys.gesture(mouse);
                if g != MouseGestureBinding::None {
                    keys.push(g.label().to_string());
                }
            }
            RowText {
                name: spec.name(),
                moonbot: spec.moonbot_name(),
                keys,
                problem,
            }
        }
        Row::LabelScroll => RowText {
            name: t!("hotkeys.label_scroll").to_string(),
            moonbot: None,
            keys: vec![wheel_label(hotkeys.label_scroll_wheel)],
            problem,
        },
        Row::RulerDrag => RowText {
            name: t!("hotkeys.ruler_drag").to_string(),
            moonbot: None,
            keys: vec![ruler_label(hotkeys.ruler_drag)],
            problem,
        },
        Row::SameForMove => RowText::default(),
    }
}

/// The localized name of a label-scroll wheel binding.
fn wheel_label(wheel: WheelModifier) -> String {
    let key = format!("hotkeys.wheel.{}", wheel.config_value());
    t!(&key).to_string()
}

/// The localized name of a ruler binding.
fn ruler_label(drag: RulerDrag) -> String {
    let key = format!("hotkeys.ruler.{}", drag.config_value());
    t!(&key).to_string()
}
