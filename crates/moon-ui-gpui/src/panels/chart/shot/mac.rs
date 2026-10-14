//! macOS caption ownership: wait for drawn text, flush, read asynchronously, then restore.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use gpui::{AnyWindowHandle, App, ClipboardItem, Entity, Image, ImageFormat};
use moon_ui::MoonWindowExt as _;
use objc2::{MainThreadMarker, rc::Retained};
use objc2_app_kit::NSView;
use objc2_foundation::{NSOperatingSystemVersion, NSProcessInfo};
use raw_window_handle::RawWindowHandle;

use super::{ShotInputs, ShotOutcome, capture_mac, mac_policy, paint_mac, rect};
use crate::{Backend, panels::ChartPanel};

/// One shot's caption lease and optional in-flight read; replacement never restores a newer lease.
struct Shot {
    backend: Entity<Backend>,
    panel: Entity<ChartPanel>,
    window: AnyWindowHandle,
    generation: u64,
    deadline: Instant,
    ticks_left: u32,
    pending: Option<Pending>,
}

/// The geometry and header belonging to the requested image, held until its callback arrives.
struct Pending {
    pixels: Receiver<anyhow::Result<Vec<u8>>>,
    rect: rect::ShotRect,
    inputs: ShotInputs,
    when_ms: i64,
}

/// Start only on macOS 14.4+, so older systems never enumerate content or request TCC consent.
pub(super) fn begin(
    backend: Entity<Backend>,
    panel: Entity<ChartPanel>,
    window: AnyWindowHandle,
    gpui_window: &mut gpui::Window,
    cx: &mut App,
) -> Option<ShotOutcome> {
    let supported =
        NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(NSOperatingSystemVersion {
            majorVersion: 14,
            minorVersion: 4,
            patchVersion: 0,
        });
    if !supported {
        return Some(ShotOutcome::Unsupported);
    }
    if !capture_mac::load_framework() {
        return Some(ShotOutcome::Failed);
    }
    if native_view(gpui_window)
        .and_then(|view| view.window())
        .is_none()
    {
        return Some(ShotOutcome::Failed);
    }
    // ScreenCaptureKit is asynchronous. The lease remains armed through its completion, and an
    // image arriving after the watchdog restored the account caption is discarded, never pasted.
    let deadline = Instant::now() + Duration::from_secs(10);
    panel.update(cx, |panel, cx| panel.arm_shot_caption(Some(deadline), cx));
    let generation = panel.read(cx).shot_caption_gen();
    Shot {
        backend,
        panel,
        window,
        generation,
        deadline,
        ticks_left: 12,
        pending: None,
    }
    .schedule(gpui_window);
    None
}

/// Borrow the live GPUI NSView only on the main thread, never across a native callback.
fn native_view(window: &gpui::Window) -> Option<Retained<NSView>> {
    let mtm = MainThreadMarker::new()?;
    let handle = raw_window_handle::HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };
    // GPUI's HasWindowHandle guarantees the view lives for this borrow; retain it before returning.
    let _ = mtm;
    unsafe { Retained::retain(handle.ns_view.as_ptr().cast::<NSView>()) }
}

impl Shot {
    /// Queue the next observation without blocking the UI thread on a ScreenCaptureKit callback.
    fn schedule(self, window: &mut gpui::Window) {
        window.on_next_frame(move |window, cx| self.hop(window, cx));
    }

    /// Refuse missing proof, moved charts, expired leases, and stale asynchronous results.
    fn hop(mut self, window: &mut gpui::Window, cx: &mut App) {
        if self.panel.read(cx).shot_caption_gen() != self.generation {
            return;
        }
        if Instant::now() >= self.deadline {
            self.finish(ShotOutcome::Failed, window, cx);
            return;
        }
        let still_ours = super::resolve(&self.backend, self.window, cx)
            .is_some_and(|panel| panel.entity_id() == self.panel.entity_id());
        if !still_ours {
            self.finish(ShotOutcome::NoChart, window, cx);
            return;
        }
        if let Some(pending) = &self.pending {
            match pending.pixels.try_recv() {
                Err(TryRecvError::Empty) => self.schedule(window),
                Err(TryRecvError::Disconnected) => self.finish(ShotOutcome::Failed, window, cx),
                Ok(result) => {
                    let geometry =
                        self.panel
                            .read(cx)
                            .shot_geometry()
                            .and_then(|(bounds, scale, size)| {
                                rect::slot_capture_rect(bounds, scale, size)
                            });
                    let safe = mac_policy::publishable(
                        self.generation,
                        self.panel.read(cx).shot_caption_gen(),
                        self.panel.read(cx).shot_caption_drawn(),
                        self.deadline,
                        Instant::now(),
                    ) && geometry == Some(pending.rect);
                    let outcome = if safe {
                        match result.and_then(|png| {
                            paint_mac::compose(&png, &pending.inputs, pending.when_ms)
                        }) {
                            Ok(png) => {
                                cx.write_to_clipboard(ClipboardItem::new_image(
                                    &Image::from_bytes(ImageFormat::Png, png),
                                ));
                                ShotOutcome::Copied
                            }
                            Err(error) => {
                                log::warn!(
                                    "chart shot: macOS capture/composition failed: {error:#}"
                                );
                                ShotOutcome::Failed
                            }
                        }
                    } else {
                        ShotOutcome::Failed
                    };
                    self.finish(outcome, window, cx);
                }
            }
            return;
        }
        if !self.panel.read(cx).shot_caption_drawn() {
            if self.ticks_left == 0 {
                log::warn!("chart shot: macOS exchange caption never reached a frame");
                self.finish(ShotOutcome::Failed, window, cx);
            } else {
                self.ticks_left -= 1;
                self.schedule(window);
            }
            return;
        }
        let request = (|| {
            let (bounds, scale, size) = self.panel.read(cx).shot_geometry()?;
            let rect = rect::slot_capture_rect(bounds, scale, size)?;
            let view = native_view(window)?;
            let native = view.window()?;
            if native.isMiniaturized() || !native.isVisible() {
                return None;
            }
            let source = capture_mac::source(&view, &native, rect, scale)?;
            let layer = view.layer()?;
            let (sender, receiver) = mpsc::channel();
            let panel = self.panel.clone();
            let backend = self.backend.clone();
            let handle = self.window;
            let generation = self.generation;
            let deadline = self.deadline;
            // CA's displayLayer delegate in MoonUI uses presentsWithTransaction, commits the
            // Metal command buffer, waitsUntilScheduled, and presents its drawable in the layer
            // transaction. Force that path, then flush it before requesting a WindowServer still.
            // Yield to the main executor first: effect-cycle defer is too early, because the
            // display-link callback is still taken until the native step() call returns.
            cx.spawn(async move |cx| {
                let safe = cx.update(|cx| {
                    let refreshed = handle.update(cx, |_, window, _| window.refresh()).is_ok();
                    refreshed
                        && mac_policy::publishable(
                            generation,
                            panel.read(cx).shot_caption_gen(),
                            panel.read(cx).shot_caption_drawn(),
                            deadline,
                            Instant::now(),
                        )
                        && super::resolve(&backend, handle, cx)
                            .is_some_and(|current| current.entity_id() == panel.entity_id())
                });
                if !safe {
                    return;
                }
                // No App borrow may be held here either: the delegate updates the application.
                layer.setNeedsDisplay();
                layer.displayIfNeeded();
                objc2_quartz_core::CATransaction::flush();
                // Device recovery during the synchronous frame invalidates the earlier proof.
                let safe = cx.update(|cx| {
                    mac_policy::publishable(
                        generation,
                        panel.read(cx).shot_caption_gen(),
                        panel.read(cx).shot_caption_drawn(),
                        deadline,
                        Instant::now(),
                    )
                });
                if !safe {
                    return;
                }
                capture_mac::request(native.windowNumber() as u32, source, sender);
            })
            .detach();
            Some(Pending {
                pixels: receiver,
                rect,
                inputs: self.panel.read(cx).shot_inputs(self.backend.read(cx)),
                when_ms: chrono::Utc::now().timestamp_millis(),
            })
        })();
        if let Some(pending) = request {
            self.pending = Some(pending);
            self.schedule(window);
        } else {
            self.finish(ShotOutcome::Failed, window, cx);
        }
    }

    /// Restore synchronously before notifying; a newer shot owns both operations instead.
    fn finish(self, outcome: ShotOutcome, window: &mut gpui::Window, cx: &mut App) {
        if self.panel.read(cx).shot_caption_gen() != self.generation {
            return;
        }
        self.panel
            .update(cx, |panel, cx| panel.arm_shot_caption(None, cx));
        window.push_notification(outcome.notification(), cx);
    }
}
