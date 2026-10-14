//! ScreenCaptureKit stills restricted to our process and our native window number.

use std::sync::OnceLock;
use std::sync::mpsc::Sender;

use anyhow::{Context as _, bail};
use block2::RcBlock;
use objc2::{
    AnyThread as _, msg_send,
    rc::{Allocated, Retained},
    runtime::{AnyClass, AnyObject},
};
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSView, NSWindow};
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSArray, NSDictionary, NSError, NSPoint, NSRect, NSSize};

use super::{mac_policy, rect::ShotRect};

/// Load only after the 14.4 gate: the application still supports macOS 11, where this framework
/// does not exist. The generated ScreenCaptureKit crate strongly links it at process launch.
/// Keep the handle for process lifetime because its Objective-C classes remain registered.
pub(super) fn load_framework() -> bool {
    static LOADED: OnceLock<bool> = OnceLock::new();
    *LOADED.get_or_init(|| unsafe {
        !libc::dlopen(
            c"/System/Library/Frameworks/ScreenCaptureKit.framework/ScreenCaptureKit".as_ptr(),
            libc::RTLD_LAZY | libc::RTLD_LOCAL,
        )
        .is_null()
    })
}

/// Window-relative top-left crop and full-window output size, both at the native backing scale.
#[derive(Clone, Copy)]
pub(super) struct Region {
    source: [f64; 4],
    window: [f64; 2],
    scale: f64,
    slot: ShotRect,
}

/// Map physical GPUI content pixels through NSView's window-base coordinates, including titlebar.
pub(super) fn source(
    view: &NSView,
    window: &NSWindow,
    rect: ShotRect,
    scale: f32,
) -> Option<Region> {
    let scale = f64::from(scale);
    let bounds = view.bounds();
    // Check the content view before converting to the larger window frame. Otherwise a negative
    // chart origin can land inside the titlebar and copy account-identifying window chrome.
    mac_policy::crop_pixels(
        [
            f64::from(rect.x),
            f64::from(rect.y),
            f64::from(rect.width),
            f64::from(rect.height),
        ],
        1.0,
        [
            (bounds.size.width * scale).round() as u32,
            (bounds.size.height * scale).round() as u32,
        ],
    )?;
    let width = f64::from(rect.width) / scale;
    let height = f64::from(rect.height) / scale;
    let top = f64::from(rect.y) / scale;
    let local = NSRect::new(
        NSPoint::new(
            bounds.origin.x + f64::from(rect.x) / scale,
            bounds.origin.y
                + if view.isFlipped() {
                    top
                } else {
                    bounds.size.height - top - height
                },
        ),
        NSSize::new(width, height),
    );
    let base = view.convertRect_toView(local, None);
    let frame = window.frame();
    let [x, y, w, h] = mac_policy::source_rect(
        [
            base.origin.x,
            base.origin.y,
            base.size.width,
            base.size.height,
        ],
        [frame.size.width, frame.size.height],
    )?;
    Some(Region {
        source: [x, y, w, h],
        window: [frame.size.width, frame.size.height],
        scale,
        slot: rect,
    })
}

/// Request a fresh composited still, sending only owned PNG bytes across the native callback.
/// No generic shareable-content listing, whole-display capture, or Screen Recording consent path.
pub(super) fn request(window_id: u32, region: Region, sender: Sender<anyhow::Result<Vec<u8>>>) {
    let Some(content_class) = AnyClass::get(c"SCShareableContent") else {
        let _ = sender.send(Err(anyhow::anyhow!(
            "ScreenCaptureKit content class is unavailable"
        )));
        return;
    };
    let handler = RcBlock::new(move |content: *mut AnyObject, error: *mut NSError| {
        // ScreenCaptureKit keeps callback arguments alive until the callback returns. Everything
        // used by the later callback is copied/retained by the native capture request.
        let content = unsafe { content.as_ref() };
        if !error.is_null() || content.is_none() {
            let _ = sender.send(Err(anyhow::anyhow!(
                "current-process window enumeration failed"
            )));
            return;
        }
        let content = content.expect("checked non-null callback content");
        let windows: Retained<NSArray<AnyObject>> = unsafe { msg_send![content, windows] };
        let Some(window) = windows.iter().find(|window| unsafe {
            let id: u32 = msg_send![&**window, windowID];
            let on_screen: bool = msg_send![&**window, isOnScreen];
            id == window_id && on_screen
        }) else {
            let _ = sender.send(Err(anyhow::anyhow!("our window is not capturable")));
            return;
        };
        let (Some(filter_class), Some(config_class), Some(manager_class)) = (
            AnyClass::get(c"SCContentFilter"),
            AnyClass::get(c"SCStreamConfiguration"),
            AnyClass::get(c"SCScreenshotManager"),
        ) else {
            let _ = sender.send(Err(anyhow::anyhow!(
                "ScreenCaptureKit still classes are unavailable"
            )));
            return;
        };
        // These are the public selectors and exact types in Apple's 14.4+ headers, also
        // checked against objc2-screen-capture-kit's generated bindings. Dynamic lookup
        // avoids importing that crate's unconditional framework link on macOS 11/12.
        let filter: Retained<AnyObject> = unsafe {
            let allocated: Allocated<AnyObject> = msg_send![filter_class, alloc];
            msg_send![allocated, initWithDesktopIndependentWindow: &*window]
        };
        let content_rect: NSRect = unsafe { msg_send![&*filter, contentRect] };
        // With shadows excluded the output must describe the same window-base coordinates we
        // measured on the UI thread. A changed/fullscreen frame is refused rather than guessed.
        if (content_rect.size.width - region.window[0]).abs() > 0.5
            || (content_rect.size.height - region.window[1]).abs() > 0.5
        {
            let _ = sender.send(Err(anyhow::anyhow!("window bounds changed before capture")));
            return;
        }
        let width = (region.window[0] * region.scale).round() as u32;
        let height = (region.window[1] * region.scale).round() as u32;
        let config: Retained<AnyObject> = unsafe { msg_send![config_class, new] };
        let completion_sender = sender.clone();
        let completion = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
            let result = if !error.is_null() {
                Err(anyhow::anyhow!("ScreenCaptureKit still failed"))
            } else if let Some(image) = unsafe { image.as_ref() } {
                let bitmap = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), image);
                png(&bitmap).and_then(|png| crop(&png, &region, [width, height]))
            } else {
                Err(anyhow::anyhow!("ScreenCaptureKit returned no image"))
            };
            let _ = completion_sender.send(result);
        });
        unsafe {
            // Apple's sourceRect documentation explicitly excludes single-window captures.
            // Request the full native size and crop its pixels after the callback instead.
            let _: () = msg_send![&*config, setWidth: width as usize];
            let _: () = msg_send![&*config, setHeight: height as usize];
            let _: () = msg_send![&*config, setShowsCursor: false];
            let _: () = msg_send![&*config, setScalesToFit: false];
            let _: () = msg_send![&*config, setIgnoreShadowsSingleWindow: true];
            let _: () = msg_send![manager_class, captureImageWithFilter: &*filter, configuration: &*config, completionHandler: &*completion];
        }
    });
    unsafe {
        let _: () = msg_send![content_class, getCurrentProcessShareableContentWithCompletionHandler: &*handler];
    }
}

/// Cut the slot out of the full-window PNG, checking dimensions before any pixel can be published.
fn crop(png: &[u8], region: &Region, expected: [u32; 2]) -> anyhow::Result<Vec<u8>> {
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png)?;
    if [image.width(), image.height()] != expected {
        bail!("unexpected window image size");
    }
    let [x, y, width, height] = mac_policy::crop_pixels(region.source, region.scale, expected)
        .context("chart slot is outside the captured window")?;
    if width != region.slot.width || height != region.slot.height {
        bail!("chart slot size changed");
    }
    let cropped = image.crop_imm(x, y, width, height);
    let mut output = std::io::Cursor::new(Vec::new());
    cropped.write_to(&mut output, image::ImageFormat::Png)?;
    Ok(output.into_inner())
}

/// Encode a native bitmap as PNG without storing anything on disk.
pub(super) fn png(bitmap: &NSBitmapImageRep) -> anyhow::Result<Vec<u8>> {
    let data = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }
    .context("AppKit PNG encoding failed")?;
    let bytes = data.to_vec();
    if bytes.is_empty() {
        bail!("AppKit produced an empty PNG");
    }
    Ok(bytes)
}
