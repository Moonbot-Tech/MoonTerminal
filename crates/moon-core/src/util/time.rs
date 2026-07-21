//! Единый источник unix-времени. До рефактора каждый модуль (feed/live, feed/synth,
//! session/order_lines, applog, db) держал свою копию `now_ms` — одна и та же формула
//! `SystemTime::now() - UNIX_EPOCH` в пяти местах. Свели сюда: f64-мс для шкалы тиков
//! чарта, i64-мс для логов/БД.
//!
//! Здесь же живёт единственная в крейте реализация григорианского календаря
//! (`civil_from_days`) — её потребляют `db` (форматирование меток отчётов),
//! `strat_db` (краткая подпись версии) и `config::backup` (имя папки снапшота).

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

/// Перевести дни от unix epoch в `(год, месяц, день)` пролептического григорианского календаря.
///
/// Алгоритм civil-from-days Говарда Хиннанта, единственная копия в крейте. Через неё работают
/// `db::fmt_unix*`, `strat_db::stats::short_date` и `config::backup`, поэтому дата в отчёте,
/// подписи версии стратегии и имени папки снимка не расходится.
pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Нижняя метка `utc_stamp_compact` для времени до года 0.
const STAMP_MIN: &str = "00000101-000000";
/// Верхняя метка `utc_stamp_compact` для времени после года 9999.
const STAMP_MAX: &str = "99991231-235959";

/// Преобразовать unix-мс в допустимую для имени файла UTC-метку `YYYYMMDD-HHMMSS`.
///
/// Два свойства обязательны для `config::backup`:
/// - **Допустимость в Windows**, где `:` запрещён, поэтому форма `HH:MM:SS` не подходит.
/// - **Лексикографический порядок равен хронологическому** благодаря фиксированной ширине,
///   ведущим нулям и старшим компонентам слева. Поэтому чистка сортирует снимки по ИМЕНИ,
///   не используя mtime, который меняют копирование и облачная синхронизация.
///
/// Используется UTC, а не локальное время: при переходе с летнего времени локальные часы идут
/// назад на час и нарушают порядок.
///
/// Поддерживаются годы 0000-9999. `{y:04}` задаёт минимальную, а не фиксированную ширину, поэтому
/// год за пределами диапазона изменил бы ДЛИНУ строки и нарушил оба свойства. Такое время
/// прижимается к граничной метке вместо выдачи неверного имени.
pub fn utc_stamp_compact(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days);
    if y < 0 {
        return STAMP_MIN.to_string();
    }
    if y > 9999 {
        return STAMP_MAX.to_string();
    }
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

#[cfg(test)]
mod tests {
    //! Контракт компактной UTC-метки для имён папок снимков.

    use super::{utc_stamp_compact, STAMP_MAX, STAMP_MIN};

    /// Метка имеет фиксированную ширину и разделитель на фиксированной позиции.
    ///
    /// Возможная поломка: перейти ради читаемости на `DD.MM.YYYY_HH-MM-SS`, используемый в
    /// `analytics/period.rs` и `analytics/toolbar.rs`. В нём первым идёт день, поэтому чистка по
    /// имени начала бы удалять по числу месяца, а не по дате.
    #[test]
    fn the_stamp_is_fixed_width_with_the_separator_at_a_fixed_offset() {
        for ms in [0_i64, 1, 1_753_100_000_000, 4_102_444_800_000] {
            let s = utc_stamp_compact(ms);
            assert_eq!(s.len(), 15, "stamp `{s}` is not 15 chars");
            assert_eq!(&s[8..9], "-", "stamp `{s}` has no separator at index 8");
            assert!(
                s.bytes()
                    .enumerate()
                    .all(|(i, b)| i == 8 || b.is_ascii_digit()),
                "stamp `{s}` holds a non-digit outside index 8"
            );
        }
    }

    /// Unix epoch отображается известной константой, фиксируя и календарное преобразование.
    ///
    /// Оракул независим от кода: 1970-01-01T00:00:00Z — определение unix epoch, а не значение,
    /// прочитанное обратно из функции.
    #[test]
    fn the_epoch_renders_as_the_start_of_1970() {
        assert_eq!(utc_stamp_compact(0), "19700101-000000");
    }

    /// Сортировка меток как СТРОК совпадает с сортировкой исходных моментов времени.
    ///
    /// На этом свойстве чистка снимков оставляет новейшие N.
    ///
    /// Моменты выбраны так, чтобы ЧИСЛО МЕСЯЦА уменьшалось на границе (31 янв -> 1 фев).
    /// Это отличает форматы: при росте числа `DD.MM.YYYY_HH-MM-SS` случайно сортируется верно,
    /// а здесь даёт `31.01.2026` > `01.02.2026` и красит тест. Удаление ведущих нулей также
    /// разворачивает эту пару.
    #[test]
    fn string_order_matches_chronological_order_across_a_month_boundary() {
        // 2026-01-31 12:00:00Z, затем следующий день.
        let jan31 = 1_769_860_800_000_i64;
        let feb01 = jan31 + 86_400_000;

        let a = utc_stamp_compact(jan31);
        let b = utc_stamp_compact(feb01);

        // Фиксирует и преобразование, чтобы ошибочная фикстура не прошла проверку случайно.
        assert_eq!(a, "20260131-120000");
        assert_eq!(b, "20260201-120000");
        assert!(
            a < b,
            "{a} must sort before {b} despite the smaller day number"
        );
    }

    /// Время вне годов 0000-9999 прижимается к границе, не меняя длину имени.
    ///
    /// Имя из 16 символов отвергается читателем снимков и неверно сортируется рядом с 15-символьным;
    /// отрицательный год добавляет ведущий `-`, который идёт ДО цифр и делает старую запись новой.
    #[test]
    fn a_year_outside_the_supported_range_clamps_instead_of_changing_width() {
        assert_eq!(utc_stamp_compact(i64::MIN / 2), STAMP_MIN);
        assert_eq!(utc_stamp_compact(i64::MAX / 2), STAMP_MAX);
        assert_eq!(STAMP_MIN.len(), 15);
        assert_eq!(STAMP_MAX.len(), 15);
    }
}
