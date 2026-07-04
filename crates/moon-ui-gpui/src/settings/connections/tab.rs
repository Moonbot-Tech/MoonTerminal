//! Сборка вкладки «Подключения»: заголовки-ветки групп (галка·иконка·имя·👁·пикер·+ядро),
//! пикер иконок, селектор источника рыночных данных и `connections_tab` (дерево
//! «группа → ядра» + глобальная кнопка добавления).

use std::collections::HashMap;
use std::sync::Arc;

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonCheckbox, MoonCheckboxSize, MoonMenuSize, MoonPalette,
    MoonSelect, MoonTooltipView, StyledExt, h_flex, v_flex,
};
use rust_i18n::t;

use super::SettingsView;
use crate::design;

impl SettingsView {
    /// Заголовок-ветка группы: галка active · иконка · имя · кол-во · win · Иконка · +ядро.
    /// `ico_el` готовится снаружи (берёт `&mut self.icons` для текстуры).
    fn group_header_row(
        &mut self,
        name: &str,
        active: bool,
        ico_el: AnyElement,
        member_count: usize,
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let nm_act = name.to_string();
        let nm_eye = name.to_string();
        let nm_pick = name.to_string();
        let nm_add = name.to_string();
        h_flex()
            .w_full()
            .gap_1()
            .items_center()
            .px_1()
            .py_0p5()
            .rounded(px(4.0))
            .bg(rgb(p.panel_high))
            .child(
                MoonCheckbox::new(SharedString::from(format!("grp-{name}")))
                    .checked(active)
                    .size(MoonCheckboxSize::Compact)
                    .on_change(cx.listener(move |this, ch: &bool, _w, cx| {
                        let v = *ch;
                        let n = nm_act.clone();
                        this.backend.update(cx, |b, bcx| {
                            if let Some(p) = b.preview.as_mut() {
                                if let Some(gc) = p.groups.iter_mut().find(|g| g.name == n) {
                                    gc.active = v;
                                    bcx.notify();
                                }
                            }
                        });
                        cx.notify();
                    })),
            )
            .child(ico_el)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_bold()
                    .child(name.to_string()),
            )
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_soft))
                    .child(t!("conn.member_count", n = member_count).to_string()),
            )
            .child(
                div()
                    .id(SharedString::from(format!("eye-tip-{name}")))
                    .tooltip(|_window, cx| {
                        cx.new(|_| MoonTooltipView::new(t!("conn.show_group").to_string()))
                            .into()
                    })
                    .child(
                        MoonButton::new(SharedString::from(format!("eye-{name}")))
                            .ghost()
                            .size(MoonButtonSize::Micro)
                            .width(34.0)
                            .label("win")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let n = nm_eye.clone();
                                this.backend.update(cx, |b, bcx| {
                                    b.show_group_request.push(n);
                                    bcx.notify();
                                });
                            }))
                            .render(),
                    ),
            )
            .child(
                MoonButton::new(SharedString::from(format!("pick-{name}")))
                    .outline()
                    .size(MoonButtonSize::Micro)
                    .width(54.0)
                    .label(t!("conn.icon_btn").to_string())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.picking = Some(nm_pick.clone());
                        cx.notify();
                    }))
                    .render(),
            )
            .child(
                MoonButton::new(SharedString::from(format!("addgrp-{name}")))
                    .outline()
                    .size(MoonButtonSize::Micro)
                    .width(56.0)
                    .label(format!("+ {}", t!("conn.add_core_short")))
                    .on_click(
                        cx.listener(move |this, _, w, cx| this.add_server(nm_add.clone(), w, cx)),
                    )
                    .render(),
            )
    }

    /// Пикер иконок под выбранной группой: строка-заголовок с «×» + скроллируемая сетка
    /// иконок (клик = назначить иконку группе и закрыть). Возвращает обе строки списком,
    /// чтобы вставить их в дерево без обёртки-контейнера.
    fn icon_picker_rows(
        &mut self,
        name: &str,
        pick_ids: &[u32],
        icon_tex: &HashMap<u32, Option<Arc<RenderImage>>>,
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut grid = h_flex().w_full().flex_wrap().gap_1();
        for id in pick_ids.iter().copied() {
            let cell: AnyElement = match icon_tex.get(&id).and_then(|t| t.clone()) {
                Some(arc) => img(arc)
                    .w(design::ui_px(cx, 22.0))
                    .h(design::ui_px(cx, 22.0))
                    .into_any_element(),
                None => continue,
            };
            let nm = name.to_string();
            grid = grid.child(
                div()
                    .id(SharedString::from(format!("ico-{id}")))
                    .p_0p5()
                    .cursor_pointer()
                    .rounded(design::ui_px(cx, 4.0))
                    .hover(move |s| s.bg(rgb(p.panel_high)))
                    .child(cell)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let n = nm.clone();
                        this.backend.update(cx, |b, bcx| {
                            if let Some(p) = b.preview.as_mut() {
                                if let Some(g) = p.groups.iter_mut().find(|g| g.name == n) {
                                    g.icon = id;
                                    bcx.notify();
                                }
                            }
                        });
                        this.picking = None;
                        cx.notify();
                    })),
            );
        }
        vec![
            h_flex()
                .w_full()
                .items_center()
                .gap_1()
                .pl(px(20.0))
                .child(
                    div()
                        .flex_1()
                        .text_size(design::t_body(cx))
                        .text_color(rgb(p.text_soft))
                        .child(t!("conn.icon_for", name = name).to_string()),
                )
                .child(
                    MoonButton::new("pick-close")
                        .ghost()
                        .size(MoonButtonSize::Micro)
                        .width(24.0)
                        .label("x")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.picking = None;
                            cx.notify();
                        }))
                        .render(),
                )
                .into_any_element(),
            div()
                .id("icon-picker")
                .pl(px(20.0))
                .max_h(px(220.0))
                .overflow_y_scroll()
                .child(grid)
                .into_any_element(),
        ]
    }

    /// Источник рыночных данных — выпадающий список (порт egui ComboBox).
    fn market_src_selector(&self) -> impl IntoElement {
        h_flex()
            .gap_2()
            .items_center()
            .child(
                div()
                    .id("market-src-lbl")
                    .font_bold()
                    .child(t!("conn.market_src").to_string())
                    .tooltip(|_window, cx| {
                        cx.new(|_| {
                            MoonTooltipView::new(t!("conn.market_src_tip").to_string())
                                .max_width(420.0)
                        })
                        .into()
                    }),
            )
            .child(
                div().w(px(260.0)).child(
                    MoonSelect::new(&self.mode)
                        .trigger_size(MoonButtonSize::Action)
                        .menu_width(260.0)
                        .menu_size(MoonMenuSize::Compact),
                ),
            )
    }

    /// Вкладка «Подключения» — порт egui `settings/connections.rs`: источник данных
    /// (выпадающий), таблица ядер слева, панель групп (с иконками/👁/пикером) справа.
    pub(in crate::settings) fn connections_tab(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        // Живой статус ядер для точек.
        let status = self.backend.read(cx).session.status_map();
        // Снимки серверов (id, active, группа) и групп (name, active, icon).
        let (servers, mut groups) = {
            let b = self.backend.read(cx);
            let d = b.preview.as_ref().unwrap_or(&b.config);
            (
                d.servers
                    .iter()
                    .map(|s| (s.id, s.active, s.group.clone()))
                    .collect::<Vec<_>>(),
                d.groups
                    .iter()
                    .map(|g| (g.name.clone(), g.active, g.icon))
                    .collect::<Vec<_>>(),
            )
        };
        // Стабильный порядок групп — по имени (заголовки-ветки в списке ядер).
        groups.sort_by(|a, b| a.0.cmp(&b.0));
        // Предзагрузить иконки (групп + весь набор, если открыт пикер) — texture() берёт
        // &mut self.icons, поэтому грузим ДО построения UI, потом читаем из карты.
        let picking = self.picking.clone();
        let mut icon_tex: HashMap<u32, Option<Arc<RenderImage>>> = HashMap::new();
        for (_, _, icon) in &groups {
            icon_tex
                .entry(*icon)
                .or_insert_with(|| self.icons.texture(*icon));
        }
        let pick_ids: Vec<u32> = if picking.is_some() {
            self.icons.ids.clone()
        } else {
            Vec::new()
        };
        for id in &pick_ids {
            icon_tex
                .entry(*id)
                .or_insert_with(|| self.icons.texture(*id));
        }

        // ── Единый список-«дерево»: заголовок-ветка группы + ядра-листья под ней ──
        // (Не настоящий tree: ветки/листья задаём отступом, без раскрытия.) Колонки
        // ядер выровнены под заголовком группы: шапка колонок с тем же левым отступом.
        let mut list_col = v_flex()
            .w_full()
            .min_w_0()
            .gap_1()
            .child(Self::hint_label(
                "h-section",
                t!("conn.groups_panel_heading").to_string(),
                t!("conn.groups_panel_tip").to_string().into(),
                p,
            ))
            .child(Self::conn_col_head_row(p, cx));

        // Нет групп (ни у одного ядра не задана) → поясняющий хинт.
        if groups.is_empty() {
            list_col = list_col.child(
                div()
                    .text_color(rgb(p.text_soft))
                    .child(t!("conn.no_groups").to_string()),
            );
        }

        for (name, active, icon) in &groups {
            let member_count = servers.iter().filter(|(_, _, g)| g == name).count();
            let ico_el: AnyElement = match icon_tex.get(icon).and_then(|t| t.clone()) {
                Some(arc) => img(arc)
                    .w(design::ui_px(cx, 20.0))
                    .h(design::ui_px(cx, 20.0))
                    .into_any_element(),
                None => div()
                    .w(design::ui_px(cx, 20.0))
                    .h(design::ui_px(cx, 20.0))
                    .into_any_element(),
            };
            list_col =
                list_col.child(self.group_header_row(name, *active, ico_el, member_count, p, cx));
            // ── Ядра-листья этой группы (с отступом + вертикальная линия ветки) ──
            // Неактивные сервера — вниз (стабильная сортировка: порядок внутри групп сохраняется).
            // `i` — исходный индекс в config.servers (нужен для мутаций draft), его сохраняем.
            let mut members: Vec<(usize, &(u64, bool, String))> = servers
                .iter()
                .enumerate()
                .filter(|(_, (_, _, g))| g == name)
                .collect();
            members.sort_by_key(|(_, (_, active, _))| !*active);
            for (i, (id, srv_active, _g)) in members {
                if let Some(row) = self.conn.get(i) {
                    let st = status.get(id).cloned();
                    list_col = list_col.child(
                        div()
                            .ml(px(8.0))
                            .pl(px(11.0))
                            .border_l_1()
                            .border_color(rgb(p.border))
                            .child(self.server_row(cx, i, row, *id, *srv_active, st)),
                    );
                }
            }
            // ── Пикер иконок под выбранной группой ──
            if picking.as_deref() == Some(name.as_str()) {
                list_col =
                    list_col.children(self.icon_picker_rows(name, &pick_ids, &icon_tex, p, cx));
            }
        }

        // Глобальная кнопка: новое ядро в группу «default» (дальше можно переписать «Группу»).
        list_col = list_col.child(
            MoonButton::new("add-srv")
                .outline()
                .small()
                .width(220.0)
                .label(format!("+ {}", t!("conn.add_core")))
                .on_click(cx.listener(|this, _, w, cx| this.add_server("default".into(), w, cx)))
                .render(),
        );

        v_flex()
            .w_full()
            .gap_2()
            // Источник рыночных данных — выпадающий список (порт egui ComboBox).
            .child(self.market_src_selector())
            .child(list_col)
    }
}
