//! Транспортный слой `MBSC7`: `MBSC7:<8 hex size>:<8 hex CRC32>:<Base16384>` →
//! распакованные payload-байты. Вход недоверенный: все длины через checked math,
//! буферы выделяются только по проверенным размерам, жёсткие лимиты на всё.

use super::ImportError;

/// Лимит СЖАТЫХ данных (из заголовка) — 16 MiB по ТЗ.
const MAX_COMPRESSED: usize = 16 * 1024 * 1024;
/// Лимит РАСПАКОВАННОГО payload — 64 MiB по ТЗ (защита от decompression bomb).
const MAX_DECOMPRESSED: u64 = 64 * 1024 * 1024;
/// Первый кодовый пункт Base16384; каждый символ несёт 14 бит `codepoint - 0x4E00`.
const B16384_BASE: u32 = 0x4E00;
/// Максимальное 14-битное значение символа Base16384.
const B16384_MAX: u32 = 0x3FFF;

/// Найти `MBSC7:` в тексте (игнорируя все символы с кодом ≤ 32), проверить hex-заголовок,
/// декодировать Base16384, сверить CRC32 сжатых байт и распаковать gzip/zlib.
pub fn decode_transport(text: &str) -> Result<Vec<u8>, ImportError> {
    // Убираем ВСЕ символы с кодом <= 32 (переносы строк, пробелы, табы): MoonBot может
    // переносить payload по строкам, а клипборд — добавлять свои переводы строк.
    let cleaned: Vec<char> = text.chars().filter(|c| (*c as u32) > 32).collect();

    let start = find_header(&cleaned)?;
    // За `MBSC7:` идут ровно 8 hex размера, ':', 8 hex CRC32, ':'.
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

    // CRC — по СЖАТЫМ байтам, до распаковки (стандартный IEEE CRC32).
    let crc_actual = crc32fast::hash(&compressed);
    if crc_actual != crc_expected {
        return Err(ImportError::CrcMismatch {
            expected: crc_expected,
            actual: crc_actual,
        });
    }

    decompress(&compressed)
}

/// Позиция начала `MBSC7:` в очищенном тексте. Если найден `MBSC<N>:` с N > 7 —
/// понятная ошибка «формат новее» вместо «не найдено».
fn find_header(cleaned: &[char]) -> Result<usize, ImportError> {
    let needle = ['M', 'B', 'S', 'C'];
    let mut newer: Option<u32> = None;
    let mut i = 0usize;
    while i + 4 <= cleaned.len() {
        if cleaned[i..i + 4] == needle {
            // Читаем номер версии (одна и более десятичных цифр) и ':'.
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

/// Ровно 8 hex-цифр (любой регистр) начиная с `pos`.
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

/// Декодер Base16384: `expected = ceil(out_len*8/14)` символов диапазона
/// `U+4E00..=U+8DFF`, биты LSB-first. Лишний символ диапазона после ожидаемых,
/// нехватка символов или ненулевые padding-биты последнего — ошибка.
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

    // out_len уже проверен лимитом MAX_COMPRESSED — выделять безопасно.
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
    // Остаток аккумулятора — padding-биты последнего символа: обязаны быть нулём.
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
    // Символ Base16384-диапазона сразу ЗА ожидаемыми — признак рассинхрона размера.
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

/// gzip (producer пишет `window_bits = 31`) с fallback на zlib — эквивалент
/// auto-detect 47. Распаковка ограничена `MAX_DECOMPRESSED`.
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
    // Тестовый producer: gzip(payload) → CRC32 → Base16384 → текст в fence, как MoonBot.
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
mod tests {
    use super::*;

    fn roundtrip(payload: &[u8]) -> Vec<u8> {
        decode_transport(&encode_mbsc7(payload)).unwrap()
    }

    #[test]
    fn roundtrip_small_and_binary() {
        assert_eq!(roundtrip(b"hello"), b"hello");
        // Все значения байтов + длины, не кратные 7/14-битной упаковке.
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
        // Пробелы/переводы строк внутри payload и мусор вокруг.
        let noisy = format!(
            "some text before\n{}\nafter",
            enc.replace(':', " : ").replace("MBSC7", "MBSC7\n")
        );
        // Разрезание самого заголовка пробелами тоже переживаем (чистка ≤32).
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
        // Портим hex размера.
        let bad_hex = enc.replacen("MBSC7:", "MBSC7:ZZ", 1);
        assert!(matches!(
            decode_transport(&bad_hex),
            Err(ImportError::BadHeader(_))
        ));
        // Портим CRC: меняем один hex-символ CRC-поля на другой.
        let idx = enc.find("MBSC7:").unwrap() + 6 + 9; // первый символ CRC
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
        // Символ вне диапазона внутри payload.
        let mut chars: Vec<char> = enc.chars().collect();
        chars[payload_at] = 'X';
        let bad: String = chars.into_iter().collect();
        assert!(matches!(
            decode_transport(&bad),
            Err(ImportError::BadBase16384(_))
        ));
        // Усечённый payload.
        let truncated: String = enc.chars().take(payload_at + 2).collect();
        assert!(matches!(
            decode_transport(&truncated),
            Err(ImportError::BadBase16384(_))
        ));
    }

    #[test]
    fn nonzero_padding_rejected() {
        // 1 байт → total_bits=8 → 1 символ, старшие 6 бит символа — padding.
        // Значение с ненулевым padding: 0b01_0000_0001 → символ BASE + 257.
        let ch = char::from_u32(B16384_BASE + 0x100).unwrap();
        // CRC заведомо неверный не подходит — нужно, чтобы падало ИМЕННО на padding,
        // поэтому CRC не проверить раньше Base16384 нельзя: порядок в decode_transport —
        // сначала Base16384, потом CRC. Берём произвольный CRC.
        let text = format!("MBSC7:00000001:00000000:{ch}");
        assert!(matches!(
            decode_transport(&text),
            Err(ImportError::BadBase16384(ref s)) if s.contains("padding")
        ));
    }

    #[test]
    fn extra_symbol_after_payload_rejected() {
        let enc = encode_mbsc7(b"xy");
        // Дописываем ещё один символ Base16384-диапазона сразу за payload (до fence).
        let extra = char::from_u32(B16384_BASE).unwrap();
        let with_extra = enc.replace("\n```", &format!("{extra}\n```"));
        assert!(matches!(
            decode_transport(&with_extra),
            Err(ImportError::BadBase16384(ref s)) if s.contains("лишний")
        ));
    }

    #[test]
    fn truncated_gzip_rejected() {
        // Валидная оболочка (size/CRC пересчитаны) вокруг ОБРЕЗАННЫХ gzip-байт.
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
        // Заголовок с размером больше лимита — ошибка ДО каких-либо выделений.
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
        // 80 MiB нулей жмутся в мелочь; распаковка должна упереться в лимит 64 MiB.
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
}
