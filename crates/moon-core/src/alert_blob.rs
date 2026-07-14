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
mod tests {
    use super::*;

    // Живые сэмплы из Moonbot (лог core 1, 2026-07-04).
    const MINA_HLINE: &str = "010d000000fff8f0ffb563c53f0100000001000000009ede3c6eec8fe640000000000000000000009c0cf1ad47c30674d5d4b2b5be48a83f0000";
    const BTC_SEGMENT: &str = "020d000000fff8f0ff9cfabd3f010000000100000000cbfb25afec8fe640000000000000000000005c5923c2ec47b217e7541714ea8fe6404df899d30adcee40d36e9fdeeb8fe64001969f155bddee40";
    // Тип 4 = ТРЕУГОЛЬНИК (3 вершины); тип 5 = КАНАЛ Moonbot (2 горизонтальные цены).
    const TRIANGLE: &str = "040d00000040c4ffff0000803f01000000010000000006a506def18fe6400000000000000000000010000000000000008c15ce11f18fe64000000000948fb03fe0791a8cf18fe640000000802dcab13f8c15ce11f18fe6401fbd30c9a947b23f";
    const CHANNEL: &str = "050d000000fff8f0ff1608e33f0100000001000000009f840ee9f18fe6400000000000000000000083ed837d5ea1c4fde57e87a2409fb03fce8de9094b3cb03f0000";
    // Горизонталь СО СТРАТЕГИЕЙ (@32 strategy_id=7394783480262116308).
    const HLINE_STRAT: &str = "010d000000fff8f0ffb563c53f010000000100000000760ab83af28fe6400000d443f363658c9f66ef5557f77024dded6eddcd531d72b33f0000";
    // Горизонтали разных Kind (@13): 0=Solid, 2=Dot, 4=DashDotDot.
    const HLINE_SOLID: &str = "010d000000fff8f0ffb563c53f00000000010000000063a69ffbf38fe6400000d443f363658c9f66bdd58d12d65717635feffe78af5ab13f0000";
    const HLINE_DOT: &str = "010d000000fff8f0ffb563c53f020000000100000000e86971fcf38fe6400000d443f363658c9f665be3d2c76ed3acae0d1afa27b858b13f0000";
    const HLINE_DDD: &str = "010d000000fff8f0ffb563c53f04000000010000000074e0eafdf38fe6400000d443f363658c9f6626640e165467db1274efe192e34eb13f0000";

    fn bytes(hx: &str) -> Vec<u8> {
        (0..hx.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hx[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn decode_kinds() {
        assert!(matches!(
            decode(&bytes(MINA_HLINE)).unwrap().kind,
            FigureKind::HLine { .. }
        ));
        assert!(matches!(
            decode(&bytes(BTC_SEGMENT)).unwrap().kind,
            FigureKind::Segment { .. }
        ));
        assert!(matches!(
            decode(&bytes(TRIANGLE)).unwrap().kind,
            FigureKind::Triangle { .. }
        ));
        assert!(matches!(
            decode(&bytes(CHANNEL)).unwrap().kind,
            FigureKind::Channel { .. }
        ));
    }

    #[test]
    fn decode_strategy_id() {
        let d = decode(&bytes(HLINE_STRAT)).unwrap();
        assert_eq!(d.strategy_id, 7394783480262116308);
    }

    #[test]
    fn decode_line_kind() {
        assert_eq!(decode(&bytes(HLINE_SOLID)).unwrap().line_kind, LineKind::Solid);
        assert_eq!(decode(&bytes(HLINE_DOT)).unwrap().line_kind, LineKind::Dot);
        assert_eq!(
            decode(&bytes(HLINE_DDD)).unwrap().line_kind,
            LineKind::DashDotDot
        );
        // Дефолтные сэмплы (@13=1) = Dash.
        assert_eq!(decode(&bytes(MINA_HLINE)).unwrap().line_kind, LineKind::Dash);
    }

    /// Заголовок (тип, kind, uid, цвет, strategy_id, line_kind) кодируется байт-в-байт.
    #[test]
    fn encode_header_byte_exact() {
        for hx in [MINA_HLINE, BTC_SEGMENT, TRIANGLE, CHANNEL, HLINE_STRAT, HLINE_SOLID, HLINE_DOT]
        {
            let orig = bytes(hx);
            let d = decode(&orig).unwrap();
            let enc = encode(
                &d.kind, d.color, d.thickness, d.line_kind, d.created_ms, d.strategy_id, d.uid,
            );
            assert_eq!(enc.len(), orig.len(), "len {hx}");
            assert_eq!(&enc[0..1], &orig[0..1], "type {hx}");
            assert_eq!(&enc[1..5], &orig[1..5], "kind {hx}");
            assert_eq!(&enc[5..9], &orig[5..9], "color {hx}");
            assert_eq!(&enc[13..17], &orig[13..17], "line_kind {hx}");
            assert_eq!(&enc[32..40], &orig[32..40], "strategy_id {hx}");
            assert_eq!(&enc[40..48], &orig[40..48], "uid {hx}");
        }
    }

    /// encode∘decode — взаимно обратны.
    #[test]
    fn roundtrip_values() {
        for hx in [MINA_HLINE, BTC_SEGMENT, TRIANGLE, CHANNEL, HLINE_STRAT, HLINE_DOT] {
            let d1 = decode(&bytes(hx)).unwrap();
            let d2 = decode(&encode(
                &d1.kind, d1.color, d1.thickness, d1.line_kind, d1.created_ms, d1.strategy_id,
                d1.uid,
            ))
            .unwrap();
            assert_eq!(d1.uid, d2.uid);
            assert_eq!(d1.strategy_id, d2.strategy_id);
            assert_eq!(d1.line_kind, d2.line_kind);
            assert_eq!(d1.color, d2.color);
            assert_eq!(d1.kind, d2.kind, "kind {hx}");
        }
    }
}
