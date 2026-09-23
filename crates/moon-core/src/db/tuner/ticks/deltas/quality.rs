//! How well the live deltas reproduce the core, over a sample of trades — what the tuner's
//! summary shows after a load, and what the `real_data` bench prints.
//!
//! The measure is the one thing the record can check: at the moment the report stamped its
//! deltas, the evaluation before the anchor against the report ([`super::StampCheck`]). The
//! anchor then puts every live field exactly on the report there, so this is not the error of the
//! values the model reads — it is how far the history the track stands on is from the core's own,
//! which is what the track's MOVES along the window inherit.

use super::DeltaTrack;
use super::field::DeltaField;

/// A stamp error at or under this counts as reproduced, per cent points.
pub const REPRODUCED_PP: f64 = 0.1;

/// One field over the sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldQuality {
    pub field: DeltaField,
    /// Trades whose track answers for the field.
    pub live: usize,
    /// The median share of the field's window the history covered at the stamp, 0 … 1.
    pub coverage_median: Option<f64>,
    /// Trades with an error at the stamp — the field evaluated, and filled in the report.
    pub checked: usize,
    /// Of those, the ones within [`REPRODUCED_PP`].
    pub reproduced: usize,
    /// The median error at the stamp, per cent points.
    pub error_median: Option<f64>,
}

/// Every field over the sample.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeltaQuality {
    /// Trades with a track at all.
    pub tracks: usize,
    /// One entry per field of [`DeltaField::ALL`], in its order.
    pub fields: Vec<FieldQuality>,
}

/// Sum the sample up.
///
/// Args:
///     tracks: The tracks of the sample's trades.
pub fn summarize<'a>(tracks: impl IntoIterator<Item = &'a DeltaTrack>) -> DeltaQuality {
    let mut count = 0usize;
    let mut live = [0usize; DeltaField::COUNT];
    let mut coverage: [Vec<f64>; DeltaField::COUNT] = std::array::from_fn(|_| Vec::new());
    let mut errors: [Vec<f64>; DeltaField::COUNT] = std::array::from_fn(|_| Vec::new());
    for track in tracks {
        count += 1;
        let stamp = track.stamp();
        for field in DeltaField::ALL {
            let i = field.index();
            if !track.is_live(field) {
                continue;
            }
            live[i] += 1;
            coverage[i].push(stamp.coverage[i]);
            if let Some(error) = stamp.error[i] {
                errors[i].push(error.abs());
            }
        }
    }
    let median = |values: &mut Vec<f64>| {
        values.sort_by(f64::total_cmp);
        values.get(values.len() / 2).copied()
    };
    let fields = DeltaField::ALL
        .iter()
        .map(|&field| {
            let i = field.index();
            let reproduced = errors[i].iter().filter(|&&e| e <= REPRODUCED_PP).count();
            FieldQuality {
                field,
                live: live[i],
                coverage_median: median(&mut coverage[i]),
                checked: errors[i].len(),
                reproduced,
                error_median: median(&mut errors[i]),
            }
        })
        .collect();
    DeltaQuality {
        tracks: count,
        fields,
    }
}
