//! Действия тюнера: автоподбор порогов («Подобрать» / «Подобрать всё») и
//! запись v1 в параметры выбранной стратегии («В стратегию», с включением
//! нужных Ignore-классов). Вынесено из tuner.rs (лимит размера файла).

use std::collections::HashMap;
use std::sync::Arc;

use gpui::*;

use rust_i18n::t;

use super::AnalyticsView;
use super::tuner::{fmt_bound, parse_num};
use super::tuner_state::SaveDialog;
use moon_core::db::tuner::{FIELDS, FieldClass, slot_type_for};

impl AnalyticsView {
    /// «Подобрать всё» — УМНЫЙ подбор (координатный спуск): максимизируем
    /// суммарный профит комбинации диапазонов ВСЕХ полей сразу; итераций и
    /// минимум сделок — из конфиг-строки. Результат → v1.
    pub(super) fn suggest_into_v1(&mut self, cx: &mut Context<Self>) {
        self.tuner.sugg_seq = self.tuner.sugg_seq.wrapping_add(1);
        let req = self.tuner.sugg_seq;
        self.tuner.sugg_busy = true;
        let q = self.tuner_query();
        let rounds = self
            .tuner
            .iters
            .trim()
            .parse::<usize>()
            .unwrap_or(4)
            .clamp(1, 1000);
        let min_n = self.suggest_min_n();
        let edges = self.suggest_edges();
        // Снятые чекбоксы: поле не перебирается; с заполненными границами —
        // участвует фиксированным фильтром.
        let locked: Vec<Option<(Option<f64>, Option<f64>)>> = (0..FIELDS.len())
            .map(|fi| {
                if self.tuner.enabled[fi] {
                    None
                } else {
                    let (from, to) = &self.tuner.bounds[0][fi];
                    Some((parse_num(from), parse_num(to)))
                }
            })
            .collect();
        self.op_started();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let sugg = executor
                .spawn(async move {
                    moon_core::db::tuner_smart::smart_suggest(&q, rounds, min_n, &locked, edges)
                })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.op_finished(cx);
                    if this.tuner.sugg_seq != req {
                        return;
                    }
                    this.tuner.sugg_busy = false;
                    if let Some(res) = sugg {
                        log::info!(
                            "аналитика: умный подбор — профит {:+.2}, сделок {}, попыток {}",
                            res.profit,
                            res.n,
                            res.rounds
                        );
                        let by_field: HashMap<&str, _> =
                            res.fields.into_iter().map(|f| (f.field, f)).collect();
                        for fi in 0..FIELDS.len() {
                            // Не перебиравшиеся поля не трогаем (фикс/выкл).
                            if !this.tuner.enabled[fi] {
                                continue;
                            }
                            let (from, to) = by_field
                                .get(FIELDS[fi].col)
                                .map(|f| (fmt_bound(f.from), fmt_bound(f.to)))
                                .unwrap_or_default();
                            this.tuner.bounds[0][fi] = (from, to);
                            this.tuner.inputs.remove(&format!("tv0f{fi}a"));
                            this.tuner.inputs.remove(&format!("tv0f{fi}b"));
                        }
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
        let field = FIELDS[fi].col.to_string();
        let q = self.tuner_query();
        let min_n = self.suggest_min_n();
        let edges = self.suggest_edges();
        self.op_started();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let sugg = executor
                .spawn(async move { moon_core::db::tuner::suggest_field(&q, &field, min_n, edges) })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.op_finished(cx);
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

    /// Копировать границы v1 → v2: строку `fi` или (None) всю колонку.
    pub(super) fn copy_v1_to_v2(&mut self, fi: Option<usize>, cx: &mut Context<Self>) {
        let range: Vec<usize> = match fi {
            Some(fi) => vec![fi],
            None => (0..FIELDS.len()).collect(),
        };
        for fi in range {
            let v = self.tuner.bounds[0][fi].clone();
            if self.tuner.bounds[1][fi] == v {
                continue;
            }
            self.tuner.bounds[1][fi] = v;
            self.tuner.inputs.remove(&format!("tv1f{fi}a"));
            self.tuner.inputs.remove(&format!("tv1f{fi}b"));
        }
        self.reload_tuner(cx);
        cx.notify();
    }

    /// Очистить ВСЮ колонку варианта (крестик в шапке сетки).
    pub(super) fn clear_variant(&mut self, vi: usize, cx: &mut Context<Self>) {
        for fi in 0..FIELDS.len() {
            self.tuner.bounds[vi][fi] = (String::new(), String::new());
            self.tuner.inputs.remove(&format!("tv{vi}f{fi}a"));
            self.tuner.inputs.remove(&format!("tv{vi}f{fi}b"));
        }
        self.reload_tuner(cx);
        cx.notify();
    }

    /// Число квантильных краёв перебора (поле со списком 4/8/…/128).
    fn suggest_edges(&self) -> usize {
        self.tuner.edges.clamp(4, 128)
    }

    /// Минимум сделок для автоподбора: из конфиг-строки; пусто = авто (1/5
    /// фактических сделок скоупа).
    fn suggest_min_n(&self) -> i64 {
        if let Ok(v) = self.tuner.min_trades.trim().parse::<i64>() {
            return v.max(1);
        }
        self.tuner
            .stats
            .as_ref()
            .and_then(|s| s.first().map(|f| f.n / 5))
            .unwrap_or(0)
            .max(1)
    }

    /// «В стратегию»: пороги v1 → параметры выбранной стратегии на всех её
    /// ядрах (sync шлёт полный набор — правки одной командой на ядро). Если
    /// классы затронутых полей игнорировались (IgnoreFilters/IgnoreDelta/
    /// IgnoreVolume) — соответствующие флаги выключаются, иначе пороги не
    /// имели бы эффекта. Пишутся поля с маппингом на параметры; поля-слоты
    /// (d1h/d15m/d5m/d1m/Pump1H/Dump1H) — через Delta2/Delta3: сначала
    /// `DeltaN_Type`, затем `DeltaN_Min/Max`; слотов два — лишние в лог.
    pub(super) fn open_save_dialog(&mut self, cx: &mut Context<Self>) {
        let Some((key, name)) = self.sel_strategy.clone() else {
            return;
        };
        let Ok(sid) = key.parse::<i64>() else { return };

        let mut changes: Vec<(String, String)> = Vec::new();
        let mut warns: Vec<String> = Vec::new();
        let (mut delta_touched, mut volume_touched, mut bvsv_touched, mut ping_touched) =
            (false, false, false, false);
        let mut base_touched = false;
        // Поля-слоты с порогами v1 — кандидаты в Delta2/Delta3.
        let mut slot_wanted: Vec<(&'static str, Option<f64>, Option<f64>)> = Vec::new();
        for (fi, spec) in FIELDS.iter().enumerate() {
            let (from, to) = &self.tuner.bounds[0][fi];
            let class = &spec.class;
            if *class == FieldClass::DeltaSlot {
                let (lo, hi) = (parse_num(from), parse_num(to));
                if lo.is_some() || hi.is_some() {
                    slot_wanted.push((spec.col, lo, hi));
                }
                continue;
            }
            for (txt, param) in [(from, spec.p_min), (to, spec.p_max)] {
                let Some(param) = param else { continue };
                let Some(v) = parse_num(txt) else { continue };
                changes.push((param.to_string(), fmt_plain(v)));
                match class {
                    FieldClass::Delta => delta_touched = true,
                    FieldClass::Volume => volume_touched = true,
                    FieldClass::BvSv => bvsv_touched = true,
                    FieldClass::Ping => ping_touched = true,
                    FieldClass::Base => base_touched = true,
                    FieldClass::Filter | FieldClass::DeltaSlot => {}
                }
            }
        }
        // Раздача слотов: своё прежнее место (если тип уже стоит) — иначе
        // свободный по порядку. Слоты, занятые типом без колонки отчёта
        // (2h/30m/Pump5m с порогами — «чужой» живой фильтр), перезаписываются
        // ПОСЛЕДНИМИ и с warn. Больше двух не влезает — остальные в лог.
        if !slot_wanted.is_empty() {
            let cur2 = self.tuner.strat.slots.iter().find(|(n, ..)| *n == 2).map(|(_, f, ..)| *f);
            let cur3 = self.tuner.strat.slots.iter().find(|(n, ..)| *n == 3).map(|(_, f, ..)| *f);
            let foreign = self.tuner.strat.foreign_slots.clone();
            let mut used = [false, false]; // [Delta2, Delta3]
            for (n, _) in &foreign {
                used[(*n - 2) as usize] = true;
            }
            let mut assigned: Vec<(u8, &'static str, Option<f64>, Option<f64>)> = Vec::new();
            let mut dropped: Vec<&'static str> = Vec::new();
            // Сначала — поля, уже сидящие в своих слотах.
            for (col, lo, hi) in &slot_wanted {
                if cur2 == Some(*col) && !used[0] {
                    used[0] = true;
                    assigned.push((2, col, *lo, *hi));
                } else if cur3 == Some(*col) && !used[1] {
                    used[1] = true;
                    assigned.push((3, col, *lo, *hi));
                }
            }
            // Затем — свободные слоты; в конце — перезапись «чужих».
            for overwrite_foreign in [false, true] {
                for (col, lo, hi) in &slot_wanted {
                    if assigned.iter().any(|(_, f, ..)| f == col) {
                        continue;
                    }
                    let Some(i) = (0..2).find(|i| {
                        !used[*i]
                            || (overwrite_foreign
                                && foreign.iter().any(|(n, _)| *n == *i as u8 + 2)
                                && !assigned.iter().any(|(n, ..)| *n == *i as u8 + 2))
                    }) else {
                        if overwrite_foreign {
                            dropped.push(col);
                        }
                        continue;
                    };
                    used[i] = true;
                    if let Some((_, ty)) = foreign.iter().find(|(n, _)| *n == i as u8 + 2) {
                        let slot = format!("Delta{}", i + 2);
                        log::warn!(
                            "аналитика: «Сохранить» — {slot} был занят типом «{ty}» (в отчёте \
                             колонки нет), перезаписываем на «{col}»"
                        );
                        warns.push(
                            t!("analytics.tuner.warn_slot_replace", slot = slot, old = ty)
                                .to_string(),
                        );
                    }
                    assigned.push((i as u8 + 2, col, *lo, *hi));
                }
            }
            if !dropped.is_empty() {
                log::warn!(
                    "аналитика: «В стратегию» — слотов Delta2/Delta3 только два, не вошли: {}",
                    dropped.join(", ")
                );
                warns.push(
                    t!("analytics.tuner.warn_slot_drop", fields = dropped.join(", ")).to_string(),
                );
            }
            for (n, col, lo, hi) in assigned {
                let Some(ty) = slot_type_for(col) else { continue };
                // Порядок важен: сначала тип слота, затем его пороги.
                changes.push((format!("Delta{n}_Type"), ty.to_string()));
                if let Some(v) = lo {
                    changes.push((format!("Delta{n}_Min"), fmt_plain(v)));
                }
                if let Some(v) = hi {
                    changes.push((format!("Delta{n}_Max"), fmt_plain(v)));
                }
                delta_touched = true;
            }
        }
        // Флаги игноров: автовключение классов, чьи пороги пишем, ПЛЮС явные
        // клики «ignore» на подзаголовках (стейдж приоритетнее автологики).
        let f = self.tuner.strat.clone();
        let mut flags: Vec<(&'static str, bool)> = Vec::new(); // (флаг, игнорировать)
        if f.ignore_filters {
            flags.push(("IgnoreFilters", false));
        }
        if delta_touched && f.ignore_delta {
            flags.push(("IgnoreDelta", false));
        }
        // BV/SV — подгруппа Filters/Volume: его пороги требуют снять и
        // IgnoreVolume, и включить сам фильтр.
        if (volume_touched || bvsv_touched) && f.ignore_volume {
            flags.push(("IgnoreVolume", false));
        }
        if bvsv_touched && !f.use_bvsv {
            flags.push(("UseBV_SV_Filter", false));
        }
        if ping_touched && f.ignore_ping {
            flags.push(("IgnorePing", false));
        }
        if base_touched && f.ignore_base {
            flags.push(("IgnoreBase", false));
        }
        for (flag, want) in self.tuner.staged_ignore.clone() {
            flags.retain(|(fl, _)| *fl != flag);
            let cur = match flag {
                "IgnoreFilters" => f.ignore_filters,
                "IgnorePing" => f.ignore_ping,
                "IgnoreDelta" => f.ignore_delta,
                "IgnoreVolume" => f.ignore_volume,
                "IgnoreBase" => f.ignore_base,
                "UseBV_SV_Filter" => !f.use_bvsv,
                _ => continue,
            };
            if want != cur {
                flags.push((flag, want));
            }
        }
        for (flag, ignore) in flags {
            // UseBV_SV_Filter — включатель (инверсная семантика игнора).
            let value = if flag == "UseBV_SV_Filter" {
                if ignore { "NO" } else { "YES" }
            } else if ignore {
                "YES"
            } else {
                "NO"
            };
            changes.push((flag.to_string(), value.to_string()));
        }

        if changes.is_empty() {
            log::info!("аналитика: «Сохранить» — нет ни порогов с маппингом, ни изменённых игноров");
            return;
        }
        // Штамп анализатора в Comment: «дд.мм.гггг чч:мм:сс (Save from
        // analyzer)» UTC. Описание пользователя сохраняем — заменяется только
        // предыдущий штамп (сегменты через «; »).
        const MARK: &str = "(Save from analyzer)";
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let ts = moon_core::db::fmt_unix_secs(now); // YYYY-MM-DD HH:MM:SS
        let (date, time) = ts.split_once(' ').unwrap_or((ts.as_str(), ""));
        let mut dmy = date.splitn(3, '-');
        let (y, m, d) = (
            dmy.next().unwrap_or(""),
            dmy.next().unwrap_or(""),
            dmy.next().unwrap_or(""),
        );
        let stamp = format!("{d}.{m}.{y} {time} {MARK}");
        let base: Vec<&str> = f
            .comment
            .split("; ")
            .map(str::trim)
            .filter(|s| !s.is_empty() && !s.contains(MARK))
            .collect();
        let comment = if base.is_empty() {
            stamp
        } else {
            format!("{}; {stamp}", base.join("; "))
        };
        changes.push(("Comment".to_string(), comment));
        self.tuner.save_dialog = Some(Arc::new(SaveDialog { sid, name, changes, warns }));
        cx.notify();
    }

    /// «Да» в окне подтверждения: отправить подготовленные правки.
    pub(super) fn confirm_save_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(dlg) = self.tuner.save_dialog.take() else {
            return;
        };
        self.tuner.staged_ignore.clear();
        self.send_strategy_changes(dlg.sid, dlg.name.clone(), dlg.changes.clone(), cx);
        cx.notify();
    }

    /// Общий хвост записи в стратегию: ядра из strategies.sqlite (не на
    /// UI-потоке), затем правки обычным путём окна Стратегий (edit_strategies,
    /// одна команда на ядро), после — перечитка чипов.
    fn send_strategy_changes(
        &mut self,
        sid: i64,
        name: String,
        changes: Vec<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        let n_fields = changes.len();
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
                                    "аналитика: правки → «{name}» ядро {core} не ушли: {e:#}"
                                ),
                            }
                        }
                    }
                    log::info!(
                        "аналитика: правки → стратегия «{name}»: {n_fields} полей, ядер {sent}/{}",
                        cores.len()
                    );
                    // Эхо снапшота обновит strategies.sqlite — перечитаем чипы.
                    this.reload_tuner(cx);
                    cx.notify();
                });
            });
            // Эхо ядра приходит с лагом — перечитываем ещё дважды, чтобы
            // чипы/игноры показали ФАКТИЧЕСКИ применённое состояние.
            for delay_ms in [1500u64, 3500] {
                executor
                    .timer(std::time::Duration::from_millis(delay_ms))
                    .await;
                let _ = cx.update(|cx| {
                    let _ = this.update(cx, |this, cx| this.reload_tuner(cx));
                });
            }
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
