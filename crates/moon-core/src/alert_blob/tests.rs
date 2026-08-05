use super::*;

// Live samples from Moonbot (core 1 log, 2026-07-04).
const MINA_HLINE: &str = "010d000000fff8f0ffb563c53f0100000001000000009ede3c6eec8fe640000000000000000000009c0cf1ad47c30674d5d4b2b5be48a83f0000";
const BTC_SEGMENT: &str = "020d000000fff8f0ff9cfabd3f010000000100000000cbfb25afec8fe640000000000000000000005c5923c2ec47b217e7541714ea8fe6404df899d30adcee40d36e9fdeeb8fe64001969f155bddee40";
// Type 4 = TRIANGLE (3 vertices); type 5 = Moonbot CHANNEL (2 horizontal prices).
const TRIANGLE: &str = "040d00000040c4ffff0000803f01000000010000000006a506def18fe6400000000000000000000010000000000000008c15ce11f18fe64000000000948fb03fe0791a8cf18fe640000000802dcab13f8c15ce11f18fe6401fbd30c9a947b23f";
const CHANNEL: &str = "050d000000fff8f0ff1608e33f0100000001000000009f840ee9f18fe6400000000000000000000083ed837d5ea1c4fde57e87a2409fb03fce8de9094b3cb03f0000";
// Horizontal line WITH A STRATEGY (@32 strategy_id=7394783480262116308).
const HLINE_STRAT: &str = "010d000000fff8f0ffb563c53f010000000100000000760ab83af28fe6400000d443f363658c9f66ef5557f77024dded6eddcd531d72b33f0000";
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
