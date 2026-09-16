//! Shared matching keys for Moonbot sound names and the terminal's flat sound catalog.

/// Returns the final slash- or backslash-separated component of `name`, trimmed,
/// ASCII-lowercased and without a trailing `.wav`. An empty tail returns an empty key.
/// This key is for matching only; keep the original name for core writes and storage.
pub fn sound_stem(name: &str) -> String {
    let tail = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let low = tail.trim().to_ascii_lowercase();
    low.strip_suffix(".wav").unwrap_or(&low).to_string()
}

#[cfg(test)]
mod tests;
