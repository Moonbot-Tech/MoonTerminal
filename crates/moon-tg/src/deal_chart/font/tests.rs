use super::*;

#[test]
fn both_faces_parse() {
    assert!(faces().mono.is_some(), "Geist Mono");
    assert!(faces().cjk.is_some(), "the CJK cut");
}

#[test]
fn latin_cyrillic_and_hanzi_resolve_to_their_faces() {
    let faces = faces();
    for ch in ['A', '0', '%', 'Ж', 'й'] {
        assert_eq!(faces.resolve(ch).map(|r| r.0), Some(Which::Mono), "{ch}");
    }
    for ch in ['币', '安', '人', '生', 'ア'] {
        assert_eq!(faces.resolve(ch).map(|r| r.0), Some(Which::Cjk), "{ch}");
    }
    // A character neither face has falls back to a question mark, not to nothing.
    assert_eq!(faces.resolve('\u{1F680}'), faces.resolve('?'));
}

#[test]
fn a_monospaced_face_gives_every_digit_one_advance() {
    let text = Text::default();
    let one = text.width("0", 20.0);
    assert!(one > 5.0 && one < 20.0, "{one}");
    assert!((text.width("0123456789", 20.0) - 10.0 * one).abs() < 1e-3);
    assert!(
        text.width("币", 20.0) > one,
        "a hanzi is wider than a digit"
    );
}

#[test]
fn simple_and_composite_glyphs_have_outlines() {
    let face = faces().mono.as_ref().unwrap();
    for ch in ['H', 'o', 'é', 'Й'] {
        let glyph = face.glyph(ch).unwrap();
        assert!(!face.outline(glyph).is_empty(), "{ch}");
    }
    assert!(face.outline(face.glyph(' ').unwrap()).is_empty());
}

#[test]
fn a_letter_fills_its_stems() {
    let faces = faces();
    let (which, glyph) = faces.resolve('H').unwrap();
    let mask = glyph_mask(faces, which, glyph, 40.0).unwrap();
    let full = mask.cover.iter().filter(|c| **c > 0.99).count();
    let partial = mask
        .cover
        .iter()
        .filter(|c| **c > 0.0 && **c < 0.99)
        .count();
    assert!(full > 50, "{full} solid pixels");
    assert!(partial > 10, "{partial} antialiased pixels");
    assert!(mask.top < 0, "an H stands above its baseline");
}

#[test]
fn a_damaged_face_reads_as_missing_glyphs_not_a_panic() {
    for cut in [0, 10, 200, 5_000, MONO.len() / 2] {
        let face = Face::parse(&MONO[..cut]);
        if let Some(face) = face {
            for ch in ['A', '0', 'é'] {
                if let Some(glyph) = face.glyph(ch) {
                    let _ = face.outline(glyph);
                    let _ = face.advance(glyph);
                }
            }
        }
    }
    let mut garbage = MONO.to_vec();
    for byte in garbage.iter_mut().skip(400).step_by(7) {
        *byte = byte.wrapping_mul(31).wrapping_add(17);
    }
    if let Some(face) = Face::parse(&garbage) {
        for code in 0x20u32..0x500 {
            if let Some(glyph) = char::from_u32(code).and_then(|ch| face.glyph(ch)) {
                let _ = face.outline(glyph);
            }
        }
    }
}
