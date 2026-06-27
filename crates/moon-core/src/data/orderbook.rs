//! Модель стакана для glass-слоя: кумулятивная глубина прямоугольниками
//! от одного ценового уровня до следующего + отдельные level-lines.
//!
//! Нормировка длины баров — НЕ по всей книге, а по максимуму среди уровней,
//! попавших в видимое ценовое окно панели (`build_instances`). Иначе при мелком
//! зуме приспредовые уровни — крошечная доля полного кумулятива, и весь стакан
//! «вытягивается в струну». По видимому окну самый крупный видимый уровень = на
//! всю ширину, и транзиентная стенка чётко выстреливает на своём уровне.

use crate::feed::OrderBook;

/// Инстанс прямоугольника стакана. Совпадает с нативными book-шейдерами.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LevelInstance {
    /// Цена центра полосы.
    pub price: f32,
    /// Signed-delta до второго ценового края полосы.
    pub span: f32,
    /// Длина полосы 0..1 (доля ширины зоны).
    pub len_norm: f32,
    /// 0 = bid fill, 1 = ask fill, 2 = bid level line, 3 = ask level line.
    pub kind: f32,
}

/// Сырой уровень книги (от окна не зависит): геометрия + объёмы. `len_norm`
/// считается позже в `build_instances` под видимое окно конкретной панели.
#[derive(Clone, Copy)]
struct RawLevel {
    price: f32,
    /// Signed-delta до второго ценового края полосы. Лучший bid/ask тянется
    /// вглубь книги, а не в спред.
    span: f32,
    /// Индивидуальный объём уровня (для отдельной линии уровня).
    qty: f32,
    /// Кумулятив от спреда до этого уровня (для полосы глубины).
    cum: f32,
    is_ask: bool,
}

#[derive(Default)]
pub struct OrderBookModel {
    /// Биды (по убыванию цены), затем аски (по возрастанию).
    raw: Vec<RawLevel>,
}

impl OrderBookModel {
    pub fn update(&mut self, book: &OrderBook) {
        self.raw.clear();

        // Книга приходит от биржи уже отсортированной (биды по убыванию, аски по
        // возрастанию) и порядок сохраняется через wire→parse→feed (moonproto не
        // пересортировывает). `push_side` считает span по соседу и ТРЕБУЕТ этот
        // порядок — страхуемся debug-проверкой, но в релизе не сортируем заново
        // (это была чистая лишняя работа на UI-потоке 20 раз/сек) и не клонируем.
        debug_assert!(
            book.bids.windows(2).all(|w| w[0].price >= w[1].price),
            "bids must arrive descending"
        );
        debug_assert!(
            book.asks.windows(2).all(|w| w[0].price <= w[1].price),
            "asks must arrive ascending"
        );

        push_side(&mut self.raw, &book.bids, false);
        push_side(&mut self.raw, &book.asks, true);
    }

    /// Строит GPU-инстансы, нормируя длину баров по максимуму среди уровней
    /// внутри видимого окна `[lo, hi]` (единицы цены). Внеоконные уровни не
    /// эмитятся: scissor остаётся защитой от краёв полос, но CPU/GPU не гоняют
    /// заведомо невидимую книгу.
    pub fn build_instances(&self, lo: f32, hi: f32, out: &mut Vec<LevelInstance>) {
        out.clear();

        // Знаменатель по видимому окну — общий для bid/ask, чтобы стенки сторон
        // были визуально сравнимы. Невидимые уровни не попадают в GPU buffer:
        // стакан рисуется обычными непрозрачными прямоугольниками по видимой цене.
        let mut max_qty = 1e-6_f32;
        let mut max_cum = 1e-6_f32;
        let mut visible: Vec<&RawLevel> = Vec::new();
        for r in &self.raw {
            if !level_overlaps(r, lo, hi) {
                continue;
            }
            max_qty = max_qty.max(r.qty);
            max_cum = max_cum.max(r.cum);
            visible.push(r);
        }

        let inv_max_cum = 1.0 / max_cum.max(1e-6);
        let inv_max_qty = 1.0 / max_qty.max(1e-6);
        out.reserve(visible.len().saturating_mul(2));

        for r in &visible {
            out.push(LevelInstance {
                price: r.price,
                span: r.span,
                len_norm: (r.cum * inv_max_cum).clamp(0.0, 1.0),
                kind: if r.is_ask { 1.0 } else { 0.0 },
            });
        }

        for r in visible {
            out.push(LevelInstance {
                price: r.price,
                span: r.span,
                len_norm: (r.qty * inv_max_qty).clamp(0.0, 1.0) * 0.85,
                kind: if r.is_ask { 3.0 } else { 2.0 },
            });
        }
    }

    /// Число уровней книги (для отладочного счётчика).
    pub fn len(&self) -> usize {
        self.raw.len()
    }

    /// Лучшие `(bid, ask)` книги. Если налита одна сторона — её лучшая цена идёт в
    /// обе позиции (нулевой спред). Пусто/невалид → `None`. `raw` хранит биды (по
    /// убыванию), затем аски (по возрастанию) → первый `!is_ask` = лучший бид, первый
    /// `is_ask` = лучший аск. Основа авто-фокуса чарта, когда трейдов ещё нет
    /// (центр/диапазон по стакану, а не по дефолтному 0).
    pub fn best_bid_ask(&self) -> Option<(f32, f32)> {
        let best_bid = self.raw.iter().find(|r| !r.is_ask).map(|r| r.price);
        let best_ask = self.raw.iter().find(|r| r.is_ask).map(|r| r.price);
        let (bid, ask) = match (best_bid, best_ask) {
            (Some(b), Some(a)) => (b, a),
            (Some(p), None) | (None, Some(p)) => (p, p),
            (None, None) => return None,
        };
        (bid.is_finite() && ask.is_finite() && bid > 0.0 && ask > 0.0).then_some((bid, ask))
    }
}

fn level_overlaps(r: &RawLevel, lo: f32, hi: f32) -> bool {
    let other = r.price + r.span;
    r.price.max(other) >= lo && r.price.min(other) <= hi
}

fn push_side(out: &mut Vec<RawLevel>, levels: &[crate::feed::Level], is_ask: bool) {
    let n = levels.len();
    let mut cum = 0.0_f32;
    for i in 0..n {
        let l = levels[i];
        cum += l.qty;
        // Signed-delta края: лучший уровень уходит вглубь книги, остальные
        // стыкуются обратно к соседу со стороны спреда. Так bid/ask не
        // перекрываются в спреде, а глубина остаётся непрерывной.
        let span = if n > 1 {
            let neighbor = if i > 0 {
                levels[i - 1].price
            } else {
                levels[1].price
            };
            neighbor - levels[i].price
        } else {
            let width = (l.price.abs() * 0.0005).max(1e-6);
            if is_ask {
                width
            } else {
                -width
            }
        }
        .clamp(-f32::MAX, f32::MAX);
        let span = if span.abs() < 1e-6 {
            if is_ask {
                1e-6
            } else {
                -1e-6
            }
        } else {
            span
        };

        out.push(RawLevel {
            price: l.price,
            span,
            qty: l.qty,
            cum,
            is_ask,
        });
    }
}
