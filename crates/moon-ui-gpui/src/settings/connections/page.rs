//! The Cores page in the live window: gathers the draft into a [`CoresModel`] and a
//! [`CoreCard`], hands the list and card builders their live parts (status dot with the verdict
//! tooltip, group icon with its picker), and runs every [`CoreAction`] against the draft.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonContextMenuWindowExt as _, MoonInput,
    MoonInputState, MoonMenuItem, MoonPalette, MoonPopover, MoonPopoverPlacement, MoonSelect,
    MoonWindowExt as _, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::SettingsView;
use super::cores::{
    CardInputs, CoreAct, CoreAction, CoreCard, ListParts, core_card, cores_empty, cores_list,
    cores_split, deletes,
};
use super::entries::{CoreDot, CoreListRow, cores_model, valid_selection};
use super::tab::{apply_group_transport, group_icon, icon_picker_grid, sorted_group_rows};
use super::table::{
    apply_pasted_key, feed_count, feed_get, feed_set, set_core_active, set_for_station, status_dot,
};
use crate::design;
use crate::settings::Tab;
use crate::settings::common::column_row;

/// Label column of the market-source and core-order card, unscaled.
const SELECT_LABEL_W: f32 = 250.0;
/// Widest the two selects beside it get, unscaled.
const SELECT_W: f32 = 320.0;
use crate::settings::frame::PageParts;
use moon_core::config::{AppConfig, ServerConfig, TransportVersion};

/// Move draft core `i` to group `name`, keeping the group rows in step.
///
/// Returns:
///     Whether the core moved.
pub(crate) fn set_core_group(p: &mut AppConfig, i: usize, name: &str) -> bool {
    let name = name.trim();
    let Some(s) = p.servers.get_mut(i) else {
        return false;
    };
    if name.is_empty() || s.group == name {
        return false;
    }
    s.group = name.to_string();
    super::sync_groups_from_servers(&p.servers, &mut p.groups);
    true
}

impl SettingsView {
    /// Run `f` on the selected draft core; notifies when it reports a change.
    fn edit_selected(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut ServerConfig) -> bool) {
        let Some(i) = self.conn_selected_index() else {
            return;
        };
        self.backend.update(cx, |b, bcx| {
            if let Some(s) = b.preview.as_mut().and_then(|p| p.servers.get_mut(i))
                && f(s)
            {
                bcx.notify();
            }
        });
        cx.notify();
    }

    /// Run one action of the Cores page against the draft.
    ///
    /// Args:
    ///     action: What the user did.
    ///     window: Settings window, for dialogs, menus and editor rebuilds.
    ///     cx: Settings context.
    pub(in crate::settings) fn on_core_action(
        &mut self,
        action: CoreAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(i) = deletes(&action) {
            self.delete_server(i, window, cx);
            return;
        }
        match action {
            CoreAction::Select(key) => {
                self.conn_selected = Some(key);
                cx.notify();
            }
            CoreAction::GroupActive(name, on) => {
                self.backend.update(cx, |b, bcx| {
                    if let Some(g) = b
                        .preview
                        .as_mut()
                        .and_then(|p| p.groups.iter_mut().find(|g| g.name == name))
                    {
                        g.active = on;
                        bcx.notify();
                    }
                });
                cx.notify();
            }
            CoreAction::ShowGroup(name) => self.backend.update(cx, |b, bcx| {
                b.show_group_request.push(name);
                bcx.notify();
            }),
            CoreAction::GroupMenu(name) => self.open_group_menu(name, window, cx),
            CoreAction::PickIcon(name) => {
                self.picking = Some(name);
                cx.notify();
            }
            CoreAction::GroupMode(name, mode) => {
                self.backend.update(cx, |b, bcx| {
                    if let Some(p) = b.preview.as_mut()
                        && apply_group_transport(&mut p.servers, &name, mode)
                    {
                        bcx.notify();
                    }
                });
                cx.notify();
            }
            CoreAction::AddCore(group) => self.add_server(group, window, cx),
            CoreAction::Import => self.start_moonbot_import(cx),
            CoreAction::SetActive(on) => self.edit_selected(cx, |s| set_core_active(s, on)),
            CoreAction::ForStation(on) => self.edit_selected(cx, |s| set_for_station(s, on)),
            CoreAction::SetGroup(name) => self.move_selected_to_group(&name, window, cx),
            CoreAction::NewGroup => self.ask_new_group(window, cx),
            CoreAction::Feed(ix, on) => self.edit_selected(cx, |s| {
                let changed = feed_get(&s.feed, ix) != on;
                feed_set(&mut s.feed, ix, on);
                changed
            }),
            CoreAction::Proto(v) => self.edit_selected(cx, |s| {
                let changed = s.transport != v;
                s.transport = v;
                changed
            }),
            CoreAction::Preset(m) => self.edit_selected(cx, |s| {
                let changed = s.workspace_membership != m;
                s.workspace_membership = m;
                changed
            }),
            CoreAction::Total(m) => self.edit_selected(cx, |s| {
                let changed = s.total_mode != m;
                s.total_mode = m;
                changed
            }),
            CoreAction::Disclose(key) => {
                if !self.more_open.remove(key) {
                    self.more_open.insert(key);
                }
                cx.notify();
            }
            CoreAction::PasteKey => self.paste_selected_key(window, cx),
            CoreAction::Reconnect => {
                let Some(i) = self.conn_selected_index() else {
                    return;
                };
                let id = {
                    let b = self.backend.read(cx);
                    b.preview
                        .as_ref()
                        .unwrap_or(&b.config)
                        .servers
                        .get(i)
                        .map(|s| s.id)
                };
                if let Some(id) = id {
                    self.backend.update(cx, |b, bcx| {
                        b.reconnect_request.push(id);
                        bcx.notify();
                    });
                }
            }
            CoreAction::AskDelete => self.ask_delete_core(window, cx),
            CoreAction::DeleteConfirmed(_) => {}
        }
    }

    /// The "⋯" menu of a group: its icon, one transport mode for every core, a new core.
    fn open_group_menu(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        let item = |key: String, label: String, action: CoreAction| {
            let weak = weak.clone();
            MoonMenuItem::with_key(key, label).on_click(move |_, window, app| {
                window.close_context_menu(app);
                let _ = weak.update(app, |this, cx| {
                    this.on_core_action(action.clone(), window, cx)
                });
            })
        };
        let mut items = vec![item(
            format!("cores-menu-icon-{name}"),
            t!("conn.list.menu_icon").to_string(),
            CoreAction::PickIcon(name.clone()),
        )];
        items.push(MoonMenuItem::separator());
        items.push(MoonMenuItem::label(t!("conn.list.menu_mode")));
        for v in TransportVersion::ALL {
            items.push(item(
                format!("cores-menu-mode-{name}-{}", v.label()),
                v.label().to_string(),
                CoreAction::GroupMode(name.clone(), v),
            ));
        }
        items.push(MoonMenuItem::separator());
        items.push(item(
            format!("cores-menu-add-{name}"),
            t!("conn.list.add_core").to_string(),
            CoreAction::AddCore(name),
        ));
        let at = window.mouse_position();
        window.open_moon_context_menu(cx, "cores-group-menu", at, items, 220.0);
    }

    /// Move the selected core to `name` in the draft and in its group field.
    fn move_selected_to_group(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.conn_selected_index() else {
            return;
        };
        let moved = self.backend.update(cx, |b, bcx| {
            let moved = b
                .preview
                .as_mut()
                .is_some_and(|p| set_core_group(p, i, name));
            if moved {
                bcx.notify();
            }
            moved
        });
        if moved && let Some(row) = self.conn.get(i) {
            let name = name.trim().to_string();
            row.group.update(cx, |st, c| st.set_value(name, window, c));
        }
        cx.notify();
    }

    /// Ask for a new group's name; the selected core moves there on confirm.
    fn ask_new_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            MoonInputState::new(window, cx).placeholder(t!("conn.card.new_group_ph").to_string())
        });
        let weak = cx.entity().downgrade();
        window.open_unique_moon_dialog("cores-new-group", cx, move |dialog, _window, cx| {
            let p = MoonPalette::active(cx);
            let create = {
                let input = input.clone();
                let weak = weak.clone();
                MoonButton::new("cores-new-group-ok")
                    .variant(MoonButtonVariant::Blue)
                    .label(t!("conn.card.new_group_ok").to_string())
                    .on_click(move |_, window, app| {
                        let name = input.read(app).value().trim().to_string();
                        if name.is_empty() {
                            return;
                        }
                        window.close_dialog(app);
                        let _ = weak.update(app, |this, cx| {
                            this.move_selected_to_group(&name, window, cx)
                        });
                    })
                    .render()
            };
            let cancel = MoonButton::new("cores-new-group-cancel")
                .ghost()
                .label(t!("conn.card.cancel").to_string())
                .on_click(|_, window, app| window.close_dialog(app))
                .render();
            let field = input.clone();
            dialog
                .w(design::ui_px(cx, 380.0))
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
                        .child(t!("conn.card.new_group_title").to_string()),
                )
                .content(move |content, _window, _cx| {
                    content.child(MoonInput::new("cores-new-group-name").state(&field))
                })
                .footer(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .child(cancel)
                        .child(create)
                        .into_any_element(),
                )
        });
    }

    /// Put the clipboard's key into the selected core, as the table's paste glyph does.
    fn paste_selected_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.conn_selected_index() else {
            return;
        };
        let Some(text) = cx
            .read_from_clipboard()
            .and_then(|it| it.text())
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
        else {
            return;
        };
        if let Some(row) = self.conn.get(i) {
            let key = row.key.clone();
            let address = row.address.clone();
            key.update(cx, |st, c| st.set_value(text.clone(), window, c));
            let placeholder = super::address_placeholder(&text);
            address.update(cx, |st, c| st.set_placeholder(placeholder, window, c));
        }
        self.edit_selected(cx, |s| {
            apply_pasted_key(s, &text);
            true
        });
    }

    /// Ask before deleting the selected core; only the dialog's confirm deletes.
    fn ask_delete_core(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::settings::common::{Confirm, ConfirmBtn, ConfirmKind, confirm};
        let Some(i) = self.conn_selected_index() else {
            return;
        };
        let name = {
            let b = self.backend.read(cx);
            b.preview
                .as_ref()
                .unwrap_or(&b.config)
                .servers
                .get(i)
                .map(|s| s.name.clone())
                .unwrap_or_default()
        };
        let weak = cx.entity().downgrade();
        confirm(
            window,
            cx,
            Confirm {
                title: t!("conn.card.delete_title").to_string(),
                body: t!("conn.card.delete_body", name = name).to_string(),
                buttons: vec![
                    ConfirmBtn {
                        label: t!("conn.card.cancel").to_string(),
                        kind: ConfirmKind::Cancel,
                        on: Rc::new(|_, _| {}),
                    },
                    ConfirmBtn {
                        label: t!("conn.card.delete_confirm").to_string(),
                        kind: ConfirmKind::DangerSolid,
                        on: Rc::new(move |window, app| {
                            let _ = weak.update(app, |this, cx| {
                                this.on_core_action(CoreAction::DeleteConfirmed(i), window, cx)
                            });
                        }),
                    },
                ],
            },
        );
    }

    /// The "+ Добавить ядро" button, carrying the first-run ring while the list is empty; once a
    /// core is added the ring moves to that core's empty key field on the card.
    fn add_core_button(&self, id: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let p = MoonPalette::active(cx);
        div()
            .relative()
            .child(
                MoonButton::new(id)
                    .variant(MoonButtonVariant::Blue)
                    .label(format!("+ {}", t!("conn.add_core_btn")))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.add_server("default".into(), window, cx)
                    }))
                    .render(),
            )
            .children(
                self.conn_hint(cx)
                    .filter(|_| {
                        let b = self.backend.read(cx);
                        b.preview.as_ref().unwrap_or(&b.config).servers.is_empty()
                    })
                    .and_then(|at| crate::pulse::attention_ring(p.accent, at)),
            )
            .into_any_element()
    }

    /// The Cores page: market source and core order on top, then the list beside (or above) the
    /// card of the selected core, or the empty state.
    ///
    /// Args:
    ///     narrow: The window is below the navigation breakpoint: the list sits above the card
    ///         and the page scrolls as a whole.
    ///     cx: Settings context.
    ///
    /// Returns:
    ///     The page parts.
    pub(in crate::settings) fn cores_page(
        &mut self,
        narrow: bool,
        body_width: f32,
        cx: &mut Context<Self>,
    ) -> PageParts {
        let p = MoonPalette::active(cx);
        let status = self.backend.read(cx).session.status_map();
        let (servers, groups, order, venues, endpoints) = {
            let b = self.backend.read(cx);
            let d = b.preview.as_ref().unwrap_or(&b.config);
            let venue_map = b.session.core_venues();
            let venues: Vec<_> = d
                .servers
                .iter()
                .map(|s| venue_map.get(&s.id).cloned())
                .collect();
            let meta: Vec<_> = d
                .servers
                .iter()
                .zip(&venues)
                .map(|(s, v)| (s.id, s.uid, s.active, s.group.clone(), v.clone()))
                .collect();
            (
                d.servers.clone(),
                sorted_group_rows(&meta, &d.groups),
                moon_core::session::core_order::CoreOrder::new(d),
                venues,
                super::endpoints::endpoint_cells(&d.servers),
            )
        };
        let keys: Vec<u64> = self.conn.iter().map(|r| r.row_key).collect();
        self.conn_selected = valid_selection(self.conn_selected, &keys);

        let add = self.add_core_button("cores-add", cx);
        let weak = cx.entity().downgrade();
        let act: CoreAct = Rc::new(move |action, window, app| {
            let _ = weak.update(app, |this, cx| this.on_core_action(action, window, cx));
        });

        if servers.is_empty() {
            let empty_add = self.add_core_button("cores-empty-add", cx);
            let body = cores_empty(&act, empty_add, cx);
            return self.page_parts(Tab::Cores, body, vec![add], cx);
        }

        let rows: Vec<CoreListRow> = servers
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let row = self.conn.get(i)?;
                let key_readable = moon_core::config::key_is_readable(s.key.expose());
                let addr = endpoints.get(i).map(|e| e.text.clone()).unwrap_or_default();
                Some(CoreListRow {
                    row_key: row.row_key,
                    draft_index: i,
                    core_id: s.id,
                    name: s.name.clone(),
                    addr,
                    feeds_on: feed_count(&s.feed),
                    active: s.active,
                    dot: CoreDot::of(s.active, key_readable, status.get(&s.id)),
                })
            })
            .collect();
        let group_of: Vec<String> = servers.iter().map(|s| s.group.clone()).collect();
        let model = cores_model(
            rows,
            &venues,
            &groups,
            &group_of,
            &order,
            &|venue| crate::controls::venue_section_label(venue),
            self.conn_selected,
        );

        // Icons: the group's own, and every pickable one while a picker is open.
        if self
            .picking
            .as_ref()
            .is_some_and(|name| !groups.iter().any(|g| g.0 == *name))
        {
            self.picking = None;
        }
        let mut icon_tex: HashMap<u32, Option<Arc<RenderImage>>> = HashMap::new();
        for (_, _, icon) in &groups {
            icon_tex
                .entry(*icon)
                .or_insert_with(|| self.icons.texture(*icon));
        }
        let pick_ids: Vec<u32> = if self.picking.is_some() {
            self.icons.ids.clone()
        } else {
            Vec::new()
        };
        for id in &pick_ids {
            icon_tex
                .entry(*id)
                .or_insert_with(|| self.icons.texture(*id));
        }

        let weak = cx.entity().downgrade();
        let picking = self.picking.clone();
        let icon = |g: &super::entries::GroupBlock, app: &App| -> AnyElement {
            let open = picking.as_deref() == Some(g.name.as_str());
            let mut popover =
                MoonPopover::new(SharedString::from(format!("cores-pick-{}", g.name)))
                    .placement(MoonPopoverPlacement::BottomStart)
                    .content_width_ui(240.0)
                    .close_on_content_click(false)
                    .overlay_closable(true)
                    .open(open)
                    .on_open_change({
                        let weak = weak.clone();
                        let name = g.name.clone();
                        move |open, _window, app| {
                            let _ = weak.update(app, |this, cx| {
                                this.picking = open.then(|| name.clone());
                                cx.notify();
                            });
                        }
                    })
                    .trigger(
                        div()
                            .id(SharedString::from(format!("cores-icon-{}", g.name)))
                            .cursor_pointer()
                            .child(group_icon(icon_tex.get(&g.icon).cloned().flatten(), app)),
                    );
            if open {
                popover = popover.content(icon_picker_grid(
                    &weak,
                    &g.name,
                    &pick_ids,
                    &icon_tex,
                    MoonPalette::active(app),
                    app,
                ));
            }
            popover.into_any_element()
        };
        let dot_weak = cx.entity().downgrade();
        let dot = |row: &CoreListRow, _: &App| -> AnyElement {
            status_dot(
                row.row_key,
                row.core_id,
                row.active,
                status.get(&row.core_id),
                dot_weak.clone(),
                p,
            )
            .into_any_element()
        };
        let parts = ListParts {
            dot: &dot,
            icon: &icon,
        };
        let list = cores_list(&model, &parts, &act, cx);

        let card = model.selected_row().cloned().and_then(|sel| {
            let i = sel.draft_index;
            let s = servers.get(i)?;
            let row = self.conn.get(i)?;
            let endpoint = endpoints.get(i);
            let mut names: Vec<String> = group_of.clone();
            names.sort();
            names.dedup();
            let log_ignored = !s.feed.log_delivery
                && self
                    .backend
                    .read(cx)
                    .session
                    .store()
                    .core(s.id)
                    .is_some_and(|c| c.log_delivery_ignored);
            let addr_note = endpoint.and_then(|e| {
                if e.invalid {
                    Some(t!("conn.endpoint_invalid").to_string())
                } else if e.localhost_fallback {
                    Some(t!("conn.endpoint_no_address").to_string())
                } else {
                    e.duplicate_name
                        .as_ref()
                        .map(|name| t!("conn.endpoint_duplicate", name = name).to_string())
                }
            });
            let card = CoreCard {
                draft_index: i,
                name: s.name.clone(),
                active: s.active,
                key_bad: !moon_core::config::key_is_readable(s.key.expose()),
                addr_note,
                addr_invalid: endpoint.is_some_and(|e| e.invalid),
                has_override: !s.endpoint_override.trim().is_empty(),
                for_station: s.endpoint_to_station,
                group: s.group.clone(),
                groups: names,
                feeds: std::array::from_fn(|ix| feed_get(&s.feed, ix)),
                log_ignored,
                proto: s.transport,
                preset: s.workspace_membership,
                total: s.total_mode,
            };
            let inputs = CardInputs {
                name: &row.name,
                key: &row.key,
                address: &row.address,
                bundle: &row.bundle,
                color: &row.color,
                key_ring: self.conn_hint(cx),
            };
            let dot = dot(&sel, cx);
            let open = |key: &'static str| self.more_open.contains(key);
            Some(core_card(&card, &inputs, dot, &open, &act, cx))
        });

        let selects = self.cores_selects(&act, cx);
        let content_w = body_width - 2.0 * design::ui_value(cx, super::super::render::BODY_PAD);
        let layout = super::cores::cores_layout(
            narrow,
            content_w,
            super::cores::card_min_w(cx),
            design::ui_value(cx, 1.0),
        );
        let stacked = layout == super::cores::CoresLayout::Stacked;
        let split = cores_split(list, card, layout, cx);
        let body = if stacked {
            div()
                .id("cores-page-scroll")
                .size_full()
                .overflow_y_scroll()
                .child(
                    v_flex()
                        .w_full()
                        .flex_none()
                        .gap(design::ui_px(cx, 10.0))
                        .child(selects)
                        .child(split),
                )
                .into_any_element()
        } else {
            v_flex()
                .size_full()
                .min_h(px(0.0))
                .gap(design::ui_px(cx, 10.0))
                .child(selects)
                .child(split)
                .into_any_element()
        };
        self.page_parts(Tab::Cores, body, vec![add], cx)
    }

    /// Market source and core order, as two rows above the list.
    fn cores_selects(&self, act: &CoreAct, cx: &App) -> AnyElement {
        let width = design::ui_px(cx, SELECT_W);
        let menu_w = design::font_w(cx, SELECT_W);
        let market = div()
            .w_full()
            .max_w(width)
            .child(
                MoonSelect::new(&self.mode)
                    .trigger_size(MoonButtonSize::density(cx))
                    .menu_width(menu_w),
            )
            .into_any_element();
        let sort = div()
            .w_full()
            .max_w(width)
            .child(
                MoonSelect::new(&self.core_sort)
                    .trigger_size(MoonButtonSize::density(cx))
                    .menu_width(menu_w),
            )
            .into_any_element();
        let toggle = |key: &'static str| -> crate::settings::common::Action {
            let act = act.clone();
            Rc::new(move |window, cx| act(CoreAction::Disclose(key), window, cx))
        };
        let p = moon_ui::MoonPalette::active(cx);
        // The mockup's form card: a bordered box, a label column and each select beside it.
        v_flex()
            .w_full()
            .flex_none()
            .px(design::ui_px(cx, 14.0))
            .border_1()
            .border_color(moon_ui::rgba_from(p.border, 1.0))
            .rounded(design::r_container(cx))
            .child(column_row(
                "conn-market-src",
                t!("conn.market_src").to_string(),
                Some(t!("conn.market_src.hint").to_string()),
                Some(t!("conn.market_src.more").to_string()),
                market,
                self.more_open.contains("conn-market-src"),
                toggle("conn-market-src"),
                SELECT_LABEL_W,
                cx,
            ))
            .child(column_row(
                "conn-core-sort",
                t!("conn.core_sort").to_string(),
                Some(t!("conn.core_sort.hint").to_string()),
                None,
                sort,
                false,
                toggle("conn-core-sort"),
                SELECT_LABEL_W,
                cx,
            ))
            .into_any_element()
    }
}
