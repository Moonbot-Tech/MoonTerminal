//! MoonBot connection packets — the base64 text behind MoonBot's "Copy Key".
//!
//! `moonproto::parse_key_info` reads such a packet but hands out only the two session keys, while
//! moving a core needs the whole 72-byte key container back: the new core must keep the very key
//! every terminal already holds, and only the endpoint around it changes. So this module decodes
//! the container itself and writes it out again with another address and port.
//!
//! The cipher, the checksum and the layout are the ones `moonproto`'s importer decodes (its
//! `key_import` module); the tests check every packet written here against that importer, which is
//! the independent oracle.

use std::net::Ipv4Addr;

use base64::Engine;

use crate::config::TransportVersion;

/// Password head of the current packet format (key container plus endpoint).
const NEW_PWD_HEAD: &str = "F$xC2";
/// Password head of the legacy, key-only format.
const OLD_PWD_HEAD: &str = "F$xC";
const PWD_TAIL: &str = "aR#d";
/// The current packet format number, written right after the inner stamp.
const FORMAT_V1: u8 = 1;
/// Size of the key container: name, filled flag, date, bot id, version, flags, two keys, hash.
const CONTAINER_SIZE: usize = 72;
/// Bytes of the container its own hash covers: everything before the hash.
const CONTAINER_HASHED: usize = CONTAINER_SIZE - 8;
/// Clear text of a legacy packet: stamp and container.
const LEGACY_PLAIN: usize = 8 + CONTAINER_SIZE;
/// Clear text of a V1 packet: stamp, format, container, port, IP family, IPv4, IPv6, mask.
const V1_PLAIN: usize = 8 + 1 + CONTAINER_SIZE + 2 + 1 + 4 + 16 + 1;
/// The IP-family byte for IPv4.
const IPV4_FAMILY: u8 = 0;
/// Days between the Delphi date origin (1899-12-30) and the Unix epoch.
const DELPHI_UNIX_DAYS: f64 = 25_569.0;
/// Length of the random key name MoonBot shows next to a key.
const NAME_LEN: usize = 16;

/// One MoonBot connection packet: the key container and the endpoint it points to.
#[derive(Clone)]
pub struct ConnectionPacket {
    /// The key container, byte for byte as MoonBot wrote it.
    container: [u8; CONTAINER_SIZE],
    /// UDP port of the core's MoonProto server.
    pub port: u16,
    /// IPv4 address of the core; `None` when the packet carries none.
    pub ipv4: Option<Ipv4Addr>,
    /// MoonProto transport mode the core expects.
    pub mode: TransportVersion,
}

impl std::fmt::Debug for ConnectionPacket {
    /// Never prints the key container: it is the full-access secret.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionPacket")
            .field("port", &self.port)
            .field("ipv4", &self.ipv4)
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

impl Drop for ConnectionPacket {
    /// Wipe the key bytes when the packet goes away.
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.container);
    }
}

impl ConnectionPacket {
    /// Parse a packet copied from MoonBot or written by [`Self::export`].
    ///
    /// A legacy packet carries no endpoint: its port is 0, its address `None` and its mode V0, and
    /// the caller supplies them before exporting. `None` for anything that is not a packet whose
    /// checksum and inner key hash both hold.
    pub fn parse(text: &str) -> Option<Self> {
        let compact: String = text.chars().filter(|c| !c.is_ascii_whitespace()).collect();
        let raw = base64::engine::general_purpose::STANDARD
            .decode(compact)
            .ok()?;
        if raw.len() < 16 + LEGACY_PLAIN {
            return None;
        }
        let stamp = i64::from_le_bytes(raw[0..8].try_into().ok()?);
        let checksum = i64::from_le_bytes(raw[8..16].try_into().ok()?);
        let encrypted = &raw[16..];
        if let Some(plain) = decrypt(encrypted, NEW_PWD_HEAD, stamp, checksum) {
            return Self::from_v1(&plain);
        }
        let plain = decrypt(encrypted, OLD_PWD_HEAD, stamp, checksum)?;
        let container = container_from(&plain, 8)?;
        Some(Self {
            container,
            port: 0,
            ipv4: None,
            mode: TransportVersion::V0,
        })
    }

    /// A brand-new key, the way MoonBot creates one: random session keys and name, today's date.
    pub fn generate(ipv4: Ipv4Addr, port: u16, mode: TransportVersion) -> anyhow::Result<Self> {
        let mut keys = [0u8; 32];
        getrandom::getrandom(&mut keys)?;
        let mut name = [0u8; NAME_LEN];
        getrandom::getrandom(&mut name)?;
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        let mut container = [0u8; CONTAINER_SIZE];
        container[0] = NAME_LEN as u8;
        for (slot, byte) in container[1..1 + NAME_LEN].iter_mut().zip(name) {
            *slot = ALPHABET[usize::from(byte) % ALPHABET.len()];
        }
        container[17] = 1; // filled
        let days = crate::util::time::now_unix_ms() / 86_400_000.0 + DELPHI_UNIX_DAYS;
        container[18..26].copy_from_slice(&days.to_le_bytes());
        container[30] = 1; // container version
        container[32..64].copy_from_slice(&keys);
        zeroize::Zeroize::zeroize(&mut keys);
        let hash = checksum_w(&container[..CONTAINER_HASHED]);
        container[CONTAINER_HASHED..].copy_from_slice(&hash.to_le_bytes());
        Ok(Self {
            container,
            port,
            ipv4: Some(ipv4),
            mode,
        })
    }

    /// The same key pointed at another endpoint.
    pub fn with_endpoint(&self, ipv4: Ipv4Addr, port: u16) -> Self {
        let mut moved = self.clone();
        moved.ipv4 = Some(ipv4);
        moved.port = port;
        moved
    }

    /// Write the packet in the current format, with fresh random stamps.
    pub fn export(&self) -> anyhow::Result<String> {
        let mut plain = zeroize::Zeroizing::new(Vec::with_capacity(V1_PLAIN));
        plain.extend_from_slice(&random_i64()?.to_le_bytes());
        plain.push(FORMAT_V1);
        plain.extend_from_slice(&self.container);
        plain.extend_from_slice(&self.port.to_le_bytes());
        plain.push(IPV4_FAMILY);
        plain.extend_from_slice(&self.ipv4.map_or(0, u32::from).to_le_bytes());
        plain.extend_from_slice(&[0u8; 16]);
        plain.push(mode_byte(self.mode));
        let checksum = checksum_w(&plain);
        let stamp = random_i64()?;
        encode_buffer(&mut plain, &password(NEW_PWD_HEAD, stamp));
        let mut raw = Vec::with_capacity(16 + plain.len());
        raw.extend_from_slice(&stamp.to_le_bytes());
        raw.extend_from_slice(&checksum.to_le_bytes());
        raw.extend_from_slice(&plain);
        Ok(base64::engine::general_purpose::STANDARD.encode(raw))
    }

    /// Whether `other` holds the same key container, whatever endpoint either points to.
    pub fn same_key(&self, other: &Self) -> bool {
        self.container == other.container
    }

    /// Read a V1 clear text: the container and the endpoint after it.
    fn from_v1(plain: &[u8]) -> Option<Self> {
        if plain.len() < V1_PLAIN || plain[8] != FORMAT_V1 {
            return None;
        }
        let container = container_from(plain, 9)?;
        let at = 9 + CONTAINER_SIZE;
        let port = u16::from_le_bytes(plain[at..at + 2].try_into().ok()?);
        let family = plain[at + 2];
        let ip4 = u32::from_le_bytes(plain[at + 3..at + 7].try_into().ok()?);
        let mode = *TransportVersion::ALL.get(usize::from(plain[at + 23]))?;
        Some(Self {
            container,
            port,
            ipv4: (family == IPV4_FAMILY && ip4 != 0).then(|| Ipv4Addr::from(ip4)),
            mode,
        })
    }
}

/// The transport-mode byte a packet stores.
fn mode_byte(mode: TransportVersion) -> u8 {
    match mode {
        TransportVersion::V0 => 0,
        TransportVersion::V1 => 1,
        TransportVersion::V2 => 2,
    }
}

/// Copy the container at `offset` out of `plain`, refusing one that is unfilled or whose own hash
/// does not hold: a core given such a key refuses it.
fn container_from(plain: &[u8], offset: usize) -> Option<[u8; CONTAINER_SIZE]> {
    let bytes: [u8; CONTAINER_SIZE] = plain
        .get(offset..offset + CONTAINER_SIZE)?
        .try_into()
        .ok()?;
    let stored = i64::from_le_bytes(bytes[CONTAINER_HASHED..].try_into().ok()?);
    (bytes[17] == 1 && checksum_w(&bytes[..CONTAINER_HASHED]) == stored).then_some(bytes)
}

/// Decrypt with one password head; `Some` only when the clear text matches `checksum`.
fn decrypt(encrypted: &[u8], head: &str, stamp: i64, checksum: i64) -> Option<Vec<u8>> {
    let mut plain = encrypted.to_vec();
    decode_buffer(&mut plain, &password(head, stamp));
    if checksum_w(&plain) == checksum {
        Some(plain)
    } else {
        zeroize::Zeroize::zeroize(&mut plain);
        None
    }
}

/// The packet password: head, decimal stamp and tail, cut to 25 bytes.
fn password(head: &str, stamp: i64) -> Vec<u8> {
    format!("{head}{stamp}{PWD_TAIL}")
        .bytes()
        .take(25)
        .collect()
}

/// A random signed 64-bit stamp.
fn random_i64() -> anyhow::Result<i64> {
    let mut bytes = [0u8; 8];
    getrandom::getrandom(&mut bytes)?;
    Ok(i64::from_le_bytes(bytes))
}

/// MoonBot's `CalculateCheckSumW`, as `moonproto` ports it.
fn checksum_w(buf: &[u8]) -> i64 {
    let mut rax = 0u64;
    for &byte in buf {
        let bl = byte ^ 0b1010_1010;
        let bh = byte ^ 0b0011_1001;
        let (al, cf) = add8(rax as u8, bl, false);
        rax = set_al(rax, al);
        let cf_out = (rax & (1 << 63)) != 0;
        rax = (rax << 1) | u64::from(cf);
        let (al, _) = add8(rax as u8, 0, cf_out);
        rax = set_al(rax, al);
        rax = rax.rotate_left(8);
        let cf = (rax & 1) != 0;
        let (ah, cf) = add8((rax >> 8) as u8, bh, cf);
        rax = (rax & !(0xFF << 8)) | (u64::from(ah) << 8);
        let (al, _) = add8(rax as u8, 0, cf);
        rax = set_al(rax, al);
        rax = rax.rotate_left(8);
        let cf = (rax & 1) != 0;
        let (al, _) = add8(rax as u8, 0, cf);
        rax = set_al(rax, al);
    }
    rax as i64
}

/// Add with carry in and carry out, on bytes.
fn add8(a: u8, b: u8, carry: bool) -> (u8, bool) {
    let sum = u16::from(a) + u16::from(b) + u16::from(carry);
    (sum as u8, sum > 0xFF)
}

/// Replace the low byte of `value`.
fn set_al(value: u64, al: u8) -> u64 {
    (value & !0xFF) | u64::from(al)
}

/// The per-byte operands both directions of the cipher share.
struct Round {
    al: u8,
    ah: u8,
    nibble: u8,
    c0: u8,
    c1: u8,
    c3: u8,
    c2_rot: u32,
    cn1_rot: u32,
    mix: u8,
    mix2: u8,
}

impl Round {
    /// Operands for the byte at `counter` under password `code`.
    fn at(counter: usize, code: &[u8]) -> Self {
        let get = |i: usize| code.get(i).copied().unwrap_or(0);
        let al = (counter & 0xFF) as u8;
        let ah = ((counter >> 8) & 0xFF) as u8;
        let nibble = counter & 0xF;
        let cn1 = get(nibble + 1);
        Self {
            al,
            ah,
            nibble: nibble as u8,
            c0: get(0),
            c1: get(1),
            c3: get(3),
            c2_rot: u32::from(get(2) & 7),
            cn1_rot: u32::from(cn1 & 7),
            mix: get(nibble + 2).wrapping_add(nibble as u8).wrapping_add(ah),
            mix2: get(nibble).wrapping_add(cn1).wrapping_add(1),
        }
    }
}

/// Encrypt in place: the inverse of [`decode_buffer`].
fn encode_buffer(buf: &mut [u8], code: &[u8]) {
    if code.is_empty() {
        return;
    }
    for (counter, byte) in buf.iter_mut().enumerate() {
        let r = Round::at(counter, code);
        let mut b = *byte;
        b = b.wrapping_add(r.al).wrapping_add(r.ah);
        b ^= r.mix2;
        b = b.wrapping_add(r.nibble).rotate_left(r.cn1_rot);
        b ^= r.mix;
        b = b.wrapping_add(r.c3);
        b ^= r.c0;
        b = b.wrapping_add(r.c1 ^ r.ah);
        b = b
            .rotate_right(r.c2_rot)
            .wrapping_add(r.al)
            .rotate_left(r.c2_rot);
        b ^= r.al;
        *byte = b.wrapping_add(r.al).wrapping_add(r.ah);
    }
}

/// Decrypt in place, as `moonproto`'s importer does.
fn decode_buffer(buf: &mut [u8], code: &[u8]) {
    if code.is_empty() {
        return;
    }
    for (counter, byte) in buf.iter_mut().enumerate() {
        let r = Round::at(counter, code);
        let mut b = *byte;
        b = b.wrapping_sub(r.ah).wrapping_sub(r.al);
        b ^= r.al;
        b = b
            .rotate_right(r.c2_rot)
            .wrapping_sub(r.al)
            .rotate_left(r.c2_rot);
        b = b.wrapping_sub(r.c1 ^ r.ah);
        b ^= r.c0;
        b = b.wrapping_sub(r.c3);
        b ^= r.mix;
        b = b.rotate_right(r.cn1_rot).wrapping_sub(r.nibble);
        b ^= r.mix2;
        *byte = b.wrapping_sub(r.ah).wrapping_sub(r.al);
    }
}

#[cfg(test)]
mod tests;
