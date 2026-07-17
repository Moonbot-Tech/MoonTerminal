//! Состояние тюнера и общие хелперы (вынесено из tuner.rs — лимит размера).

use std::collections::HashMap;
use std::sync::Arc;

use gpui::*;
use moon_ui::{MoonInputState, MoonPalette, h_flex, v_flex};

use super::AnalyticsView;
use crate::design;
use crate::design::moon;
use moon_core::db::tuner::{Bound, FIELDS, FieldClass, HistBucket, StratFilters, VarStats, Variant};

/// Вариантов помимо «Факта» (V3 держит 8 — начинаем с двух).
pub(super) const N_VAR: usize = 2;

/// Подготовленное сохранение в стратегию: адресат + список правок.
pub(super) struct SaveDialog {
    pub(super) sid: i64,
    pub(super) name: String,
    pub(super) changes: Vec<(String, String)>,
    /// Предупреждения (перезапись чужого слот-типа, не вошедшие поля) —
    /// показываются в окне подтверждения, не только в логе.
    pub(super) warns: Vec<String>,
}

/// Состояние тюнера внутри `AnalyticsView`.
pub(super) struct TunerState {
    /// Границы вариантов текстом: `[вариант][индекс поля] = (от, до)`.
    pub(super) bounds: Vec<Vec<(String, String)>>,
    /// Кэш инпутов границ (ленивое создание в render).
    pub(super) inputs: HashMap<String, Entity<MoonInputState>>,
    /// Поле гистограммы (индекс в `FIELDS`).
    pub(super) sel_field: usize,
    pub(super) stats: Option<Arc<Vec<VarStats>>>,
    pub(super) hist: Option<Arc<Vec<HistBucket>>>,
    /// Фильтровая карточка выбранной стратегии (Ignore-флаги + пороги).
    pub(super) strat: Arc<StratFilters>,
    /// Автоподбор (кнопки «Подобрать») выполняется в фоне.
    pub(super) sugg_busy: bool,
    /// Открытое окно подтверждения сохранения (список правок).
    pub(super) save_dialog: Option<Arc<SaveDialog>>,
    /// Данные устарели (сменился скоуп/фильтры) — показываем старое без
    /// «Загрузка…» (не мерцаем), но при входе в режим пересчитываем.
    pub(super) dirty: bool,
    /// Стейдж кликабельных «ignore» подзаголовков: флаг → желаемое состояние
    /// игнора (семантика «игнорировать», для UseBV_SV_Filter инверсна).
    pub(super) staged_ignore: HashMap<&'static str, bool>,
    /// Параметры умного подбора: попыток, минимум сделок (пусто = авто 1/5)
    /// и число квантильных краёв перебора (список 4..128, деф. 64).
    pub(super) iters: String,
    pub(super) min_trades: String,
    pub(super) edges: usize,
    /// Участие поля в автопереборе (чекбоксы); выключенное с границами —
    /// фиксированный фильтр.
    pub(super) enabled: Vec<bool>,
    /// «Округление результата»: границы из подбора округляются до 3 значащих
    /// цифр НАРУЖУ (from вниз, to вверх) — диапазон не теряет найденных сделок.
    pub(super) round_results: bool,
    pub(super) seq: u64,
    pub(super) hist_seq: u64,
    pub(super) sugg_seq: u64,
}

impl TunerState {
    /// Свежее состояние: границы ПУСТЫЕ, чекбоксы участия включены у всех
    /// полей С МАППИНГОМ на параметры стратегии (немаппленные — da1m/d5s —
    /// автоподбор по умолчанию игнорирует: подобранное некуда записать).
    /// Намеренно не персистятся — каждое открытие окна с чистого листа.
    pub(super) fn load() -> Self {
        let bounds = vec![vec![(String::new(), String::new()); FIELDS.len()]; N_VAR];
        let enabled = FIELDS.iter().map(|s| s.mapped()).collect();
        Self {
            bounds,
            inputs: HashMap::new(),
            sel_field: 0,
            stats: None,
            hist: None,
            strat: Arc::new(StratFilters::default()),
            sugg_busy: false,
            save_dialog: None,
            dirty: false,
            staged_ignore: HashMap::new(),
            iters: "4".to_string(),
            min_trades: String::new(),
            edges: 64,
            round_results: true,
            enabled,
            seq: 0,
            hist_seq: 0,
            sugg_seq: 0,
        }
    }

    /// Пометить расчёты устаревшими (смена скоупа/фильтров/периода): данные
    /// НЕ обнуляем — старое остаётся на экране до прихода нового (никаких
    /// вспышек «Загрузка…»), пересчёт при входе в режим или явным reload_*.
    pub(super) fn invalidate(&mut self) {
        self.dirty = true;
        self.save_dialog = None;
        self.staged_ignore.clear();
    }

    /// Нужен ли пересчёт при входе в режим «Фильтры».
    pub(super) fn needs_reload(&self) -> bool {
        self.stats.is_none() || self.dirty
    }

    /// Варианты для запроса: [пустой «Факт», v1..vN].
    pub(super) fn variants(&self) -> Vec<Variant> {
        let mut out = vec![Variant::default()];
        for v in &self.bounds {
            let bounds = v
                .iter()
                .enumerate()
                .filter_map(|(fi, (from, to))| {
                    let from = parse_num(from);
                    let to = parse_num(to);
                    (from.is_some() || to.is_some()).then(|| Bound {
                        field: FIELDS[fi].col.to_string(),
                        from,
                        to,
                    })
                })
                .collect();
            out.push(Variant { bounds });
        }
        out
    }
}

/// Число из поля ввода: запятая как точка, суффиксы k/M/B/T (и кириллические
/// к/м), пусто/мусор = None.
pub(super) fn parse_num(s: &str) -> Option<f64> {
    let s = s.trim().replace(',', ".");
    if s.is_empty() {
        return None;
    }
    let (mut t, mut mult) = (s.as_str(), 1.0f64);
    if let Some(c) = s.chars().last() {
        let m = match c {
            'k' | 'K' | 'к' | 'К' => 1e3,
            'm' | 'M' | 'м' | 'М' => 1e6,
            'b' | 'B' => 1e9,
            't' | 'T' => 1e12,
            _ => 1.0,
        };
        if m != 1.0 {
            mult = m;
            t = &s[..s.len() - c.len_utf8()];
        }
    }
    t.trim()
        .parse::<f64>()
        .ok()
        .map(|v| v * mult)
        .filter(|v| v.is_finite())
}


/// Формат числа для границ/чипов: крупные — с суффиксом k/M/B/T (обратно
/// понимается `parse_num`), прочие — до 4 знаков без хвостовых нулей.
pub(super) fn fmt_bound(v: f64) -> String {
    let a = v.abs();
    let (div, suf) = if a >= 1e12 {
        (1e12, "T")
    } else if a >= 1e9 {
        (1e9, "B")
    } else if a >= 1e6 {
        (1e6, "M")
    } else if a >= 1e3 {
        (1e3, "k")
    } else {
        (1.0, "")
    };
    let x = v / div;
    let mut s = if suf.is_empty() {
        format!("{x:.4}")
    } else {
        format!("{x:.2}")
    };
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    s.push_str(suf);
    s
}

/// Карточка с заголовком и подзаголовком (общий вид карточек Аналитики).
pub(super) fn card(
    title: String,
    sub: String,
    body: AnyElement,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    let mut head = h_flex()
        .w_full()
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 8.0))
        .items_center()
        .gap(design::ui_px(cx, 8.0))
        .child(
            div()
                .text_size(design::t_title(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        );
    if !sub.is_empty() {
        head = head.child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .child(sub),
        );
    }
    v_flex()
        .w_full()
        // В скролл-колонке карточки не должны ужиматься под высоту вьюпорта.
        .flex_none()
        .rounded(design::ui_px(cx, 8.0))
        .bg(moon(p.panel))
        .border_1()
        .border_color(moon(p.border))
        .overflow_hidden()
        .child(head)
        .child(body)
        .into_any_element()
}

/// Есть ли стейджи «ignore», отличающиеся от текущих флагов стратегии.
pub(super) fn staged_dirty(
    f: &StratFilters,
    staged: &HashMap<&'static str, bool>,
) -> bool {
    staged.iter().any(|(flag, want)| {
        let cur = match *flag {
            "IgnoreFilters" => f.ignore_filters,
            "IgnorePing" => f.ignore_ping,
            "IgnoreDelta" => f.ignore_delta,
            "IgnoreVolume" => f.ignore_volume,
            "IgnoreBase" => f.ignore_base,
            "UseBV_SV_Filter" => !f.use_bvsv,
            _ => return false,
        };
        *want != cur
    })
}

/// Флаг игнора группы + его ТЕКУЩЕЕ состояние у стратегии (семантика
/// «игнорируется»; UseBV_SV_Filter инверсный — включатель).
pub(super) fn flag_of(class: FieldClass, f: &StratFilters) -> (&'static str, bool) {
    match class {
        FieldClass::Filter => ("IgnoreFilters", f.ignore_filters),
        FieldClass::Ping => ("IgnorePing", f.ignore_ping),
        FieldClass::Base => ("IgnoreBase", f.ignore_base),
        FieldClass::BvSv => ("UseBV_SV_Filter", !f.use_bvsv),
        FieldClass::Delta | FieldClass::DeltaSlot => ("IgnoreDelta", f.ignore_delta),
        FieldClass::Volume => ("IgnoreVolume", f.ignore_volume),
    }
}
