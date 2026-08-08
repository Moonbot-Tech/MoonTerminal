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

// Moonbot's own Fibonacci (type 3, 145 bytes), captured live from core 17 «QQ» on ETHUSD_PERP,
// 2026-08-05. Three different geometries, one of them the same object upserted twice with a
// strategy attached — which is how @32 was identified.
const MB_FIB_A: &str = "030d000000fff8f0ff0000803f010000000100000000f746d566f893e640000000000000000000007b68ac514b9ed3b12219274dfce79b40c76e73c9358a9c404e754131f593e64000000000000000000000000000000000002219274dfce79b407af99042450e9c40c8539890f4259c40f4434d0b19399c406217b1853d4c9c40aecd417a7e679c4006408abe7eb09c40";
const MB_FIB_B: &str = "030d000000fff8f0ff0000803f010000000100000000cc0915cef893e64000000000000000000000bee001c0d1f10397cb7e1eb26cac9c406362215c2d099f40e0b24482f593e6400000000000000000000000000000000000cb7e1eb26cac9c40ac25c16d253b9d40269d4ea970939d4097f01f07cdda9d40b3e3c26329229e40957c8898c2879e4023678e16e6979f40";
const MB_FIB_STRAT: &str = "030d000000fff8f0ff0000803f010000000100000000cee06542fa93e640000063e465ae763c0c688789efaa4ccfadcdfe7ddda452f59c40e3795d9872b69e40f498bcb7f693e6400000000000000000000000000000000000fe7ddda452f59c40d229f9f7505f9d4006a60f75e3a09d40f07b9d9ee2d59d40e1c14ac7e10a9e408ec73fbe55569e40083293ea70209f40";

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

/// The tools the core knows are registered in ITS order — line, segment, fibo, triangle, channel.
///
/// The toolbar shows that group in registry order and calls it "the ones Moonbot draws too", so a
/// reorder there silently changes what a Moonbot user reads. The oracle is the type byte the core
/// itself assigns, not a list repeated in the test.
#[test]
fn alertable_tools_are_registered_in_the_cores_type_order() {
    use crate::figures::FigNode;
    // DISTINCT nodes: a tool may refuse a degenerate figure, and a ratio scale with no height is
    // one — every level would collapse onto a single price.
    let nodes =
        |n: usize| -> Vec<FigNode> { (0..n).map(|i| FigNode::new(1.0, 2.0 + i as f64)).collect() };
    let types: Vec<u8> = crate::figures::tools::REGISTRY
        .iter()
        .filter(|d| d.alertable)
        .map(|d| {
            let kind = (d.make)(&nodes(d.clicks as usize)).expect("full node set builds");
            let blob = encode(&kind, [1, 2, 3, 4], 1.0, LineKind::Dash, 0.0, 0, 1)
                .expect("an alertable tool encodes");
            blob[0]
        })
        .collect();
    let mut sorted = types.clone();
    sorted.sort_unstable();
    assert_eq!(
        types, sorted,
        "registry order no longer follows the blob types"
    );
}

/// The registry's `alertable` flag and this codec must agree: a tool marked alertable that the
/// blob refuses would arm locally and send nothing, and a blob for a tool the core does not know
/// would make it draw something else.
#[test]
fn every_alertable_tool_encodes_and_no_other_one_does() {
    use crate::figures::FigNode;
    for def in crate::figures::tools::REGISTRY {
        // Distinct nodes, for the same reason as above.
        // Prices differ, times do not: a sub-millisecond time cannot survive the TDateTime hop
        // exactly, and this test is about the KIND surviving, not about the clock.
        let nodes: Vec<FigNode> = (0..def.clicks)
            .map(|i| FigNode::new(1_700_000_000_000.0, 100.0 + i as f64))
            .collect();
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

/// A Moonbot Fibonacci survives decode-then-encode BYTE FOR BYTE.
///
/// The strongest check this codec can have, and the only one that proves the layout rather than
/// asserting it: the bytes are Moonbot's, all 145 of them, including the three constant fields at
/// @72/@80/@88 that nothing here interprets. A misread offset, a dropped tail or a rewritten
/// constant all show up as a differing byte.
///
/// It covers the CODEC, not the whole terminal round trip: the production path stores the creation
/// instant as whole milliseconds on the way through, which this test does not model. That hop has
/// its own test below.
#[test]
fn a_moonbot_fibonacci_round_trips_byte_for_byte() {
    for hx in [MB_FIB_A, MB_FIB_B, MB_FIB_STRAT] {
        let orig = bytes(hx);
        let d = decode(&orig).expect("a fibo decodes");
        assert!(matches!(d.kind, FigureKind::MbFib(_)), "type 3 is a fibo");
        let enc = encode(
            &d.kind,
            d.color,
            d.thickness,
            d.line_kind,
            d.created_ms,
            d.strategy_id,
            d.uid,
        )
        .expect("a fibo encodes");
        assert_eq!(enc, orig, "fibo blob changed on the round trip: {hx}");
    }
}

/// The seven level prices are read in Moonbot's own order and none is invented.
///
/// The oracle is the ratio set visible on the Moonbot chart that produced the sample; a shifted
/// offset would still decode seven finite prices, so length alone proves nothing.
#[test]
fn a_fibonacci_carries_seven_levels_at_moonbots_ratios() {
    let FigureKind::MbFib(f) = decode(&bytes(MB_FIB_A)).unwrap().kind else {
        panic!("type 3 is a fibo");
    };
    assert_eq!(f.levels.len(), 7);
    let want = [0.0, 0.236, 0.382, 0.5, 0.618, 0.786, 1.236];
    for (price, want) in f.levels.iter().zip(want) {
        let got = f.ratio_of(*price).expect("the move has height");
        assert!((got - want).abs() < 1e-6, "level {price} reads as {got}");
    }
    // The anchors are the move itself, not two more levels.
    assert_eq!(f.a, f.levels[0], "ratio 0 sits on the move's start");
    assert!(
        f.b > f.levels[5] && f.b < f.levels[6],
        "ratio 1 is between .786 and 1.236"
    );
}

/// A truncated fibo is refused rather than read past its end.
///
/// A fibo is refused unless it is EXACTLY the one shape sampled from Moonbot — reading part of an
/// unknown one and writing 145 bytes back would delete the rest from Moonbot's copy. The checked
/// accessors behind the guard are exercised too, by a blob of the right length full of nothing.
#[test]
fn a_truncated_fibonacci_is_refused() {
    let full = bytes(MB_FIB_A);
    for cut in [56, 89, 100, 144] {
        assert!(
            decode(&full[..cut]).is_none(),
            "a fibo cut to {cut} bytes decoded anyway"
        );
    }
    assert!(decode(&full).is_some(), "the whole blob still decodes");
    // Right length, wrong everything: refused for its TYPE, without reading past its end.
    assert!(
        decode(&vec![0u8; full.len()]).is_none(),
        "type 0 is not a figure"
    );
}

/// What the terminal actually sends back after an edit, creation instant included.
///
/// The codec speaks `f64` while the figure store keeps whole milliseconds, so a re-upsert cannot be
/// byte-identical at @22 in general — this pins how far off it is allowed to be, and that NOTHING
/// ELSE moves. Rounding rather than truncating is what keeps the error centred: truncation walked
/// an object Moonbot stamped on an exact millisecond a whole millisecond backwards.
#[test]
fn the_terminals_own_round_trip_moves_only_the_creation_instant_and_only_by_rounding() {
    for hx in [MB_FIB_A, MB_FIB_B, MB_FIB_STRAT] {
        let orig = bytes(hx);
        let d = decode(&orig).expect("a fibo decodes");
        // Exactly what `sync_remote_alerts` stores and `figure_blob` sends back.
        let stored_ms = d.created_ms.round() as i64;
        let enc = encode(
            &d.kind,
            d.color,
            d.thickness,
            d.line_kind,
            stored_ms as f64,
            d.strategy_id,
            d.uid,
        )
        .expect("a fibo encodes");
        assert_eq!(enc.len(), orig.len());
        assert_eq!(&enc[..22], &orig[..22], "header before @22 moved: {hx}");
        assert_eq!(&enc[30..], &orig[30..], "everything after @22 moved: {hx}");
        let back = decode(&enc).expect("what we send we can read");
        assert!(
            (back.created_ms - d.created_ms).abs() <= 0.5,
            "creation instant moved by more than a rounded millisecond: {hx}"
        );
    }
}
