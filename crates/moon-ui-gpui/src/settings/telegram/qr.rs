//! Encode a Telegram login link as a module grid and draw it with GPUI quads.
//!
//! The grid is the only thing retained: the input string is a live account-takeover token and
//! must not be stored or logged. Colours are fixed black on white regardless of theme so a phone
//! camera can scan the code; a themed QR on a dark background inverts the contrast a scanner
//! depends on, and this is the one element in the app that is read by a machine rather than by a
//! person.

use gpui::*;

use crate::design;

/// A finished QR module grid: `width` modules per side, `dark[y * width + x]` per module.
pub(super) struct QrMatrix {
    pub width: usize,
    pub dark: Vec<bool>,
}

/// Encode a `tg://login?token=...` link. `None` when the crate refuses the input.
pub(super) fn encode(link: &str) -> Option<QrMatrix> {
    let code = qrcode::QrCode::new(link.as_bytes()).ok()?;
    let width = code.width();
    let dark = code
        .to_colors()
        .into_iter()
        .map(|c| c == qrcode::Color::Dark)
        .collect();
    Some(QrMatrix { width, dark })
}

/// Edge of one module the element asks the layout for, in unscaled logical pixels.
const MODULE_PX: f32 = 4.0;

/// Quiet zone on each side of the code, in modules: the minimum the QR standard asks for.
const QUIET_MODULES: usize = 4;

/// Where the module grid sits inside the area the layout granted, in device pixels.
///
/// Private to this file, fields included: [`fit_grid`] is the only place that builds one, and that
/// is what keeps both fields whole numbers.
#[derive(Debug, PartialEq)]
struct GridFit {
    /// Edge of one module: a whole number of device pixels, at least one.
    module: f32,
    /// Offset of the grid from the area's top-left corner, the same on both axes.
    inset: f32,
}

/// Place a grid of `modules` per side inside the square area the layout granted.
///
/// The grid is sized from what was granted, never from what was asked for: the layout snaps every
/// element to whole device pixels, so at a fractional scale factor the granted side is routinely a
/// fraction of a pixel short of the request. Refusing to draw into such an area left the login
/// step blank at every scale where the requested side was not a whole number of device pixels.
/// Modules are whole device pixels so that all of them are the same size, which a scanner needs
/// more than it needs the last spare pixels; those widen the quiet zone, the odd one on the far
/// side.
///
/// Args:
///     extent: Side of the granted area, in logical pixels.
///     scale_factor: Device pixels per logical pixel, interface zoom included.
///     modules: Modules per side, quiet zone included.
///
/// Returns:
///     The module edge and the grid's offset in device pixels, or `None` when the area cannot
///     give every module at least one device pixel.
fn fit_grid(extent: f32, scale_factor: f32, modules: usize) -> Option<GridFit> {
    // The granted side is a whole number of device pixels divided by the scale factor, so the
    // product is that whole number give or take float noise; rounding recovers it.
    let device = (extent * scale_factor).round();
    let module = (device / modules as f32).floor();
    // Also the exit for a zero grid or a broken scale factor: both leave `module` non-finite.
    if !module.is_finite() || module < 1.0 {
        return None;
    }
    let inset = ((device - module * modules as f32) / 2.0).floor();
    Some(GridFit { module, inset })
}

/// Draw the grid as GPUI quads, asking the layout for a side a phone camera can read.
///
/// The request is not a guarantee: the code is drawn into whatever area the layout grants, down to
/// one device pixel a module (see [`fit_grid`]), because an empty square tells the user less than
/// a small code does.
///
/// Args:
///     matrix: Module grid from [`encode`].
///     cx: App for UI-scale conversion of the module size.
///
/// Returns:
///     A square canvas: quiet zone of at least 4 modules on every side, white background, black
///     modules.
pub(super) fn qr_element(matrix: &QrMatrix, cx: &App) -> AnyElement {
    let modules = matrix.width + 2 * QUIET_MODULES;
    let module = design::ui_px(cx, MODULE_PX).max(px(MODULE_PX));
    let side = px(f32::from(module) * modules as f32);
    let width = matrix.width;
    let dark = matrix.dark.clone();
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let scale = window.scale_factor();
            let extent = f32::from(bounds.size.width).min(f32::from(bounds.size.height));
            let Some(fit) = fit_grid(extent, scale, modules) else {
                return;
            };
            let white = hsla(0.0, 0.0, 1.0, 1.0);
            let black = hsla(0.0, 0.0, 0.0, 1.0);
            window.paint_quad(fill(bounds, white));
            let (ox, oy) = (bounds.origin.x, bounds.origin.y);
            // Every module edge is a whole number of device pixels from the area's corner, so
            // neighbours share their edge exactly: no seam to cover and no module wider than the
            // next.
            let edge = |index: usize| px((fit.inset + index as f32 * fit.module) / scale);
            for y in 0..width {
                for x in 0..width {
                    if !dark.get(y * width + x).copied().unwrap_or(false) {
                        continue;
                    }
                    let (col, row) = (x + QUIET_MODULES, y + QUIET_MODULES);
                    window.paint_quad(fill(
                        Bounds::from_corners(
                            gpui::point(ox + edge(col), oy + edge(row)),
                            gpui::point(ox + edge(col + 1), oy + edge(row + 1)),
                        ),
                        black,
                    ));
                }
            }
        },
    )
    .w(side)
    .h(side)
    .into_any_element()
}

#[cfg(test)]
mod tests;
