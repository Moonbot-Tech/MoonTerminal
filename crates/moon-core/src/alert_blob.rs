//! Codec for the Moonbot chart-object blob (`TChartObject.Save()`) used to send chart alerts
//! to the core (`upsert`). The little-endian format was reverse-engineered from six live
//! samples.
//!
//! Layout (48-byte header + type-specific payload):
//! ```text
//! @0   u8   figure type (1=horizontal line, 2=segment, 3=fibo, 4=triangle, 5=channel)
//! @1   u32  kind = 13 (alert object)
//! @5   [u8;4] color: the `$AARRGGBB` word little-endian, i.e. B,G,R,A — NOT RGBA (see `swap_rb`)
//! @9   f32  line thickness
//! @13  u32  line kind (TPenStyle): 0=Solid,1=Dash,2=Dot,3=DashDot,4=DashDotDot
//! @17  u8   = 1
//! @18  u32  = 0
//! @22  f64  creation TDateTime (Delphi days)
//! @30  u16  = 0
//! @32  u64  strategy_id (0 = no strategy; alerts use the id of the "Alerts" strategy)
//! @40  u64  obj_uid
//! @48  type-specific payload:
//!        hline(1)    = price f64 + u16 0
//!        segment(2)  = 2×(t,price)
//!        triangle(4) = 3×(t,price)  — three vertices
//!        channel(5)  = 2×price f64 + u16 0  — two horizontal prices (without time)
//! ```
//! Node = `(TDateTime f64, price f64)`. Fibo (type 3), 145 bytes, reads as:
//! ```text
//! @48  f64   price A — the level the scale calls zero
//! @56  f64   price B — the level it calls one
//! @64  f64   TDateTime — the ONLY time in the object
//! @72  f64   0          (unchanged across samples of different geometry)
//! @80  f64   0          (likewise)
//! @88  u8    0          (likewise)
//! @89  7×f64 the drawn LEVEL PRICES, ratios exactly 0, .236, .382, .5, .618, .786, 1.236
//! ```
//! The arithmetic closes exactly: 48 + 5×8 + 1 + 7×8 = 145. Confirmed by four live samples
//! (ETHUSD_PERP, 2026-08-05):
//! - the level COUNT is fixed at seven, but the ratios are NOT fixed: the samples sit at
//!   0/.236/.382/.5/.618/.786/**1.236** while a Moonbot chart drawn later showed the seventh at
//!   **1.618**. That is why the object stores prices and not ratios — the set is a user setting,
//!   and the prices are the only thing that survives it. Read them as given; do not re-derive them
//!   from our own scale, which has eleven levels of its own;
//! - the figure spans the WHOLE chart: no start, no end, the levels run into the order book. The
//!   one time at @64 is where it was drawn from, not an edge — which is why there is no second
//!   time to find;
//! - `@32` is the strategy id for this type as well — the same object upserted twice differs in
//!   those eight bytes and nowhere else once a strategy is attached.
//!
//! Both directions are implemented for it, into [`crate::figures::tools::MbFib`] — a tool of its
//! own rather than a reading of ours, for the reasons in that module.

use crate::figures::tools::{Channel, HLine, MB_FIB_LEVELS, MbFib, Segment, Triangle};
use crate::figures::{DrawStyle, FigNode, FigureKind, LineKind};

/// Figure type in the blob.
const T_HLINE: u8 = 1;
const T_SEGMENT: u8 = 2;
const T_FIBO: u8 = 3;
const T_TRIANGLE: u8 = 4;
const T_CHANNEL: u8 = 5;

/// Alert-object `kind` (= 13 in every sample).
const KIND_ALERT: u32 = 13;

/// Start of the payload (after the 40-byte header + 8-byte uid).
const PAYLOAD_OFF: usize = 48;

/// Start of a fibo's level prices: the payload's two anchors, its one time, two zero `f64`s and a
/// zero byte. `48 + 5×8 + 1 = 89`, and `89 + 7×8 = 145` closes the sampled length exactly.
const MB_FIB_LEVELS_OFF: usize = PAYLOAD_OFF + 5 * 8 + 1;

/// The whole of a fibo object. A blob of any other length is a shape this codec has not seen.
const MB_FIB_LEN: usize = MB_FIB_LEVELS_OFF + MB_FIB_LEVELS * 8;

/// Widths a line can be drawn at. Hairlines and skyscrapers both come off the wire from another
/// process; neither is a width, and the vertex builder has no opinion about either.
const MIN_THICKNESS: f32 = 0.1;
const MAX_THICKNESS: f32 = 20.0;

/// Delphi TDateTime: days since 1899-12-30. The Unix epoch is day 25569.
const DELPHI_UNIX_DAYS: f64 = 25569.0;
const MS_PER_DAY: f64 = 86_400_000.0;

/// Converts a colour between the wire's BGRA and our RGBA by swapping red and blue.
///
/// The blob writes the colour as one `$AARRGGBB` word, little-endian, which lands in memory as
/// B, G, R, A. Measured, not assumed: a line drawn in Moonbot with its picker reading `#FF5000F4`
/// (violet) arrives as `f4 00 50 ff`. Reading those bytes in order turns every red into a blue and
/// back, which is exactly what this codec did until the sample settled it.
///
/// The same `$AARRGGBB` word is what Moonbot's INI export writes in its HEX form, so the two
/// readers agree there — see `config::moonbot_import::plan::parse_tcolor`, whose decimal branch is
/// a separate question this says nothing about.
///
/// The swap is its own inverse, so one function serves both directions and the two can never
/// disagree on red and blue. The FOURTH byte is passed through untouched and is `ff` in every
/// sample taken so far, so what Moonbot does with a partly transparent colour is untested.
fn swap_rb(c: [u8; 4]) -> [u8; 4] {
    [c[2], c[1], c[0], c[3]]
}

fn unix_ms_to_tdatetime(ms: f64) -> f64 {
    ms / MS_PER_DAY + DELPHI_UNIX_DAYS
}

fn tdatetime_to_unix_ms(dt: f64) -> f64 {
    (dt - DELPHI_UNIX_DAYS) * MS_PER_DAY
}

/// Decoded chart object (for displaying server alerts / round trips).
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedAlert {
    pub kind: FigureKind,
    /// RGBA — the wire's BGRA is already swapped back by `decode` (see [`swap_rb`]), so this is
    /// the same convention as everywhere else in the figure model.
    pub color: [u8; 4],
    pub thickness: f32,
    /// Line kind (@13 TPenStyle).
    pub line_kind: LineKind,
    pub created_ms: f64,
    /// Associated strategy (id; 0 = no strategy).
    pub strategy_id: u64,
    pub uid: u64,
}

/// Builds an alert-figure blob for `upsert`. `created_ms` is the Unix creation time,
/// `line_kind` is the line kind (@13), `strategy_id` is the associated strategy (0 = none),
/// and `uid` is the same obj_uid.
#[allow(clippy::too_many_arguments)]
pub fn encode(
    kind: &FigureKind,
    color: [u8; 4],
    thickness: f32,
    line_kind: LineKind,
    created_ms: f64,
    strategy_id: u64,
    uid: u64,
) -> Option<Vec<u8>> {
    // A tool the core has no chart-object type for gets no blob at all: returning one of the
    // wrong type would have the core draw something else entirely. The registry flag is the
    // single source of that truth, checked HERE so no caller can bypass it.
    if !kind.tool().def().alertable {
        return None;
    }
    let ty = match kind {
        FigureKind::HLine(_) => T_HLINE,
        FigureKind::Segment(_) => T_SEGMENT,
        FigureKind::Triangle(_) => T_TRIANGLE,
        FigureKind::Channel(_) => T_CHANNEL,
        FigureKind::MbFib(_) => T_FIBO,
        // OUR Fibonacci is a different object from Moonbot's: eleven ratios between two points
        // against seven stored prices across the whole chart. Sending one as the other would have
        // the core draw something the user did not draw, so it stays local. See `MbFib`.
        // Local-only kinds. A ray has no core type at all: the format's five are hline, segment,
        // fibo, triangle and zone, and none of them is half-infinite.
        FigureKind::FibRetracement(_)
        | FigureKind::Rect(_)
        | FigureKind::Ray(_)
        | FigureKind::Position(_) => return None,
    };
    let mut out = Vec::with_capacity(96);
    out.push(ty);
    out.extend_from_slice(&KIND_ALERT.to_le_bytes());
    out.extend_from_slice(&swap_rb(color)); // @5 BGRA on the wire.
    out.extend_from_slice(&thickness.to_le_bytes());
    out.extend_from_slice(&line_kind.to_pen().to_le_bytes()); // @13 TPenStyle
    out.push(1u8); // @17
    out.extend_from_slice(&0u32.to_le_bytes()); // @18
    out.extend_from_slice(&unix_ms_to_tdatetime(created_ms).to_le_bytes()); // @22
    out.extend_from_slice(&0u16.to_le_bytes()); // @30
    out.extend_from_slice(&strategy_id.to_le_bytes()); // @32
    out.extend_from_slice(&uid.to_le_bytes()); // @40
    debug_assert_eq!(out.len(), PAYLOAD_OFF);
    let node = |n: &FigNode, out: &mut Vec<u8>| {
        out.extend_from_slice(&unix_ms_to_tdatetime(n.time_ms).to_le_bytes());
        out.extend_from_slice(&n.price.to_le_bytes());
    };
    match kind {
        FigureKind::HLine(HLine { price }) => {
            out.extend_from_slice(&price.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // Tail (= 0 in every hline sample).
        }
        FigureKind::Segment(Segment { a, b }) => {
            node(a, &mut out);
            node(b, &mut out);
        }
        FigureKind::Triangle(Triangle { a, b, c }) => {
            node(a, &mut out);
            node(b, &mut out);
            node(c, &mut out);
        }
        FigureKind::Channel(Channel { price1, price2 }) => {
            out.extend_from_slice(&price1.to_le_bytes());
            out.extend_from_slice(&price2.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
        }
        FigureKind::MbFib(f) => {
            out.extend_from_slice(&f.a.to_le_bytes()); // @48
            out.extend_from_slice(&f.b.to_le_bytes()); // @56
            out.extend_from_slice(&unix_ms_to_tdatetime(f.time_ms).to_le_bytes()); // @64
            // @72, @80 and @88 are zero in every sample, including across samples whose geometry
            // differs, so they are written back as the constants they were read as rather than
            // guessed at.
            out.extend_from_slice(&0f64.to_le_bytes());
            out.extend_from_slice(&0f64.to_le_bytes());
            out.push(0u8);
            for price in f.levels {
                out.extend_from_slice(&price.to_le_bytes());
            }
        }
        // Unreachable: the type byte above already refused these kinds.
        FigureKind::FibRetracement(_)
        | FigureKind::Rect(_)
        | FigureKind::Ray(_)
        | FigureKind::Position(_) => return None,
    }
    Some(out)
}

/// Decodes a chart-object blob. Returns `None` if it is too short, if a fibo is not exactly the
/// length this codec knows, or if the type is one our figure model has no tool for.
pub fn decode(blob: &[u8]) -> Option<DecodedAlert> {
    if blob.len() < PAYLOAD_OFF + 8 {
        return None;
    }
    let rd_f64 = |off: usize| -> Option<f64> {
        blob.get(off..off + 8)
            .map(|s| f64::from_le_bytes(s.try_into().unwrap()))
    };
    let rd_node = |off: usize| -> Option<FigNode> {
        Some(FigNode {
            time_ms: tdatetime_to_unix_ms(rd_f64(off)?),
            price: rd_f64(off + 8)?,
        })
    };
    let ty = blob[0];
    // The alpha slot is passed through as it arrives. `ff` in every sample so far, and what a `00`
    // would mean is unknown — an alpha of zero, or a Delphi `TColor` with no alpha field at all.
    // Substituting opaque was tried and reverted: `decode` feeds `encode`, so the substitute would
    // be written back to the core on the first drag, replacing a byte we do not understand with one
    // we invented. A figure that draws invisibly is a bug to find; a rewritten Moonbot object is a
    // bug to find in someone else's program.
    let color = swap_rb([blob[5], blob[6], blob[7], blob[8]]);
    // Thickness, unlike the alpha, has values that cannot mean anything: a non-finite, non-positive
    // or absurd width reaches the vertex builder and draws a line nobody can see, indistinguishable
    // from a figure that never arrived. Substituting there repairs rather than invents, so the
    // write-back that ruled the alpha out is acceptable here.
    let thickness = match f32::from_le_bytes(blob[9..13].try_into().ok()?) {
        t if t.is_finite() && t > 0.0 => t.clamp(MIN_THICKNESS, MAX_THICKNESS),
        _ => DrawStyle::default().thickness,
    };
    let line_kind = LineKind::from_pen(u32::from_le_bytes(blob[13..17].try_into().ok()?));
    let created_ms = tdatetime_to_unix_ms(rd_f64(22)?);
    let strategy_id = u64::from_le_bytes(blob[32..40].try_into().ok()?);
    let uid = u64::from_le_bytes(blob[40..48].try_into().ok()?);
    let kind = match ty {
        T_HLINE => FigureKind::HLine(HLine {
            price: rd_f64(PAYLOAD_OFF)?,
        }),
        T_SEGMENT => FigureKind::Segment(Segment {
            a: rd_node(PAYLOAD_OFF)?,
            b: rd_node(PAYLOAD_OFF + 16)?,
        }),
        T_TRIANGLE => FigureKind::Triangle(Triangle {
            a: rd_node(PAYLOAD_OFF)?,
            b: rd_node(PAYLOAD_OFF + 16)?,
            c: rd_node(PAYLOAD_OFF + 32)?,
        }),
        T_CHANNEL => FigureKind::Channel(Channel {
            price1: rd_f64(PAYLOAD_OFF)?,
            price2: rd_f64(PAYLOAD_OFF + 8)?,
        }),
        T_FIBO => {
            // Refused unless it is EXACTLY the shape sampled from Moonbot. The ratio set behind the
            // levels is a user setting on that side, so a build that offers a different count would
            // send a longer or shorter object; reading the first seven of a longer one and encoding
            // 145 bytes back would delete the rest from Moonbot's own copy on the first edit here.
            // A figure that fails to appear is a bug to notice; a silently truncated one is not.
            if blob.len() != MB_FIB_LEN {
                return None;
            }
            let mut levels = [0f64; MB_FIB_LEVELS];
            for (i, level) in levels.iter_mut().enumerate() {
                *level = rd_f64(MB_FIB_LEVELS_OFF + i * 8)?;
            }
            FigureKind::MbFib(MbFib {
                a: rd_f64(PAYLOAD_OFF)?,
                b: rd_f64(PAYLOAD_OFF + 8)?,
                time_ms: tdatetime_to_unix_ms(rd_f64(PAYLOAD_OFF + 16)?),
                levels,
            })
        }
        _ => return None,
    };
    Some(DecodedAlert {
        kind,
        color,
        thickness,
        line_kind,
        created_ms,
        strategy_id,
        uid,
    })
}

#[cfg(test)]
mod tests;
