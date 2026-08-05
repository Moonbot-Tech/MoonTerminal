//! Codec for the Moonbot chart-object blob (`TChartObject.Save()`) used to send chart alerts
//! to the core (`upsert`). The little-endian format was reverse-engineered from six live
//! samples.
//!
//! Layout (48-byte header + type-specific payload):
//! ```text
//! @0   u8   figure type (1=horizontal line, 2=segment, 3=fibo, 4=triangle, 5=channel)
//! @1   u32  kind = 13 (alert object)
//! @5   [u8;4] color
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
//! Node = `(TDateTime f64, price f64)`. Fibo (type 3) is not yet part of our figure model —
//! `decode` skips it, and `encode` is never called for it.

use crate::figures::tools::{Channel, HLine, Segment, Triangle};
use crate::figures::{FigNode, FigureKind, LineKind};

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

/// Delphi TDateTime: days since 1899-12-30. The Unix epoch is day 25569.
const DELPHI_UNIX_DAYS: f64 = 25569.0;
const MS_PER_DAY: f64 = 86_400_000.0;

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
    };
    let mut out = Vec::with_capacity(96);
    out.push(ty);
    out.extend_from_slice(&KIND_ALERT.to_le_bytes());
    out.extend_from_slice(&color);
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
    }
    Some(out)
}

/// Decodes a chart-object blob. Returns `None` if it is too short or has a type absent from
/// our figure model (fibo).
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
    let color = [blob[5], blob[6], blob[7], blob[8]];
    let thickness = f32::from_le_bytes(blob[9..13].try_into().ok()?);
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
        T_FIBO => return None,
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
