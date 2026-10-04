//! The canvas as a PNG: the signature, then IHDR, one IDAT and IEND, each chunk with its CRC.
//! Rows go unfiltered: the picture is large flat areas, which deflate packs well as they are.

use std::io::Write;

use flate2::Compression;
use flate2::write::ZlibEncoder;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

/// An 8-bit RGB image of `w` × `h` from its rows of pixels.
pub(super) fn encode(w: usize, h: usize, rgb: &[u8]) -> Vec<u8> {
    let stride = w * 3;
    let mut rows = Vec::with_capacity((stride + 1) * h);
    for row in rgb.chunks_exact(stride).take(h) {
        rows.push(0); // filter: none
        rows.extend_from_slice(row);
    }
    let mut zlib = ZlibEncoder::new(Vec::with_capacity(rows.len() / 8), Compression::fast());
    // Writing into memory does not fail; an empty image is the honest answer if it did.
    let image: Vec<u8> = zlib
        .write_all(&rows)
        .and_then(|()| zlib.finish())
        .unwrap_or_default();
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&(w as u32).to_be_bytes());
    header.extend_from_slice(&(h as u32).to_be_bytes());
    // Bit depth 8, colour type 2 (RGB), deflate, adaptive filtering, no interlace.
    header.extend_from_slice(&[8, 2, 0, 0, 0]);

    let mut png = Vec::with_capacity(image.len() + 64);
    png.extend_from_slice(&SIGNATURE);
    for (kind, body) in [
        (b"IHDR", header.as_slice()),
        (b"IDAT", image.as_slice()),
        (b"IEND", &[][..]),
    ] {
        png.extend_from_slice(&(body.len() as u32).to_be_bytes());
        png.extend_from_slice(kind);
        png.extend_from_slice(body);
        let mut crc = flate2::Crc::new();
        crc.update(kind);
        crc.update(body);
        png.extend_from_slice(&crc.sum().to_be_bytes());
    }
    png
}
