//! Панель «Версии» окна «Стратегии» (между деревом и разделами): история версий
//! выбранной стратегии из strategies.sqlite со статистикой «дд.мм (изменено)(профит$)».
//! Профит — ленивый кэш version_stats (`strat_db::stats`), считается с background
//! executor. Верхняя строка — текущая версия (живые параметры); клик по старой
//! подставляет её поля в панели разделов/параметров (только чтение).

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use super::{Key, StrategiesView, logic};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::feed::StrategyRow;
use moon_core::strat_db::stats::{VersionInfo, short_date};

/// Состояние панели версий.
#[derive(Default)]
pub(super) struct VersionsState {
    pub list: Vec<VersionInfo>,
    /// Для какой стратегии загружен `list`.
    pub key: Option<Key>,
    /// Поколение strat_db на момент загрузки — новая запись = перезагрузка.
    pub db_gen: u64,
    pub inflight: bool,
    /// valid_from выбранной СТАРОЙ версии; None = текущая (живые параметры).
    pub sel: Option<i64>,
    /// Синтетическая строка старой версии (поля из raw_json) для панелей справа.
    pub row: Option<(Key, i64, StrategyRow)>,
    /// Изменённые в выбранной версии поля: имя(lowercase) → старое значение
    /// (display; пустое = поля не было). Пусто — дифф отсутствует (created).
    pub changed: std::collections::HashMap<String, String>,
    /// Раздел в режиме просмотра версии: None = псевдораздел «Все» (только
    /// изменённые поля всех разделов), Some(i) = раздел схемы (тоже фильтр).
    pub section: Option<usize>,
}

impl StrategiesView {
    /// Смотрим старую версию? (панели параметров — только чтение).
    pub(super) fn viewing_version(&self) -> bool {
        self.versions.sel.is_some()
    }

    /// Синтетическая строка выбранной версии, если она про текущий выбор.
    pub(super) fn version_override(&self) -> Option<(Key, &StrategyRow)> {
        let vf = self.versions.sel?;
        let (key, row_vf, row) = self.versions.row.as_ref()?;
        (Some(*key) == self.selected && *row_vf == vf).then_some((*key, row))
    }

    /// Фильтр «только изменённые поля» активен? (просмотр версии с непустым диффом).
    pub(super) fn version_changed_filter(
        &self,
    ) -> Option<&std::collections::HashMap<String, String>> {
        if self.viewing_version() && !self.versions.changed.is_empty() {
            Some(&self.versions.changed)
        } else {
            None
        }
    }

    /// Перезагрузка списка версий при смене выбора/поколения БД (fire-and-forget).
    fn ensure_versions(&mut self, cx: &mut Context<Self>) {
        let key = self.selected;
        let db_gen = moon_core::strat_db::generation();
        if self.versions.key == key && (self.versions.db_gen == db_gen || self.versions.inflight) {
            return;
        }
        if self.versions.key != key {
            // Другая стратегия — сброс выбора версии и списка.
            self.versions.list.clear();
            self.versions.sel = None;
            self.versions.row = None;
            self.versions.changed.clear();
            self.versions.section = None;
        }
        self.versions.key = key;
        self.versions.db_gen = db_gen;
        let Some((core, id)) = key else { return };
        self.versions.inflight = true;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let list = executor
                .spawn(async move {
                    moon_core::strat_db::stats::versions_with_stats(core, id as i64)
                })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.versions.inflight = false;
                    if this.versions.key == Some((core, id)) {
                        this.versions.list = list;
                        cx.notify();
                    }
                });
            });
        })
        .detach();
    }

    /// Выбор версии: None = текущая (живая), Some = старая (грузим её поля фоном).
    fn select_version(&mut self, vf: Option<i64>, cx: &mut Context<Self>) {
        if self.versions.sel == vf {
            return;
        }
        self.versions.sel = vf;
        self.versions.row = None;
        self.versions.changed.clear();
        self.versions.section = None; // по умолчанию — «Все» (только изменения)
        cx.notify();
        let (Some((core, id)), Some(vf)) = (self.selected, vf) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let view = executor
                .spawn(async move {
                    moon_core::strat_db::stats::version_view(core, id as i64, vf)
                })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    let Some(view) = view else { return };
                    if this.versions.sel != Some(vf) || this.selected != Some((core, id)) {
                        return; // выбор уже ушёл
                    }
                    // Синтетическая строка: живой снапшот (вид/папка/галка) +
                    // поля версии; имя — из версии (могло быть переименование).
                    let live = {
                        let b = this.backend.read(cx);
                        let store = b.session.store();
                        logic::row(store, core, id).cloned()
                    };
                    if let Some(mut r) = live {
                        if let Some((_, n)) = view.fields.iter().find(|(k, _)| k == "StrategyName")
                        {
                            if !n.is_empty() {
                                r.name = n.clone();
                            }
                        }
                        r.fields = view.fields;
                        this.versions.changed = view
                            .changed
                            .into_iter()
                            .map(|(k, old)| (k.to_lowercase(), old))
                            .collect();
                        this.versions.row = Some(((core, id), vf, r));
                        cx.notify();
                    }
                });
            });
        })
        .detach();
    }

    /// Панель «Версии» (колонка между деревом и разделами).
    pub(super) fn versions_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = MoonPalette::active(cx);
        let border = moon(p.border);
        let mut col = v_flex()
            .w(design::font_w_px(cx, 158.0))
            .flex_none()
            .h_full()
            .bg(moon(p.shell_high))
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .line_height(design::line_px(cx, 14.0))
            .border_r_1()
            .border_color(border)
            .px(design::ui_px(cx, 8.0))
            .py(design::ui_px(cx, 12.0))
            .gap(design::ui_px(cx, 7.0))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t!("strat.versions").to_string()),
            )
            .child(div().w_full().h(px(1.0)).bg(border));

        let hint = |s: String| div().mt_2().text_color(moon(p.text_muted)).child(s);
        if self.selected.is_none() {
            return col.child(hint(t!("strat.no_selection").to_string())).into_any_element();
        }
        // Мультивыбор: версии недоступны (панели показывают объединение живых).
        if self.sel.len() > 1 {
            self.versions.sel = None;
            self.versions.row = None;
            return col
                .child(hint(t!("strat.versions_multi").to_string()))
                .into_any_element();
        }
        self.ensure_versions(cx);
        if self.versions.list.is_empty() {
            let text = if self.versions.inflight {
                "…".to_string()
            } else {
                t!("strat.versions_empty").to_string()
            };
            return col.child(hint(text)).into_any_element();
        }

        let mut list = v_flex().w_full().gap_0();
        for (i, v) in self.versions.list.iter().enumerate() {
            let is_current = i == 0;
            let on = if is_current {
                self.versions.sel.is_none()
            } else {
                self.versions.sel == Some(v.valid_from)
            };
            let profit_col = if v.profit > 0.0 {
                p.green
            } else if v.profit < 0.0 {
                p.orange
            } else {
                p.text_muted
            };
            let profit = format!(
                "({}{}$)",
                if v.profit > 0.0 { "+" } else { "" },
                moon_core::util::fmt::compact(v.profit, 2)
            );
            let vf = v.valid_from;
            let mut row = h_flex()
                .id(SharedString::from(format!("ver-{vf}")))
                .w_full()
                .h(design::fit_h_px(cx, 24.0, 14.0, 5.0))
                .px(design::ui_px(cx, 6.0))
                .rounded(design::ui_px(cx, 3.0))
                .border_1()
                .border_color(moon_alpha(p.border, 0.0))
                .items_center()
                .gap_1()
                .cursor_pointer()
                .child(
                    div()
                        .text_color(moon(p.text))
                        .child(short_date(v.valid_from)),
                )
                .child(
                    div()
                        .text_color(moon(p.text_soft))
                        .child(format!("({})", v.n_changed)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(moon(profit_col))
                        .child(profit),
                )
                .when(is_current, |r| {
                    r.child(
                        div()
                            .flex_none()
                            .text_size(design::t_caption(cx))
                            .text_color(moon(p.text_muted))
                            .child(t!("strat.versions_current").to_string()),
                    )
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.select_version(if is_current { None } else { Some(vf) }, cx);
                }));
            if on {
                row = row
                    .bg(moon_alpha(p.amber, 0.16))
                    .border_color(moon_alpha(p.amber, 0.55));
            } else {
                row = row.hover(move |s| s.bg(moon_alpha(p.panel, 0.74)));
            }
            list = list.child(row);
        }
        col = col.child(
            div()
                .id("strat-versions-scroll")
                .flex_1()
                .w_full()
                .overflow_y_scroll()
                .child(list),
        );
        col.into_any_element()
    }
}
