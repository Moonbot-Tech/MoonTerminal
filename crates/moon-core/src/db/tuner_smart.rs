//! «Умный» автоподбор порогов тюнера: КООРДИНАТНЫЙ СПУСК по всем полям сразу.
//!
//! Обычный «Подобрать всё» оптимизирует каждое поле независимо — комбинация
//! диапазонов не максимальна (поля взаимодействуют: отсечённые одним полем
//! сделки меняют оптимум другого). Здесь состояние = диапазон (или «нет
//! фильтра») на КАЖДОЕ поле; на каждом шаге одно поле переоптимизируется
//! полным перебором пар квантильных краёв при зафиксированных остальных;
//! проход по всем полям = итерация; сходится, когда полный проход ничего не
//! меняет (обычно 2–4 итерации).
//!
//! Семантика совпадает с вариантами тюнера: NULL поля = 0 (COALESCE), границы
//! включительные. Один скан БД, дальше всё в памяти — вызывать ТОЛЬКО с
//! background executor.

use super::analytics::{Query, unified_from};
use super::tuner::FIELDS;

const EDGES: usize = 16;
/// Бин «ниже первого края» (значения COALESCE-0 при положительном минимуме).
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
    /// Сколько итераций реально прошло (≤ запрошенных; меньше = сошлось).
    pub rounds: usize,
}

/// Координатный спуск: максимум суммарного профита при сохранении ≥`min_n`
/// сделок. `rounds` — максимум проходов по всем полям (1..=50).
pub fn smart_suggest(q: &Query, rounds: usize, min_n: i64) -> Option<SmartResult> {
    let conn = super::open_reader()?;
    let mut q = q.clone();
    if q.from < 0 {
        q.from = 1;
    }
    let src = unified_from(&conn, &q)?;
    let nf = FIELDS.len();
    let cols = FIELDS
        .iter()
        .map(|(c, _, _)| format!("o.\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!("SELECT {cols}, COALESCE(o.profitbtc,0) FROM {src}");

    // Скан в память: профит + эффективные значения (COALESCE 0) колоночно.
    let mut profits: Vec<f64> = Vec::new();
    let mut vals: Vec<Vec<f64>> = vec![Vec::new(); nf];
    {
        let mut stmt = conn.prepare(&sql).ok()?;
        let mut rows = stmt.query(rusqlite::params![q.from, q.to]).ok()?;
        while let Ok(Some(r)) = rows.next() {
            profits.push(r.get(nf).unwrap_or(0.0));
            for (fi, col) in vals.iter_mut().enumerate() {
                let v = r
                    .get::<_, Option<f64>>(fi)
                    .ok()
                    .flatten()
                    .filter(|v| v.is_finite())
                    .unwrap_or(0.0);
                col.push(v);
            }
        }
    }
    let n = profits.len();
    let min_n = min_n.max(1) as usize;
    if n < min_n {
        return None;
    }

    // Квантильные края и бин каждой сделки по каждому полю.
    let mut edges: Vec<Vec<f64>> = Vec::with_capacity(nf);
    let mut bins: Vec<Vec<u8>> = Vec::with_capacity(nf);
    for col in &vals {
        let mut sorted = col.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let e: Vec<f64> = (0..=EDGES).map(|k| sorted[k * (n - 1) / EDGES]).collect();
        let b = col
            .iter()
            .map(|v| {
                if *v < e[0] {
                    BELOW
                } else {
                    // Последний край включительно — клампим в верхний бин.
                    let mut lo = 0usize;
                    let mut hi = EDGES;
                    while lo + 1 < hi {
                        let m = (lo + hi) / 2;
                        if *v >= e[m] { lo = m } else { hi = m }
                    }
                    lo.min(EDGES - 1) as u8
                }
            })
            .collect();
        edges.push(e);
        bins.push(b);
    }

    // Состояние: выбранная пара краёв на поле (None = без фильтра) + маски.
    let mut sel: Vec<Option<(usize, usize)>> = vec![None; nf];
    let mut pass: Vec<Vec<bool>> = vec![vec![true; n]; nf];
    let mut fail: Vec<u16> = vec![0; n];
    let mut done_rounds = 0usize;
    for _round in 0..rounds.clamp(1, 50) {
        done_rounds += 1;
        let mut changed = false;
        for fi in 0..nf {
            // Суммы по бинам среди сделок, проходящих ВСЕ прочие поля.
            let selfpass = &pass[fi];
            let mut bp = [0.0f64; 17]; // 16 бинов + BELOW
            let mut bc = [0usize; 17];
            let mut tot_p = 0.0f64;
            for t in 0..n {
                let others_ok = fail[t] == u16::from(!selfpass[t]);
                if !others_ok {
                    continue;
                }
                tot_p += profits[t];
                let b = bins[fi][t];
                let idx = if b == BELOW { 16 } else { b as usize };
                bp[idx] += profits[t];
                bc[idx] += 1;
            }
            // Кандидаты: «без фильтра» и все пары краёв (i, j).
            let mut pre_p = [0.0f64; EDGES + 1];
            let mut pre_c = [0usize; EDGES + 1];
            for k in 0..EDGES {
                pre_p[k + 1] = pre_p[k] + bp[k];
                pre_c[k + 1] = pre_c[k] + bc[k];
            }
            let mut best: Option<(usize, usize)> = None;
            let mut best_p = tot_p; // None-вариант (n = tot_n всегда допустим)
            for i in 0..EDGES {
                for j in (i + 1)..=EDGES {
                    let c = pre_c[j] - pre_c[i];
                    if c < min_n {
                        continue;
                    }
                    let p = pre_p[j] - pre_p[i];
                    if p > best_p {
                        best_p = p;
                        best = Some((i, j));
                    }
                }
            }
            if best != sel[fi] {
                // Пересобрать маску поля и счётчики отказов.
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

    let (mut profit, mut cnt) = (0.0f64, 0i64);
    for t in 0..n {
        if fail[t] == 0 {
            profit += profits[t];
            cnt += 1;
        }
    }
    let fields = sel
        .iter()
        .enumerate()
        .filter_map(|(fi, s)| {
            s.map(|(i, j)| SmartField {
                field: FIELDS[fi].0,
                from: edges[fi][i],
                to: edges[fi][j],
            })
        })
        .collect();
    Some(SmartResult { fields, profit, n: cnt, rounds: done_rounds })
}
