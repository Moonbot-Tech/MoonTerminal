//! Windows mouse storm: raise the chart window, then drive both a posted `WM_MOUSEMOVE` and the
//! real cursor position so the run exercises the same path a hand on the mouse would.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, HWND_TOP, PostMessageW, SW_RESTORE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
    SetCursorPos, SetForegroundWindow, SetWindowPos, ShowWindow, WM_MOUSEMOVE,
};

use crate::diag;
use crate::firetest::logging::firetest_info;
use crate::firetest::probe::ChartProbe;

use super::MouseStorm;

/// Spawn the storm thread over `probe`'s client rect, restoring the cursor when it ends.
pub(in crate::firetest) fn start_mouse_storm(
    probe: ChartProbe,
    duration: Duration,
    mouse_hz: f64,
) -> Result<MouseStorm, String> {
    let hwnd = probe
        .hwnd
        .ok_or_else(|| "Windows mouse storm needs a Win32 HWND probe".to_string())?;
    firetest_info(&format!(
        "[firetest] mouse_storm target hwnd={hwnd:?} client_rect=({:.1},{:.1},{:.1},{:.1}) scale={:.3}",
        probe.left, probe.top, probe.width, probe.height, probe.scale_factor
    ));

    let stop = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let thread_done = done.clone();
    std::thread::Builder::new()
        .name("moon-firetest-mouse".to_string())
        .spawn(move || {
            let start = Instant::now();
            let hwnd = HWND(hwnd as *mut _);
            unsafe {
                let _ = ShowWindow(hwnd, SW_RESTORE);
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOP),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
                );
                let _ = SetForegroundWindow(hwnd);
            }
            let mut restore = POINT { x: 0, y: 0 };
            let restore_cursor = unsafe { GetCursorPos(&mut restore).is_ok() };
            let left = probe.left;
            let top = probe.top;
            let width = probe.width;
            let height = probe.height;
            let cx = left + width * 0.5;
            let cy = top + height * 0.5;
            let r = (width.min(height) * 0.35).max(12.0);
            let step = (2.0 * std::f32::consts::PI) / 96.0;
            let mut sent = 0_u64;
            while start.elapsed() < duration && !thread_stop.load(Ordering::Relaxed) {
                let angle = sent as f32 * step;
                let x = (cx + angle.cos() * r).round() as i32;
                let y = (cy + angle.sin() * r).round() as i32;
                let mut point = POINT { x, y };
                let moved = unsafe {
                    let posted = PostMessageW(
                        Some(hwnd),
                        WM_MOUSEMOVE,
                        WPARAM(0),
                        LPARAM(((y as isize) << 16) | (x as u16 as isize)),
                    )
                    .is_ok();
                    let cursor_moved = ClientToScreen(hwnd, &mut point).as_bool()
                        && SetCursorPos(point.x, point.y).is_ok();
                    posted || cursor_moved
                };
                if moved {
                    diag::bump(&diag::FIRETEST_MOUSE_SENT);
                } else {
                    diag::bump(&diag::FIRETEST_MOUSE_POST_FAIL);
                }
                sent = sent.wrapping_add(1);
                let target = Duration::from_secs_f64(sent as f64 / mouse_hz.max(1.0));
                let elapsed = start.elapsed();
                if target > elapsed {
                    std::thread::sleep(target - elapsed);
                } else if sent % 128 == 0 {
                    std::thread::yield_now();
                }
            }
            if restore_cursor {
                unsafe {
                    let _ = SetCursorPos(restore.x, restore.y);
                }
            }
            thread_done.store(true, Ordering::Relaxed);
        })
        .map_err(|e| format!("failed to spawn mouse storm thread: {e}"))?;
    Ok(MouseStorm { stop, done })
}
