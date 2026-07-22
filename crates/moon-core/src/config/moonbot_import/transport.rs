//! `MBSC7` transport layer: `MBSC7:<8 hex size>:<8 hex CRC32>:<Base16384>` →
//! decompressed payload bytes. Input is untrusted: all lengths use checked arithmetic,
//! buffers are allocated only for validated sizes, and hard limits apply throughout.

use super::ImportError;

/// COMPRESSED data limit from the header: 16 MiB per the specification.
const MAX_COMPRESSED: usize = 16 * 1024 * 1024;
/// DECOMPRESSED payload limit: 64 MiB per the specification, protecting against decompression bombs.
const MAX_DECOMPRESSED: u64 = 64 * 1024 * 1024;
/// First Base16384 code point; each character carries 14 bits of `codepoint - 0x4E00`.
const B16384_BASE: u32 = 0x4E00;
/// Maximum 14-bit Base16384 character value.
const B16384_MAX: u32 = 0x3FFF;

/// Finds `MBSC7:` in text while ignoring all characters with code points ≤ 32, validates the
/// hex header, decodes Base16384, verifies the compressed bytes' CRC32, and decompresses gzip/zlib.
pub fn decode_transport(text: &str) -> Result<Vec<u8>, ImportError> {
    // Remove ALL characters with code points <= 32 (newlines, spaces, tabs): MoonBot may wrap
    // the payload across lines, and the clipboard may add its own line breaks.
    let cleaned: Vec<char> = text.chars().filter(|c| (*c as u32) > 32).collect();

    let start = find_header(&cleaned)?;
    // `MBSC7:` is followed by exactly 8 size hex digits, ':', 8 CRC32 hex digits, ':'.
    let mut pos = start + 6;
    let size = read_hex8(&cleaned, pos)? as usize;
    pos += 8;
    expect_colon(&cleaned, pos)?;
    pos += 1;
    let crc_expected = read_hex8(&cleaned, pos)?;
    pos += 8;
    expect_colon(&cleaned, pos)?;
    pos += 1;

    if size > MAX_COMPRESSED {
        return Err(ImportError::TooLarge(format!(
            "сжатые данные {size} байт (лимит {MAX_COMPRESSED})"
        )));
    }
    if size == 0 {
        return Err(ImportError::BadHeader(
            "нулевой размер сжатых данных".into(),
        ));
    }

    let compressed = base16384_decode(&cleaned[pos..], size)?;

    // CRC covers the COMPRESSED bytes before decompression (standard IEEE CRC32).
    let crc_actual = crc32fast::hash(&compressed);
    if crc_actual != crc_expected {
        return Err(ImportError::CrcMismatch {
            expected: crc_expected,
            actual: crc_actual,
        });
    }

    decompress(&compressed)
}

/// Returns the start position of `MBSC7:` in cleaned text.
///
/// If `MBSC<N>:` with N > 7 is found, returns a clear "newer format" error instead of
/// "not found".
fn find_header(cleaned: &[char]) -> Result<usize, ImportError> {
    let needle = ['M', 'B', 'S', 'C'];
    let mut newer: Option<u32> = None;
    let mut i = 0usize;
    while i + 4 <= cleaned.len() {
        if cleaned[i..i + 4] == needle {
            // Read the version number (one or more decimal digits) and ':'.
            let mut j = i + 4;
            let mut ver: u32 = 0;
            let mut digits = 0u32;
            while j < cleaned.len() && cleaned[j].is_ascii_digit() && digits < 4 {
                ver = ver * 10 + (cleaned[j] as u32 - '0' as u32);
                j += 1;
                digits += 1;
            }
            if digits > 0 && j < cleaned.len() && cleaned[j] == ':' {
                if ver == 7 {
                    return Ok(i);
                }
                if ver > 7 {
                    newer.get_or_insert(ver);
                }
            }
        }
        i += 1;
    }
    match newer {
        Some(found) => Err(ImportError::NewerFormat { found }),
        None => Err(ImportError::NotFound),
    }
}

/// Reads exactly 8 case-insensitive hex digits starting at `pos`.
fn read_hex8(cleaned: &[char], pos: usize) -> Result<u32, ImportError> {
    let end = pos
        .checked_add(8)
        .filter(|e| *e <= cleaned.len())
        .ok_or_else(|| ImportError::BadHeader("обрыв на hex-поле заголовка".into()))?;
    let mut v: u32 = 0;
    for &c in &cleaned[pos..end] {
        let d = c
            .to_digit(16)
            .ok_or_else(|| ImportError::BadHeader(format!("не hex-цифра «{c}» в заголовке")))?;
        v = (v << 4) | d;
    }
    Ok(v)
}

fn expect_colon(cleaned: &[char], pos: usize) -> Result<(), ImportError> {
    if cleaned.get(pos) != Some(&':') {
        return Err(ImportError::BadHeader("ожидался «:» в заголовке".into()));
    }
    Ok(())
}

/// Decodes Base16384: `expected = ceil(out_len*8/14)` characters in the
/// `U+4E00..=U+8DFF` range, with bits stored LSB-first.
///
/// An extra in-range character after the expected data, missing characters, or nonzero
/// padding bits in the last character is an error.
fn base16384_decode(chars: &[char], out_len: usize) -> Result<Vec<u8>, ImportError> {
    let total_bits = out_len
        .checked_mul(8)
        .ok_or_else(|| ImportError::TooLarge("переполнение размера Base16384".into()))?;
    let expected_chars = total_bits.div_ceil(14);
    if chars.len() < expected_chars {
        return Err(ImportError::BadBase16384(format!(
            "ожидалось {expected_chars} символов, доступно {}",
            chars.len()
        )));
    }

    // out_len has already passed the MAX_COMPRESSED limit check, so allocation is safe.
    let mut out = Vec::with_capacity(out_len);
    let mut acc: u32 = 0;
    let mut acc_bits: u32 = 0;
    for (i, &ch) in chars[..expected_chars].iter().enumerate() {
        let v = (ch as u32).wrapping_sub(B16384_BASE);
        if v > B16384_MAX {
            return Err(ImportError::BadBase16384(format!(
                "символ #{i} вне диапазона Base16384"
            )));
        }
        acc |= v << acc_bits;
        acc_bits += 14;
        while acc_bits >= 8 && out.len() < out_len {
            out.push(acc as u8);
            acc >>= 8;
            acc_bits -= 8;
        }
    }
    // The remaining accumulator contains the last character's padding bits, which must be zero.
    if acc != 0 {
        return Err(ImportError::BadBase16384(
            "ненулевые padding-биты последнего символа".into(),
        ));
    }
    if out.len() != out_len {
        return Err(ImportError::BadBase16384(format!(
            "декодировано {} байт вместо {out_len}",
            out.len()
        )));
    }
    // An in-range Base16384 character immediately AFTER the expected data indicates a size mismatch.
    if let Some(&next) = chars.get(expected_chars) {
        let v = (next as u32).wrapping_sub(B16384_BASE);
        if v <= B16384_MAX {
            return Err(ImportError::BadBase16384(
                "лишний символ Base16384 после заявленного размера".into(),
            ));
        }
    }
    Ok(out)
}

/// Decompresses gzip (the producer writes `window_bits = 31`) with a zlib fallback, equivalent
/// to auto-detect 47. Decompression is limited to `MAX_DECOMPRESSED`.
fn decompress(compressed: &[u8]) -> Result<Vec<u8>, ImportError> {
    use std::io::Read;
    let mut out = Vec::new();
    let take = MAX_DECOMPRESSED + 1;
    let read: std::io::Result<usize> = if compressed.starts_with(&[0x1F, 0x8B]) {
        flate2::read::GzDecoder::new(compressed)
            .take(take)
            .read_to_end(&mut out)
    } else {
        flate2::read::ZlibDecoder::new(compressed)
            .take(take)
            .read_to_end(&mut out)
    };
    read.map_err(|e| ImportError::Decompress(e.to_string()))?;
    if out.len() as u64 > MAX_DECOMPRESSED {
        return Err(ImportError::TooLarge(format!(
            "распакованный payload больше {MAX_DECOMPRESSED} байт"
        )));
    }
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
pub(super) fn encode_mbsc7(payload: &[u8]) -> String {
    // Test producer: gzip(payload) → CRC32 → Base16384 → fenced text, matching MoonBot.
    use std::io::Write;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(payload).unwrap();
    let compressed = enc.finish().unwrap();
    let crc = crc32fast::hash(&compressed);
    let mut b16 = String::new();
    let mut acc: u32 = 0;
    let mut acc_bits: u32 = 0;
    for &b in &compressed {
        acc |= (b as u32) << acc_bits;
        acc_bits += 8;
        while acc_bits >= 14 {
            b16.push(char::from_u32(B16384_BASE + (acc & B16384_MAX)).unwrap());
            acc >>= 14;
            acc_bits -= 14;
        }
    }
    if acc_bits > 0 {
        b16.push(char::from_u32(B16384_BASE + acc).unwrap());
    }
    format!(
        "```mbcfg\nMBSC7:{:08X}:{:08X}:{}\n```",
        compressed.len(),
        crc,
        b16
    )
}

#[cfg(test)]
mod tests;
