//! Linux mouse storm: XTest fake motion on the X11 display of the real test session.
//!
//! Wayland without XWayland/XTest is not covered here — it needs a synthetic platform hook,
//! `uinput`, or a compositor-specific runner.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use x11::{xlib, xtest};

use crate::diag;
use crate::firetest::logging::firetest_info;
use crate::firetest::probe::ChartProbe;

use super::MouseStorm;

/// Spawn the storm thread over `probe`'s root-window rect, restoring the pointer when it ends.
pub(in crate::firetest) fn start_mouse_storm(
    probe: ChartProbe,
    duration: Duration,
    mouse_hz: f64,
) -> Result<MouseStorm, String> {
    firetest_info(&format!(
        "[firetest] mouse_storm target x11 root_rect=({:.1},{:.1},{:.1},{:.1}) scale={:.3}",
        probe.screen_left + probe.left,
        probe.screen_top + probe.top,
        probe.width,
        probe.height,
        probe.scale_factor
    ));

    let stop = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let thread_done = done.clone();
    std::thread::Builder::new()
        .name("moon-firetest-mouse".to_string())
        .spawn(move || {
            let start = Instant::now();
            let left = probe.screen_left + probe.left;
            let top = probe.screen_top + probe.top;
            let width = probe.width;
            let height = probe.height;
            let cx = left + width * 0.5;
            let cy = top + height * 0.5;
            let r = (width.min(height) * 0.35).max(12.0);
            let step = (2.0 * std::f32::consts::PI) / 96.0;
            let mut sent = 0_u64;

            unsafe {
                let display = xlib::XOpenDisplay(std::ptr::null());
                if display.is_null() {
                    diag::bump(&diag::FIRETEST_MOUSE_POST_FAIL);
                    thread_done.store(true, Ordering::Relaxed);
                    return;
                }

                let mut event_base = 0;
                let mut error_base = 0;
                let mut major = 0;
                let mut minor = 0;
                if xtest::XTestQueryExtension(
                    display,
                    &mut event_base,
                    &mut error_base,
                    &mut major,
                    &mut minor,
                ) == 0
                {
                    diag::bump(&diag::FIRETEST_MOUSE_POST_FAIL);
                    xlib::XCloseDisplay(display);
                    thread_done.store(true, Ordering::Relaxed);
                    return;
                }

                let screen = xlib::XDefaultScreen(display);
                let root = xlib::XDefaultRootWindow(display);
                let mut restore_root = 0;
                let mut restore_child = 0;
                let mut restore_x = 0;
                let mut restore_y = 0;
                let mut win_x = 0;
                let mut win_y = 0;
                let mut mask = 0;
                let restore_cursor = xlib::XQueryPointer(
                    display,
                    root,
                    &mut restore_root,
                    &mut restore_child,
                    &mut restore_x,
                    &mut restore_y,
                    &mut win_x,
                    &mut win_y,
                    &mut mask,
                ) != 0;

                while start.elapsed() < duration && !thread_stop.load(Ordering::Relaxed) {
                    let angle = sent as f32 * step;
                    let x = (cx + angle.cos() * r).round() as i32;
                    let y = (cy + angle.sin() * r).round() as i32;
                    if xtest::XTestFakeMotionEvent(display, screen, x, y, 0) != 0 {
                        diag::bump(&diag::FIRETEST_MOUSE_SENT);
                    } else {
                        diag::bump(&diag::FIRETEST_MOUSE_POST_FAIL);
                    }
                    sent = sent.wrapping_add(1);
                    if sent % 16 == 0 {
                        xlib::XFlush(display);
                    }
                    let target = Duration::from_secs_f64(sent as f64 / mouse_hz.max(1.0));
                    let elapsed = start.elapsed();
                    if target > elapsed {
                        std::thread::sleep(target - elapsed);
                    } else if sent % 128 == 0 {
                        std::thread::yield_now();
                    }
                }

                if restore_cursor {
                    xtest::XTestFakeMotionEvent(display, screen, restore_x, restore_y, 0);
                }
                xlib::XSync(display, 0);
                xlib::XCloseDisplay(display);
            }

            thread_done.store(true, Ordering::Relaxed);
        })
        .map_err(|e| format!("failed to spawn mouse storm thread: {e}"))?;
    Ok(MouseStorm { stop, done })
}
