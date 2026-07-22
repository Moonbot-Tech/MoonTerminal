use super::*;

fn roundtrip(payload: &[u8]) -> Vec<u8> {
    decode_transport(&encode_mbsc7(payload)).unwrap()
}

#[test]
fn roundtrip_small_and_binary() {
    assert_eq!(roundtrip(b"hello"), b"hello");
    // All byte values plus lengths not divisible by the 7/14-bit packing boundaries.
    let all: Vec<u8> = (0u8..=255).cycle().take(1000).collect();
    assert_eq!(roundtrip(&all), all);
    for len in [1usize, 2, 3, 6, 7, 8, 13, 14, 15, 255] {
        let data: Vec<u8> = (0..len).map(|i| (i * 37 % 256) as u8).collect();
        assert_eq!(roundtrip(&data), data);
    }
}

#[test]
fn finds_header_inside_noise_and_whitespace() {
    let enc = encode_mbsc7(b"payload");
    // Spaces/newlines inside the payload and noise around it.
    let noisy = format!(
        "some text before\n{}\nafter",
        enc.replace(':', " : ").replace("MBSC7", "MBSC7\n")
    );
    // Splitting the header itself with whitespace also works because code points ≤32 are removed.
    assert_eq!(decode_transport(&noisy).unwrap(), b"payload");
}

#[test]
fn single_line_without_fence() {
    let enc = encode_mbsc7(b"abc");
    let line = enc.lines().find(|l| l.contains("MBSC7:")).unwrap();
    assert_eq!(decode_transport(line).unwrap(), b"abc");
}

#[test]
fn not_found_and_newer_format() {
    assert_eq!(
        decode_transport("no header here"),
        Err(ImportError::NotFound)
    );
    assert_eq!(
        decode_transport("MBSC8:00000001:00000000:一"),
        Err(ImportError::NewerFormat { found: 8 })
    );
}

#[test]
fn bad_hex_and_bad_crc() {
    let enc = encode_mbsc7(b"data");
    // Corrupt the size hex field.
    let bad_hex = enc.replacen("MBSC7:", "MBSC7:ZZ", 1);
    assert!(matches!(
        decode_transport(&bad_hex),
        Err(ImportError::BadHeader(_))
    ));
    // Corrupt the CRC by changing one hex character in its field.
    let idx = enc.find("MBSC7:").unwrap() + 6 + 9; // First CRC character.
    let mut chars: Vec<char> = enc.chars().collect();
    chars[idx] = if chars[idx] == '0' { '1' } else { '0' };
    let bad_crc: String = chars.into_iter().collect();
    assert!(matches!(
        decode_transport(&bad_crc),
        Err(ImportError::CrcMismatch { .. })
    ));
}

#[test]
fn bad_base16384_char_and_truncated() {
    let enc = encode_mbsc7(b"some payload data");
    let start = enc.find("MBSC7:").unwrap();
    let payload_at = start + 6 + 8 + 1 + 8 + 1;
    // Out-of-range character inside the payload.
    let mut chars: Vec<char> = enc.chars().collect();
    chars[payload_at] = 'X';
    let bad: String = chars.into_iter().collect();
    assert!(matches!(
        decode_transport(&bad),
        Err(ImportError::BadBase16384(_))
    ));
    // Truncated payload.
    let truncated: String = enc.chars().take(payload_at + 2).collect();
    assert!(matches!(
        decode_transport(&truncated),
        Err(ImportError::BadBase16384(_))
    ));
}

#[test]
fn nonzero_padding_rejected() {
    // 1 byte → total_bits=8 → 1 character, whose high 6 bits are padding.
    // A value with nonzero padding: 0b01_0000_0001 → character BASE + 257.
    let ch = char::from_u32(B16384_BASE + 0x100).unwrap();
    // A deliberately invalid CRC is acceptable because this must fail SPECIFICALLY on padding.
    // CRC cannot be checked before Base16384: decode_transport processes Base16384 first, then
    // CRC. Use an arbitrary CRC.
    let text = format!("MBSC7:00000001:00000000:{ch}");
    assert!(matches!(
        decode_transport(&text),
        Err(ImportError::BadBase16384(ref s)) if s.contains("padding")
    ));
}

#[test]
fn extra_symbol_after_payload_rejected() {
    let enc = encode_mbsc7(b"xy");
    // Append another in-range Base16384 character immediately after the payload, before the fence.
    let extra = char::from_u32(B16384_BASE).unwrap();
    let with_extra = enc.replace("\n```", &format!("{extra}\n```"));
    assert!(matches!(
        decode_transport(&with_extra),
        Err(ImportError::BadBase16384(ref s)) if s.contains("лишний")
    ));
}

#[test]
fn truncated_gzip_rejected() {
    // Valid wrapper with recalculated size/CRC around TRUNCATED gzip bytes.
    use std::io::Write;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(b"payload payload payload").unwrap();
    let full = enc.finish().unwrap();
    let cut = &full[..full.len() / 2];
    let crc = crc32fast::hash(cut);
    let mut b16 = String::new();
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for &b in cut {
        acc |= (b as u32) << bits;
        bits += 8;
        while bits >= 14 {
            b16.push(char::from_u32(B16384_BASE + (acc & B16384_MAX)).unwrap());
            acc >>= 14;
            bits -= 14;
        }
    }
    if bits > 0 {
        b16.push(char::from_u32(B16384_BASE + acc).unwrap());
    }
    let text = format!("MBSC7:{:08X}:{crc:08X}:{b16}", cut.len());
    assert!(matches!(
        decode_transport(&text),
        Err(ImportError::Decompress(_))
    ));
}

#[test]
fn compressed_size_limit_enforced() {
    // A header size above the limit fails BEFORE any allocation.
    let text = format!("MBSC7:{:08X}:00000000:一", MAX_COMPRESSED + 1);
    assert!(matches!(
        decode_transport(&text),
        Err(ImportError::TooLarge(_))
    ));
}

#[test]
fn zero_size_rejected() {
    let text = "MBSC7:00000000:00000000:";
    assert!(matches!(
        decode_transport(text),
        Err(ImportError::BadHeader(_))
    ));
}

#[test]
fn decompression_bomb_rejected() {
    // 80 MiB of zeros compresses to very little; decompression must hit the 64 MiB limit.
    use std::io::Write;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let chunk = vec![0u8; 1024 * 1024];
    for _ in 0..80 {
        enc.write_all(&chunk).unwrap();
    }
    let compressed = enc.finish().unwrap();
    let crc = crc32fast::hash(&compressed);
    let mut b16 = String::new();
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for &b in &compressed {
        acc |= (b as u32) << bits;
        bits += 8;
        while bits >= 14 {
            b16.push(char::from_u32(B16384_BASE + (acc & B16384_MAX)).unwrap());
            acc >>= 14;
            bits -= 14;
        }
    }
    if bits > 0 {
        b16.push(char::from_u32(B16384_BASE + acc).unwrap());
    }
    let text = format!("MBSC7:{:08X}:{crc:08X}:{b16}", compressed.len());
    assert!(matches!(
        decode_transport(&text),
        Err(ImportError::TooLarge(_))
    ));
}
