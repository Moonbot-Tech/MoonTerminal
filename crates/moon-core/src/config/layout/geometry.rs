//! Window geometry and legacy detached-window state.

use super::*;

/// Window rectangle (outer position + inner size, logical pixels).
///
/// Compared as a whole when deciding whether a move is worth persisting: the display is part of the
/// placement, and on macOS — where coordinates are relative to the window's own screen — the same
/// x/y on a different monitor is a real move that a coordinates-only comparison would discard.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeomRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    /// Whether the window was left MAXIMIZED.
    ///
    /// The rectangle above stays the RESTORE rectangle while this is set: the platform reports
    /// where a maximized window will go once it is un-maximized, and that is what has to survive
    /// a restart. The flag rides beside it rather than replacing it.
    ///
    /// Absent from older config files, and not written out when false, so an untouched file keeps
    /// its previous shape. Decoded leniently for the same reason [`Self::display_uuid`] is: one
    /// mistyped value here would otherwise reject the WHOLE document and cost the user every
    /// window position and column width in it.
    #[serde(
        default,
        deserialize_with = "de_lenient_bool",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub maximized: bool,
    /// macOS fullscreen state (`WindowBounds::Fullscreen`). Separate from [`Self::maximized`]:
    /// the green macOS button produces Fullscreen rather than Maximized, and it must be restored
    /// using its own variant or the window will open normally.
    #[serde(
        default,
        deserialize_with = "de_lenient_bool",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub fullscreen: bool,
    /// Display this window was last seen on, when the platform could name one.
    ///
    /// `x`/`y` alone identify a monitor only where window coordinates are global — Windows and X11.
    /// macOS reports them RELATIVE to the window's own screen and reports a zero origin for every
    /// display, so there the saved point cannot say which monitor it belongs to and the window comes
    /// back on whichever display the app happens to open it on. This is that missing half; it is a
    /// hint, never a requirement — an unplugged or renumbered monitor simply fails to resolve and
    /// the caller falls back to the coordinate and owner-window routes it used before.
    ///
    /// Absent from older config files and from a window whose platform reports no display id, hence
    /// `Option` plus `serde(default)`; it is not written out when absent so an untouched file keeps
    /// its previous shape.
    #[serde(
        default,
        deserialize_with = "de_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub display_uuid: Option<uuid::Uuid>,
}

impl GeomRect {
    /// Restore onto the chosen work area, replacing unreachable geometry with `fallback`.
    ///
    /// `displays` uses the same logical coordinate space as this rectangle. Reachability and
    /// containment are separate: even a reachable oversized window must shrink and move inward.
    /// Window state and display identity survive replacement of the rectangle.
    #[must_use]
    pub fn restored_on(
        self,
        displays: &[(i32, i32, u32, u32)],
        work: ScreenRect,
        fallback: ScreenRect,
    ) -> Self {
        let candidate = if self.is_reachable_on(displays, MIN_VISIBLE_PX) {
            ScreenRect {
                x: self.x as f32,
                y: self.y as f32,
                w: self.w as f32,
                h: self.h as f32,
            }
        } else {
            fallback
        };
        let rect = candidate.clamped_to(work);
        Self {
            x: rect.x as i32,
            y: rect.y as i32,
            w: rect.w as u32,
            h: rect.h as u32,
            ..self
        }
    }

    /// Keep a previously known display when the platform cannot name one right now.
    ///
    /// `None` from the platform means "unknown", not "moved to nowhere": off macOS it is the normal
    /// answer (the identity is not read there at all), and even on macOS a window mid-move can
    /// briefly report no display. Treating that as a change would erase a good identity — and,
    /// because the comparison that decides whether to save is now whole-struct, would also dirty
    /// the layout on every such blip.
    ///
    /// # Arguments
    ///
    /// * `previous` - Geometry this window was last saved with, if any.
    ///
    /// # Returns
    ///
    /// This rectangle, keeping the earlier identity when it has none of its own.
    #[must_use]
    pub fn keeping_display_of(mut self, previous: Option<GeomRect>) -> Self {
        self.display_uuid = self
            .display_uuid
            .or_else(|| previous.and_then(|previous| previous.display_uuid));
        self
    }

    /// Whether this rectangle still lands somewhere the user can actually reach.
    ///
    /// A saved geometry outlives the monitors it was saved on: a laptop undocked from a second
    /// screen, a display rearranged, a resolution changed. Restoring such a rectangle opens the
    /// window at coordinates no monitor covers, where it is invisible and — for a window that
    /// hides its taskbar button, as the trade window does — unreachable. The caller falls back to
    /// its own default placement instead.
    ///
    /// The test is OVERLAP AREA, not containment: a window deliberately hanging off the edge of a
    /// screen is a placement the user chose, and demanding full containment would move it back on
    /// every reopen. What it rejects is a rectangle whose intersection with every attached display
    /// is too small to grab — the title bar has to be on a screen for the window to be draggable.
    ///
    /// Pure, and free of any window-system type, so the rule is unit-testable without a display:
    /// `displays` is simply the attached monitors as `(x, y, w, h)` in the same coordinate space
    /// the rectangle was saved in. An EMPTY list means the caller could not enumerate displays at
    /// all, which is "unknown" rather than "nowhere" — the saved rectangle is kept, exactly as
    /// [`Self::keeping_display_of`] keeps an unknown identity.
    ///
    /// # Arguments
    ///
    /// * `displays` - Attached display rectangles as `(x, y, w, h)`.
    /// * `min_visible` - Smallest visible area, in square pixels, that still counts as reachable.
    ///
    /// # Returns
    ///
    /// `true` when the rectangle is usable as-is.
    #[must_use]
    pub fn is_reachable_on(&self, displays: &[(i32, i32, u32, u32)], min_visible: u64) -> bool {
        if self.w == 0 || self.h == 0 {
            return false;
        }
        if displays.is_empty() {
            return true;
        }
        let (left, top) = (i64::from(self.x), i64::from(self.y));
        let (right, bottom) = (left + i64::from(self.w), top + i64::from(self.h));
        displays.iter().any(|&(dx, dy, dw, dh)| {
            let (dleft, dtop) = (i64::from(dx), i64::from(dy));
            let (dright, dbottom) = (dleft + i64::from(dw), dtop + i64::from(dh));
            let overlap_w = right.min(dright) - left.max(dleft);
            let overlap_h = bottom.min(dbottom) - top.max(dtop);
            overlap_w > 0 && overlap_h > 0 && (overlap_w as u64) * (overlap_h as u64) >= min_visible
        })
    }
}

/// Legacy egui detached-tab compatibility record; live detached state uses `detached.json`.
#[derive(Clone, Serialize, Deserialize)]
pub struct DetachedLayout {
    /// Legacy tab index.
    pub tab: u8,
    /// Legacy owner group name.
    pub owner_group: String,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}
