//! Тюнер порогов по рыночным полям отчёта (приём из «Аналитики V3») — режим
//! «Фильтры» вкладки «Стратегии»: матрица KPI «Факт vs варианты», конструктор
//! диапазонов от/до по полям и гистограмма профита по квантильным вёдрам
//! выбранного поля. Скоуп — выбранная в списке стратегия (или все). Границы
//! персистятся в layout (строками, как ввёл юзер).

use std::collections::HashMap;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonCheckbox, MoonCheckboxSize, MoonInput, MoonInputEvent, MoonInputState, MoonPalette, h_flex,
    v_flex,
};
use rust_i18n::t;

use super::super::{AnalyticsView, LoadState};
pub(super) use super::state::{
    N_VAR, TunerKind, card, flag_of, fmt_bound, parse_num, staged_dirty,
};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::tuner::{FIELDS, FieldClass, StratFilters};

/// Вёдер гистограммы.
const HIST_BUCKETS: usize = 14;

impl AnalyticsView {
    /// Запрос тюнера: общие фильтры + скоуп выбранной стратегии.
    pub(super) fn tuner_query(&self) -> moon_core::db::analytics::Query {
        let mut q = self.query();
        // Ключ строки — `strategyid@core_uid`: скоуп по стратегии И конкретному ядру.
        if let Some((sid, core)) = self
            .sel_strategy
            .as_ref()
            .and_then(|(k, _)| super::parse_strat_key(k))
        {
            q.strategy = Some(sid);
            q.strat_core = core;
        }
        q
    }

    /// Фоновый пересчёт матрицы KPI по вариантам (+ пороги выбранной стратегии).
    pub(in crate::analytics) fn reload_tuner(&mut self, cx: &mut Context<Self>) {
        self.tuner.seq = self.tuner.seq.wrapping_add(1);
        let req = self.tuner.seq;
        let q = self.tuner_query();
        let sid = q.strategy;
        let variants = self.tuner.variants();
        // Дефолты схемы ядра (числовые поля): чипы прячут значения, равные
        // дефолту — «фильтр не настроен» порогом не является.
        let defaults: HashMap<String, f64> = {
            let b = self.backend.read(cx);
            let store = b.session.store();
            let mut out = HashMap::new();
            for (_, cd) in store.cores() {
                let Some(sch) = cd.schema.as_ref() else {
                    continue;
                };
                for k in &sch.kinds {
                    for s in &k.sections {
                        for f in &s.fields {
                            let Some(d) = f.default.as_ref() else {
                                continue;
                            };
                            if let Ok(v) = d
                                .trim()
                                .trim_end_matches('%')
                                .replace(',', ".")
                                .parse::<f64>()
                            {
                                out.entry(f.name.to_ascii_lowercase()).or_insert(v);
                            }
                        }
                    }
                }
                break; // схемы ядер совпадают — одной достаточно
            }
            out
        };
        // Автоподбор НЕ сбрасываем: правка границ не меняет распределение
        // факта, по которому он считался (сброс — в invalidate()).
        // Carry current numbers as stale during recomputation; any completed
        // non-data result drops them.
        self.tuner.stats.begin();
        self.op_started();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let (stats, strat) = executor
                .spawn(async move {
                    let stats = moon_core::db::tuner::variant_stats(&q, &variants);
                    let sf = sid
                        .map(|sid| moon_core::db::tuner::strategy_filters(sid, &defaults))
                        .unwrap_or_default();
                    (stats, sf)
                })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.op_finished(cx);
                    if this.tuner.seq != req {
                        return;
                    }
                    // A completed non-data result clears stale numbers because
                    // values under a changed period label must belong to it.
                    this.tuner.stats.apply(stats);
                    this.tuner.strat = Arc::new(strat);
                    this.tuner.dirty = false;
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Фоновая гистограмма выбранного поля.
    pub(in crate::analytics) fn reload_hist(&mut self, cx: &mut Context<Self>) {
        self.tuner.hist_seq = self.tuner.hist_seq.wrapping_add(1);
        let req = self.tuner.hist_seq;
        let q = self.tuner_query();
        let field = FIELDS[self.tuner.sel_field].col.to_string();
        self.tuner.hist.begin();
        self.op_started();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let hist = executor
                .spawn(async move { moon_core::db::tuner::histogram(&q, &field, HIST_BUCKETS) })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.op_finished(cx);
                    if this.tuner.hist_seq != req {
                        return;
                    }
                    this.tuner.hist.apply(hist);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Коммит границы (Blur/Enter инпута): состояние → layout → пересчёт.
    fn commit_bound(
        &mut self,
        vi: usize,
        fi: usize,
        is_to: bool,
        value: String,
        cx: &mut Context<Self>,
    ) {
        let slot = &mut self.tuner.bounds[vi][fi];
        let cur = if is_to { &mut slot.1 } else { &mut slot.0 };
        if *cur == value {
            return;
        }
        *cur = value;
        self.reload_tuner(cx);
        cx.notify();
    }

    /// Программная установка ОБЕИХ границ поля (чип стратегии / очистка /
    /// автоподбор): состояние + тихая синхронизация инпутов + пересчёт.
    pub(super) fn apply_bounds(
        &mut self,
        vi: usize,
        fi: usize,
        from: String,
        to: String,
        cx: &mut Context<Self>,
    ) {
        if self.tuner.bounds[vi][fi] == (from.clone(), to.clone()) {
            return;
        }
        self.tuner.bounds[vi][fi] = (from, to);
        // Пересоздаём инпуты (сброс кэша): свежий default_value рисуется с
        // НАЧАЛА строки; sync_value оставлял курсор в конце — длинное значение
        // «уезжало» вправо и виден был только хвост.
        self.tuner.inputs.remove(&format!("tv{vi}f{fi}a"));
        self.tuner.inputs.remove(&format!("tv{vi}f{fi}b"));
        self.reload_tuner(cx);
        cx.notify();
    }

    /// Инпут границы с ленивым кэшем (паттерн field_input_state Стратегий).
    fn bound_input(
        &mut self,
        vi: usize,
        fi: usize,
        is_to: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        let id = format!("tv{vi}f{fi}{}", if is_to { "b" } else { "a" });
        if let Some(state) = self.tuner.inputs.get(&id) {
            return state.clone();
        }
        let slot = &self.tuner.bounds[vi][fi];
        let value = if is_to {
            slot.1.clone()
        } else {
            slot.0.clone()
        };
        let state = cx.new(|cx| MoonInputState::new(window, cx).default_value(value));
        cx.subscribe(&state, move |this, state, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                let value = state.read(cx).value().to_string();
                this.commit_bound(vi, fi, is_to, value, cx);
            }
        })
        .detach();
        self.tuner.inputs.insert(id, state.clone());
        state
    }

    /// Конструктор диапазонов: строки-поля, клик по имени — гистограмма поля.
    pub(super) fn fields_grid(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let in_w = 60.0;
        let mut head = h_flex()
            .w_full()
            .px(design::ui_px(cx, 8.0))
            .h(design::fit_h_px(cx, 22.0, 12.0, 5.0))
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .text_size(design::t_caption(cx))
            .text_color(moon(p.text_soft))
            .bg(moon(p.table_head))
            .child(
                div().flex_none().child(
                    MoonCheckbox::new("tun-en-all")
                        // Немаппленные поля (нет параметра в стратегии) мастер
                        // игнорирует: «все включены» = все МАППЛЕННЫЕ.
                        .checked(
                            FIELDS
                                .iter()
                                .enumerate()
                                .filter(|(_, s)| s.mapped())
                                .all(|(fi, _)| self.tuner.enabled[fi]),
                        )
                        .size(MoonCheckboxSize::Compact)
                        .on_change({
                            let view = cx.entity();
                            move |ch: &bool, _w, app| {
                                let on = *ch;
                                view.update(app, |this, cx| {
                                    for (fi, spec) in FIELDS.iter().enumerate() {
                                        this.tuner.enabled[fi] = on && spec.mapped();
                                    }
                                    cx.notify();
                                });
                            }
                        }),
                ),
            )
            .child(
                div()
                    .w(design::font_w_px(cx, 58.0))
                    .flex_none()
                    .child(t!("analytics.tuner.field").to_string()),
            )
            // Колонка чипа: пороги выбранной стратегии (клик — в v1).
            .child(
                div()
                    .flex_1()
                    .child(t!("analytics.tuner.strat_chip").to_string()),
            );
        for vi in 0..N_VAR {
            head = head
                .child(
                    div()
                        .w(design::font_w_px(cx, in_w))
                        .flex_none()
                        .text_center()
                        .child(format!("v{} {}", vi + 1, t!("analytics.tuner.from"))),
                )
                // v2: копирование ВСЕЙ колонки v1 → v2 (между «от» и «до»).
                .when(vi == 1, |el| {
                    el.child(
                        div()
                            .id("tun-cp-col")
                            .w(design::ui_px(cx, 12.0))
                            .flex_none()
                            .cursor_pointer()
                            .text_color(moon(p.text_muted))
                            .hover(move |st| st.text_color(moon(p.amber)))
                            .child("→")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.copy_v1_to_v2(None, cx);
                            })),
                    )
                })
                .child(
                    div()
                        .w(design::font_w_px(cx, in_w))
                        .flex_none()
                        .text_center()
                        .child(t!("analytics.tuner.to").to_string()),
                )
                // Очистка ВСЕЙ колонки варианта — над строчными крестиками.
                .child(
                    div()
                        .id(SharedString::from(format!("tun-clr-col-{vi}")))
                        .w(design::ui_px(cx, 12.0))
                        .flex_none()
                        .cursor_pointer()
                        .text_color(moon(p.text_muted))
                        .hover(move |st| st.text_color(moon(p.orange)))
                        .child("✕")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.clear_variant(vi, cx);
                        })),
                );
        }

        let strat = self.tuner.strat.clone();
        let mut grid = v_flex().w_full().child(head);
        let mut last_class: Option<FieldClass> = None;
        for fi in 0..FIELDS.len() {
            let class = FIELDS[fi].class;
            // Заголовки: секция MoonBot (родитель), затем подгруппа с
            // отступом (BV/SV внутри Объёмов, слоты Δ2/Δ3 внутри Дельт).
            let sub = class.parent() != class;
            if last_class != Some(class) {
                if sub && last_class.map(|c| c.parent()) != Some(class.parent()) {
                    grid = grid.child(self.group_header(class.parent(), &strat, p, cx));
                }
                last_class = Some(class);
                grid = grid.child(self.group_header(class, &strat, p, cx));
            }
            let selected = self.tuner.sel_field == fi;
            let unmapped = !FIELDS[fi].mapped();
            let mut row = h_flex()
                .id(SharedString::from(format!("tun-field-{fi}")))
                .w_full()
                .px(design::ui_px(cx, 8.0))
                .when(sub, |el| el.pl(design::ui_px(cx, 22.0)))
                .py(design::ui_px(cx, 2.0))
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .border_t_1()
                .border_color(moon_alpha(p.border, 0.5))
                .child(
                    div().flex_none().child(
                        MoonCheckbox::new(SharedString::from(format!("tun-en-{fi}")))
                            .checked(self.tuner.enabled[fi])
                            .size(MoonCheckboxSize::Compact)
                            .on_change({
                                let view = cx.entity();
                                move |ch: &bool, _w, app| {
                                    let on = *ch;
                                    view.update(app, |this, cx| {
                                        this.tuner.enabled[fi] = on;
                                        cx.notify();
                                    });
                                }
                            }),
                    ),
                )
                .child(
                    div()
                        .w(design::font_w_px(cx, 58.0))
                        .flex_none()
                        .truncate()
                        .cursor_pointer()
                        .text_color(if selected {
                            moon(p.amber)
                        } else if unmapped {
                            // Поле без параметров стратегии — приглушаем.
                            moon(p.text_muted)
                        } else {
                            moon(p.text)
                        })
                        .child(FIELDS[fi].label.to_string()),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.tuner.sel_field != fi {
                        this.tuner.sel_field = fi;
                        // Another field's histogram is not this field's stale
                        // data — drop it outright rather than carrying it.
                        this.tuner.hist = LoadState::default();
                        this.reload_hist(cx);
                        cx.notify();
                    }
                }));
            if selected {
                row = row.bg(moon_alpha(p.amber, 0.08));
            }
            // Чип: НЕдефолтные пороги выбранной стратегии (справочно). Поля-
            // слоты показывают назначение «Δ2/Δ3» + пороги слота. Если класс
            // игнорируется флагами — значений НЕ показываем (метка «ignore»
            // стоит на подзаголовке группы).
            let chip: Option<(Option<u8>, Option<f64>, Option<f64>)> =
                if strat.found && !strat.class_ignored(class) {
                    if class == FieldClass::DeltaSlot {
                        strat
                            .slot_of(FIELDS[fi].col)
                            .map(|(n, lo, hi)| (Some(n), lo, hi))
                    } else {
                        strat
                            .bounds
                            .get(FIELDS[fi].col)
                            .copied()
                            .map(|(lo, hi)| (None, lo, hi))
                    }
                } else {
                    None
                };
            row = row.child(match chip {
                Some((slot, lo, hi)) => {
                    // Диапазон компактно: «(от…до)»; открытая сторона пустая.
                    let range = (lo.is_some() || hi.is_some()).then(|| {
                        format!(
                            "({}…{})",
                            lo.map(fmt_bound).unwrap_or_default(),
                            hi.map(fmt_bound).unwrap_or_default()
                        )
                    });
                    let text = [slot.map(|n| format!("Δ{n}")), range]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" ");
                    // Клик по чипу: значения стратегии → v1 (замена) + снять
                    // галку участия — поле становится ФИКСИРОВАННЫМ фильтром,
                    // перебор его не трогает.
                    let (from_s, to_s) = (
                        lo.map(fmt_bound).unwrap_or_default(),
                        hi.map(fmt_bound).unwrap_or_default(),
                    );
                    div()
                        .id(SharedString::from(format!("tun-chip-{fi}")))
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .cursor_pointer()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.amber))
                        .hover(move |st| st.text_color(moon(p.text)))
                        .child(text)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.tuner.enabled[fi] = false;
                            this.apply_bounds(0, fi, from_s.clone(), to_s.clone(), cx);
                            cx.notify();
                        }))
                        .into_any_element()
                }
                // Немаппленное поле: вместо чипа — пометка, что подобранный
                // порог записать в стратегию нечем (параметра нет).
                None if unmapped => div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(design::t_caption(cx))
                    .text_color(moon_alpha(p.text_muted, 0.7))
                    .child(t!("analytics.tuner.no_param").to_string())
                    .into_any_element(),
                None => div().flex_1().into_any_element(),
            });
            for vi in 0..N_VAR {
                for is_to in [false, true] {
                    // v2: копирование строки v1 → v2 (между «от» и «до»).
                    if vi == 1 && is_to {
                        row = row.child(
                            div()
                                .id(SharedString::from(format!("tun-cp-{fi}")))
                                .flex_none()
                                .px(design::ui_px(cx, 2.0))
                                .cursor_pointer()
                                .text_size(design::t_caption(cx))
                                .text_color(moon(p.text_muted))
                                .hover(move |st| st.text_color(moon(p.amber)))
                                .child("→")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.copy_v1_to_v2(Some(fi), cx);
                                })),
                        );
                    }
                    let input = self.bound_input(vi, fi, is_to, window, cx);
                    row = row.child(
                        div().w(design::font_w_px(cx, in_w)).flex_none().child(
                            MoonInput::new(SharedString::from(format!("tun-in-{vi}-{fi}-{is_to}")))
                                .state(&input)
                                .small(),
                        ),
                    );
                }
                // Очистка обеих границ варианта в этой строке.
                row = row.child(
                    div()
                        .id(SharedString::from(format!("tun-clr-{vi}-{fi}")))
                        .flex_none()
                        .px(design::ui_px(cx, 2.0))
                        .cursor_pointer()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_muted))
                        .hover(move |s| s.text_color(moon(p.orange)))
                        .child("✕")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.apply_bounds(vi, fi, String::new(), String::new(), cx);
                        })),
                );
            }
            grid = grid.child(row);
        }
        // Строка подбора и тулбар — из ОБЩЕЙ оболочки (tuner_shell), ось
        // «По фильтру»: один код для всех тюнеров, действия диспатчатся по оси.
        let cfg_row = self.shell_config_row(TunerKind::Filter, p, window, cx);
        let header = self.shell_toolbar(
            TunerKind::Filter,
            t!("analytics.tuner.fields_title").to_string(),
            p,
            cx,
        );
        v_flex()
            .w_full()
            .flex_none()
            .rounded(design::ui_px(cx, 8.0))
            .bg(moon(p.panel))
            .border_1()
            .border_color(moon(p.border))
            .overflow_hidden()
            .child(header)
            .child(cfg_row)
            .child(grid)
            .into_any_element()
    }

    /// Подзаголовок группы: подпись + кликабельное «ignore» (стейджится) и
    /// «применить», когда стейдж отличается от текущего флага стратегии —
    /// пишет В СТРАТЕГИЮ только этот флаг (вернуть игнор назад так же легко,
    /// как снять его сохранением порогов).
    fn group_header(
        &self,
        class: FieldClass,
        strat: &StratFilters,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let label = match class {
            FieldClass::Filter => t!("analytics.tuner.grp_filter"),
            FieldClass::BvSv => t!("analytics.tuner.grp_bvsv"),
            FieldClass::Ping => t!("analytics.tuner.grp_ping"),
            FieldClass::Base => t!("analytics.tuner.grp_base"),
            FieldClass::DeltaSlot => t!("analytics.tuner.grp_slot"),
            FieldClass::Delta => t!("analytics.tuner.grp_delta"),
            FieldClass::Volume => t!("analytics.tuner.grp_volume"),
        }
        .to_string();
        // Подгруппа секции MoonBot (BV/SV в Объёмах, слоты Δ2/Δ3 в Дельтах):
        // отступ + бледнее фон. У слотов НЕТ своего флага (гейт — IgnoreDelta
        // родителя), кликабельный ignore не рисуем; у BV/SV — свой включатель.
        let sub = class.parent() != class;
        let mut hdr = h_flex()
            .w_full()
            .px(design::ui_px(cx, 8.0))
            .when(sub, |el| el.pl(design::ui_px(cx, 22.0)))
            .py(design::ui_px(cx, 2.0))
            .gap(design::ui_px(cx, 6.0))
            .items_center()
            .bg(moon_alpha(p.table_head, if sub { 0.35 } else { 0.6 }))
            .border_t_1()
            .border_color(moon_alpha(p.border, 0.7))
            .text_size(design::t_caption(cx))
            .child(div().text_color(moon(p.text_soft)).child(label));
        if strat.found && !(sub && class != FieldClass::BvSv) {
            let (flag, cur_ignore) = flag_of(class, strat);
            let staged = self.tuner.staged_ignore.get(flag).copied();
            let shown = staged.unwrap_or(cur_ignore);
            hdr = hdr.child(
                div()
                    .id(SharedString::from(format!("tun-ign-{flag}")))
                    .cursor_pointer()
                    .text_color(if shown {
                        // Фильтры класса ИГНОРИРУЮТСЯ — тёмно-серый.
                        moon_alpha(p.text_muted, 0.7)
                    } else {
                        // Фильтры работают — зелёный.
                        moon(p.green)
                    })
                    .child(if shown { "ignore=YES" } else { "ignore=NO" })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let (_, cur) = flag_of(class, &this.tuner.strat.clone());
                        let now = this.tuner.staged_ignore.get(flag).copied().unwrap_or(cur);
                        if !now == cur {
                            this.tuner.staged_ignore.remove(flag);
                        } else {
                            this.tuner.staged_ignore.insert(flag, !now);
                        }
                        cx.notify();
                    })),
            );
        }
        hdr.into_any_element()
    }
}
