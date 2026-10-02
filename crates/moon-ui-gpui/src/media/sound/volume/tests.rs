//! Fixed PCM vectors catch signedness, gain, RIFF-offset and original-buffer regressions.

use super::scale;
use std::sync::Arc;

/// Build stereo PCM with an odd-sized metadata chunk and two independent data chunks.
fn wav(bits: u16, samples: &[u8]) -> Arc<[u8]> {
    let mut bytes = b"RIFF\0\0\0\0WAVE".to_vec();
    bytes.extend_from_slice(b"JUNK\x03\0\0\0abc\0");
    bytes.extend_from_slice(b"fmt \x10\0\0\0");
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&8000u32.to_le_bytes());
    bytes.extend_from_slice(&(8000 * 2 * u32::from(bits / 8)).to_le_bytes());
    bytes.extend_from_slice(&(2 * (bits / 8)).to_le_bytes());
    bytes.extend_from_slice(&bits.to_le_bytes());
    for _ in 0..2 {
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        bytes.extend_from_slice(samples);
        if !samples.len().is_multiple_of(2) {
            bytes.push(0);
        }
    }
    let len = bytes.len() as u32 - 8;
    bytes[4..8].copy_from_slice(&len.to_le_bytes());
    Arc::from(bytes)
}

/// Scaling at full gain must not copy, alter metadata, or lose even one sample bit.
#[test]
fn full_volume_preserves_every_embedded_clip_and_its_buffer() {
    for (_, bytes) in super::super::embedded::SOUNDS {
        let original: Arc<[u8]> = Arc::from(*bytes);
        let scaled = scale(original.clone(), 100).unwrap();
        assert!(Arc::ptr_eq(&original, &scaled));
        assert_eq!(&*scaled, *bytes);
        assert!(
            scale(original, 50).is_some(),
            "embedded PCM must support gain"
        );
    }
}

/// Treating 8-bit PCM as signed, forgetting 24-bit sign extension, or scaling RIFF headers
/// corrupts these independently calculated half-gain and digital-silence vectors.
#[test]
fn pcm_widths_scale_both_channels_and_data_chunks_without_touching_metadata() {
    type Vector<'a> = (u16, &'a [u8], &'a [u8], &'a [u8]);
    let vectors: &[Vector<'_>] = &[
        (8, &[0, 128, 255, 64], &[64, 128, 191, 96], &[128; 4]),
        (
            16,
            &[0, 128, 254, 127, 0, 0, 0, 192],
            &[0, 192, 255, 63, 0, 0, 0, 224],
            &[0; 8],
        ),
        (
            24,
            &[0, 0, 128, 254, 255, 127],
            &[0, 0, 192, 255, 255, 63],
            &[0; 6],
        ),
        (
            32,
            &[0, 0, 0, 128, 254, 255, 255, 127],
            &[0, 0, 0, 192, 255, 255, 255, 63],
            &[0; 8],
        ),
    ];
    for &(bits, input, half, silence) in vectors {
        let original = wav(bits, input);
        for (gain, expected) in [(50, half), (0, silence)] {
            let scaled = scale(original.clone(), gain).unwrap();
            assert_eq!(&*scaled, &*wav(bits, expected));
            assert_eq!(&*original, &*wav(bits, input));
        }
    }
}

/// A reduced-volume clip that cannot be safely scaled must never fall back to full loudness.
#[test]
fn unsupported_or_truncated_pcm_is_refused() {
    assert!(scale(wav(64, &[0; 16]), 50).is_none());
    let valid = wav(16, &[0; 4]);
    let truncated: Arc<[u8]> = Arc::from(&valid[..valid.len() - 1]);
    assert!(scale(truncated, 50).is_none());
}
