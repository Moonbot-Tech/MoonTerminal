//! Sizes in the user's language: the terminal's Storage tab and the station's status share them.

use rust_i18n::t;

const KIB: f64 = 1024.0;

/// Bytes in kilobytes (whole), megabytes (one decimal) or gigabytes (two decimals), each rounded
/// to the nearest; binary units, as the operating system counts them.
pub fn size_text(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= KIB * KIB * KIB {
        t!(
            "common.size_gb",
            value = format!("{:.2}", b / KIB / KIB / KIB)
        )
        .to_string()
    } else if b >= KIB * KIB {
        t!("common.size_mb", value = format!("{:.1}", b / KIB / KIB)).to_string()
    } else {
        t!("common.size_kb", value = format!("{:.0}", b / KIB)).to_string()
    }
}

#[cfg(test)]
mod tests;
