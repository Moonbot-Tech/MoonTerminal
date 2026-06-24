//! Small chart-axis helpers shared by the native chart text path.

/// Local time offset from UTC, in seconds. Mirrors the old egui chart overlay behavior.
#[cfg(windows)]
pub fn local_offset_sec() -> i64 {
    use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};

    let (local, utc) = unsafe { (GetLocalTime(), GetSystemTime()) };
    let local_sec = local.wHour as i64 * 3600 + local.wMinute as i64 * 60 + local.wSecond as i64;
    let utc_sec = utc.wHour as i64 * 3600 + utc.wMinute as i64 * 60 + utc.wSecond as i64;
    let mut offset = local_sec - utc_sec;
    if offset > 43_200 {
        offset -= 86_400;
    } else if offset < -43_200 {
        offset += 86_400;
    }
    offset
}

#[cfg(not(windows))]
pub fn local_offset_sec() -> i64 {
    0
}
