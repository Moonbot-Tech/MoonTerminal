//! Trade-only PCM gain, preserving RIFF metadata and the catalog's original shared bytes.

use std::sync::Arc;

/// Scale integer PCM samples toward silence, retaining the exact original buffer at 100%.
/// Supports unsigned 8-bit and signed 16/24/32-bit PCM, including multiple data chunks.
/// Malformed or unsupported reduced-volume clips return `None` rather than playing at full gain.
pub(super) fn scale(wav: Arc<[u8]>, percent: u8) -> Option<Arc<[u8]>> {
    let percent = percent.min(100);
    if percent == 100 {
        return Some(wav);
    }
    if wav.get(..4)? != b"RIFF" || wav.get(8..12)? != b"WAVE" {
        return None;
    }
    let end = 8usize.checked_add(u32::from_le_bytes(wav.get(4..8)?.try_into().ok()?) as usize)?;
    if end > wav.len() {
        return None;
    }
    let mut offset = 12usize;
    let mut sample_bytes = None;
    let mut data = Vec::new();
    while offset.checked_add(8)? <= end {
        let tag = wav.get(offset..offset + 4)?;
        let len = u32::from_le_bytes(wav.get(offset + 4..offset + 8)?.try_into().ok()?) as usize;
        let start = offset.checked_add(8)?;
        let stop = start.checked_add(len)?;
        if stop > end {
            return None;
        }
        if tag == b"fmt " {
            let chunk = wav.get(start..stop)?;
            let encoding = u16::from_le_bytes(chunk.get(..2)?.try_into().ok()?);
            let bits = u16::from_le_bytes(chunk.get(14..16)?.try_into().ok()?);
            if encoding != 1 || !matches!(bits, 8 | 16 | 24 | 32) {
                return None;
            }
            sample_bytes = Some(usize::from(bits / 8));
        } else if tag == b"data" {
            data.push(start..stop);
        }
        offset = stop.checked_add(len & 1)?;
    }
    let width = sample_bytes?;
    if data.is_empty() || data.iter().any(|range| !range.len().is_multiple_of(width)) {
        return None;
    }
    let mut scaled = wav.to_vec();
    for range in data {
        for sample in scaled[range].chunks_exact_mut(width) {
            let value = match width {
                1 => i64::from(sample[0]) - 128,
                2 => i64::from(i16::from_le_bytes([sample[0], sample[1]])),
                3 => i64::from(i32::from_le_bytes([
                    sample[0],
                    sample[1],
                    sample[2],
                    if sample[2] & 0x80 != 0 { 0xff } else { 0 },
                ])),
                4 => i64::from(i32::from_le_bytes([
                    sample[0], sample[1], sample[2], sample[3],
                ])),
                _ => unreachable!(),
            };
            let value = value * i64::from(percent) / 100;
            if width == 1 {
                sample[0] = (value + 128) as u8;
            } else {
                sample.copy_from_slice(&value.to_le_bytes()[..width]);
            }
        }
    }
    Some(Arc::from(scaled))
}

#[cfg(test)]
mod tests;
