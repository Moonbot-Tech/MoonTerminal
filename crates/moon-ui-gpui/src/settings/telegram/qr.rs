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

/// Draw the grid as GPUI quads, sized so a phone camera can read it.
///
/// Args:
///     matrix: Module grid from [`encode`].
///     cx: App for UI-scale conversion of the module size.
///
/// Returns:
///     A square canvas: quiet zone of 4 modules on every side, white background, black modules.
pub(super) fn qr_element(matrix: &QrMatrix, cx: &App) -> AnyElement {
    let module = design::ui_px(cx, 4.0).max(px(4.0));
    let side = px(f32::from(module) * (matrix.width + 8) as f32);
    let width = matrix.width;
    let dark = matrix.dark.clone();
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let w = f32::from(bounds.size.width);
            let h = f32::from(bounds.size.height);
            let need = f32::from(side);
            if w < need || h < need {
                return;
            }
            let white = hsla(0.0, 0.0, 1.0, 1.0);
            let black = hsla(0.0, 0.0, 0.0, 1.0);
            window.paint_quad(fill(bounds, white));
            let (ox, oy) = (bounds.origin.x, bounds.origin.y);
            let m = f32::from(module);
            let quiet = 4.0 * m;
            for y in 0..width {
                for x in 0..width {
                    if !dark.get(y * width + x).copied().unwrap_or(false) {
                        continue;
                    }
                    // Floor the origin and ceil the far edge so adjacent dark modules overlap by
                    // at least a fraction of a pixel and no white seam shows at fractional sizes.
                    let x0 = (quiet + x as f32 * m).floor();
                    let y0 = (quiet + y as f32 * m).floor();
                    let x1 = (quiet + (x + 1) as f32 * m).ceil();
                    let y1 = (quiet + (y + 1) as f32 * m).ceil();
                    window.paint_quad(fill(
                        Bounds::from_corners(
                            gpui::point(ox + px(x0), oy + px(y0)),
                            gpui::point(ox + px(x1.max(x0 + 1.0)), oy + px(y1.max(y0 + 1.0))),
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
