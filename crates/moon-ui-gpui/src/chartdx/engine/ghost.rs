//! Weak comparison crosshair handles and initial chart palette.

use super::*;

/// Weak handle for another engine's ghost crosshair in comparison mode. A hovered panel holds one
/// handle per peer chart in the same tab stack and writes the cursor price to each on mouse movement.
/// Each peer draws a horizontal line plus volume and percentage from its own data. Like the real
/// cursor, this bypasses GPUI notification.
#[derive(Clone)]
pub struct ChartGhostCursor {
    pub(super) state: std::rc::Weak<RefCell<RenderState>>,
}

impl ChartGhostCursor {
    pub fn set_price(&self, price: Option<f64>) {
        if let Some(state) = self.state.upgrade() {
            let mut state = state.borrow_mut();
            // Receiving a price means the mouse is over another chart in the tab, so this chart
            // cannot have a real cursor. Clear a stale crosshair left when a neighbor's fast-path
            // mouse-move stop_propagation consumed panel hover-out; otherwise the ghost cannot appear
            // because the real cursor takes precedence in `render_state.rs::sync_cursor_params`. This is
            // idempotent for panels whose cursor is already None. Do not clear on price=None because
            // the mouse may have just entered this chart, where its real cursor is valid.
            if price.is_some() {
                state.set_cursor(None);
            }
            state.set_ghost_price(price.map(|p| p as f32));
        }
    }
}

pub(super) fn hex3(rgb: [u8; 3]) -> u32 {
    ((rgb[0] as u32) << 16) | ((rgb[1] as u32) << 8) | rgb[2] as u32
}

pub(super) fn initial_palette_from_theme(theme: &ChartTheme) -> moon_ui::MoonPalette {
    let base = moon_ui::MoonPalette::default();
    let panel = hex3(theme.panel_bg);
    let chart_bg = hex3(theme.bg);
    let border = hex3(theme.grid);
    let accent = hex3(theme.cross);
    let green = hex3(theme.book_bid);
    let orange = hex3(theme.book_ask);
    moon_ui::MoonPalette {
        shell: panel,
        shell_high: panel,
        window: panel,
        surface: chart_bg,
        panel,
        panel_high: panel,
        chrome: panel,
        tabbar: panel,
        panel_head: panel,
        gutter: panel,
        chart_bg,
        card: panel,
        row_alt: panel,
        head_row: panel,
        border,
        border_soft: border,
        border_card: border,
        border_hover: border,
        row_line: border,
        shadow: base.shadow,
        overlay: base.overlay,
        on_accent: base.on_accent,
        text: accent,
        text_soft: border,
        text_dim: accent,
        text_muted: border,
        text_faint: border,
        table_head: panel,
        table_body: panel,
        table_selected: panel,
        table_hover: panel,
        green,
        green_btn: green,
        green_text: green,
        red: orange,
        red_text: orange,
        red_soft_bd: orange,
        orange,
        amber: accent,
        blue: accent,
        accent,
        accent_fg: accent,
        accent_tint_a: base.accent_tint_a,
        yellow: accent,
    }
}
