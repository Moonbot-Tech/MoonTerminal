use super::*;

// Live samples from Moonbot (core 1 log, 2026-07-04).
const MINA_HLINE: &str = "010d000000fff8f0ffb563c53f0100000001000000009ede3c6eec8fe640000000000000000000009c0cf1ad47c30674d5d4b2b5be48a83f0000";
const BTC_SEGMENT: &str = "020d000000fff8f0ff9cfabd3f010000000100000000cbfb25afec8fe640000000000000000000005c5923c2ec47b217e7541714ea8fe6404df899d30adcee40d36e9fdeeb8fe64001969f155bddee40";
// Type 4 = TRIANGLE (3 vertices); type 5 = Moonbot CHANNEL (2 horizontal prices).
// The TRIANGLE is the one sample NOT drawn in Moonbot: `uid = 16` is a sequential `FigureStore`
// id where every Moonbot-drawn sample here carries a random u64, and its colour and thickness are
// byte-for-byte `DrawStyle::default()`. It is one of our own figures echoed back by the core,
// encoded before the colour order was fixed — usable for geometry, but it says nothing about what
// colour Moonbot picks, and its `40 c4 ff ff` is our own sky blue mis-sent.
const TRIANGLE: &str = "040d00000040c4ffff0000803f01000000010000000006a506def18fe6400000000000000000000010000000000000008c15ce11f18fe64000000000948fb03fe0791a8cf18fe640000000802dcab13f8c15ce11f18fe6401fbd30c9a947b23f";
const CHANNEL: &str = "050d000000fff8f0ff1608e33f0100000001000000009f840ee9f18fe6400000000000000000000083ed837d5ea1c4fde57e87a2409fb03fce8de9094b3cb03f0000";
// Horizontal line WITH A STRATEGY (@32 strategy_id=7394783480262116308).
const HLINE_STRAT: &str = "010d000000fff8f0ffb563c53f010000000100000000760ab83af28fe6400000d443f363658c9f66ef5557f77024dded6eddcd531d72b33f0000";
// The sample that settles the colour byte order: a line drawn in Moonbot whose picker read
// `#FF5000F4`, arriving as `f4 00 50 ff` (core 17 «QQ», ETHUSD_PERP, 2026-08-05 19:41:50 UTC).
//
// The oracle is the picker's rendered SWATCH, not its hex text — the text alone would be circular,
// since its own byte order is the question. The swatch was a blue-violet, which is `#5000F4`;
// `#F40050`, what the other reading gives, is a hot pink. Corroborated independently by the seven
// other Moonbot-drawn samples' `ff f8 f0`: under this order that is `#F0F8FF`, Delphi's own
// `TColors.Aliceblue` ($00FFF8F0), while reading the bytes in order names no colour at all.
const HLINE_VIOLET: &str = "010d000000f40050ff7a9ee73f01000000010000000004126243fa93e640000063e465ae763c0c68d5e7b728c8272e8a295c8fc2f5229f400000";
// Horizontal lines with different Kind values (@13): 0=Solid, 2=Dot, 4=DashDotDot.
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
        FigureKind::HLine(_)
    ));
    assert!(matches!(
        decode(&bytes(BTC_SEGMENT)).unwrap().kind,
        FigureKind::Segment(_)
    ));
    assert!(matches!(
        decode(&bytes(TRIANGLE)).unwrap().kind,
        FigureKind::Triangle(_)
    ));
    assert!(matches!(
        decode(&bytes(CHANNEL)).unwrap().kind,
        FigureKind::Channel(_)
    ));
}

#[test]
fn decode_strategy_id() {
    let d = decode(&bytes(HLINE_STRAT)).unwrap();
    assert_eq!(d.strategy_id, 7394783480262116308);
}

/// The wire keeps a colour in Delphi's BGRA order, so decoding must hand back RGBA.
///
/// The oracle is the colour a human read off Moonbot's own picker, not anything this codec
/// computes: `#FF5000F4`. Reading the bytes in order — what the codec did before the sample —
/// yields `[f4, 00, 50, ff]`, a crimson where the user drew a violet.
#[test]
fn decode_swaps_red_and_blue_out_of_the_wires_bgra() {
    let d = decode(&bytes(HLINE_VIOLET)).unwrap();
    assert_eq!(d.color, [0x50, 0x00, 0xF4, 0xFF]);
}

/// Encoding puts the swap back, so a figure sent to Moonbot is drawn in the colour it was given.
#[test]
fn encode_writes_the_colour_back_as_bgra() {
    let blob = encode(
        &FigureKind::HLine(HLine { price: 1.0 }),
        [0x50, 0x00, 0xF4, 0xFF],
        1.0,
        LineKind::Dash,
        0.0,
        0,
        1,
    )
    .expect("an hline is alertable");
    assert_eq!(&blob[5..9], &[0xF4, 0x00, 0x50, 0xFF]);
}

/// The alpha slot survives decoding untouched, whatever it holds.
///
/// Not a formality: substituting a value here would be written straight back to the core by the
/// first drag of the figure, so anything this codec cannot read it must at least not overwrite.
#[test]
fn the_alpha_byte_is_passed_through_and_never_substituted() {
    for a in [0x00u8, 0x7F, 0xFF] {
        let mut b = bytes(HLINE_VIOLET);
        b[8] = a;
        assert_eq!(decode(&b).unwrap().color[3], a, "alpha {a:#04x}");
    }
}

/// A thickness the wire cannot mean never reaches the vertex builder.
#[test]
fn an_unusable_thickness_falls_back_to_the_model_default() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -3.0] {
        let mut b = bytes(HLINE_VIOLET);
        b[9..13].copy_from_slice(&bad.to_le_bytes());
        assert_eq!(
            decode(&b).unwrap().thickness,
            DrawStyle::default().thickness,
            "thickness {bad}"
        );
    }
}

/// A finite but absurd width is pulled back to something drawable rather than passed on.
#[test]
fn an_absurd_thickness_is_clamped_into_range() {
    for (raw, want) in [(1e30f32, MAX_THICKNESS), (1e-40, MIN_THICKNESS)] {
        let mut b = bytes(HLINE_VIOLET);
        b[9..13].copy_from_slice(&raw.to_le_bytes());
        assert_eq!(decode(&b).unwrap().thickness, want, "thickness {raw}");
    }
}

#[test]
fn decode_line_kind() {
    assert_eq!(
        decode(&bytes(HLINE_SOLID)).unwrap().line_kind,
        LineKind::Solid
    );
    assert_eq!(decode(&bytes(HLINE_DOT)).unwrap().line_kind, LineKind::Dot);
    assert_eq!(
        decode(&bytes(HLINE_DDD)).unwrap().line_kind,
        LineKind::DashDotDot
    );
    // Default samples (@13=1) = Dash.
    assert_eq!(
        decode(&bytes(MINA_HLINE)).unwrap().line_kind,
        LineKind::Dash
    );
}

/// Selected header fields (type, kind, uid, color, strategy_id, line_kind) are encoded byte-for-byte.
#[test]
fn encode_header_byte_exact() {
    for hx in [
        MINA_HLINE,
        BTC_SEGMENT,
        TRIANGLE,
        CHANNEL,
        HLINE_STRAT,
        HLINE_SOLID,
        HLINE_DOT,
        HLINE_VIOLET,
    ] {
        let orig = bytes(hx);
        let d = decode(&orig).unwrap();
        let enc = encode(
            &d.kind,
            d.color,
            d.thickness,
            d.line_kind,
            d.created_ms,
            d.strategy_id,
            d.uid,
        )
        .expect("every sampled type is encodable");
        assert_eq!(enc.len(), orig.len(), "len {hx}");
        assert_eq!(&enc[0..1], &orig[0..1], "type {hx}");
        assert_eq!(&enc[1..5], &orig[1..5], "kind {hx}");
        assert_eq!(&enc[5..9], &orig[5..9], "color {hx}");
        assert_eq!(&enc[13..17], &orig[13..17], "line_kind {hx}");
        assert_eq!(&enc[32..40], &orig[32..40], "strategy_id {hx}");
        assert_eq!(&enc[40..48], &orig[40..48], "uid {hx}");
    }
}

/// Selected decoded values survive an encode-decode round trip.
#[test]
fn roundtrip_values() {
    for hx in [
        MINA_HLINE,
        BTC_SEGMENT,
        TRIANGLE,
        CHANNEL,
        HLINE_STRAT,
        HLINE_DOT,
        HLINE_VIOLET,
    ] {
        let d1 = decode(&bytes(hx)).unwrap();
        let blob = encode(
            &d1.kind,
            d1.color,
            d1.thickness,
            d1.line_kind,
            d1.created_ms,
            d1.strategy_id,
            d1.uid,
        )
        .expect("every sampled type is encodable");
        let d2 = decode(&blob).unwrap();
        assert_eq!(d1.uid, d2.uid);
        assert_eq!(d1.strategy_id, d2.strategy_id);
        assert_eq!(d1.line_kind, d2.line_kind);
        assert_eq!(d1.color, d2.color);
        assert_eq!(d1.kind, d2.kind, "kind {hx}");
    }
}

/// The registry's `alertable` flag and this codec must agree: a tool marked alertable that the
/// blob refuses would arm locally and send nothing, and a blob for a tool the core does not know
/// would make it draw something else.
#[test]
fn every_alertable_tool_encodes_and_no_other_one_does() {
    use crate::figures::FigNode;
    let node = FigNode::new(1_700_000_000_000.0, 100.0);
    for def in crate::figures::tools::REGISTRY {
        let nodes = vec![node; def.clicks as usize];
        let kind = (def.make)(&nodes).expect("full node set must build");
        let blob = encode(&kind, [1, 2, 3, 4], 1.0, LineKind::Dash, 0.0, 0, 1);
        assert_eq!(
            blob.is_some(),
            def.alertable,
            "{} disagrees with its alertable flag",
            def.key
        );
        if let Some(blob) = blob {
            let back = decode(&blob).expect("what we encode we must decode");
            assert_eq!(back.kind, kind, "{} does not survive the wire", def.key);
        }
    }
}
