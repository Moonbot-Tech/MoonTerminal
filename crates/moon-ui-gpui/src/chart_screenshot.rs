//! Chart screenshot helpers.
//!
//! The chart renderer is an own-pass GPU layer, so the hotkey captures the last screen rectangle
//! occupied by a chart panel and asks the host OS to put that region on the image clipboard.

use std::process::Command;

#[derive(Clone, Copy, Debug)]
pub(crate) struct ScreenRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct WindowCrop {
    pub window_number: u32,
    pub rect: ScreenRect,
    pub scale_factor: f32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ScreenshotTarget {
    pub screen_rect: ScreenRect,
    pub window_crop: Option<WindowCrop>,
}

impl ScreenRect {
    pub(crate) fn new(x: f32, y: f32, w: f32, h: f32) -> Option<Self> {
        if !(w.is_finite() && h.is_finite()) || w < 8.0 || h < 8.0 {
            return None;
        }
        Some(Self {
            x: x.round() as i32,
            y: y.round() as i32,
            w: w.round().max(8.0) as u32,
            h: h.round().max(8.0) as u32,
        })
    }
}

impl ScreenshotTarget {
    pub(crate) fn new(screen_rect: ScreenRect, window_crop: Option<WindowCrop>) -> Self {
        Self {
            screen_rect,
            window_crop,
        }
    }
}

pub(crate) fn copy_region_to_clipboard(target: ScreenshotTarget) -> anyhow::Result<()> {
    copy_region_to_clipboard_impl(target)
}

#[cfg(target_os = "macos")]
fn copy_region_to_clipboard_impl(target: ScreenshotTarget) -> anyhow::Result<()> {
    if let Some(crop) = target.window_crop {
        match copy_window_crop_to_clipboard(crop) {
            Ok(()) => return Ok(()),
            Err(error) => {
                log::warn!(
                    "chart screenshot window capture failed, falling back to screen region: {error}"
                );
            }
        }
    }
    copy_screen_region_to_clipboard(target.screen_rect)
}

#[cfg(target_os = "macos")]
fn copy_screen_region_to_clipboard(rect: ScreenRect) -> anyhow::Result<()> {
    let path = std::env::temp_dir().join("moonterminal_chart_screenshot.jpg");
    let region = format!("{},{},{},{}", rect.x, rect.y, rect.w, rect.h);
    let status = Command::new("screencapture")
        .args(["-x", "-t", "jpg", "-R", &region])
        .arg(&path)
        .status()?;
    if !status.success() {
        anyhow::bail!("screencapture failed with status {status}");
    }

    let script = format!(
        "set the clipboard to (read (POSIX file \"{}\") as JPEG picture)",
        path.display()
    );
    let status = Command::new("osascript").args(["-e", &script]).status()?;
    if !status.success() {
        anyhow::bail!("osascript clipboard write failed with status {status}");
    }
    let _ = std::fs::remove_file(path);
    Ok(())
}

#[cfg(target_os = "macos")]
fn copy_window_crop_to_clipboard(crop: WindowCrop) -> anyhow::Result<()> {
    let tmp = std::env::temp_dir();
    let window_path = tmp.join("moonterminal_chart_window.png");
    let chart_path = tmp.join("moonterminal_chart_screenshot.png");
    let status = Command::new("screencapture")
        .args(["-x", "-o", "-l", &crop.window_number.to_string()])
        .arg(&window_path)
        .status()?;
    if !status.success() {
        anyhow::bail!("screencapture window failed with status {status}");
    }

    let scale = crop.scale_factor.max(0.1);
    let image = image::open(&window_path)?;
    let img_w = image.width();
    let img_h = image.height();
    let x = ((crop.rect.x.max(0) as f32) * scale).round() as u32;
    let y = ((crop.rect.y.max(0) as f32) * scale).round() as u32;
    let w = ((crop.rect.w as f32) * scale).round().max(8.0) as u32;
    let h = ((crop.rect.h as f32) * scale).round().max(8.0) as u32;
    if x >= img_w || y >= img_h {
        anyhow::bail!("chart crop origin outside window image: crop={x},{y} image={img_w}x{img_h}");
    }
    let w = w.min(img_w - x);
    let h = h.min(img_h - y);
    image.crop_imm(x, y, w, h).save(&chart_path)?;

    let script = format!(
        "set the clipboard to (read (POSIX file \"{}\") as «class PNGf»)",
        chart_path.display()
    );
    let status = Command::new("osascript").args(["-e", &script]).status()?;
    if !status.success() {
        anyhow::bail!("osascript clipboard write failed with status {status}");
    }
    let _ = std::fs::remove_file(window_path);
    let _ = std::fs::remove_file(chart_path);
    Ok(())
}

#[cfg(windows)]
fn copy_region_to_clipboard_impl(target: ScreenshotTarget) -> anyhow::Result<()> {
    let rect = target.screen_rect;
    let script = format!(
        r#"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$bmp = New-Object System.Drawing.Bitmap({w}, {h})
$gfx = [System.Drawing.Graphics]::FromImage($bmp)
$gfx.CopyFromScreen({x}, {y}, 0, 0, $bmp.Size)
$gfx.Dispose()
[System.Windows.Forms.Clipboard]::SetImage($bmp)
"#,
        x = rect.x,
        y = rect.y,
        w = rect.w,
        h = rect.h
    );
    let status = Command::new("powershell.exe")
        .args(["-NoProfile", "-STA", "-Command", &script])
        .status()?;
    if !status.success() {
        anyhow::bail!("PowerShell clipboard screenshot failed with status {status}");
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn copy_region_to_clipboard_impl(target: ScreenshotTarget) -> anyhow::Result<()> {
    let rect = target.screen_rect;
    let path = std::env::temp_dir().join("moonterminal_chart_screenshot.png");
    let grim_geometry = format!("{},{} {}x{}", rect.x, rect.y, rect.w, rect.h);
    let maim_geometry = format!("{}x{}+{}+{}", rect.w, rect.h, rect.x, rect.y);
    let captured = Command::new("grim")
        .args(["-g", &grim_geometry])
        .arg(&path)
        .status()
        .ok()
        .is_some_and(|s| s.success())
        || Command::new("maim")
            .args(["-g", &maim_geometry])
            .arg(&path)
            .status()
            .ok()
            .is_some_and(|s| s.success());
    if !captured {
        anyhow::bail!("no supported Linux screenshot tool found (grim or maim)");
    }
    let copied = std::fs::File::open(&path)
        .ok()
        .and_then(|file| {
            Command::new("wl-copy")
                .args(["--type", "image/png"])
                .stdin(std::process::Stdio::from(file))
                .status()
                .ok()
        })
        .is_some_and(|s| s.success())
        || std::fs::File::open(&path)
            .ok()
            .and_then(|file| {
                Command::new("xclip")
                    .args(["-selection", "clipboard", "-t", "image/png", "-i"])
                    .stdin(std::process::Stdio::from(file))
                    .status()
                    .ok()
            })
            .is_some_and(|s| s.success());
    let _ = std::fs::remove_file(path);
    if !copied {
        anyhow::bail!("no supported Linux clipboard tool found (wl-copy or xclip)");
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
fn copy_region_to_clipboard_impl(_target: ScreenshotTarget) -> anyhow::Result<()> {
    anyhow::bail!("chart screenshot is not implemented on this platform")
}
