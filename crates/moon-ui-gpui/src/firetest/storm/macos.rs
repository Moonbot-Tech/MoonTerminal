//! macOS mouse storm: post real HID-level `MouseMoved` events through CoreGraphics.
//!
//! The test machine may need Accessibility or Input Monitoring permission — that is macOS policy
//! for programmatic mouse events, not a FireTest setting.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use core_graphics::event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;

use crate::diag;
use crate::firetest::probe::ChartProbe;

use super::MouseStorm;

/// Spawn the storm thread over `probe`'s screen rect.
pub(in crate::firetest) fn start_mouse_storm(
    probe: ChartProbe,
    duration: Duration,
    mouse_hz: f64,
) -> Result<MouseStorm, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let thread_done = done.clone();
    std::thread::Builder::new()
        .name("moon-firetest-mouse".to_string())
        .spawn(move || {
            let Ok(source) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) else {
                diag::bump(&diag::FIRETEST_MOUSE_POST_FAIL);
                thread_done.store(true, Ordering::Relaxed);
                return;
            };
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
            while start.elapsed() < duration && !thread_stop.load(Ordering::Relaxed) {
                let angle = sent as f32 * step;
                let point =
                    CGPoint::new((cx + angle.cos() * r) as f64, (cy + angle.sin() * r) as f64);
                match CGEvent::new_mouse_event(
                    source.clone(),
                    CGEventType::MouseMoved,
                    point,
                    CGMouseButton::Left,
                ) {
                    Ok(event) => {
                        event.post(CGEventTapLocation::HID);
                        diag::bump(&diag::FIRETEST_MOUSE_SENT);
                    }
                    Err(_) => diag::bump(&diag::FIRETEST_MOUSE_POST_FAIL),
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
            thread_done.store(true, Ordering::Relaxed);
        })
        .map_err(|e| format!("failed to spawn mouse storm thread: {e}"))?;
    Ok(MouseStorm { stop, done })
}
