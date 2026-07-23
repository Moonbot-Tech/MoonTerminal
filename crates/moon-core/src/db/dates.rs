// ============================================================================
//  Date and time without external crates (cross-platform)
// ============================================================================

/// Convert Unix seconds to `YYYY-MM-DD HH:MM` in UTC; return empty for values <= 0.
pub fn fmt_unix(secs: i64) -> String {
    if secs <= 0 {
        return String::new();
    }
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi) = (rem / 3600, (rem % 3600) / 60);
    let (y, m, d) = crate::util::time::civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}")
}

/// unix-seconds → "YYYY-MM-DD" in UTC. EMPTY for <= 0 — callers rendering a date column
/// must turn that into their own "not known" marker rather than a blank cell.
///
/// A function of its own rather than a substring of [`fmt_unix`]: the time is part of that
/// one's contract, and a column slicing it off would drift silently on any format change.
pub fn fmt_unix_date(secs: i64) -> String {
    if secs <= 0 {
        return String::new();
    }
    let (y, m, d) = crate::util::time::civil_from_days(secs.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// Convert Unix seconds to `YYYY-MM-DD HH:MM:SS` in UTC for the command log.
pub fn fmt_unix_secs(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, m, d) = crate::util::time::civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

/// Convert `YYYY-MM-DD` to Unix seconds at the start of that UTC day.
pub fn parse_ymd(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut it = s.split('-');
    let y: i64 = it.next()?.trim().parse().ok()?;
    let m: i64 = it.next()?.trim().parse().ok()?;
    let d: i64 = it.next()?.trim().parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(days_from_civil(y, m, d) * 86_400)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
