//! A deal's prints as the table keeps them between replays.
//!
//! The table holds every fit row's tape so the variant columns and the search can replay it on
//! demand, and a [`Tick`] is 24 bytes: an `f64` time, the price, the quantity and the side. The
//! model reads the time, the price and the side, never the quantity — so between replays a
//! print is kept as 8: its offset from the tape's first print in milliseconds and its price,
//! the side in the price's sign bit. Measured on this machine's reports (2026-09-23, 265
//! MoonShot deals): 48.9 MiB of held prints as `Tick`s, 16.3 MiB packed.
//!
//! A tape that cannot be packed without loss — a fractional millisecond, a span past what a
//! `u32` of milliseconds reaches (49 days), a price the sign bit cannot carry — is kept as it
//! came. Unpacking is done off the UI thread, right before a replay, and lives only as long as
//! that replay.

use std::collections::HashMap;
use std::sync::Arc;

use moon_core::db::tuner::ticks::Deal;
use moon_core::db::tuner::ticks::search::{PreparedDeal, clip_to_horizon, common_horizon_ms};
use moon_core::feed::types::{Side, Tick};

/// One deal's prints, packed where that loses nothing.
#[derive(Clone, Debug)]
pub(in crate::analytics::tuner) enum PackedTape {
    /// Offsets in ms from `base_ms` and prices, a sell stored as the negated price.
    Packed {
        base_ms: i64,
        prints: Arc<[(u32, f32)]>,
    },
    /// The prints as they came, for a tape [`PackedTape::pack`] could not take.
    Plain(Arc<[Tick]>),
}

impl PackedTape {
    /// Keep `ticks`, packed when every print survives it unchanged.
    pub(in crate::analytics::tuner) fn pack(ticks: Vec<Tick>) -> Self {
        match packed(&ticks) {
            Some((base_ms, prints)) => Self::Packed {
                base_ms,
                prints: prints.into(),
            },
            None => Self::Plain(ticks.into()),
        }
    }

    /// How many prints the tape holds.
    pub(in crate::analytics::tuner) fn len(&self) -> usize {
        match self {
            Self::Packed { prints, .. } => prints.len(),
            Self::Plain(ticks) => ticks.len(),
        }
    }

    /// What the tape takes in memory, in bytes — what the table's budget is counted in.
    pub(in crate::analytics::tuner) fn bytes(&self) -> usize {
        match self {
            Self::Packed { prints, .. } => std::mem::size_of_val(&prints[..]),
            Self::Plain(ticks) => std::mem::size_of_val(&ticks[..]),
        }
    }

    /// The prints as the model replays them. A packed tape's quantities come back as zero: the
    /// model never reads one, and a caller that ever needs them must hold the tape [`Plain`].
    ///
    /// [`Plain`]: PackedTape::Plain
    pub(in crate::analytics::tuner) fn unpack(&self) -> Arc<[Tick]> {
        match self {
            Self::Packed { base_ms, prints } => prints
                .iter()
                .map(|&(offset, signed)| Tick {
                    time_ms: (base_ms + i64::from(offset)) as f64,
                    price: signed.abs(),
                    qty: 0.0,
                    side: if signed.is_sign_negative() {
                        Side::Sell
                    } else {
                        Side::Buy
                    },
                })
                .collect(),
            Self::Plain(ticks) => Arc::clone(ticks),
        }
    }
}

/// A replayable row taken off the table with its tape still packed — cheap to take on the UI
/// thread; [`PendingDeal::prepare`] unpacks it off it.
#[derive(Clone, Debug)]
pub(in crate::analytics::tuner) struct PendingDeal {
    pub(in crate::analytics::tuner) deal: Deal,
    pub(in crate::analytics::tuner) tape: PackedTape,
    pub(in crate::analytics::tuner) entry_line: Option<Arc<[(i64, f64)]>>,
    pub(in crate::analytics::tuner) trail_ms: i64,
    pub(in crate::analytics::tuner) own: Arc<HashMap<String, String>>,
}

impl PendingDeal {
    /// The deal as the model replays it, its tape unpacked.
    pub(in crate::analytics::tuner) fn prepare(self) -> PreparedDeal {
        PreparedDeal {
            ticks: self.tape.unpack(),
            deal: self.deal,
            entry_line: self.entry_line,
            trail_ms: self.trail_ms,
            own: self.own,
        }
    }
}

/// A sample as the columns and the search replay it: every tape unpacked and cut at the
/// sample's one exit horizon (`clip_to_horizon`), so no variant is judged on more tape than
/// another. Off the UI thread: it is the whole sample's prints at full width.
pub(in crate::analytics::tuner) fn prepare_sample(pending: Vec<PendingDeal>) -> Vec<PreparedDeal> {
    let mut deals: Vec<PreparedDeal> = pending.into_iter().map(PendingDeal::prepare).collect();
    if let Some(horizon_ms) = common_horizon_ms(&deals) {
        clip_to_horizon(&mut deals, horizon_ms);
    }
    deals
}

/// `ticks` packed, or `None` when a print would not come back as it went in.
fn packed(ticks: &[Tick]) -> Option<(i64, Vec<(u32, f32)>)> {
    let base = ticks.first()?.time_ms;
    if base.fract() != 0.0 || !base.is_finite() {
        return None;
    }
    let base_ms = base as i64;
    ticks
        .iter()
        .map(|t| {
            // Whole milliseconds only: the tile store keeps integers, and anything else would be
            // rounded away here.
            let offset = t.time_ms - base;
            let price_ok = t.price.is_finite() && t.price > 0.0;
            (offset.fract() == 0.0 && (0.0..=f64::from(u32::MAX)).contains(&offset) && price_ok)
                .then(|| {
                    let signed = match t.side {
                        Side::Buy => t.price,
                        Side::Sell => -t.price,
                    };
                    (offset as u32, signed)
                })
        })
        .collect::<Option<Vec<_>>>()
        .map(|prints| (base_ms, prints))
}

#[cfg(test)]
mod tests;
