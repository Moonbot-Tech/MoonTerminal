//! How many Flat-column tiers a narrow Core Status dock drops.
//!
//! Dropping follows measured widths, in a fixed order. Core ping and the core name (which carries
//! the status dot) are never in a tier, so a dock that is only a few pixels wide still shows who
//! is offline and what their ping is.

/// Column ids removed together, cheapest tier first.
///
/// The strings are the Flat column ids from `table.rs::COLUMN_KEYS`: server CPU, startup,
/// free RAM, exchange ping, process memory, server name. Level `n` removes tiers `0..n`.
pub(super) const DROP_TIERS: [&[&str]; 3] = [
    &["cpu_sys", "startup"],
    &["free_phys", "ping_exch"],
    &["mem_used", "server"],
];

/// Smallest drop level at which the measured columns fit.
///
/// Level 0 removes nothing. Level 1 removes the first tier, level 2 the first two, level 3
/// every tier. A key that does not appear in `measured` contributes 0. The fit test is
/// `sum(widths of keys still shown) <= available + 0.5`, the half-pixel guard on the measuring
/// canvas. When even level 3 does not fit, the result is still 3: there is nothing further to
/// drop.
///
/// Args:
///     measured: Column id and measured width, in any order. Duplicate ids are summed.
///     available: Width the dock can give the table, in pixels.
///
/// Returns:
///     A level in `0..=3`.
pub(super) fn drop_level(measured: &[(&str, f32)], available: f32) -> usize {
    let fits = |level: usize| shown_width(measured, level) <= available + 0.5;
    (0..DROP_TIERS.len())
        .find(|&level| fits(level))
        .unwrap_or(DROP_TIERS.len())
}

/// Column ids that remain after removing tiers `0..level`.
///
/// Order follows `keys`. A level past the last tier removes every tier and nothing else.
/// Unknown ids are kept.
///
/// Args:
///     keys: Column ids in display order.
///     level: Drop level from [`drop_level`], or zero to keep every key.
///
/// Returns:
///     The ids that are not members of a removed tier.
pub(super) fn apply_drop<'a>(keys: &[&'a str], level: usize) -> Vec<&'a str> {
    keys.iter()
        .copied()
        .filter(|key| !tier_drops(key, level))
        .collect()
}

fn shown_width(measured: &[(&str, f32)], level: usize) -> f32 {
    measured
        .iter()
        .filter(|(key, _)| !tier_drops(key, level))
        .map(|(_, width)| *width)
        .sum()
}

fn tier_drops(key: &str, level: usize) -> bool {
    DROP_TIERS
        .iter()
        .take(level)
        .flat_map(|tier| tier.iter().copied())
        .any(|dropped| dropped == key)
}

#[cfg(test)]
mod tests;
