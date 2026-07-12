//! Локальный кэш klines (свечей) — отдельная БД `klines.sqlite` РЯДОМ с остальными
//! данными (НЕ reports.sqlite). Решает две проблемы протокола moonproto:
//! ядро держит ОДИН свечной ТФ на ядро, а `GetCoinCardCandles` не умеет частичную
//! догрузку (параметры = монета+kind, ответ всегда полный ринг). Поэтому один раз
//! полученную историю храним локально: глубина крупных ТФ переживает рестарты и
//! периоды, когда слот ядра занят мелким ТФ, а повторные полные перекачки с биржи
//! (весовые лимиты!) не нужны.
//!
//! Схема: одна таблица `chunks` — упакованный блоб суточных рядов на ключ
//! (биржа, рынок, kind, сутки). Биржа = стабильный `ExchangeId` (код+dex-хеш), НЕ
//! CoreId: ядра одной биржи делят кэш. Дедуп бесплатный — PRIMARY KEY + merge по
//! t_open внутри чанка (входящие ряды авторитетнее лежащих). Ряд = 24 байта
//! (u32 offset_ms + 5×f32) → сутки 1м ≈ 34КБ на монету. Ретеншн по kind при
//! старте: 1м — 60 суток, 5м — год, крупнее — 10 лет.
//!
//! Все операции — на выделенном потоке (Connection не Sync); запись неблокирующая
//! (очередь), чтение — с ответным каналом и таймаутом (пустой результат хуже, чем
//! подвешенный prepare).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use super::candles::ChartCandle;

const DAY_MS: i64 = 86_400_000;
const ROW_BYTES: usize = 24;
/// Таймаут ответа на чтение: кэш-поток занят/умер → рисуем без префикса, не виснем.
const READ_TIMEOUT: Duration = Duration::from_millis(250);

/// Ретеншн по kind (суток): мелкие ТФ тяжёлые, крупные — копейки. 1м пишется фоновым
/// регистратором ПО ВСЕМ рынкам (~35КБ/сутки/монета) — 30 суток, чтобы база не
/// разрасталась в гигабайты (из 1м любой крупный ТФ строится ресемплом).
fn retention_days(kind_min: u32) -> i64 {
    match kind_min {
        0..=1 => 30,
        2..=5 => 90,
        _ => 3650,
    }
}

enum Op {
    Merge {
        exchange: String,
        market: String,
        kind_min: u32,
        rows: Vec<ChartCandle>,
    },
    Read {
        exchange: String,
        market: String,
        kind_min: u32,
        from_ms: i64,
        to_ms: i64,
        reply: mpsc::Sender<Vec<ChartCandle>>,
    },
}

/// Хэндл кэша: дешёвый Clone, все операции уезжают на поток БД.
#[derive(Clone)]
pub struct KlineCache {
    tx: mpsc::Sender<Op>,
}

impl KlineCache {
    /// Открыть/создать БД и поднять поток. Ошибка открытия — None (кэш опционален,
    /// чарты живут без него).
    pub fn open(path: PathBuf) -> Option<Self> {
        let conn = match rusqlite::Connection::open(&path) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("kline cache open failed {}: {e}", path.display());
                return None;
            }
        };
        if let Err(e) = init_schema(&conn) {
            log::warn!("kline cache schema failed {}: {e}", path.display());
            return None;
        }
        let (tx, rx) = mpsc::channel::<Op>();
        std::thread::Builder::new()
            .name("kline-cache".into())
            .spawn(move || run(conn, rx))
            .ok()?;
        log::info!("kline cache открыт: {}", path.display());
        Some(Self { tx })
    }

    /// Дописать/обновить ряды (неблокирующе). Входящие авторитетнее лежащих
    /// (серверные OHLC поверх прежних), пустые/мусорные строки отбрасываются.
    pub fn merge(&self, exchange: String, market: String, kind_min: u32, rows: Vec<ChartCandle>) {
        if rows.is_empty() {
            return;
        }
        let _ = self.tx.send(Op::Merge {
            exchange,
            market,
            kind_min,
            rows,
        });
    }

    /// Прочитать ряды диапазона [from_ms, to_ms] (по t_open). Блокирует не дольше
    /// `READ_TIMEOUT`; на таймауте/ошибке — пусто.
    pub fn read_range(
        &self,
        exchange: &str,
        market: &str,
        kind_min: u32,
        from_ms: i64,
        to_ms: i64,
    ) -> Vec<ChartCandle> {
        let (reply, rx) = mpsc::channel();
        if self
            .tx
            .send(Op::Read {
                exchange: exchange.to_string(),
                market: market.to_string(),
                kind_min,
                from_ms,
                to_ms,
                reply,
            })
            .is_err()
        {
            return Vec::new();
        }
        rx.recv_timeout(READ_TIMEOUT).unwrap_or_default()
    }
}

fn init_schema(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS chunks(
            exchange TEXT NOT NULL,
            market TEXT NOT NULL,
            kind INTEGER NOT NULL,
            day INTEGER NOT NULL,
            rows BLOB NOT NULL,
            updated_ms INTEGER NOT NULL,
            PRIMARY KEY(exchange, market, kind, day)
        );",
    )?;
    // Ретеншн при старте: суточные чанки старше лимита своего kind — под нож.
    let today = now_unix_ms() / DAY_MS;
    let mut del = conn.prepare("DELETE FROM chunks WHERE kind = ?1 AND day < ?2")?;
    for kind in [0u32, 1, 5, 30, 60, 240, 1440] {
        let _ = del.execute(rusqlite::params![kind, today - retention_days(kind)]);
    }
    Ok(())
}

fn run(conn: rusqlite::Connection, rx: mpsc::Receiver<Op>) {
    // Первый merge по ключу логируем INFO (разово): видно в обычном логе, что кэш живёт.
    let mut seen: std::collections::HashSet<(String, String, u32)> =
        std::collections::HashSet::new();
    while let Ok(op) = rx.recv() {
        match op {
            Op::Merge {
                exchange,
                market,
                kind_min,
                rows,
            } => {
                if let Err(e) = merge_rows(&conn, &exchange, &market, kind_min, &rows) {
                    log::warn!("kline cache merge failed {exchange}/{market}/{kind_min}: {e}");
                } else if seen.insert((exchange.clone(), market.clone(), kind_min)) {
                    log::info!(
                        "kline cache: первые ряды {exchange}/{market}/kind{kind_min}: {}",
                        rows.len()
                    );
                }
            }
            Op::Read {
                exchange,
                market,
                kind_min,
                from_ms,
                to_ms,
                reply,
            } => {
                let rows = read_rows(&conn, &exchange, &market, kind_min, from_ms, to_ms)
                    .unwrap_or_else(|e| {
                        log::warn!("kline cache read failed {exchange}/{market}/{kind_min}: {e}");
                        Vec::new()
                    });
                let _ = reply.send(rows);
            }
        }
    }
}

fn merge_rows(
    conn: &rusqlite::Connection,
    exchange: &str,
    market: &str,
    kind_min: u32,
    rows: &[ChartCandle],
) -> rusqlite::Result<()> {
    // Группируем по суткам; внутри дня merge по t_open (BTreeMap держит сортировку).
    let mut by_day: BTreeMap<i64, Vec<&ChartCandle>> = BTreeMap::new();
    for r in rows {
        if !(r.t_open_ms.is_finite() && r.t_open_ms > 0.0) || !(r.high > 0.0) {
            continue;
        }
        by_day.entry(r.t_open_ms as i64 / DAY_MS).or_default().push(r);
    }
    if by_day.is_empty() {
        return Ok(());
    }
    let now = now_unix_ms();
    let tx = conn.unchecked_transaction()?;
    for (day, day_rows) in by_day {
        let existing: Option<Vec<u8>> = tx
            .query_row(
                "SELECT rows FROM chunks WHERE exchange=?1 AND market=?2 AND kind=?3 AND day=?4",
                rusqlite::params![exchange, market, kind_min, day],
                |r| r.get(0),
            )
            .ok();
        let day_start = day * DAY_MS;
        let mut merged: BTreeMap<u32, ChartCandle> = BTreeMap::new();
        if let Some(blob) = existing {
            for c in unpack_rows(&blob, day_start) {
                merged.insert((c.t_open_ms as i64 - day_start) as u32, c);
            }
        }
        for r in day_rows {
            merged.insert((r.t_open_ms as i64 - day_start) as u32, r.clone());
        }
        let blob = pack_rows(merged.values(), day_start);
        tx.execute(
            "INSERT OR REPLACE INTO chunks(exchange, market, kind, day, rows, updated_ms)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![exchange, market, kind_min, day, blob, now],
        )?;
    }
    tx.commit()
}

fn read_rows(
    conn: &rusqlite::Connection,
    exchange: &str,
    market: &str,
    kind_min: u32,
    from_ms: i64,
    to_ms: i64,
) -> rusqlite::Result<Vec<ChartCandle>> {
    let mut stmt = conn.prepare(
        "SELECT day, rows FROM chunks
         WHERE exchange=?1 AND market=?2 AND kind=?3 AND day BETWEEN ?4 AND ?5
         ORDER BY day",
    )?;
    let mut out = Vec::new();
    let mut q = stmt.query(rusqlite::params![
        exchange,
        market,
        kind_min,
        from_ms / DAY_MS,
        to_ms / DAY_MS
    ])?;
    while let Some(row) = q.next()? {
        let day: i64 = row.get(0)?;
        let blob: Vec<u8> = row.get(1)?;
        for c in unpack_rows(&blob, day * DAY_MS) {
            let t = c.t_open_ms as i64;
            if t >= from_ms && t <= to_ms {
                out.push(c);
            }
        }
    }
    Ok(out)
}

fn pack_rows<'a>(rows: impl Iterator<Item = &'a ChartCandle>, day_start: i64) -> Vec<u8> {
    let mut out = Vec::new();
    for r in rows {
        out.extend_from_slice(&((r.t_open_ms as i64 - day_start) as u32).to_le_bytes());
        for v in [r.open, r.high, r.low, r.close, r.volume] {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

fn unpack_rows(blob: &[u8], day_start: i64) -> Vec<ChartCandle> {
    let mut out = Vec::with_capacity(blob.len() / ROW_BYTES);
    for chunk in blob.chunks_exact(ROW_BYTES) {
        let off = u32::from_le_bytes(chunk[0..4].try_into().unwrap());
        let f = |i: usize| f32::from_le_bytes(chunk[i..i + 4].try_into().unwrap());
        out.push(ChartCandle {
            t_open_ms: (day_start + off as i64) as f64,
            open: f(4),
            high: f(8),
            low: f(12),
            close: f(16),
            volume: f(20),
        });
    }
    out
}

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candle(t: f64, p: f32) -> ChartCandle {
        ChartCandle {
            t_open_ms: t,
            open: p,
            high: p + 1.0,
            low: p - 1.0,
            close: p + 0.5,
            volume: 10.0,
        }
    }

    #[test]
    fn pack_unpack_roundtrip() {
        let day_start = 19_000i64 * DAY_MS;
        let rows = vec![
            candle(day_start as f64, 5.0),
            candle((day_start + 60_000) as f64, 6.0),
        ];
        let blob = pack_rows(rows.iter(), day_start);
        assert_eq!(blob.len(), 2 * ROW_BYTES);
        let back = unpack_rows(&blob, day_start);
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].t_open_ms, rows[0].t_open_ms);
        assert_eq!(back[1].open, 6.0);
        assert_eq!(back[1].close, 6.5);
    }

    #[test]
    fn merge_dedups_and_read_filters() {
        let dir = std::env::temp_dir().join(format!("kline-cache-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("klines-test.sqlite");
        let _ = std::fs::remove_file(&path);
        let cache = KlineCache::open(path.clone()).expect("open cache");
        let day = (now_unix_ms() / DAY_MS) * DAY_MS;
        // Две записи с перекрытием: вторая заливка обновляет свечу t=day.
        cache.merge(
            "7:0".into(),
            "BTCUSDT".into(),
            1,
            vec![candle(day as f64, 5.0), candle((day + 60_000) as f64, 6.0)],
        );
        cache.merge("7:0".into(), "BTCUSDT".into(), 1, vec![candle(day as f64, 9.0)]);
        // Дать потоку прожевать очередь: read идёт тем же каналом, порядок сохранён.
        let rows = cache.read_range("7:0", "BTCUSDT", 1, day, day + DAY_MS);
        assert_eq!(rows.len(), 2, "дедуп по t_open внутри дня");
        assert_eq!(rows[0].open, 9.0, "поздняя заливка авторитетнее");
        // Чужой kind/рынок не видны.
        assert!(cache.read_range("7:0", "BTCUSDT", 5, day, day + DAY_MS).is_empty());
        assert!(cache.read_range("7:0", "ETHUSDT", 1, day, day + DAY_MS).is_empty());
        let _ = std::fs::remove_file(&path);
    }
}
