//! Preview импорта настроек MoonBot (ТЗ docs-internal/MOONBOT_CONFIG_IMPORT.md §2):
//! оверлей поверх окна настроек. Стадии: «Загрузка…» (парс+план на background
//! executor — большой буфер НЕ вешает UI), «Ошибка» (текст + Закрыть) и «Готово» —
//! четыре группы (Терминал / График и линии / Ядро / Не перенесено), чекбоксы на
//! каждый пункт, колонки Светлая|Тёмная со свотчами цветов, выбор целевых ядер.
//! «Применить» пишет выбранное в draft (`apply_local`) — на диск по «Сохранить».

use std::collections::HashSet;

use gpui::*;
use moon_ui::{
    MoonButton, MoonCheckbox, MoonPalette, MoonScrollableElement, StyledExt, h_flex, rgba_from,
    v_flex,
};
use rust_i18n::t;

use super::{SettingsView, interface, lines};
use crate::design;
use moon_core::config::moonbot_import::plan::SettingChange;
use moon_core::config::moonbot_import::{self, MoonBotImportPlan, PlanContext};
use moon_core::config::servers::default_order_sizes;

/// Готовый план + выбор пользователя.
pub(super) struct ImportReady {
    plan: MoonBotImportPlan,
    /// Выбранные id пунктов (дефолт — все локальные группы).
    selected: HashSet<String>,
    /// Целевые ядра per-core группы: (ServerConfig.id, имя, выбран). Дефолт —
    /// только первое активное ядро (ТЗ §2: не применять молча ко всем).
    cores: Vec<(u64, String, bool)>,
}

/// Стадия открытого preview.
pub(super) enum ImportState {
    /// Буфер похож на настройки MoonBot — парсим/строим план в фоне.
    Loading,
    /// Разбор не удался — текст ошибки в окне.
    Error(String),
    /// План готов — выбор и «Применить».
    Ready(ImportReady),
}

impl SettingsView {
    fn import_ready(&self) -> Option<&ImportReady> {
        match self.import.as_ref()? {
            ImportState::Ready(r) => Some(r),
            _ => None,
        }
    }

    /// Кнопка «Импорт из MoonBot»: буфер → быстрый sniff → окно «Загрузка…» →
    /// парс и план на background executor → Ready/Error. Не-настройки в буфере —
    /// ошибка в статус-строку БЕЗ окна (окно только когда «опознали, что это вставка»).
    pub(super) fn start_moonbot_import(&mut self, cx: &mut Context<Self>) {
        if matches!(self.import, Some(ImportState::Loading)) {
            return; // уже в полёте
        }
        let Some(text) = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .filter(|t| !t.trim().is_empty())
        else {
            self.status = Some((super::StatusMsg::Key("import.clipboard_empty"), true));
            cx.notify();
            return;
        };
        // Дешёвый sniff «это вообще MBSC?» — без декодирования. Не похоже → статус.
        if !text.contains("MBSC") {
            self.status = Some((
                super::StatusMsg::Text(moonbot_import::ImportError::NotFound.to_string()),
                true,
            ));
            cx.notify();
            return;
        }
        self.status = None;
        self.import = Some(ImportState::Loading);
        cx.notify();

        // Срезы draft для плана — клонами в фоновую задачу (build_plan чистый).
        let (hotkeys, theme, orders, ui_light, order_sizes, order_size_sel, cores) = {
            let b = self.backend.read(cx);
            let d = b.preview.as_ref().unwrap_or(&b.config);
            let first_active = d.servers.iter().find(|s| s.active).or(d.servers.first());
            let first_active_id = first_active.map(|s| s.id);
            (
                d.hotkeys.clone(),
                d.theme.clone(),
                d.orders.clone(),
                d.ui_theme_mode == moon_core::config::UiThemeMode::Light,
                first_active
                    .map(|s| s.order_sizes_or_default("USDT"))
                    .unwrap_or_else(|| default_order_sizes("USDT")),
                first_active.and_then(|s| s.order_size_sel),
                {
                    // Canonicalize the checkbox list only; `first_active_id` seeds preset values.
                    let order = crate::core_order::CoreOrder::new(d);
                    let mut cores: Vec<(u64, String, bool)> = d
                        .servers
                        .iter()
                        .map(|s| (s.id, s.name.clone(), Some(s.id) == first_active_id))
                        .collect();
                    order.sort_by(&mut cores, |(id, _, _)| *id);
                    cores
                },
            )
        };
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let result = executor
                .spawn(async move {
                    let mb = moonbot_import::parse_clipboard(&text)?;
                    let plan = moonbot_import::plan::build_plan(
                        &mb,
                        &PlanContext {
                            hotkeys: &hotkeys,
                            theme: &theme,
                            orders: &orders,
                            ui_theme_light: ui_light,
                            order_sizes,
                            order_size_sel,
                        },
                    );
                    Ok::<_, moonbot_import::ImportError>(plan)
                })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    // Пользователь мог закрыть окно во время загрузки — не воскрешаем.
                    if !matches!(this.import, Some(ImportState::Loading)) {
                        return;
                    }
                    this.import = Some(match result {
                        Ok(plan) => {
                            // Дефолт выбора — только реальные ИЗМЕНЕНИЯ; совпадающие
                            // (same) видны, но не отмечены (их применение — no-op).
                            let selected = plan
                                .local_items()
                                .filter(|c| !c.same)
                                .map(|c| c.id.clone())
                                .collect();
                            ImportState::Ready(ImportReady {
                                plan,
                                selected,
                                cores,
                            })
                        }
                        Err(e) => ImportState::Error(e.to_string()),
                    });
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// «Применить»: выбранные пункты → draft (`apply_local`), пересборка editor-стейтов
    /// затронутых вкладок, статус с числом пунктов. Диск — по «Сохранить».
    fn apply_moonbot_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ImportState::Ready(state)) = self.import.take() else {
            self.import = None;
            return;
        };
        let core_ids: Vec<u64> = state
            .cores
            .iter()
            .filter(|(_, _, on)| *on)
            .map(|(id, _, _)| *id)
            .collect();
        let theme_selected = state.selected.contains("ui.theme_mode");
        let applied = self.backend.update(cx, |b, bcx| {
            let Some(p) = b.preview.as_mut() else {
                return 0;
            };
            let out = moonbot_import::apply_local(p, &state.plan, &state.selected, &core_ids);
            for id in &out.unknown_ids {
                log::warn!("moonbot import: неизвестный id пункта «{id}» — пропущен");
            }
            // Смена темы UI применяется ЖИВЬЁМ (как тогл на «Общих») — иначе до
            // «Сохранить» палитра не переключится и импорт выглядит несработавшим.
            // Цвета чарта спец-вызова не требуют: окна читают draft каждый кадр.
            if out.applied > 0 && theme_selected {
                crate::install_moon_theme_for_config(p, bcx);
            }
            if out.applied > 0 {
                bcx.notify();
            }
            out.applied
        });
        // Цвета/хоткеи живут в editor-стейтах, собранных при build — пересобрать
        // под новые значения draft (паттерн paste_tab).
        if applied > 0 {
            self.iface = interface::build(&self.backend, window, cx);
            self.lines = lines::build(&self.backend, window, cx);
        }
        self.status = Some((
            super::StatusMsg::Text(t!("import.applied", n = applied).to_string()),
            false,
        ));
        cx.notify();
    }

    /// Оверлей preview (рендерится поверх тела настроек, когда `import` открыт).
    pub(super) fn import_overlay(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let state = self.import.as_ref()?;
        let p = MoonPalette::active(cx);
        let backdrop = || {
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba_from(p.shadow, 0.72))
                .occlude()
        };

        // ── Стадии «Загрузка…» (плашка как в Аналитике) и «Ошибка» ──
        match state {
            ImportState::Loading => {
                return Some(
                    backdrop()
                        .child(
                            h_flex()
                                .px(design::ui_px(cx, 14.0))
                                .py(design::ui_px(cx, 7.0))
                                .rounded(design::ui_px(cx, 6.0))
                                .bg(rgba_from(p.panel_high, 1.0))
                                .border_1()
                                .border_color(rgba_from(p.border, 1.0))
                                .text_size(design::t_body(cx))
                                .text_color(rgba_from(p.text_soft, 1.0))
                                .child(t!("import.loading").to_string()),
                        )
                        .into_any_element(),
                );
            }
            ImportState::Error(e) => {
                let card = v_flex()
                    .w(design::font_w_px(cx, 520.0))
                    .gap(design::ui_px(cx, 10.0))
                    .p(design::ui_px(cx, 14.0))
                    .rounded(design::r_container(cx))
                    .border_1()
                    .border_color(rgba_from(p.border, 1.0))
                    .bg(rgba_from(p.shell, 1.0))
                    .occlude()
                    .child(
                        div()
                            .font_bold()
                            .text_size(design::t_title(cx))
                            .child(t!("import.title").to_string()),
                    )
                    .child(div().text_color(rgba_from(p.red, 1.0)).child(e.clone()))
                    .child(
                        MoonButton::new("import-close")
                            .outline()
                            .small()
                            .width(110.0)
                            .label(t!("import.cancel").to_string())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.import = None;
                                cx.notify();
                            }))
                            .render(),
                    );
                return Some(backdrop().child(card).into_any_element());
            }
            ImportState::Ready(_) => {}
        }
        let state = self.import_ready()?;

        let mut card = v_flex()
            .w(design::font_w_px(cx, 760.0))
            // ФИКСИРОВАННАЯ высота 88% (не max_h!): flex_1-тело со скроллом требует
            // детерминированной высоты родителя — при авто-высоте оно схлопывается в 0.
            .h(relative(0.88))
            .gap(design::ui_px(cx, 8.0))
            .p(design::ui_px(cx, 14.0))
            .rounded(design::r_container(cx))
            .border_1()
            .border_color(rgba_from(p.border, 1.0))
            // Плотный непрозрачный фон + occlude: вкладка под оверлеем не просвечивает
            // и не ловит клики.
            .bg(rgba_from(p.shell, 1.0))
            .occlude()
            .child(
                div()
                    .font_bold()
                    .text_size(design::t_title(cx))
                    .child(t!("import.title").to_string()),
            );

        // ── Скроллируемое тело: группы пунктов ──
        let mut body = v_flex().gap(design::ui_px(cx, 10.0)).w_full();
        if state.plan.is_empty() {
            body = body.child(
                div()
                    .text_color(rgba_from(p.text_soft, 1.0))
                    .child(t!("import.nothing").to_string()),
            );
        }
        body = body
            .children(self.import_group(cx, "import.group.terminal", &state.plan.terminal, p))
            .children(self.hotkeys_group(cx, &state.plan, p))
            .children(self.chart_columns(cx, &state.plan.chart, p))
            .children(self.import_group(cx, "import.group.core", &state.plan.per_core, p));

        // Выбор целевых ядер — только когда есть per-core пункты.
        if !state.plan.per_core.is_empty() {
            let all_on = state.cores.iter().all(|(_, _, on)| *on);
            let mut row = h_flex()
                .flex_wrap()
                .gap(design::ui_px(cx, 10.0))
                .child(
                    div()
                        .text_color(rgba_from(p.text_soft, 1.0))
                        .child(t!("import.core_targets").to_string()),
                )
                // «Все» — тогл всего списка разом.
                .child(
                    MoonCheckbox::new("imp-core-all")
                        .checked(all_on)
                        .label(t!("import.all_cores").to_string())
                        .on_change(cx.listener(|this, ch: &bool, _, cx| {
                            if let Some(ImportState::Ready(st)) = this.import.as_mut() {
                                let v = *ch;
                                for c in st.cores.iter_mut() {
                                    c.2 = v;
                                }
                                cx.notify();
                            }
                        })),
                );
            for (idx, (_, name, on)) in state.cores.iter().enumerate() {
                row = row.child(
                    MoonCheckbox::new(SharedString::from(format!("imp-core-{idx}")))
                        .checked(*on)
                        .label(name.clone())
                        .on_change(cx.listener(move |this, ch: &bool, _, cx| {
                            if let Some(ImportState::Ready(st)) = this.import.as_mut() {
                                if let Some(c) = st.cores.get_mut(idx) {
                                    c.2 = *ch;
                                    cx.notify();
                                }
                            }
                        })),
                );
            }
            body = body.child(row);
        }

        // Core-команды (fixed-sell): показать, что приедет ядрам, но пока не шлём.
        if !state.plan.core_commands.is_empty() {
            let mut group = v_flex()
                .gap_1()
                .child(
                    div()
                        .font_bold()
                        .child(t!("import.group.core_commands").to_string()),
                )
                .child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(rgba_from(p.text_muted, 1.0))
                        .child(t!("import.core_commands_hint").to_string()),
                );
            for item in &state.plan.core_commands {
                group = group.child(
                    div()
                        .text_color(rgba_from(p.text_soft, 1.0))
                        .child(format!("{}: {}", item.label, item.new)),
                );
            }
            body = body.child(group);
        }

        // Warnings + «Не перенесено» (с причинами).
        for w in &state.plan.warnings {
            body = body.child(div().text_color(rgba_from(p.yellow, 1.0)).child(w.clone()));
        }
        if !state.plan.unsupported.is_empty() {
            let mut group = v_flex().gap_0p5().child(
                div()
                    .font_bold()
                    .child(t!("import.group.unsupported").to_string()),
            );
            for u in &state.plan.unsupported {
                group = group.child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(rgba_from(p.text_muted, 1.0))
                        .child(format!("{} — {}", u.name, u.reason)),
                );
            }
            body = body.child(group);
        }

        // Скролл-схема из фикса п.41: внешний flex_1+min_h(0) тянется в остаток карточки,
        // внутренний size_full со scrollbar — иначе Scrollable разваливает высоту.
        card = card.child(
            div().flex_1().min_h(px(0.0)).w_full().child(
                div()
                    .id("import-preview-body")
                    .size_full()
                    .child(body)
                    .overflow_y_scrollbar(),
            ),
        );

        // ── Футер карточки: Применить / Отмена (отбит линией от тела) ──
        card = card.child(
            h_flex()
                .gap(design::ui_px(cx, 8.0))
                .pt(design::ui_px(cx, 8.0))
                .border_t_1()
                .border_color(rgba_from(p.border, 1.0))
                .child(
                    MoonButton::new("import-apply")
                        .primary()
                        .small()
                        .width(130.0)
                        .label(t!("import.apply").to_string())
                        .on_click(cx.listener(|this, _, w, cx| this.apply_moonbot_import(w, cx)))
                        .render(),
                )
                .child(
                    MoonButton::new("import-cancel")
                        .outline()
                        .small()
                        .width(110.0)
                        .label(t!("import.cancel").to_string())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.import = None;
                            cx.notify();
                        }))
                        .render(),
                ),
        );

        Some(backdrop().child(card).into_any_element())
    }

    /// Одна строка пункта: чекбокс + «было → станет» рядом (цвета — свотчами).
    fn change_row(&self, cx: &Context<Self>, item: &SettingChange, p: MoonPalette) -> AnyElement {
        let state_checked = self
            .import_ready()
            .is_some_and(|st| st.selected.contains(&item.id));
        let id = item.id.clone();
        h_flex()
            .w_full()
            .flex_wrap()
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .child(
                MoonCheckbox::new(SharedString::from(format!("imp-{}", item.id)))
                    .checked(state_checked)
                    .label(item.label.clone())
                    .on_change(cx.listener(move |this, ch: &bool, _, cx| {
                        if let Some(ImportState::Ready(st)) = this.import.as_mut() {
                            if *ch {
                                st.selected.insert(id.clone());
                            } else {
                                st.selected.remove(&id);
                            }
                            cx.notify();
                        }
                    })),
            )
            .child(value_el(item, p, cx))
            .into_any_element()
    }

    /// Группа пунктов (Терминал/Ядро): заголовок + однострочные пункты.
    fn import_group(
        &self,
        cx: &Context<Self>,
        title_key: &'static str,
        items: &[SettingChange],
        p: MoonPalette,
    ) -> Option<AnyElement> {
        if items.is_empty() {
            return None;
        }
        let mut group = v_flex()
            .gap_0p5()
            .child(div().font_bold().child(t!(title_key).to_string()));
        for item in items {
            group = group.child(self.change_row(cx, item, p));
        }
        Some(group.into_any_element())
    }

    /// Группа «Хоткеи»: все сопоставленные сочетания (включая совпадающие) + внутри
    /// же — «не перенесено» с причинами и сводка по пустым слотам MoonBot.
    fn hotkeys_group(
        &self,
        cx: &Context<Self>,
        plan: &moon_core::config::moonbot_import::MoonBotImportPlan,
        p: MoonPalette,
    ) -> Option<AnyElement> {
        if plan.hotkeys.is_empty() && plan.unsupported_hotkeys.is_empty() {
            return None;
        }
        let mut group = v_flex().gap_0p5().child(
            div()
                .font_bold()
                .child(t!("import.group.hotkeys").to_string()),
        );
        for item in &plan.hotkeys {
            group = group.child(self.change_row(cx, item, p));
        }
        if plan.hotkeys_empty > 0 {
            group = group.child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(rgba_from(p.text_muted, 1.0))
                    .child(t!("import.hotkeys_empty_note", n = plan.hotkeys_empty).to_string()),
            );
        }
        if !plan.unsupported_hotkeys.is_empty() {
            group = group.child(
                div()
                    .text_color(rgba_from(p.text_soft, 1.0))
                    .child(t!("import.not_transferred_label").to_string()),
            );
            for u in &plan.unsupported_hotkeys {
                group = group.child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(rgba_from(p.text_muted, 1.0))
                        .child(format!("{} — {}", u.name, u.reason)),
                );
            }
        }
        Some(group.into_any_element())
    }

    /// «График и линии»: цвета двумя колонками — Светлая | Тёмная (side из id пункта).
    fn chart_columns(
        &self,
        cx: &Context<Self>,
        items: &[SettingChange],
        p: MoonPalette,
    ) -> Option<AnyElement> {
        if items.is_empty() {
            return None;
        }
        let column = |title_key: &'static str, side: &str| -> AnyElement {
            let mut col = v_flex().flex_1().min_w(px(0.0)).gap_0p5().child(
                div()
                    .text_color(rgba_from(p.text_soft, 1.0))
                    .child(t!(title_key).to_string()),
            );
            for item in items.iter().filter(|c| c.id.ends_with(side)) {
                col = col.child(self.change_row(cx, item, p));
            }
            col.into_any_element()
        };
        Some(
            v_flex()
                .gap_0p5()
                .child(
                    div()
                        .font_bold()
                        .child(t!("import.group.chart").to_string()),
                )
                .child(
                    h_flex()
                        .w_full()
                        .items_start()
                        .gap(design::ui_px(cx, 18.0))
                        .child(column("import.col_light", ".light"))
                        .child(column("import.col_dark", ".dark")),
                )
                .into_any_element(),
        )
    }
}

/// Визуал значения пункта: для цвета — свотчи «было → станет», иначе muted-текст.
/// Совпадающий (`same`) — одно значение + пометка «совпадает» вместо стрелки.
fn value_el(item: &SettingChange, p: MoonPalette, cx: &Context<SettingsView>) -> AnyElement {
    use moon_core::config::moonbot_import::plan::PlannedValue;
    let muted = |s: String| {
        div()
            .text_size(design::t_caption(cx))
            .text_color(rgba_from(p.text_muted, 1.0))
            .child(s)
    };
    if let PlannedValue::Rgb(new_rgb) = item.value {
        let swatch = |rgb: [u8; 3]| {
            div()
                .w(design::ui_px(cx, 14.0))
                .h(design::ui_px(cx, 14.0))
                .rounded(px(3.0))
                .border_1()
                .border_color(rgba_from(p.border, 1.0))
                .bg(gpui::rgb(u32::from_be_bytes([0, rgb[0], rgb[1], rgb[2]])))
        };
        let mut row = h_flex().items_center().gap(design::ui_px(cx, 4.0));
        if item.same {
            return row
                .child(swatch(new_rgb))
                .child(muted(format!("· {}", t!("import.same"))))
                .into_any_element();
        }
        if let Some(cur) = hex_to_rgb(&item.current) {
            row = row.child(swatch(cur));
        }
        return row
            .child(muted("→".into()))
            .child(swatch(new_rgb))
            .into_any_element();
    }
    if item.same {
        return muted(format!("{} · {}", item.new, t!("import.same"))).into_any_element();
    }
    muted(format!("{} → {}", item.current, item.new)).into_any_element()
}

/// `#RRGGBB` (формат `plan::rgb_hex`) → RGB.
fn hex_to_rgb(s: &str) -> Option<[u8; 3]> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}
