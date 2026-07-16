//! Действия тюнера: автоподбор порогов («Подобрать» / «Подобрать всё») и
//! запись v1 в параметры выбранной стратегии («В стратегию», с включением
//! нужных Ignore-классов). Вынесено из tuner.rs (лимит размера файла).

use std::collections::HashMap;
use std::sync::Arc;

use gpui::*;

use super::AnalyticsView;
use super::tuner::{fmt_bound, parse_num};
use moon_core::db::tuner::{FIELDS, FieldClass, params_for};

impl AnalyticsView {
    /// «Подобрать всё»: лучшие диапазоны ВСЕХ полей одним сканом → v1.
    pub(super) fn suggest_into_v1(&mut self, cx: &mut Context<Self>) {
        self.tuner.sugg_seq = self.tuner.sugg_seq.wrapping_add(1);
        let req = self.tuner.sugg_seq;
        self.tuner.sugg_busy = true;
        let q = self.tuner_query();
        let min_n = self.suggest_min_n();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let sugg = executor
                .spawn(async move { moon_core::db::tuner::suggest_all(&q, min_n) })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.tuner.sugg_seq != req {
                        return;
                    }
                    this.tuner.sugg_busy = false;
                    if let Some(res) = sugg {
                        let by_field: HashMap<&str, _> = res.into_iter().collect();
                        for fi in 0..FIELDS.len() {
                            let (from, to) = by_field
                                .get(FIELDS[fi].0)
                                .map(|s| {
                                    (
                                        s.from.map(fmt_bound).unwrap_or_default(),
                                        s.to.map(fmt_bound).unwrap_or_default(),
                                    )
                                })
                                .unwrap_or_default();
                            this.tuner.bounds[0][fi] = (from, to);
                            this.tuner.inputs.remove(&format!("tv0f{fi}a"));
                            this.tuner.inputs.remove(&format!("tv0f{fi}b"));
                        }
                        this.persist_tuner(cx);
                        this.reload_tuner(cx);
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// «Подобрать»: лучший диапазон ВЫБРАННОГО поля → его строка v1.
    pub(super) fn suggest_one_into_v1(&mut self, cx: &mut Context<Self>) {
        self.tuner.sugg_seq = self.tuner.sugg_seq.wrapping_add(1);
        let req = self.tuner.sugg_seq;
        self.tuner.sugg_busy = true;
        let fi = self.tuner.sel_field;
        let field = FIELDS[fi].0.to_string();
        let q = self.tuner_query();
        let min_n = self.suggest_min_n();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let sugg = executor
                .spawn(async move { moon_core::db::tuner::suggest_field(&q, &field, min_n) })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.tuner.sugg_seq != req {
                        return;
                    }
                    this.tuner.sugg_busy = false;
                    if let Some(s) = sugg {
                        let from = s.from.map(fmt_bound).unwrap_or_default();
                        let to = s.to.map(fmt_bound).unwrap_or_default();
                        this.apply_bounds(0, fi, from, to, cx);
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Минимум сделок для автоподбора: не «оптимизируемся» в пару счастливых
    /// сделок — держим ≥1/5 факта (но не меньше 30).
    fn suggest_min_n(&self) -> i64 {
        self.tuner
            .stats
            .as_ref()
            .and_then(|s| s.first().map(|f| f.n / 5))
            .unwrap_or(0)
            .max(30)
    }

    /// «В стратегию»: пороги v1 → параметры выбранной стратегии на всех её
    /// ядрах (sync шлёт полный набор — правки одной командой на ядро). Если
    /// классы затронутых полей игнорировались (IgnoreFilters/IgnoreDelta/
    /// IgnoreVolume) — соответствующие флаги выключаются, иначе пороги не
    /// имели бы эффекта. Пишутся ТОЛЬКО поля с маппингом на параметры.
    pub(super) fn save_v1_to_strategy(&mut self, cx: &mut Context<Self>) {
        let Some((key, name)) = self.sel_strategy.clone() else {
            return;
        };
        let Ok(sid) = key.parse::<i64>() else { return };

        let mut changes: Vec<(String, String)> = Vec::new();
        let (mut delta_touched, mut volume_touched) = (false, false);
        for (fi, (col, _, class)) in FIELDS.iter().enumerate() {
            let (pmin, pmax) = params_for(col);
            let (from, to) = &self.tuner.bounds[0][fi];
            for (txt, param) in [(from, pmin), (to, pmax)] {
                let Some(param) = param else { continue };
                let Some(v) = parse_num(txt) else { continue };
                changes.push((param.to_string(), fmt_plain(v)));
                match class {
                    FieldClass::Delta => delta_touched = true,
                    FieldClass::Volume => volume_touched = true,
                    FieldClass::Filter => {}
                }
            }
        }
        if changes.is_empty() {
            log::info!("аналитика: «В стратегию» — в v1 нет полей с маппингом на параметры");
            return;
        }
        // Включаем затронутые классы фильтров, если стратегия их игнорировала.
        let f = self.tuner.strat.clone();
        if f.ignore_filters {
            changes.push(("IgnoreFilters".to_string(), "NO".to_string()));
        }
        if delta_touched && f.ignore_delta {
            changes.push(("IgnoreDelta".to_string(), "NO".to_string()));
        }
        if volume_touched && f.ignore_volume {
            changes.push(("IgnoreVolume".to_string(), "NO".to_string()));
        }

        let n_fields = changes.len();
        // Ядра стратегии — из strategies.sqlite (не на UI-потоке), затем
        // правки с UI-потока обычным путём окна Стратегий (edit_strategies).
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let cores = executor
                .spawn(async move { moon_core::db::tuner::strategy_cores(sid) })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    let changes = Arc::new(changes);
                    let mut sent = 0usize;
                    {
                        let b = this.backend.read(cx);
                        for core in &cores {
                            let edits = vec![(sid as u64, changes.as_ref().clone())];
                            match b.session.edit_strategies(*core, edits) {
                                Ok(()) => sent += 1,
                                Err(e) => log::warn!(
                                    "аналитика: пороги → «{name}» ядро {core} не ушли: {e:#}"
                                ),
                            }
                        }
                    }
                    log::info!(
                        "аналитика: пороги v1 → стратегия «{name}»: {n_fields} полей, ядер {sent}/{}",
                        cores.len()
                    );
                    // Эхо снапшота обновит strategies.sqlite — перечитаем чипы.
                    this.reload_tuner(cx);
                    cx.notify();
                });
            });
        })
        .detach();
    }
}

/// Число для параметра стратегии: простой десятичный формат без суффиксов.
fn fmt_plain(v: f64) -> String {
    let mut s = format!("{v:.4}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    s
}
