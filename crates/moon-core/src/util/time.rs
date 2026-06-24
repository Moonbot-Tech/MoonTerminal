//! Единый источник unix-времени. До рефактора каждый модуль (feed/live, feed/synth,
//! session/order_lines, applog, db) держал свою копию `now_ms` — одна и та же формула
//! `SystemTime::now() - UNIX_EPOCH` в пяти местах. Свели сюда: f64-мс для шкалы тиков
//! чарта, i64-мс для логов/БД.

use std::time::{SystemTime, UNIX_EPOCH};

/// Текущее unix-время в мс (f64). Та же шкала, что `time_ms` тиков рынка.
pub fn now_unix_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// Текущее unix-время в целых мс (i64) — для меток логов и записей БД.
pub fn now_unix_ms_i64() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
