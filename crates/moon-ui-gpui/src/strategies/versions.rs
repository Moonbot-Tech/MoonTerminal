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
    /// Выбрать ПОСЛЕДНЮЮ версию, как только список догрузится (клик по
    /// удалённой стратегии: живого режима нет — сразу её финальные параметры).
    pub pending_latest: bool,
    /// Панель свёрнута влево в узкую полоску (виден только счётчик версий).
    pub collapsed: bool,
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
                        // Удалённая стратегия: живого режима нет — сразу
                        // открываем последнюю известную версию.
                        if this.versions.pending_latest {
                            this.versions.pending_latest = false;
                            if let Some(vf) = this.versions.list.first().map(|v| v.valid_from) {
                                this.select_version(Some(vf), cx);
                            }
                        }
                        cx.notify();
                    }
                });
            });
        })
        .detach();
    }

    /// Фоновая загрузка удалённых стратегий (папка «Удалённые» дерева): по
    /// смене поколения strat_db (soft-delete пишется на FullSet-снапшоте).
    pub(super) fn ensure_deleted(&mut self, cx: &mut Context<Self>) {
        let db_gen = moon_core::strat_db::generation();
        if self.deleted_gen == db_gen || self.deleted_inflight {
            return;
        }
        self.deleted_gen = db_gen;
        self.deleted_inflight = true;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let heads = executor
                .spawn(async move { moon_core::strat_db::stats::deleted_heads() })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.deleted_inflight = false;
                    let mut map: std::collections::HashMap<
                        moon_core::session::CoreId,
                        Vec<moon_core::strat_db::stats::HeadRow>,
                    > = std::collections::HashMap::new();
                    for h in heads {
                        map.entry(h.core_uid).or_default().push(h);
                    }
                    if this.deleted != map {
                        this.deleted = map;
                        cx.notify();
                    }
                });
            });
        })
        .detach();
    }

    /// Восстановить удалённую стратегию под её СТАРЫМ id (ПКМ → «Восстановить»):
    /// head + поля последней версии грузятся фоном, затем `RestoreStrategy` в
    /// ядро. Эхо-снапшот оживит head (restored-версия), профит склеится по id.
    pub(super) fn restore_deleted_strategy(
        &mut self,
        core: moon_core::session::CoreId,
        id: u64,
        cx: &mut Context<Self>,
    ) {
        let backend = self.backend.clone();
        cx.spawn(async move |_this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let payload = executor
                .spawn(async move {
                    let head = moon_core::strat_db::stats::head_row(core, id as i64)?;
                    let fields =
                        moon_core::strat_db::stats::latest_version_fields(core, id as i64)?;
                    Some((head, fields))
                })
                .await;
            let _ = cx.update(|cx| {
                let Some((head, fields)) = payload else {
                    log::warn!("восстановление {id}: нет head/версий в strat_db");
                    return;
                };
                if let Err(e) = backend.read(cx).session.restore_strategy(
                    core,
                    id,
                    head.kind_ordinal,
                    head.folder_path,
                    fields,
                ) {
                    log::warn!("restore strategy {id} failed: {e}");
                }
            });
        })
        .detach();
    }

    /// Выбор УДАЛЁННОЙ стратегии из дерева (папка «Удалённые»): обычный выбор +
    /// автопереход на последнюю версию (живых параметров у неё нет).
    pub(super) fn select_deleted_strategy(&mut self, key: Key, cx: &mut Context<Self>) {
        self.sel.clear();
        self.sel.insert(key);
        self.anchor = Some(key);
        self.selected = Some(key);
        self.selected_folder = None;
        if self.versions.key == Some(key) && !self.versions.list.is_empty() {
            let vf = self.versions.list[0].valid_from;
            self.versions.pending_latest = false;
            self.select_version(Some(vf), cx);
        } else {
            self.versions.pending_latest = true;
        }
        cx.notify();
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
            let (view, head) = executor
                .spawn(async move {
                    (
                        moon_core::strat_db::stats::version_view(core, id as i64, vf),
                        moon_core::strat_db::stats::head_row(core, id as i64),
                    )
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
                    // Удалённой стратегии в сторе нет — база из head-строки БД.
                    let live = {
                        let b = this.backend.read(cx);
                        let store = b.session.store();
                        logic::row(store, core, id).cloned()
                    };
                    let base = live.or_else(|| {
                        head.map(|h| StrategyRow {
                            id,
                            name: h.name,
                            kind: h.kind,
                            kind_ordinal: h.kind_ordinal,
                            folder_path: h.folder_path,
                            checked: false,
                            is_short: h.is_short,
                            fields: Vec::new(),
                        })
                    });
                    if let Some(mut r) = base {
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

    /// Панель «Версии» (колонка между деревом и разделами). Сворачивается влево
    /// в узкую полоску: стрелка + счётчик версий выбранной стратегии.
    pub(super) fn versions_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = MoonPalette::active(cx);
        let border = moon(p.border);
        let single = self.selected.is_some() && self.sel.len() <= 1;
        if self.versions.collapsed {
            if single {
                self.ensure_versions(cx); // счётчик на полоске должен быть свежим
            }
            let count = single.then(|| self.versions.list.len()).filter(|n| *n > 0);
            return v_flex()
                .id("versions-collapsed")
                .w(design::ui_px(cx, 22.0))
                .flex_none()
                .h_full()
                .bg(moon(p.shell_high))
                .border_r_1()
                .border_color(border)
                .items_center()
                .pt(design::ui_px(cx, 12.0))
                .gap(design::ui_px(cx, 6.0))
                .cursor_pointer()
                .font_family(design::mono())
                .text_size(design::t_body(cx))
                .text_color(moon(p.text_muted))
                .hover(move |s| s.bg(moon_alpha(p.panel, 0.74)))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.versions.collapsed = false;
                    cx.notify();
                }))
                .child(div().child("▸"))
                .when_some(count, |s, n| {
                    s.child(
                        div()
                            .text_size(design::t_caption(cx))
                            .child(n.to_string()),
                    )
                })
                .into_any_element();
        }
        let mut col = v_flex()
            .w(design::font_w_px(cx, 166.0))
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
                h_flex()
                    .w_full()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("strat.versions").to_string()),
                    )
                    // Свернуть панель влево (останется полоска со счётчиком).
                    .child(
                        div()
                            .id("versions-collapse")
                            .px(design::ui_px(cx, 4.0))
                            .rounded(design::ui_px(cx, 3.0))
                            .cursor_pointer()
                            .text_color(moon(p.text_muted))
                            .hover(move |s| s.bg(moon_alpha(p.panel, 0.74)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.versions.collapsed = true;
                                cx.notify();
                            }))
                            .child("◂"),
                    ),
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

        // Удалённая стратегия (есть в БД, нет в живом сторе): живого режима нет —
        // строка «Текущая» не показывается, список только исторический.
        let live_exists = self
            .selected
            .map(|(c, id)| {
                let b = self.backend.read(cx);
                logic::row(b.session.store(), c, id).is_some()
            })
            .unwrap_or(false);
        let mut list = v_flex().w_full().gap_0();
        // «Текущая» — живой режим (все поля, редактирование), выбор по умолчанию.
        // Ниже отдельной строкой идёт та же текущая версия («тек.»), но кликом
        // открывается её ДИФФ+профит, как у исторических.
        if live_exists {
            let live_on = self.versions.sel.is_none();
            let mut live_row = h_flex()
                .id("ver-live")
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
                        .flex_1()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(moon(p.text))
                        .child(t!("strat.versions_live").to_string()),
                )
                // Дата последней версии (той, что сейчас в ядре) — справа, тускло.
                .child(
                    div()
                        .flex_none()
                        .text_color(moon(p.text_muted))
                        .child(
                            self.versions
                                .list
                                .first()
                                .map(|v| short_date(v.valid_from))
                                .unwrap_or_default(),
                        ),
                )
                .on_click(cx.listener(|this, _, _, cx| this.select_version(None, cx)));
            if live_on {
                live_row = live_row
                    .bg(moon_alpha(p.amber, 0.16))
                    .border_color(moon_alpha(p.amber, 0.55));
            } else {
                live_row = live_row.hover(move |s| s.bg(moon_alpha(p.panel, 0.74)));
            }
            list = list.child(live_row);
        }
        for (i, v) in self.versions.list.iter().enumerate() {
            let is_current = i == 0;
            let on = self.versions.sel == Some(v.valid_from);
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
                .when(is_current && live_exists, |r| {
                    r.child(
                        div()
                            .flex_none()
                            .text_size(design::t_caption(cx))
                            .text_color(moon(p.text_muted))
                            .child(t!("strat.versions_current").to_string()),
                    )
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.select_version(Some(vf), cx);
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
