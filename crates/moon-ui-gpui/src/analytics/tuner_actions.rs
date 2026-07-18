//! Tuner actions for threshold suggestions and applying v1 to the selected
//! strategy, including the required ignore-class changes.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::*;

use moon_ui::{MoonInputEvent, MoonInputState};
use rust_i18n::t;

use super::AnalyticsView;
use super::tuner::{fmt_bound, parse_num, staged_dirty};
use super::tuner_state::SaveDialog;
use moon_core::db::tuner::{FIELDS, FieldClass, slot_type_for};

impl AnalyticsView {
    /// Suggest all v1 ranges jointly with coordinate descent.
    ///
    /// `NotReady` and failed reads are published through the shared KPI load
    /// state; a valid result updates only fields enabled for search.
    pub(super) fn suggest_into_v1(&mut self, cx: &mut Context<Self>) {
        self.tuner.sugg_seq = self.tuner.sugg_seq.wrapping_add(1);
        let req = self.tuner.sugg_seq;
        // Suggestion failures share `tuner.stats` with KPI reads, so both the
        // suggestion and KPI generations must still match before publishing.
        let stats_req = self.tuner.seq;
        self.tuner.sugg_busy = true;
        let q = self.tuner_query();
        let rounds = self
            .tuner
            .iters
            .trim()
            .parse::<usize>()
            .unwrap_or(20)
            .clamp(1, 1000);
        let min_n = self.suggest_min_n();
        let edges = self.suggest_edges();
        let round = self.tuner.round_results;
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
                    moon_core::db::tuner_smart::smart_suggest(
                        &q, rounds, min_n, &locked, edges, round,
                    )
                })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.op_finished(cx);
                    if this.tuner.sugg_seq != req {
                        return;
                    }
                    this.tuner.sugg_busy = false;
                    // A non-successful read must not look like "found nothing": the
                    // button would just stop spinning and leave no trace.
                    let sugg = match sugg {
                        Ok(v) => v,
                        Err(e) => {
                            log::warn!("аналитика: умный подбор не выполнен — {e}");
                            if this.tuner.seq == stats_req {
                                this.tuner.stats.apply(Err(e));
                            }
                            cx.notify();
                            return;
                        }
                    };
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

    /// Suggest the best v1 range for the selected field.
    ///
    /// `NotReady` and failed reads are published through the shared KPI load
    /// state; a valid `None` means no threshold improves on the baseline.
    pub(super) fn suggest_one_into_v1(&mut self, cx: &mut Context<Self>) {
        self.tuner.sugg_seq = self.tuner.sugg_seq.wrapping_add(1);
        let req = self.tuner.sugg_seq;
        // The shared KPI error channel requires the current KPI generation too.
        let stats_req = self.tuner.seq;
        self.tuner.sugg_busy = true;
        let fi = self.tuner.sel_field;
        let field = FIELDS[fi].col.to_string();
        let q = self.tuner_query();
        let min_n = self.suggest_min_n();
        let edges = self.suggest_edges();
        let round = self.tuner.round_results;
        self.op_started();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let sugg = executor
                .spawn(async move {
                    moon_core::db::tuner::suggest_field(&q, &field, min_n, edges, round)
                })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.op_finished(cx);
                    if this.tuner.sugg_seq != req {
                        return;
                    }
                    this.tuner.sugg_busy = false;
                    match sugg {
                        Ok(Some(s)) => {
                            let from = s.from.map(fmt_bound).unwrap_or_default();
                            let to = s.to.map(fmt_bound).unwrap_or_default();
                            this.apply_bounds(0, fi, from, to, cx);
                        }
                        // A genuine "no threshold beats the baseline".
                        Ok(None) => {}
                        Err(e) => {
                            log::warn!("аналитика: подбор порога не выполнен — {e}");
                            if this.tuner.seq == stats_req {
                                this.tuner.stats.apply(Err(e));
                            }
                        }
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

    /// Очистить колонку варианта (крестик в шапке сетки) — только строки с
    /// ВКЛЮЧЁННЫМ чекбоксом: снятые = фиксированные фильтры, их не трогаем.
    pub(super) fn clear_variant(&mut self, vi: usize, cx: &mut Context<Self>) {
        for fi in 0..FIELDS.len() {
            if !self.tuner.enabled[fi] {
                continue;
            }
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

    /// Есть ли что записывать: стейджи «ignore» ИЛИ пороги v1, отличающиеся
    /// от текущих параметров стратегии (кнопка «Сохранить» горит янтарным).
    pub(super) fn save_dirty(&self) -> bool {
        if staged_dirty(&self.tuner.strat, &self.tuner.staged_ignore) {
            return true;
        }
        let near = |a: Option<f64>, b: Option<f64>| match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => (a - b).abs() <= a.abs().max(b.abs()).max(1.0) * 1e-9,
            _ => false,
        };
        let f = &self.tuner.strat;
        for (fi, spec) in FIELDS.iter().enumerate() {
            let (from, to) = &self.tuner.bounds[0][fi];
            let (lo, hi) = (parse_num(from), parse_num(to));
            if lo.is_none() && hi.is_none() {
                continue;
            }
            // Немаппленные поля в стратегию не пишутся — не считаются.
            if !spec.mapped() {
                continue;
            }
            let cur = if spec.class == FieldClass::DeltaSlot {
                f.slot_of(spec.col).map(|(_, l, h)| (l, h))
            } else {
                f.bounds.get(spec.col).copied()
            };
            match cur {
                Some((cl, ch)) if near(lo, cl) && near(hi, ch) => {}
                _ => return true,
            }
        }
        false
    }

    /// Минимум сделок для автоподбора: из конфиг-строки; пусто = авто (1/5
    /// фактических сделок скоупа).
    fn suggest_min_n(&self) -> i64 {
        if let Ok(v) = self.tuner.min_trades.trim().parse::<i64>() {
            return v.max(1);
        }
        self.tuner
            .stats
            .data()
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
        let Some((sid, name, mut changes, warns)) = self.build_strategy_changes() else {
            return;
        };
        if changes.is_empty() {
            log::info!(
                "аналитика: «Сохранить» — нет ни порогов с маппингом, ни изменённых игноров"
            );
            return;
        }
        changes.push(self.analyzer_comment());
        self.open_change_dialog(sid, name, changes, warns, false, cx);
    }

    /// «Сделать копию»: та же сборка правок, но адресат — НОВАЯ стратегия
    /// (копия текущей с применёнными порогами) на всех ядрах исходной. Имя
    /// авто-уникальное, правится в окне подтверждения. Пустой список правок
    /// допустим — это просто копия.
    pub(super) fn open_copy_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((sid, name, mut changes, warns)) = self.build_strategy_changes() else {
            return;
        };
        changes.push(self.analyzer_comment());
        // Авто-имя: уникальное среди имён стратегий ВСЕХ ядер (имена в
        // Moonbot глобальны в пределах ядра; берём объединение — надёжно).
        let mut taken = std::collections::HashSet::new();
        {
            let b = self.backend.read(cx);
            for (_, cd) in b.session.store().cores() {
                for r in &cd.strategies {
                    taken.insert(r.name.clone());
                }
            }
        }
        let proposed = crate::strategies::tree_ops::unique_name(&taken, &name);
        self.tuner.copy_name = proposed.clone();
        // Свежий инпут имени на каждое открытие (дефолт рисуется с начала).
        self.tuner.inputs.remove("copy-name");
        let state = cx.new(|cx| MoonInputState::new(window, cx).default_value(proposed));
        cx.subscribe(&state, |this, st, ev: &MoonInputEvent, cx| {
            if matches!(
                ev,
                MoonInputEvent::Change | MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }
            ) {
                this.tuner.copy_name = st.read(cx).value().to_string();
            }
        })
        .detach();
        self.tuner.inputs.insert("copy-name".to_string(), state);
        self.open_change_dialog(sid, name, changes, warns, true, cx);
    }

    /// Собрать правки из v1 + стейджей «ignore» (без штампа Comment): пороги
    /// с маппингом, слоты Delta2/3 (+предупреждения), Ignore-флаги.
    fn build_strategy_changes(&self) -> Option<(i64, String, Vec<(String, String)>, Vec<String>)> {
        let (key, name) = self.sel_strategy.clone()?;
        let sid = key.parse::<i64>().ok()?;

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
            let cur2 = self
                .tuner
                .strat
                .slots
                .iter()
                .find(|(n, ..)| *n == 2)
                .map(|(_, f, ..)| *f);
            let cur3 = self
                .tuner
                .strat
                .slots
                .iter()
                .find(|(n, ..)| *n == 3)
                .map(|(_, f, ..)| *f);
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
                    t!(
                        "analytics.tuner.warn_slot_drop",
                        fields = dropped.join(", ")
                    )
                    .to_string(),
                );
            }
            for (n, col, lo, hi) in assigned {
                let Some(ty) = slot_type_for(col) else {
                    continue;
                };
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
        Some((sid, name, changes, warns))
    }

    /// Штамп анализатора для Comment: «дд.мм.гггг чч:мм:сс (Save from
    /// analyzer)» UTC. Описание пользователя сохраняется — заменяется только
    /// предыдущий штамп (сегменты через «; »).
    fn analyzer_comment(&self) -> (String, String) {
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
        let base: Vec<&str> = self
            .tuner
            .strat
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
        ("Comment".to_string(), comment)
    }

    /// Открыть окно подтверждения (запись/копия): текущие значения параметров
    /// («сейчас → будет») тянутся фоном из strategies.sqlite.
    fn open_change_dialog(
        &mut self,
        sid: i64,
        name: String,
        changes: Vec<(String, String)>,
        warns: Vec<String>,
        copy: bool,
        cx: &mut Context<Self>,
    ) {
        let keys: Vec<String> = changes.iter().map(|(k, _)| k.clone()).collect();
        self.op_started();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let olds_map = executor
                .spawn(async move { moon_core::db::tuner::strategy_current_values(sid, &keys) })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.op_finished(cx);
                    let olds = changes
                        .iter()
                        .map(|(k, _)| olds_map.get(k).cloned())
                        .collect();
                    this.tuner.save_dialog = Some(Arc::new(SaveDialog {
                        sid,
                        name,
                        changes,
                        olds,
                        warns,
                        copy,
                    }));
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// «Да» в окне подтверждения: запись в стратегию или создание копии.
    pub(super) fn confirm_save_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(dlg) = self.tuner.save_dialog.take() else {
            return;
        };
        if dlg.copy {
            self.create_strategy_copy(&dlg, cx);
        } else {
            self.tuner.staged_ignore.clear();
            self.send_strategy_changes(dlg.sid, dlg.name.clone(), dlg.changes.clone(), cx);
        }
        cx.notify();
    }

    /// Создать копию стратегии с применёнными правками на всех ядрах, где
    /// живёт исходная (по живому store). Имя — из инпута окна (пустое =
    /// авто); на каждом ядре дополнительно уникализируется. Копия приходит
    /// ВЫКЛЮЧЕННОЙ (правило create_strategies) — безопасно.
    fn create_strategy_copy(
        &mut self,
        dlg: &super::tuner_state::SaveDialog,
        cx: &mut Context<Self>,
    ) {
        use crate::strategies::tree_ops::{STRATEGY_NAME_FIELD, set_field, unique_name};
        let sid = dlg.sid as u64;
        let desired = {
            let t = self.tuner.copy_name.trim();
            if t.is_empty() {
                dlg.name.clone()
            } else {
                t.to_string()
            }
        };
        let b = self.backend.read(cx);
        let mut plans: Vec<(
            moon_core::session::CoreId,
            moon_core::feed::NewStrategySpec,
            String,
        )> = Vec::new();
        for (cid, cd) in b.session.store().cores() {
            let Some(row) = cd.strategies.iter().find(|r| r.id == sid) else {
                continue;
            };
            let taken: std::collections::HashSet<String> =
                cd.strategies.iter().map(|r| r.name.clone()).collect();
            let name = unique_name(&taken, &desired);
            let mut fields = row.fields.clone();
            for (k, v) in &dlg.changes {
                set_field(&mut fields, k, v);
            }
            set_field(&mut fields, STRATEGY_NAME_FIELD, &name);
            plans.push((
                cid,
                moon_core::feed::NewStrategySpec {
                    kind_ordinal: row.kind_ordinal,
                    folder_path: row.folder_path.clone(),
                    fields,
                },
                name,
            ));
        }
        if plans.is_empty() {
            log::warn!(
                "аналитика: «Сделать копию» — стратегия {} не найдена в живом store",
                dlg.sid
            );
            return;
        }
        let total = plans.len();
        let mut sent = 0usize;
        for (cid, spec, name) in plans {
            match b.session.create_strategies(cid, vec![spec]) {
                Ok(()) => {
                    sent += 1;
                    log::info!("аналитика: копия «{name}» создана на ядре {cid}");
                }
                Err(e) => log::warn!("аналитика: копия на ядро {cid} не ушла: {e:#}"),
            }
        }
        log::info!("аналитика: «Сделать копию» — ядер {sent}/{total}");
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
