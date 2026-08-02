//! Network-address redaction for everything that reaches a log.
//!
//! Cores are identified by NAME in the log; their address is not. Our own log calls simply do not
//! format one, but two sources we do not own still carry addresses into the same output: moonproto
//! (`server address resolve failed for <host>:<port>`, NTP and socket lines) and the MoonBot core's
//! own `ServerLog` text. [`super`] describes which sink calls this and why.
//!
//! Only literal addresses are covered. A DNS name (`core-01.internal.lan:3000`) still reads through:
//! masking every dotted word would swallow exchange endpoints, file names and symbols, and our own
//! endpoint is always an `IpAddr`.
//!
//! The scan is hand-rolled rather than a regex — `regex` is already in the tree, so this is not
//! about dependencies: the accept rules need lookbehind (an address may not start mid-token) and
//! rejections a pattern cannot express (a version, a clock, a Rust path). It runs on every log line,
//! including the high-volume core stream, so a line without an address is returned borrowed and
//! allocates nothing.

use std::borrow::Cow;

/// Placeholder written in place of an address (and its port, when one follows).
const MASK: &str = "<addr>";

/// Longest colon run worth scanning as an IPv6 literal; the longest legal form is 45 characters,
/// and the extra room lets a trailing `:port` or a malformed tail be seen and rejected as a whole.
const MAX_IPV6: usize = 64;

/// Replaces every IPv4/IPv6 literal — with its `:port` suffix, when present — by [`MASK`].
///
/// A colon run counts as IPv6 only in the shapes [`is_ipv6_text`] admits, so clocks, durations and
/// colon-separated ids survive. A dotted quad counts as IPv4 on a token boundary with every octet
/// in `0..=255`: `v8.1.2.3` and a five-component version keep their text, but a BARE four-component
/// version (`8.5.1.212`) is indistinguishable from an address and is masked — deliberately, since
/// hiding the address is what this exists for.
///
/// Args:
///     msg: One log line, in any language and encoding valid UTF-8 allows.
///
/// Returns:
///     The input borrowed unchanged when it holds no address, or an owned redacted copy.
pub fn addresses(msg: &str) -> Cow<'_, str> {
    let bytes = msg.as_bytes();
    let mut out: Option<String> = None;
    // Start of the tail not yet copied into `out`; everything before it is already emitted.
    let mut copied = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if !is_boundary(bytes, i) {
            i += 1;
            continue;
        }
        let Some(end) = match_address(bytes, i) else {
            i += 1;
            continue;
        };
        let end = port_suffix(bytes, end);
        let buf = out.get_or_insert_with(|| String::with_capacity(msg.len()));
        buf.push_str(&msg[copied..i]);
        buf.push_str(MASK);
        copied = end;
        i = end;
    }
    match out {
        Some(mut buf) => {
            buf.push_str(&msg[copied..]);
            Cow::Owned(buf)
        }
        None => Cow::Borrowed(msg),
    }
}

/// Whether an address may start at `i`: the previous byte must not continue a token, so the tail of
/// a version (`v8.1.2.3`), an identifier or a longer number is never taken for an address.
///
/// A colon or hyphen does NOT continue a token here, because the core writes `host:10.0.0.7` and
/// `10.0.0.1-10.0.0.9` without spaces. IPv6 needs the stricter rule; see [`match_address`].
fn is_boundary(bytes: &[u8], i: usize) -> bool {
    let Some(prev) = i.checked_sub(1).map(|p| bytes[p]) else {
        return true;
    };
    !(prev.is_ascii_alphanumeric() || matches!(prev, b'.' | b'_'))
}

/// End index of the address starting at `i`, in either family, brackets and zone id included.
///
/// The caller has already established [`is_boundary`]; IPv6 additionally refuses to start after a
/// colon, or the scan would begin mid-path in `moon_core::feed::live`, whose segments are hex.
fn match_address(bytes: &[u8], i: usize) -> Option<usize> {
    if bytes[i] == b'[' {
        // `[::]:port` is the unspecified bind address; its empty literal fails the ordinary rules.
        let inner = if bytes[i + 1..].starts_with(b"::]") {
            i + 3
        } else {
            // Both families appear in brackets: `[::1]:3000` from a dual-stack bind, `[10.0.0.1]:80`
            // from a URL.
            match ipv6(bytes, i + 1) {
                Some(end) => zone_suffix(bytes, end),
                None => ipv4(bytes, i + 1)?,
            }
        };
        return (bytes.get(inner) == Some(&b']')).then_some(inner + 1);
    }
    if let Some(end) = ipv4(bytes, i) {
        return Some(end);
    }
    if i.checked_sub(1).is_some_and(|p| bytes[p] == b':') {
        return None;
    }
    Some(zone_suffix(bytes, ipv6(bytes, i)?))
}

/// End index past a `%zone` interface suffix at `end`, or `end` itself when none follows. A zone id
/// names the local interface, which is machine identity of the same kind as the address.
fn zone_suffix(bytes: &[u8], end: usize) -> usize {
    if bytes.get(end) != Some(&b'%') {
        return end;
    }
    let mut pos = end + 1;
    while pos < bytes.len() && (bytes[pos].is_ascii_alphanumeric() || bytes[pos] == b'.') {
        pos += 1;
    }
    if pos == end + 1 {
        end
    } else {
        pos
    }
}

/// End index of a dotted-quad IPv4 literal starting at `i`.
fn ipv4(bytes: &[u8], i: usize) -> Option<usize> {
    let mut pos = i;
    for octet in 0..4 {
        if octet > 0 {
            if bytes.get(pos) != Some(&b'.') {
                return None;
            }
            pos += 1;
        }
        let start = pos;
        let mut value = 0u16;
        while pos < bytes.len() && bytes[pos].is_ascii_digit() && pos - start < 3 {
            value = value * 10 + u16::from(bytes[pos] - b'0');
            pos += 1;
        }
        if pos == start || value > 255 {
            return None;
        }
    }
    ends_token(bytes, pos)
}

/// `pos` when the token ends there, or `None` when the following byte continues it.
///
/// A fifth dotted component means a version string, not an address; a sentence-ending dot does not,
/// so a dot only continues the token when a digit follows it.
fn ends_token(bytes: &[u8], pos: usize) -> Option<usize> {
    match bytes.get(pos) {
        Some(b'.') => bytes
            .get(pos + 1)
            .is_none_or(|b| !b.is_ascii_digit())
            .then_some(pos),
        Some(b) if b.is_ascii_alphanumeric() || *b == b'_' => None,
        _ => Some(pos),
    }
}

/// End index of an IPv6 literal starting at `i`, including the `::ffff:1.2.3.4` mapped form.
///
/// Also matches colon-separated hex runs of the same shape, such as a MAC address, which is the
/// intent: those identify a machine too.
fn ipv6(bytes: &[u8], i: usize) -> Option<usize> {
    let limit = (i + MAX_IPV6).min(bytes.len());
    let mut end = i;
    let mut colons = 0usize;
    while end < limit && (bytes[end].is_ascii_hexdigit() || bytes[end] == b':') {
        colons += usize::from(bytes[end] == b':');
        end += 1;
    }
    // A trailing colon is a separator unless it closes a `::`.
    while end > i && bytes[end - 1] == b':' && !(end - i >= 2 && bytes[end - 2] == b':') {
        end -= 1;
        colons -= 1;
    }
    // Cheap gates before validating the text: every accepted shape needs at least two colons, and a
    // run that continues into a word was never an address. Ordinary words start with hex letters
    // often enough that skipping this costs an allocation per word.
    if end <= i || colons < 2 {
        return None;
    }
    let mapped = bytes.get(end) == Some(&b'.');
    if !mapped
        && bytes
            .get(end)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
    {
        return None;
    }
    let text = std::str::from_utf8(&bytes[i..end]).ok()?;
    if !is_ipv6_text(text) {
        return None;
    }
    if mapped {
        // `::ffff:1.2.3.4`: the run stopped at the dot, so continue through the mapped IPv4 tail,
        // whose first octet is the last group scanned above. A dot that does NOT open a mapped tail
        // is just a sentence ending — the literal before it still has to be masked.
        let quad = text.rfind(':').map(|colon| i + colon + 1);
        return Some(quad.and_then(|start| ipv4(bytes, start)).unwrap_or(end));
    }
    Some(end)
}

/// Whether `text` is an address literal, judged by `std`'s own IPv6 parser plus two deliberate
/// deviations.
///
/// `std` settles the hard part — group widths, the single `::`, the `::ffff:1.2.3.4` tail — and it
/// already rejects the colon-separated counters foreign logs are full of (`1:02:03:04` uptime,
/// `0:00:01:23` elapsed, `abc:def:012:345` ids). The deviations: a MAC is accepted, because it
/// identifies a machine just as an address does; and a digit-free COMPRESSED literal is rejected,
/// because `feed::live` and `dead::beef` parse as valid IPv6 while being a Rust path and a word.
/// The uncompressed form needs no such guard — nothing writes seven colons by accident — so an
/// all-letter address such as `cafe:babe:dead:beef:cafe:babe:dead:beef` is still masked.
fn is_ipv6_text(text: &str) -> bool {
    if is_mac(text) {
        return true;
    }
    text.parse::<std::net::Ipv6Addr>().is_ok()
        && (!text.contains("::") || text.bytes().any(|b| b.is_ascii_digit()))
}

/// Whether `text` is a six-group MAC address (`aa:bb:cc:dd:ee:ff`).
fn is_mac(text: &str) -> bool {
    let mut groups = 0usize;
    for group in text.split(':') {
        if group.len() != 2 || !group.bytes().all(|b| b.is_ascii_hexdigit()) {
            return false;
        }
        groups += 1;
    }
    groups == 6
}

/// End index past a `:port` suffix at `end`, or `end` itself when no port follows the address.
///
/// `addr:port: message` is the shape moonproto errors take, so a following colon still ends the
/// port — [`ends_token`] treats it as a terminator.
fn port_suffix(bytes: &[u8], end: usize) -> usize {
    if bytes.get(end) != Some(&b':') {
        return end;
    }
    let start = end + 1;
    let mut pos = start;
    while pos < bytes.len() && bytes[pos].is_ascii_digit() && pos - start < 5 {
        pos += 1;
    }
    if pos == start {
        return end;
    }
    ends_token(bytes, pos).unwrap_or(end)
}

#[cfg(test)]
mod tests;
