use super::*;

/// Serialization test helper that mirrors the Delphi writer for unit tests.
pub(crate) fn push_string(out: &mut Vec<u8>, s: &str) {
    let units: Vec<u16> = s.encode_utf16().collect();
    out.extend_from_slice(&(units.len() as i32).to_le_bytes());
    for u in units {
        out.extend_from_slice(&u.to_le_bytes());
    }
}

#[test]
fn primitives_roundtrip() {
    let mut buf = Vec::new();
    buf.push(1u8); // bool true
    buf.push(0u8); // bool false
    buf.extend_from_slice(&0xBEEFu16.to_le_bytes());
    buf.extend_from_slice(&(-42i32).to_le_bytes());
    buf.extend_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
    buf.extend_from_slice(&1.5f32.to_le_bytes());
    buf.extend_from_slice(&(-2.25f64).to_le_bytes());
    let mut r = Reader::new(&buf);
    assert!(r.bool("b1").unwrap());
    assert!(!r.bool("b2").unwrap());
    assert_eq!(r.u16_le("w").unwrap(), 0xBEEF);
    assert_eq!(r.i32_le("i").unwrap(), -42);
    assert_eq!(r.u32_le("c").unwrap(), 0xDEAD_BEEF);
    assert_eq!(r.f32_le("f").unwrap(), 1.5);
    assert_eq!(r.f64_le("d").unwrap(), -2.25);
    assert!(r.is_empty());
}

#[test]
fn invalid_bool_rejected() {
    let buf = [2u8];
    assert!(matches!(
        Reader::new(&buf).bool("x"),
        Err(ImportError::BadValue(_))
    ));
}

#[test]
fn truncated_primitive() {
    let buf = [0u8; 3];
    assert!(matches!(
        Reader::new(&buf).i32_le("x"),
        Err(ImportError::Truncated(_))
    ));
}

#[test]
fn string_roundtrip_including_cyrillic() {
    let mut buf = Vec::new();
    push_string(&mut buf, "Привет, 世界! ok");
    push_string(&mut buf, "");
    let mut r = Reader::new(&buf);
    assert_eq!(r.string_x("s").unwrap(), "Привет, 世界! ok");
    assert_eq!(r.string_x("e").unwrap(), "");
    assert!(r.is_empty());
}

#[test]
fn negative_and_oversized_string_len() {
    let mut buf = Vec::new();
    buf.extend_from_slice(&(-1i32).to_le_bytes());
    assert!(matches!(
        Reader::new(&buf).string_x("s"),
        Err(ImportError::BadValue(_))
    ));
    let mut buf = Vec::new();
    buf.extend_from_slice(&(MAX_STRING_UNITS as i32 + 1).to_le_bytes());
    assert!(matches!(
        Reader::new(&buf).string_x("s"),
        Err(ImportError::TooLarge(_))
    ));
    // The length is valid but data is absent: Truncated WITHOUT allocating its buffer.
    let mut buf = Vec::new();
    buf.extend_from_slice(&1000i32.to_le_bytes());
    assert!(matches!(
        Reader::new(&buf).string_x("s"),
        Err(ImportError::Truncated(_))
    ));
}

#[test]
fn invalid_utf16_rejected() {
    // Lone high surrogate 0xD800.
    let mut buf = Vec::new();
    buf.extend_from_slice(&1i32.to_le_bytes());
    buf.extend_from_slice(&0xD800u16.to_le_bytes());
    assert!(matches!(
        Reader::new(&buf).string_x("s"),
        Err(ImportError::BadValue(_))
    ));
}

#[test]
fn ini_roundtrip() {
    let mut buf = Vec::new();
    buf.extend_from_slice(&2i32.to_le_bytes());
    push_string(&mut buf, "ColorsLight");
    buf.extend_from_slice(&2i32.to_le_bytes());
    push_string(&mut buf, "graphBK");
    push_string(&mut buf, "16777215");
    push_string(&mut buf, "CandleGreen");
    push_string(&mut buf, "65280");
    push_string(&mut buf, "Empty");
    buf.extend_from_slice(&0i32.to_le_bytes());
    let mut r = Reader::new(&buf);
    let sections = r.ini_list("theme").unwrap();
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0].name, "ColorsLight");
    assert_eq!(
        sections[0].entries[0],
        ("graphBK".to_string(), "16777215".to_string())
    );
    assert_eq!(sections[1].name, "Empty");
    assert!(sections[1].entries.is_empty());
    assert!(r.is_empty());
}

#[test]
fn ini_entry_count_vs_remaining() {
    // Declares 1,000 entries with no data, so the count sanity check returns Truncated.
    let mut buf = Vec::new();
    buf.extend_from_slice(&1i32.to_le_bytes());
    push_string(&mut buf, "S");
    buf.extend_from_slice(&1000i32.to_le_bytes());
    assert!(matches!(
        Reader::new(&buf).ini_list("ini"),
        Err(ImportError::Truncated(_))
    ));
}

#[test]
fn ini_limits() {
    let mut buf = Vec::new();
    buf.extend_from_slice(&(MAX_INI_SECTIONS as i32 + 1).to_le_bytes());
    assert!(matches!(
        Reader::new(&buf).ini_list("ini"),
        Err(ImportError::TooLarge(_))
    ));
    let mut buf = Vec::new();
    buf.extend_from_slice(&(-5i32).to_le_bytes());
    assert!(matches!(
        Reader::new(&buf).ini_list("ini"),
        Err(ImportError::BadValue(_))
    ));
}

#[test]
fn sub_reader_bounds() {
    let buf = [1u8, 2, 3, 4, 5];
    let mut r = Reader::new(&buf);
    let mut sub = r.sub_reader(3, "block").unwrap();
    assert_eq!(sub.u8("a").unwrap(), 1);
    // Reading past the sub-block is an error even if the parent has more bytes.
    assert!(matches!(sub.i32_le("b"), Err(ImportError::Truncated(_))));
    // The parent has already advanced past the block.
    assert_eq!(r.u8("c").unwrap(), 4);
    // The sub-block is larger than the parent's remainder.
    assert!(matches!(
        r.sub_reader(10, "d"),
        Err(ImportError::Truncated(_))
    ));
}
