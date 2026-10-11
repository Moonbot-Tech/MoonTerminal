//! Stream fixed public archive layouts, retaining only prints inside the requested spans.

use std::io::BufRead;

use super::ArchiveError;
use crate::feed::types::{Side, Tick};
use crate::market::trade_replay::Coverage;

/// Decode Binance spot or futures aggregate trades; return the number of retained prints.
/// Futures headers are optional, and spot microseconds are detected independently per row.
/// Malformed data returns `Corrupt`; input errors return `Transient`.
pub(crate) fn parse_binance_agg(
    r: impl BufRead,
    spans: &Coverage,
    out: &mut Vec<Tick>,
) -> Result<usize, ArchiveError> {
    let start = out.len();
    let mut first_row = true;
    for (index, line) in r.lines().enumerate() {
        let line = line.map_err(|error| ArchiveError::Transient(error.to_string()))?;
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() {
            continue;
        }
        let cells: Vec<&str> = line.split(',').collect();
        let header = first_row && cells[0].parse::<u64>().is_err();
        first_row = false;
        if header {
            continue;
        }
        let tick = binance_row(&cells).ok_or_else(|| malformed(index))?;
        if tick.qty != 0.0 && spans.contains_ms(tick.time_ms as i64) {
            out.push(tick);
        }
    }
    out[start..].sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
    Ok(out.len() - start)
}

/// Parse the fixed seven-column futures or eight-column spot layout without altering contracts.
fn binance_row(cells: &[&str]) -> Option<Tick> {
    if !matches!(cells.len(), 7 | 8) {
        return None;
    }
    // Validate identifiers too: a corrupted row must not become a covered but incomplete tape.
    for index in [0, 3, 4] {
        cells[index].parse::<u64>().ok()?;
    }
    let mut time_ms = cells[5].parse::<i64>().ok()?;
    if time_ms < 0 {
        return None;
    }
    if time_ms >= 100_000_000_000_000 {
        time_ms /= 1_000;
    }
    if cells.len() == 8 {
        boolean(cells[7])?;
    }
    Some(Tick {
        time_ms: time_ms as f64,
        price: number(cells[1])?,
        qty: number(cells[2])?,
        side: match boolean(cells[6])? {
            true => Side::Sell,
            false => Side::Buy,
        },
    })
}

/// Decode Bybit trading rows using header names, retaining base quantity and rounded milliseconds.
/// Missing columns or malformed rows return `Corrupt`; input errors return `Transient`.
pub(crate) fn parse_bybit_trading(
    mut r: impl BufRead,
    spans: &Coverage,
    out: &mut Vec<Tick>,
) -> Result<usize, ArchiveError> {
    let mut header = String::new();
    loop {
        header.clear();
        let count = r
            .read_line(&mut header)
            .map_err(|error| ArchiveError::Transient(error.to_string()))?;
        if count == 0 || !header.trim().trim_start_matches('\u{feff}').is_empty() {
            break;
        }
    }
    let columns: Vec<&str> = header
        .trim()
        .trim_start_matches('\u{feff}')
        .split(',')
        .collect();
    let column = |name| columns.iter().position(|&cell| cell == name);
    let time = column("timestamp").ok_or_else(|| malformed(0))?;
    let side = column("side").ok_or_else(|| malformed(0))?;
    let price = column("price").ok_or_else(|| malformed(0))?;
    // ASSUMED: older archives without homeNotional express base quantity in size.
    let qty = column("homeNotional")
        .or_else(|| column("size"))
        .ok_or_else(|| malformed(0))?;
    let start = out.len();
    for (index, line) in r.lines().enumerate() {
        let line = line.map_err(|error| ArchiveError::Transient(error.to_string()))?;
        if line.trim().is_empty() {
            continue;
        }
        let cells: Vec<&str> = line.trim().split(',').collect();
        let tick = bybit_row(&cells, columns.len(), time, side, price, qty)
            .ok_or_else(|| malformed(index + 1))?;
        if tick.qty != 0.0 && spans.contains_ms(tick.time_ms as i64) {
            out.push(tick);
        }
    }
    out[start..].sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
    Ok(out.len() - start)
}

/// Decode one Bybit row using the resolved indices, rejecting truncated and non-finite cells.
fn bybit_row(
    cells: &[&str],
    count: usize,
    time: usize,
    side: usize,
    price: usize,
    qty: usize,
) -> Option<Tick> {
    if cells.len() != count {
        return None;
    }
    let time_ms = (cells[time].parse::<f64>().ok()? * 1_000.0).round();
    if !time_ms.is_finite() || time_ms < 0.0 || time_ms >= i64::MAX as f64 {
        return None;
    }
    Some(Tick {
        time_ms,
        price: number(cells[price])?,
        qty: number(cells[qty])?,
        side: match cells[side] {
            "Buy" => Side::Buy,
            "Sell" => Side::Sell,
            _ => return None,
        },
    })
}

/// Accept only finite nonnegative quantities and prices, as archive prints must be numeric.
fn number(cell: &str) -> Option<f32> {
    let value = cell.parse::<f32>().ok()?;
    (value.is_finite() && value >= 0.0).then_some(value)
}

/// Accept the boolean spellings used by futures and spot archives.
fn boolean(cell: &str) -> Option<bool> {
    match cell {
        "true" | "True" => Some(true),
        "false" | "False" => Some(false),
        _ => None,
    }
}

/// Identify a malformed CSV row without copying public payloads into the diagnostic.
fn malformed(index: usize) -> ArchiveError {
    ArchiveError::Corrupt(format!("malformed archive CSV row {}", index + 1))
}

#[cfg(test)]
mod tests;
