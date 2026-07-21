//! Свечи чарта: агрегация из трейдов, ресемпл серверной истории и слитная серия
//! per-pane. Источники: CoinCard deep history (честные OHLC, по требованию) →
//! retained `candles_5m` moonproto (авто-снимок с ядра; несёт только high/low) →
//! трейд-ринг (живой край и суб-5м ТФ). Чистые функции + серия с ревизией: рендер
//! перезаливает GPU-буфер только по смене ревизии.
//!
//! Правило шва: серверные свечи авторитетны для «прошлого», локальные (из трейдов) —
//! от первого ПОЛНОГО бакета, покрытого трейд-рингом (первый бакет ринга обычно
//! частичный — его o/h/l врут, если есть серверная свеча этого бакета, берём её).

use serde::{Deserialize, Serialize};

use crate::feed::Tick;

/// Допустимые таймфреймы свечей, минуты. 30с (код 0) УДАЛЁН из набора по просьбе
/// пользователя (2026-07-12): суб-минутный ТФ жил только из трейдов, без deep-базы.
/// База: CoinCard-история родного ТФ (1/5/30/60/240/1440), фолбэк — 5м-снимок ядра.
pub const CANDLE_TF_CHOICES_MIN: [u32; 6] = [1, 5, 30, 60, 240, 1440];

/// Режим отрисовки свечей (см. `CandleViewCfg::mode`).
pub const CANDLE_MODE_FILLED: u8 = 0;
pub const CANDLE_MODE_OUTLINE: u8 = 1;
pub const CANDLE_MODE_OUTLINE_IN_ZONE: u8 = 2;
/// Свечи выключены вовсе: чистый тик-чарт (трейды на всё окно, слой свечей пуст).
pub const CANDLE_MODE_OFF: u8 = 3;

/// Глобальные настройки отображения свечей/трейдов на чарте (кнопка «свеча» в полоске
/// вкладок). Persist — `layout.toml` (`WindowLayout::candle_view`), один набор на всё
/// приложение.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CandleViewCfg {
    /// Таймфрейм свечей, минуты (одно из [`CANDLE_TF_CHOICES_MIN`]).
    pub tf_min: u32,
    /// Режим: 0 = заполненные, 1 = контуры, 2 = контуры в зоне трейдов.
    pub mode: u8,
    /// Зона трейдов: сколько ПОСЛЕДНИХ свечей перерисовываем трейдами (кресты рисуются
    /// только внутри этих бакетов). 0 = трейды не рисуем вовсе (только свечи).
    pub trade_candles: u16,
    /// Сколько ПОСЛЕДНИХ свечей НЕ рисовать вовсе (в этих бакетах остаются только
    /// трейды). 0 = показываем все свечи. Обычно ≤ `trade_candles`.
    pub hide_candles: u16,
    /// Жёсткий лимит числа отображаемых трейдов (страховка на всплеск).
    pub trades_limit: u32,
    /// Толщина контура свечи, лог. px.
    pub outline_px: f32,
    /// Рисовать тени (фитили) у свечей в зоне трейдов.
    pub wicks_in_zone: bool,
    /// Красить свечи в зоне трейдов нейтральным цветом (не спорят с окраской крестов).
    pub neutral_in_zone: bool,
    /// Рисовать линии цены last/mark (оранжевая LastPrice + голубая MarkPrice).
    pub price_lines: bool,
}

impl Default for CandleViewCfg {
    fn default() -> Self {
        Self {
            tf_min: 5,
            mode: CANDLE_MODE_OUTLINE_IN_ZONE,
            trade_candles: 3,
            hide_candles: 0,
            trades_limit: 50_000,
            outline_px: 1.0,
            wicks_in_zone: true,
            neutral_in_zone: false,
            price_lines: true,
        }
    }
}

impl CandleViewCfg {
    /// Таймфрейм в миллисекундах (клампится к допустимому набору; легаси 30с (код 0)
    /// сведён к 1м — суб-минутные удалены из настроек).
    pub fn tf_ms(&self) -> i64 {
        let tf = if self.tf_min == 0 {
            1
        } else if CANDLE_TF_CHOICES_MIN.contains(&self.tf_min) {
            self.tf_min
        } else {
            5
        };
        tf as i64 * 60_000
    }
}

/// Одна свеча чарта (время — unix ms открытия бакета).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ChartCandle {
    pub t_open_ms: f64,
    pub open: f32,
    pub high: f32,
    pub low: f32,
    pub close: f32,
    /// Суммарный объём сделок бакета (базовая валюта).
    pub volume: f32,
}

/// Начало бакета ТФ для момента времени (floor по сетке от unix-эпохи).
pub fn bucket_open_ms(time_ms: f64, tf_ms: i64) -> f64 {
    let tf = tf_ms.max(1) as f64;
    (time_ms / tf).floor() * tf
}

/// Родной ТФ CoinCard-истории (мин) для ТФ серии: точный где есть (1/5/30/60/240/1440);
/// суб-минутные ТФ базы не имеют (только трейды), остальное — ресемпл из 5м.
pub fn deep_kind_min_for_tf(tf_min: u32) -> u32 {
    match tf_min {
        1 => 1,
        30 => 30,
        60 => 60,
        240 => 240,
        1440 => 1440,
        _ => 5,
    }
}

/// Ориентация свечей «только диапазон». Замер показал: bulk-снимок 5м с ядра несёт
/// ТОЛЬКО high/low (в полях open==high, close==low) — честных open/close там нет,
/// тела рисовались всегда «падающими» и без теней. Пока не приехала CoinCard-история
/// (настоящие OHLC), ориентируем такие строки по направлению против предыдущей:
/// вверх → open=low/close=high, вниз — как пришла.
pub fn orient_range_rows(rows: &mut [ChartCandle]) {
    let mut prev_mid: Option<f32> = None;
    for c in rows.iter_mut() {
        let mid = (c.high + c.low) * 0.5;
        if c.open == c.high && c.close == c.low {
            if let Some(pm) = prev_mid {
                if mid >= pm {
                    c.open = c.low;
                    c.close = c.high;
                }
            }
        }
        prev_mid = Some(mid);
    }
}

/// Нормализация o/h/l/c серверной свечи: перепутанный wire-порядок (high,low,open,close)
/// в полях (open,close,high,low) детектим по инварианту корректной свечи
/// `h ≥ max(o,c) && l ≤ min(o,c)` и разворачиваем ТОЛЬКО нарушившие его строки —
/// корректные ряды (CoinCard-история, live-запечатанные) проходят как есть.
pub fn normalize_ohlc(o: f32, h: f32, l: f32, c: f32) -> (f32, f32, f32, f32) {
    if h >= o.max(c) && l <= o.min(c) {
        return (o, h, l, c); // корректная свеча
    }
    if o >= h.max(l) && c <= h.min(l) {
        // (o,c,h,l)-поля содержат (high,low,open,close): real o=h, h=o, l=c, c=l.
        return (h, o, c, l);
    }
    // Неопознанный мусор: диапазон растягиваем на все четыре, o/c оставляем.
    let hi = o.max(c).max(h).max(l);
    let lo = o.min(c).min(h).min(l);
    (o, hi, lo, c)
}

/// Свечи из трейдов (любой ТФ). Трейды почти отсортированы по времени; поздние
/// resend-строки (UDP) попадают в СТАРЫЙ бакет — обновляем его h/l/vol (o/c по времени
/// не уточняем: для чарта это визуально неразличимо, а точный порядок ринг не хранит).
/// Пустые бакеты (нет трейдов) не создаются — разрежённая серия, как сама лента.
pub fn aggregate_trades(trades: &[Tick], tf_ms: i64, out: &mut Vec<ChartCandle>) {
    out.clear();
    for t in trades {
        if !(t.price.is_finite() && t.price > 0.0) {
            continue;
        }
        let open_ms = bucket_open_ms(t.time_ms, tf_ms);
        match out.last_mut() {
            Some(last) if last.t_open_ms == open_ms => {
                last.high = last.high.max(t.price);
                last.low = last.low.min(t.price);
                last.close = t.price;
                last.volume += t.qty.max(0.0);
            }
            Some(last) if open_ms > last.t_open_ms => {
                out.push(candle_from_tick(open_ms, t));
            }
            None => out.push(candle_from_tick(open_ms, t)),
            _ => {
                // Поздний resend в старый бакет: ищем его с хвоста (обычно рядом).
                if let Some(c) = out.iter_mut().rev().find(|c| c.t_open_ms == open_ms) {
                    c.high = c.high.max(t.price);
                    c.low = c.low.min(t.price);
                    c.volume += t.qty.max(0.0);
                }
                // Бакета нет (трейд старше всей серии) — игнорируем: окно уехало.
            }
        }
    }
}

fn candle_from_tick(open_ms: f64, t: &Tick) -> ChartCandle {
    ChartCandle {
        t_open_ms: open_ms,
        open: t.price,
        high: t.price,
        low: t.price,
        close: t.price,
        volume: t.qty.max(0.0),
    }
}

/// Ресемпл свечей в более крупный ТФ (5м → 15м и т.п.). Вход отсортирован
/// по времени; кратность не проверяем жёстко — некратный ТФ просто даст сетку floor.
pub fn resample(rows: &[ChartCandle], tf_ms: i64, out: &mut Vec<ChartCandle>) {
    out.clear();
    for r in rows {
        let open_ms = bucket_open_ms(r.t_open_ms, tf_ms);
        match out.last_mut() {
            Some(last) if last.t_open_ms == open_ms => {
                last.high = last.high.max(r.high);
                last.low = last.low.min(r.low);
                last.close = r.close;
                last.volume += r.volume;
            }
            _ => out.push(ChartCandle {
                t_open_ms: open_ms,
                ..*r
            }),
        }
    }
}

/// Слитная per-pane серия свечей: серверная база + локальный хвост из трейдов.
/// Живёт в `ChartHistoryCursor`; перестраивается на combo-reset, живой край —
/// `push_trades` из того же дренажа, что кормит кресты.
#[derive(Default)]
pub struct CandleSeries {
    tf_ms: i64,
    candles: Vec<ChartCandle>,
    revision: u64,
    valid: bool,
    /// Скретч ресемпла (переиспользуем аллокацию между rebuild).
    scratch: Vec<ChartCandle>,
}

impl CandleSeries {
    pub fn is_valid(&self) -> bool {
        self.valid
    }

    pub fn tf_ms(&self) -> i64 {
        self.tf_ms
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn candles(&self) -> &[ChartCandle] {
        &self.candles
    }

    pub fn invalidate(&mut self) {
        self.valid = false;
        self.candles.clear();
    }

    /// Полная пересборка: `base` — серверные свечи родного ТФ `base_tf_ms`
    /// (CoinCard-история либо 5м-снимок; отсортированы), `trades` — трейды видимого
    /// окна (почти отсортированы). Серверные ряды берём для бакетов ДО первого
    /// полного трейдового, локальные — дальше (включая живой).
    pub fn rebuild(&mut self, tf_ms: i64, base: &[ChartCandle], base_tf_ms: i64, trades: &[Tick]) {
        let tf_ms = tf_ms.max(1);
        self.tf_ms = tf_ms;
        self.candles.clear();

        // Локальный хвост из трейдов — во временный буфер (scratch переживёт clear).
        let mut local = std::mem::take(&mut self.scratch);
        aggregate_trades(trades, tf_ms, &mut local);

        // Серверная база: родной ТФ должен делить ТФ серии (иначе ресемпл соврёт).
        if base_tf_ms > 0 && tf_ms >= base_tf_ms && tf_ms % base_tf_ms == 0 && !base.is_empty() {
            if tf_ms == base_tf_ms {
                self.candles.extend_from_slice(base);
            } else {
                let mut resampled = Vec::new();
                resample(base, tf_ms, &mut resampled);
                self.candles = resampled;
            }
        }

        // Шов: первый ПОЛНЫЙ локальный бакет (первый — частичный, если трейд-ринг
        // начинается посреди бакета; его серверная версия честнее). Если серверной базы
        // нет вовсе — берём весь локальный ряд, включая частичный первый.
        let overlay_from = match local.first() {
            None => f64::INFINITY,
            Some(first) if self.candles.is_empty() => first.t_open_ms,
            Some(first) => {
                // Частичность первого бакета определяем по покрытию: если серверная база
                // содержит этот бакет — начинаем со следующего, иначе рискуем дырой и
                // берём и частичный.
                let covered = self.candles.iter().any(|c| c.t_open_ms == first.t_open_ms);
                if covered {
                    first.t_open_ms + tf_ms as f64
                } else {
                    first.t_open_ms
                }
            }
        };
        self.candles.retain(|c| c.t_open_ms < overlay_from);
        self.candles
            .extend(local.iter().filter(|c| c.t_open_ms >= overlay_from));

        local.clear();
        self.scratch = local;
        self.valid = true;
        self.revision = self.revision.wrapping_add(1);
    }

    /// Живой край: подать новые трейды (тот же дренаж, что кормит кресты). Обновляет
    /// последнюю свечу или открывает новую на кросс-бакете. `true` — серия изменилась.
    pub fn push_trades(&mut self, trades: &[Tick]) -> bool {
        if !self.valid || trades.is_empty() {
            return false;
        }
        let tf_ms = self.tf_ms.max(1);
        let mut changed = false;
        for t in trades {
            if !(t.price.is_finite() && t.price > 0.0) {
                continue;
            }
            let open_ms = bucket_open_ms(t.time_ms, tf_ms);
            match self.candles.last_mut() {
                Some(last) if last.t_open_ms == open_ms => {
                    last.high = last.high.max(t.price);
                    last.low = last.low.min(t.price);
                    last.close = t.price;
                    last.volume += t.qty.max(0.0);
                    changed = true;
                }
                Some(last) if open_ms > last.t_open_ms => {
                    self.candles.push(candle_from_tick(open_ms, t));
                    changed = true;
                }
                None => {
                    self.candles.push(candle_from_tick(open_ms, t));
                    changed = true;
                }
                _ => {
                    // Поздний resend в недавний старый бакет (обновляем h/l/vol).
                    if let Some(c) = self
                        .candles
                        .iter_mut()
                        .rev()
                        .take(4)
                        .find(|c| c.t_open_ms == open_ms)
                    {
                        c.high = c.high.max(t.price);
                        c.low = c.low.min(t.price);
                        c.volume += t.qty.max(0.0);
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.revision = self.revision.wrapping_add(1);
        }
        changed
    }

    /// Диапазон цен (low..high) свечей, пересекающих окно времени — для авто-Y чарта.
    pub fn price_range(&self, from_ms: f64, to_ms: f64) -> Option<(f32, f32)> {
        let tf = self.tf_ms.max(1) as f64;
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for c in &self.candles {
            if c.t_open_ms + tf <= from_ms || c.t_open_ms > to_ms {
                continue;
            }
            lo = lo.min(c.low);
            hi = hi.max(c.high);
        }
        (lo <= hi).then_some((lo, hi))
    }
}

#[cfg(test)]
mod tests;
