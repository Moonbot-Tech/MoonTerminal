//! Pure colour decision for a chart's arrival border, independent of pulse scheduling.

/// Return the core's colour made readable against `background`, or `accent` when the core has none.
///
/// A readable pick is painted as is; an unreadable one is lifted by lightness with its hue kept
/// (`core_color::readable`), never swapped for the accent — a swap made every pale core on the
/// light theme and every dark core on the dark one the same colour, which is exactly the identity
/// the setting exists to give. The accent is left for the one case with nothing to lift: a core
/// whose server is gone from the config.
pub(super) fn arrival_color(core: Option<[u8; 3]>, background: [u8; 3], accent: u32) -> u32 {
    match core {
        Some(core) => crate::design::rgb_to_u32(crate::core_color::readable(core, background)),
        None => accent,
    }
}

#[cfg(test)]
mod tests;
