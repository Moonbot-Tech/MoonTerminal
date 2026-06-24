//! Ретейн-стор линий ордеров для чарта (per-core). moonproto держит терминальные
//! ордера лишь до deferred-cleanup, поэтому отменённые/исполненные мы храним САМИ —
//! всю сессию (с safety-cap по памяти), рисуя их полупрозрачными.
//!
//! Каждая линия — «лестница» ступеней `(t_ms, price)`: с момента `t_ms` цена держится
//! `price` до следующей ступени. Перестановка цены добавляет ступень (узелок). Начало
//! линии = время создания ордера, конец = время закрытия (или живой правый край).
//! Это уникальный источник старта/узлов/конца для маркеров и отрезков (рисует чарт).

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet, VecDeque};

use crate::feed::{OrderRow, OrderTrace};
use crate::util::now_unix_ms;

/// Виды трассируемых линий (у каждой свой старт/узлы/конец). Ликвидация — отдельно
/// (непрерывная линия без маркеров), хранится как `RetainedOrder::liq`.
pub const TRACED_KINDS: usize = 7;

/// Индексы видов в `RetainedOrder::lines` (совпадают с порядком стилей).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Buy = 0,
    Sell = 1,
    Stop = 2,
    Trailing = 3,
    TakeProfit = 4,
    VStop = 5,
    PendingCond = 6,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderCloseReason {
    Cancel,
    Filled,
    BackstopMissing,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrderLineState {
    pub uid: u64,
    pub closed_reason: Option<OrderCloseReason>,
    pub closed_store_ms: Option<f64>,
    pub closed_rev: Option<u64>,
    pub active: bool,
}

/// Кап кольца ЗАКРЫТЫХ ордеров на ядро (= верх слайдера `max_closed_orders`): свежие
/// толкаем в хвост, старейшие выпадают из головы сами — без сорта и прун-скана.
/// Открытые НЕ капаются (живут пока активны). Единственный кап на закрытые.
const CLOSED_RING_CAP: usize = 5000;

/// Грейс перед пометкой ордера закрытым после исчезновения из снимка, мс. Снимок
/// ордеров может кратко прийти пустым/частичным (реконнект, churn подписки) — без
/// грейса линии мигали бы active↔closed. Закрываем, только если ордер не виделся
/// дольше этого срока.
const CLOSE_GRACE_MS: f64 = 2500.0;

/// Относительный порог «цена изменилась» (защита от float-дрожания → ложных узлов).
fn price_eps(p: f32) -> f32 {
    p.abs() * 1e-5 + 1e-9
}

/// Одна линия ордера как лестница ступеней.
#[derive(Clone, Default)]
pub struct LineTrace {
    /// Ступени `(t_ms, price)`: с `t_ms` цена = `price` до следующей ступени.
    pub steps: Vec<(f64, f32)>,
    /// Точная серверная polyline-трасса для buy/sell. Когда она есть, рендерит
    /// именно её; `steps` остаётся fallback для старых/неполных снимков.
    pub server_points: Vec<(f64, f32)>,
    /// Живая temp-точка серверной трассы: рисуется пунктиром от последней точки.
    pub tmp_point: Option<(f64, f32)>,
    /// Линия выключена (цена стала недоступна), но ордер ещё жив. Конец линии.
    pub off_ms: Option<f64>,
}

impl LineTrace {
    /// Обновляет лестницу новым значением цены. `start_ms` — время первой ступени
    /// (для линии входа = создание ордера; для стопов = момент фила). Возвращает
    /// true при изменении (новая ступень / выключение) — для бампа ревизии стора.
    fn update(&mut self, price: Option<f32>, start_ms: f64, now_ms: f64) -> bool {
        let had_server = !self.server_points.is_empty() || self.tmp_point.is_some();
        if had_server {
            self.server_points.clear();
            self.tmp_point = None;
        }
        match price {
            Some(p) if p.is_finite() && p > 0.0 => {
                let was_off = self.off_ms.take().is_some();
                match self.steps.last().copied() {
                    None => {
                        let t0 = if start_ms > 1.0 { start_ms } else { now_ms };
                        self.steps.push((t0, p));
                        true
                    }
                    Some((_, last_p)) => {
                        if (last_p - p).abs() > price_eps(p) {
                            self.steps.push((now_ms, p));
                            true
                        } else {
                            was_off || had_server
                        }
                    }
                }
            }
            _ => {
                if !self.steps.is_empty() && self.off_ms.is_none() {
                    self.off_ms = Some(now_ms);
                    true
                } else {
                    had_server
                }
            }
        }
    }

    fn update_server(&mut self, trace: Option<&OrderTrace>) -> bool {
        let Some(trace) = trace else {
            return false;
        };
        let points: Vec<(f64, f32)> = trace.points.iter().map(|p| (p.time_ms, p.price)).collect();
        let tmp = trace.tmp_point.map(|p| (p.time_ms, p.price));
        let changed =
            self.server_points != points || self.tmp_point != tmp || self.off_ms.is_some();
        if changed {
            self.server_points = points;
            self.tmp_point = tmp;
            self.off_ms = None;
        }
        changed
    }

    pub fn current_price(&self) -> Option<f32> {
        self.server_points
            .last()
            .map(|(_, p)| *p)
            .or_else(|| self.steps.last().map(|(_, p)| *p))
    }
}

/// Один удержанный ордер с трассами линий.
pub struct RetainedOrder {
    /// uid ордера — ключ стора; понадобится для hit-test/drag линий (этап 5).
    #[allow(dead_code)]
    pub uid: u64,
    pub market: String,
    pub is_short: bool,
    pub pending: bool,
    pub panic_sell: bool,
    pub is_moon_shot: bool,
    pub corridor_price_down: f32,
    pub corridor_price_up: f32,
    /// Время создания (начало линий), unix мс.
    pub create_ms: f64,
    /// Время закрытия (отмена/исполнение); None = ордер активен.
    pub closed_ms: Option<f64>,
    pub closed_reason: Option<OrderCloseReason>,
    pub closed_store_ms: Option<f64>,
    pub closed_rev: Option<u64>,
    /// Когда ордер в последний раз был в снимке (для грейса закрытия).
    last_seen_ms: f64,
    /// Порядок появления (для cap-обрезки старых закрытых).
    pub seq: u64,
    /// Трассы по видам (индекс = LineKind as usize).
    pub lines: [LineTrace; TRACED_KINDS],
    /// Текущая цена ликвидации (непрерывная линия без маркеров).
    pub liq: Option<f32>,
}

impl RetainedOrder {
    fn new(r: &OrderRow, now_ms: f64, seq: u64) -> Self {
        // Старт не может быть в будущем (часы ядра могут опережать локальные) —
        // иначе сегмент линии вырождается/уходит за правый край.
        let create_ms = if r.create_time_ms > 1.0 {
            r.create_time_ms.min(now_ms)
        } else {
            now_ms
        };
        Self {
            uid: r.uid,
            market: r.market.clone(),
            is_short: r.is_short,
            pending: r.pending,
            panic_sell: r.panic_sell,
            is_moon_shot: r.is_moon_shot,
            corridor_price_down: r.corridor_price_down,
            corridor_price_up: r.corridor_price_up,
            create_ms,
            closed_ms: None,
            closed_reason: None,
            closed_store_ms: None,
            closed_rev: None,
            last_seen_ms: now_ms,
            seq,
            lines: Default::default(),
            liq: None,
        }
    }
}

/// Стор линий ордеров одного ядра (все рынки).
#[derive(Default)]
pub struct OrderLineStore {
    orders: HashMap<u64, RetainedOrder>,
    /// Кольцо uid ЗАКРЫТЫХ в порядке закрытия — единственный кап на закрытые,
    /// без сорта/прун-скана: пришёл новый закрытый → в хвост, переполнено → из головы.
    closed_ring: VecDeque<u64>,
    /// Кэш диапазона цен buy/sell открытых ордеров по рынку (для авто-Y). Пересобирается
    /// ТОЛЬКО при изменении ордеров (вместе с rev), а не каждый prepare — buy_sell_range
    /// раньше сканировал все ордера ядра 60 раз/сек на каждую панель.
    buy_sell_ranges: HashMap<String, (f32, f32)>,
    /// Растёт при реальном изменении геометрии (новый ордер/узел/закрытие/liq).
    pub rev: u64,
    seq_counter: u64,
}

impl OrderLineStore {
    /// Применяет свежий снимок ордеров: обновляет активные, фиксирует узлы при
    /// перестановках, помечает исчезнувшие закрытыми. Бампит rev при изменениях.
    pub fn update(&mut self, rows: &[OrderRow]) -> bool {
        let now_ms = now_unix_ms();
        let mut changed = false;
        let mut seen: HashSet<u64> = HashSet::with_capacity(rows.len());
        // uid'ы, закрытые в этом апдейте по ЯВНОМУ флагу job_is_done — remember_closed
        // после цикла (внутри цикла держим &mut-заём self.orders через entry).
        let mut close_now: Vec<u64> = Vec::new();
        let mut closed_this_update: Vec<u64> = Vec::new();

        for r in rows {
            seen.insert(r.uid);
            let order = match self.orders.entry(r.uid) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    let seq = self.seq_counter;
                    self.seq_counter = self.seq_counter.wrapping_add(1);
                    changed = true;
                    entry.insert(RetainedOrder::new(r, now_ms, seq))
                }
            };
            order.last_seen_ms = now_ms;
            // Воскрешение: ранее закрытый uid снова АКТИВЕН (НЕ job_is_done) → опять живой.
            // Терминальный (job_is_done) ордер может оставаться в снимке весь deferred-window
            // ядра — его НЕ воскрешаем, иначе линия мигала бы closed→open каждый апдейт.
            if order.closed_ms.is_some() && !r.job_is_done {
                order.closed_ms = None;
                order.closed_reason = None;
                order.closed_store_ms = None;
                order.closed_rev = None;
                changed = true;
            }
            order.is_short = r.is_short;
            order.pending = r.pending;
            if order.panic_sell != r.panic_sell
                || order.is_moon_shot != r.is_moon_shot
                || order.corridor_price_down != r.corridor_price_down
                || order.corridor_price_up != r.corridor_price_up
            {
                order.panic_sell = r.panic_sell;
                order.is_moon_shot = r.is_moon_shot;
                order.corridor_price_down = r.corridor_price_down;
                order.corridor_price_up = r.corridor_price_up;
                changed = true;
            }
            let f = r.filled;
            // Вход (для long и short) — всегда BUY pending-ордер: видна сразу, старт =
            // создание. SELL (закрытие, в противоположную сторону) появляется только
            // после исполнения входа, старт = момент фила. Стопы/TP/vstop/liq — тоже
            // только после фила.
            let new_liq = if f { r.liq.map(|v| v as f32) } else { None };
            if order.liq != new_liq {
                order.liq = new_liq;
                changed = true;
            }
            let g = |show: bool, v: f64| (show && v.is_finite() && v > 0.0).then_some(v as f32);
            let go = |show: bool, v: Option<f64>| if show { v.map(|x| x as f32) } else { None };
            // (значение, время первой ступени) по видам.
            changed |= order.lines[LineKind::Buy as usize].update_server(r.buy_trace.as_ref());
            if r.buy_trace.is_none() {
                changed |= order.lines[LineKind::Buy as usize].update(
                    g(true, r.buy_price),
                    order.create_ms,
                    now_ms,
                );
            }
            changed |= order.lines[LineKind::Sell as usize].update_server(r.sell_trace.as_ref());
            if r.sell_trace.is_none() {
                changed |=
                    order.lines[LineKind::Sell as usize].update(g(f, r.sell_price), now_ms, now_ms);
            }

            let vals: [(Option<f32>, f64, usize); TRACED_KINDS - 2] = [
                (go(f, r.stop_loss), now_ms, LineKind::Stop as usize),
                (go(f, r.trailing), now_ms, LineKind::Trailing as usize),
                (go(f, r.take_profit), now_ms, LineKind::TakeProfit as usize),
                (go(f, r.vstop), now_ms, LineKind::VStop as usize),
                // Pending-условие осмысленно только до фила (старт = создание).
                (
                    go(!f, r.pending_cond),
                    order.create_ms,
                    LineKind::PendingCond as usize,
                ),
            ];
            for (v, start_ms, i) in vals {
                changed |= order.lines[i].update(v, start_ms, now_ms);
            }
            // Закрытие по ЯВНОМУ флагу ядра: job_is_done = ордер терминальный (исполнен/
            // отменён), ждёт deferred-removal. Помечаем закрытым СРАЗУ, пока он ещё в
            // снимке — не дожидаясь исчезновения + грейса.
            if r.job_is_done && order.closed_ms.is_none() {
                order.closed_ms = Some(now_ms);
                order.closed_reason = Some(explicit_close_reason(r));
                order.closed_store_ms = Some(now_ms);
                close_now.push(r.uid);
                closed_this_update.push(r.uid);
                changed = true;
            }
        }

        for uid in close_now {
            self.remember_closed(uid);
        }

        // BACKSTOP: ордер ИСЧЕЗ из снимка дольше грейса → закрыт. Основной путь — job_is_done
        // выше (закрывает, пока ордер ещё в снимке). Сюда падают лишь ордера, убранные ядром
        // БЕЗ виденного нами job_is_done (пропущенный кадр/гэп); грейс гасит ложное мигание
        // на кратком пустом/частичном снимке (реконнект/churn подписки).
        let mut newly_closed = Vec::new();
        for (uid, ord) in self.orders.iter_mut() {
            if !seen.contains(uid)
                && ord.closed_ms.is_none()
                && now_ms - ord.last_seen_ms > CLOSE_GRACE_MS
            {
                ord.closed_ms = Some(ord.last_seen_ms);
                ord.closed_reason = Some(OrderCloseReason::BackstopMissing);
                ord.closed_store_ms = Some(now_ms);
                newly_closed.push(*uid);
                closed_this_update.push(*uid);
                changed = true;
            }
        }

        for uid in newly_closed {
            self.remember_closed(uid);
            changed = true;
        }
        if changed {
            self.rev = self.rev.wrapping_add(1);
            for uid in closed_this_update {
                if let Some(order) = self.orders.get_mut(&uid) {
                    order.closed_rev = Some(self.rev);
                }
            }
            self.rebuild_buy_sell_ranges();
        }
        changed
    }

    /// Пересобирает кэш buy/sell-диапазонов по рынкам из текущих открытых ордеров.
    /// Зовётся только при реальном изменении (`changed`) — цены линий мутируют лишь в
    /// `update`, поэтому кэш всегда свежий, но скан O(ордера) идёт 4 Гц, не 60.
    fn rebuild_buy_sell_ranges(&mut self) {
        self.buy_sell_ranges.clear();
        for o in self.orders.values() {
            if o.closed_ms.is_some() {
                continue;
            }
            for idx in [LineKind::Buy as usize, LineKind::Sell as usize] {
                if let Some(p) = o.lines[idx].current_price() {
                    if p.is_finite() && p > 0.0 {
                        let e = self
                            .buy_sell_ranges
                            .entry(o.market.clone())
                            .or_insert((p, p));
                        e.0 = e.0.min(p);
                        e.1 = e.1.max(p);
                    }
                }
            }
        }
    }

    /// Запоминает закрытый uid и срезает старейшие закрытые сверх safety-cap.
    fn remember_closed(&mut self, uid: u64) {
        self.closed_ring.push_back(uid);
        while self.closed_ring.len() > CLOSED_RING_CAP {
            let Some(old_uid) = self.closed_ring.pop_front() else {
                break;
            };
            if self
                .orders
                .get(&old_uid)
                .is_some_and(|order| order.closed_ms.is_some())
            {
                self.orders.remove(&old_uid);
            }
        }
    }

    /// Ордера данного рынка (для рендера линий конкретной панели).
    pub fn iter_market<'a>(
        &'a self,
        market: &'a str,
    ) -> impl Iterator<Item = &'a RetainedOrder> + 'a {
        self.orders.values().filter(move |o| o.market == market)
    }

    /// Ордера рынка ДЛЯ ОТРИСОВКИ: все открытые + новейшие `max_closed` закрытых, в
    /// порядке кольца (новые-первые), БЕЗ сортировки. Кап на закрытые задаёт сам стор
    /// кольцом — отдельного сорта/отбора в рендере больше нет.
    pub fn market_draw_orders(&self, market: &str, max_closed: usize) -> Vec<&RetainedOrder> {
        let mut out: Vec<&RetainedOrder> = self
            .orders
            .values()
            .filter(|o| o.market == market && o.closed_ms.is_none())
            .collect();
        let mut taken = 0usize;
        let mut seen: HashSet<u64> = HashSet::new();
        for uid in self.closed_ring.iter().rev() {
            if taken >= max_closed {
                break;
            }
            // Воскресший→переоткрытый uid может лежать в кольце дважды — дедупим.
            if !seen.insert(*uid) {
                continue;
            }
            if let Some(o) = self.orders.get(uid) {
                if o.closed_ms.is_some() && o.market == market {
                    out.push(o);
                    taken += 1;
                }
            }
        }
        out
    }

    /// Диапазон цен (min,max) текущих линий BUY и SELL открытых (не закрытых)
    /// ордеров рынка — для авто-масштаба Y. ТОЛЬКО buy/sell (не стопы/liq/прочее).
    /// Готовый кэш (`rebuild_buy_sell_ranges` при изменении ордеров), не скан per-prepare.
    pub fn buy_sell_range(&self, market: &str) -> Option<(f32, f32)> {
        self.buy_sell_ranges.get(market).copied()
    }

    pub fn order_state(&self, uid: u64) -> Option<OrderLineState> {
        let order = self.orders.get(&uid)?;
        Some(OrderLineState {
            uid,
            closed_reason: order.closed_reason,
            closed_store_ms: order.closed_store_ms,
            closed_rev: order.closed_rev,
            active: order.closed_ms.is_none(),
        })
    }
}

fn explicit_close_reason(row: &OrderRow) -> OrderCloseReason {
    if row.filled || row.fill_pct >= 99.95 {
        OrderCloseReason::Filled
    } else if row.job_is_done {
        OrderCloseReason::Cancel
    } else {
        OrderCloseReason::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(uid: u64) -> OrderRow {
        OrderRow {
            market: "BTCUSDT".into(),
            is_short: false,
            size: 0.01,
            sl_on: false,
            ts_on: false,
            vstop_on: false,
            buy_price: 60_000.0,
            sell_price: 0.0,
            create_time_ms: 1_000.0,
            price: 60_000.0,
            fill_pct: 0.0,
            strat: "test".into(),
            uid,
            emulator: false,
            job_is_done: false,
            pending: false,
            filled: false,
            stop_loss: None,
            trailing: None,
            take_profit: None,
            vstop: None,
            pending_cond: None,
            liq: None,
            panic_sell: false,
            is_moon_shot: false,
            corridor_price_down: 0.0,
            corridor_price_up: 0.0,
            buy_trace: None,
            sell_trace: None,
        }
    }

    #[test]
    fn missing_order_does_not_close_without_terminal_status_or_backstop_grace() {
        let mut store = OrderLineStore::default();
        assert!(store.update(&[order(42)]));

        assert!(!store.update(&[]));

        let state = store.order_state(42).expect("retained order must stay");
        assert_eq!(state.closed_reason, None);
        assert!(state.closed_store_ms.is_none());
        assert!(state.closed_rev.is_none());
        assert!(state.active);
    }

    #[test]
    fn terminal_status_closes_order_immediately() {
        let mut store = OrderLineStore::default();
        assert!(store.update(&[order(42)]));

        let mut done = order(42);
        done.job_is_done = true;
        assert!(store.update(&[done]));

        let state = store.order_state(42).expect("retained order must stay");
        assert_eq!(state.closed_reason, Some(OrderCloseReason::Cancel));
        assert!(state.closed_store_ms.is_some());
        assert!(state.closed_rev.is_some());
        assert!(!state.active);
    }
}
