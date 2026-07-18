//! «Умный» автоподбор порогов тюнера: координатный спуск со СЛУЧАЙНЫМИ
//! РЕСТАРТАМИ (random-restart hill climbing) по всем полям сразу.
//!
//! Наивный подбор оптимизирует каждое поле независимо — комбинация не
//! максимальна (поля взаимодействуют). Здесь состояние = диапазон (или «нет
//! фильтра») на КАЖДОЕ поле; шаг — переоптимизация одного поля полным
//! перебором пар КВАНТИЛЬНЫХ краёв (квантили = реальные значения данных;
//! число краёв настраивается, деф. 64) при зафиксированных остальных;
//! проходы до сходимости. Спуск
//! жадный и застревает в локальных оптимумах — поэтому несколько РЕСТАРТОВ:
//! первый с пустого состояния, остальные со случайной инициализации и
//! перетасованным порядком полей; берётся лучший результат.
//!
//! Семантика совпадает с вариантами тюнера: NULL поля = 0 (COALESCE), границы
//! включительные. Один скан БД, дальше всё в памяти — вызывать ТОЛЬКО с
//! background executor.

use super::analytics::{unified_from, Query};
use super::read_fail::read_fail;
use super::tuner::{FieldClass, FIELDS};
use super::{ReadFail, ReadResult};

/// Бин «ниже первого края» (значения COALESCE-0 при положительном минимуме).
/// Число краёв ≤128 — индексы бинов не сталкиваются с сентинелом.
const BELOW: u8 = u8::MAX;

/// Итоговый диапазон одного поля.
#[derive(Clone, Debug)]
pub struct SmartField {
    pub field: &'static str,
    pub from: f64,
    pub to: f64,
}

/// Результат умного подбора: диапазоны + ожидаемый профит/сделки комбинации.
#[derive(Clone, Debug)]
pub struct SmartResult {
    pub fields: Vec<SmartField>,
    pub profit: f64,
    pub n: i64,
    /// Сколько рестартов сделано.
    pub rounds: usize,
}

/// Простой PRNG (xorshift64*) — rand-зависимость не нужна. Зерно берётся от
/// времени запуска: каждый клик «Подобрать всё» пробует СВОИ случайные старты
/// (намеренно, по просьбе пользователя — повторные переборы продолжают искать
/// новые локальные оптимумы, а не повторяют одну последовательность).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Maximize combined profit with random-restart coordinate descent while
/// retaining at least `min_n` trades.
///
/// `restarts` is clamped to 1..=1000, and each attempt runs at most 16 passes.
/// `edges_want` controls quantile resolution per field. At most two Delta2/3
/// slot fields may carry ranges because the strategy format cannot store more.
/// `locked[fi]` is `None` for a searched field, a fixed range for an active
/// locked field, or `(None, None)` for an excluded field.
/// `Ok(None)` means no candidate beats the baseline or the sample is too small;
/// `NotReady` means the replica or required schema is absent, while `Failed`
/// means opening or scanning the replica failed.
pub fn smart_suggest(
    q: &Query,
    restarts: usize,
    min_n: i64,
    locked: &[Option<(Option<f64>, Option<f64>)>],
    edges_want: usize,
    round: bool,
) -> ReadResult<Option<SmartResult>> {
    const CTX: &str = "tuner: smart_suggest";
    let ne = edges_want.clamp(4, 128);
    let conn = super::open_reader()?;
    let mut q = q.clone();
    if q.from < 0 {
        q.from = 1;
    }
    let Some(src) = unified_from(&conn, &q)? else {
        return Err(ReadFail::NotReady);
    };
    let nf = FIELDS.len();
    let cols = FIELDS
        .iter()
        .map(|s| format!("o.\"{}\"", s.col))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!("SELECT {cols}, COALESCE(o.profitbtc,0) FROM {src}");

    // Скан в память: профит + эффективные значения (COALESCE 0) колоночно.
    let mut profits: Vec<f64> = Vec::new();
    let mut vals: Vec<Vec<f64>> = vec![Vec::new(); nf];
    {
        let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
        let mut rows = stmt
            .query(rusqlite::params![q.from, q.to])
            .map_err(|e| read_fail(CTX, e))?;
        // Every value feeds the optimiser, so an unreadable cell aborts the
        // calculation. NULL deliberately maps to 0.0; a read error does not.
        while let Some(r) = rows.next().map_err(|e| read_fail(CTX, e))? {
            profits.push(r.get(nf).map_err(|e| read_fail(CTX, e))?);
            for (fi, col) in vals.iter_mut().enumerate() {
                let v = r.get::<_, Option<f64>>(fi).map_err(|e| read_fail(CTX, e))?;
                col.push(v.filter(|v| v.is_finite()).unwrap_or(0.0));
            }
        }
    }
    let n = profits.len();
    let min_n = min_n.max(1) as usize;
    if n < min_n {
        // Not enough sample — a legitimate "no suggestion", not a failure.
        return Ok(None);
    }

    // Квантильные края и бин каждой сделки по каждому полю.
    let mut edges: Vec<Vec<f64>> = Vec::with_capacity(nf);
    let mut bins: Vec<Vec<u8>> = Vec::with_capacity(nf);
    for col in &vals {
        let mut sorted = col.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let e: Vec<f64> = (0..=ne).map(|k| sorted[k * (n - 1) / ne]).collect();
        let b = col
            .iter()
            .map(|v| {
                if *v < e[0] {
                    BELOW
                } else {
                    // Последний край включительно — клампим в верхний бин.
                    let mut lo = 0usize;
                    let mut hi = ne;
                    while lo + 1 < hi {
                        let m = (lo + hi) / 2;
                        if *v >= e[m] {
                            lo = m
                        } else {
                            hi = m
                        }
                    }
                    lo.min(ne - 1) as u8
                }
            })
            .collect();
        edges.push(e);
        bins.push(b);
    }

    // Мульти-старт: рестарт 0 — жадный с пустого состояния, дальше —
    // случайная инициализация + перетасованный порядок полей.
    let restarts = restarts.clamp(1, 1000);
    let is_slot: Vec<bool> = FIELDS
        .iter()
        .map(|s| s.class == FieldClass::DeltaSlot)
        .collect();
    // Перебираемые поля; фиксированные маски залипают в fail-счётчиках базой.
    let free: Vec<bool> = (0..nf)
        .map(|fi| locked.get(fi).is_none_or(|l| l.is_none()))
        .collect();
    let mut base_fail: Vec<u16> = vec![0; n];
    for fi in 0..nf {
        let Some(Some((lo, hi))) = locked.get(fi) else {
            continue;
        };
        if lo.is_none() && hi.is_none() {
            continue;
        }
        for t in 0..n {
            let v = vals[fi][t];
            let ok = lo.is_none_or(|l| v >= l) && hi.is_none_or(|h| v <= h);
            if !ok {
                base_fail[t] += 1;
            }
        }
    }
    // Занятые фикс-полями слоты уменьшают лимит пары Delta2/Delta3.
    let locked_slots: usize = (0..nf)
        .filter(|fi| {
            is_slot[*fi]
                && matches!(locked.get(*fi), Some(Some((lo, hi))) if lo.is_some() || hi.is_some())
        })
        .count();
    // Зерно от времени запуска: каждый перебор — свои случайные старты
    // (рестарт 0 всё равно жадный с пустого, он стабилен). Нулевое зерно
    // xorshift'у запрещено — подмешиваем константу.
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut rng = Rng(seed ^ 0x9E37_79B9_7F4A_7C15 | 1);
    let mut best_global: Option<(f64, i64, Vec<Option<(usize, usize)>>)> = None;
    for restart in 0..restarts {
        let mut sel: Vec<Option<(usize, usize)>> = vec![None; nf];
        if restart > 0 {
            // ~30% полей стартуют со случайного диапазона (слотов — макс 2).
            let mut slots_used = 0usize;
            for (fi, s) in sel.iter_mut().enumerate() {
                if !free[fi] {
                    continue;
                }
                if rng.below(100) < 30 {
                    if is_slot[fi] {
                        if slots_used >= 2 {
                            continue;
                        }
                        slots_used += 1;
                    }
                    let i = rng.below(ne);
                    let j = i + 1 + rng.below(ne - i);
                    *s = Some((i, j));
                }
            }
        }
        // Маски из инициализации (поверх фиксированных фильтров).
        let mut pass: Vec<Vec<bool>> = vec![vec![true; n]; nf];
        let mut fail: Vec<u16> = base_fail.clone();
        for fi in 0..nf {
            if let Some((i, j)) = sel[fi] {
                for t in 0..n {
                    let b = bins[fi][t];
                    let ok = b != BELOW && (i..j).contains(&(b as usize));
                    if !ok {
                        fail[t] += 1;
                    }
                    pass[fi][t] = ok;
                }
            }
        }
        let mut order: Vec<usize> = (0..nf).filter(|fi| free[*fi]).collect();
        for _pass_no in 0..16 {
            if restart > 0 {
                // Fisher-Yates gives each pass its own field order.
                for k in (1..order.len()).rev() {
                    order.swap(k, rng.below(k + 1));
                }
            }
            let mut changed = false;
            for &fi in &order {
                // Sum bins among trades that pass every other field.
                let selfpass = &pass[fi];
                let mut bp = vec![0.0f64; ne + 1]; // бины + BELOW последним
                let mut bc = vec![0usize; ne + 1];
                let (mut tot_p, mut tot_c) = (0.0f64, 0usize);
                for t in 0..n {
                    let others_ok = fail[t] == u16::from(!selfpass[t]);
                    if !others_ok {
                        continue;
                    }
                    tot_p += profits[t];
                    tot_c += 1;
                    let b = bins[fi][t];
                    let idx = if b == BELOW { ne } else { b as usize };
                    bp[idx] += profits[t];
                    bc[idx] += 1;
                }
                // Candidates are no filter and every edge pair `(i, j)`.
                let mut pre_p = vec![0.0f64; ne + 1];
                let mut pre_c = vec![0usize; ne + 1];
                for k in 0..ne {
                    pre_p[k + 1] = pre_p[k] + bp[k];
                    pre_c[k + 1] = pre_c[k] + bc[k];
                }
                let mut best: Option<(usize, usize)> = None;
                // The no-filter candidate is the baseline. A range must beat it
                // beyond floating-point noise because bin sums and `tot_p`
                // accumulate in different orders; otherwise an equivalent trade
                // set can win by about 1e-12 and leave a no-op filter.
                let mut best_p = tot_p + tot_p.abs().max(1.0) * 1e-9;
                // Only two Delta2/Delta3 slots exist. If two other slots already
                // have ranges, this field may only choose no filter.
                let slot_full = is_slot[fi]
                    && locked_slots
                        + sel
                            .iter()
                            .enumerate()
                            .filter(|(o, s)| *o != fi && is_slot[*o] && s.is_some())
                            .count()
                        >= 2;
                if !slot_full {
                    for i in 0..ne {
                        for j in (i + 1)..=ne {
                            let c = pre_c[j] - pre_c[i];
                            if c < min_n {
                                continue;
                            }
                            // A candidate that passes every trade in the current
                            // combination is functionally no filter, including a
                            // full-span range, so reject it by exact integer count.
                            if c == tot_c {
                                continue;
                            }
                            let p = pre_p[j] - pre_p[i];
                            if p > best_p {
                                best_p = p;
                                best = Some((i, j));
                            }
                        }
                    }
                }
                if best != sel[fi] {
                    // Rebuild this field's pass mask and failure counts.
                    sel[fi] = best;
                    changed = true;
                    for t in 0..n {
                        let ok = match best {
                            None => true,
                            Some((i, j)) => {
                                let b = bins[fi][t];
                                b != BELOW && (i..j).contains(&(b as usize))
                            }
                        };
                        if ok != pass[fi][t] {
                            if ok {
                                fail[t] -= 1;
                            } else {
                                fail[t] += 1;
                            }
                            pass[fi][t] = ok;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
        // Финальная зачистка совместно-избыточных диапазонов: спуск мог
        // остановиться по лимиту проходов, не успев снять поле, все сделки
        // которого уже режут ДРУГИЕ фильтры (уникально режет row t только
        // поле с !pass && fail==1). Такой диапазон — пустышка: удаление не
        // меняет ни профит, ни счёт. Чистим до стабильности.
        loop {
            let mut removed = false;
            for fi in 0..nf {
                if !free[fi] || sel[fi].is_none() {
                    continue;
                }
                let redundant = (0..n).all(|t| pass[fi][t] || fail[t] > 1);
                if redundant {
                    sel[fi] = None;
                    for t in 0..n {
                        if !pass[fi][t] {
                            fail[t] -= 1;
                            pass[fi][t] = true;
                        }
                    }
                    removed = true;
                }
            }
            if !removed {
                break;
            }
        }
        // Итог рестарта — против глобального лучшего.
        let (mut profit, mut cnt) = (0.0f64, 0i64);
        for t in 0..n {
            if fail[t] == 0 {
                profit += profits[t];
                cnt += 1;
            }
        }
        if best_global.as_ref().is_none_or(|(bp, _, _)| profit > *bp) {
            best_global = Some((profit, cnt, sel.clone()));
        }
    }

    // No variant beat the baseline — a legitimate "no suggestion".
    let Some((profit, cnt, sel)) = best_global else {
        return Ok(None);
    };
    let fields = sel
        .iter()
        .enumerate()
        .filter_map(|(fi, s)| {
            // Пара на обоих краях данных ничего не режет — не подставляем.
            let (i, j) = (*s).filter(|(i, j)| !(*i == 0 && *j == ne))?;
            let (mut from, mut to) = (edges[fi][i], edges[fi][j]);
            if round {
                let (rf, rt) = (
                    super::tuner::round_bound(from, false),
                    super::tuner::round_bound(to, true),
                );
                // Outward rounding can move both distribution-edge bounds past
                // the observed range and turn the pair into a no-op filter.
                // Preserve raw bounds in that case.
                if rf > edges[fi][0] || rt < edges[fi][ne] {
                    (from, to) = (rf, rt);
                }
            }
            Some(SmartField {
                field: FIELDS[fi].col,
                from,
                to,
            })
        })
        .collect();
    Ok(Some(SmartResult {
        fields,
        profit,
        n: cnt,
        rounds: restarts,
    }))
}
