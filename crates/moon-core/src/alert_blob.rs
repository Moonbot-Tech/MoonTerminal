//! Кодек blob'а chart-объекта Moonbot (`TChartObject.Save()`) — им chart-алерты
//! ездят в ядро (`upsert`). Формат восстановлен реверсом 6 живых сэмплов
//! (см. заметку chart-alerts-research). Всё little-endian.
//!
//! Раскладка (заголовок 48 байт + payload по типу):
//! ```text
//! @0   u8   тип фигуры (1=горизонталь, 2=отрезок, 3=fibo, 4=параллельные)
//! @1   u32  kind = 13 (объект-алерт)
//! @5   [u8;4] цвет
//! @9   f32  толщина линии
//! @13  u32  вид линии (TPenStyle): 0=Solid,1=Dash,2=Dot,3=DashDot,4=DashDotDot
//! @17  u8   = 1
//! @18  u32  = 0
//! @22  f64  TDateTime создания (дни Delphi)
//! @30  u16  = 0
//! @32  u64  strategy_id (0 = без стратегии; для алертов — id стратегии вида «Alerts»)
//! @40  u64  obj_uid
//! @48  payload по типу:
//!        hline(1)    = цена f64 + u16 0
//!        segment(2)  = 2×(t,цена)
//!        triangle(4) = 3×(t,цена)  — три вершины
//!        channel(5)  = 2×цена f64 + u16 0  — две горизонтальные цены (без времени)
//! ```
//! Узел = `(TDateTime f64, цена f64)`. Fibo (тип 3) в нашей модели фигур пока нет —
//! `decode` его пропускает, `encode` для него не вызывается.

use crate::figures::{FigNode, FigureKind, LineKind};

/// Тип фигуры в blob'е.
const T_HLINE: u8 = 1;
const T_SEGMENT: u8 = 2;
const T_FIBO: u8 = 3;
const T_TRIANGLE: u8 = 4;
const T_CHANNEL: u8 = 5;

/// `kind` объекта-алерта (во всех сэмплах = 13).
const KIND_ALERT: u32 = 13;

/// Начало payload'а (после 40-байтного заголовка + 8-байтного uid).
const PAYLOAD_OFF: usize = 48;

/// Delphi TDateTime: дни с 1899-12-30. Unix-эпоха = 25569-й день.
const DELPHI_UNIX_DAYS: f64 = 25569.0;
const MS_PER_DAY: f64 = 86_400_000.0;

fn unix_ms_to_tdatetime(ms: f64) -> f64 {
    ms / MS_PER_DAY + DELPHI_UNIX_DAYS
}

fn tdatetime_to_unix_ms(dt: f64) -> f64 {
    (dt - DELPHI_UNIX_DAYS) * MS_PER_DAY
}

/// Раскодированный chart-объект (для отображения серверных алертов / round-trip).
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedAlert {
    pub kind: FigureKind,
    pub color: [u8; 4],
    pub thickness: f32,
    /// Вид линии (@13 TPenStyle).
    pub line_kind: LineKind,
    pub created_ms: f64,
    /// Привязанная стратегия (id; 0 = без стратегии).
    pub strategy_id: u64,
    pub uid: u64,
}

/// Собрать blob фигуры-алерта для `upsert`. `created_ms` — unix-время создания,
/// `line_kind` — вид линии (@13), `strategy_id` — привязанная стратегия (0 = без),
/// `uid` — тот же obj_uid.
#[allow(clippy::too_many_arguments)]
pub fn encode(
    kind: &FigureKind,
    color: [u8; 4],
    thickness: f32,
    line_kind: LineKind,
    created_ms: f64,
    strategy_id: u64,
    uid: u64,
) -> Vec<u8> {
    let ty = match kind {
        FigureKind::HLine { .. } => T_HLINE,
        FigureKind::Segment { .. } => T_SEGMENT,
        FigureKind::Triangle { .. } => T_TRIANGLE,
        FigureKind::Channel { .. } => T_CHANNEL,
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
        FigureKind::HLine { price } => {
            out.extend_from_slice(&price.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // хвост (во всех hline-сэмплах = 0)
        }
        FigureKind::Segment { a, b } => {
            node(a, &mut out);
            node(b, &mut out);
        }
        FigureKind::Triangle { a, b, c } => {
            node(a, &mut out);
            node(b, &mut out);
            node(c, &mut out);
        }
        FigureKind::Channel { price1, price2 } => {
            out.extend_from_slice(&price1.to_le_bytes());
            out.extend_from_slice(&price2.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
        }
    }
    out
}

/// Разобрать blob chart-объекта. `None` — слишком короткий или тип, которого нет в
/// нашей модели фигур (fibo).
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
        T_HLINE => FigureKind::HLine {
            price: rd_f64(PAYLOAD_OFF)?,
        },
        T_SEGMENT => FigureKind::Segment {
            a: rd_node(PAYLOAD_OFF)?,
            b: rd_node(PAYLOAD_OFF + 16)?,
        },
        T_TRIANGLE => FigureKind::Triangle {
            a: rd_node(PAYLOAD_OFF)?,
            b: rd_node(PAYLOAD_OFF + 16)?,
            c: rd_node(PAYLOAD_OFF + 32)?,
        },
        T_CHANNEL => FigureKind::Channel {
            price1: rd_f64(PAYLOAD_OFF)?,
            price2: rd_f64(PAYLOAD_OFF + 8)?,
        },
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
