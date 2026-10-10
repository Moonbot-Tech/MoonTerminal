//! Backend-independent rectangle occupancy for order captions at the book edge.
//!
//! Ordinary numbers reserve their own rows before expendable size/depth captions. Only
//! horizontally overlapping numbers, pinned captions, or interacting captions displace them.

/// One caption's logical-pixel rectangle and visibility policy.
#[derive(Clone, Debug)]
pub(in crate::chartdx) struct OrderCaption {
    /// Index in the chart-label list, or in the book-label list for depth captions.
    pub source: usize,
    /// Depth captions draw inside the book; chart captions draw to its left.
    pub book: bool,
    /// Identity keeps a single order's size/depth pair intact across the edge.
    pub uid: u64,
    /// Unnumbered size and depth are expendable; numbered captions must remain.
    pub secondary: bool,
    /// Hovered and dragged orders retain every caption.
    pub interacting: bool,
    /// Pinned labels already have an edge stack and bypass row suppression/displacement.
    pub pinned: bool,
    /// Distance of the line from the current price, for secondary-caption arbitration.
    pub distance: f32,
    /// Text top before and after layout. `None` means the secondary caption is hidden.
    pub top: Option<f32>,
    /// Padded horizontal extent in logical pixels, measured with the drawing font.
    pub x: [f32; 2],
    /// Text line height, including its backing plate's vertical insets.
    pub height: f32,
}

/// Whether padded horizontal extents overlap, preserving a same-order pair across the edge.
fn competes(a: &OrderCaption, b: &OrderCaption) -> bool {
    !(a.uid == b.uid && a.book != b.book) && a.x[0] < b.x[1] && b.x[0] < a.x[1]
}

/// Whether a proposed row intersects an accepted caption's padded band.
fn occupied(caption: &OrderCaption, top: f32, accepted: &[OrderCaption]) -> bool {
    accepted.iter().any(|other| {
        other.top.is_some_and(|y| {
            competes(caption, other) && top < y + other.height && y < top + caption.height
        })
    })
}

/// Resolve captions in place without shaping text or allocating another frame buffer.
///
/// Pinned rows and interacting sizes reserve space before interacting numbers, so even two
/// protected orders cannot cover a number with a later size. Ordinary numbers reserve their own
/// rows before secondary captions, which keep only the nearest line in an overlapping rectangle.
/// Numbers move to the closest free band only around horizontally overlapping reserved captions.
/// The input uses padded logical-pixel rectangles; output retains source indices after sorting.
/// Displaced rows prefer the supplied visible band; when it is completely full they overflow
/// rather than dropping a numbered caption or clamping it onto an occupied row.
pub(in crate::chartdx::text) fn layout_order_captions(
    captions: &mut [OrderCaption],
    visible: [f32; 2],
) {
    captions.sort_unstable_by(|a, b| {
        let rank = |c: &OrderCaption| {
            if c.pinned {
                0
            } else if c.interacting && c.secondary {
                1
            } else if c.interacting {
                2
            } else if !c.secondary {
                3
            } else {
                4
            }
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| a.distance.total_cmp(&b.distance))
            .then_with(|| a.uid.cmp(&b.uid))
            .then_with(|| a.book.cmp(&b.book))
            .then_with(|| a.source.cmp(&b.source))
    });
    for i in 0..captions.len() {
        let (accepted, rest) = captions.split_at_mut(i);
        let caption = &mut rest[0];
        let Some(top) = caption.top else { continue };
        if caption.pinned || !occupied(caption, top, accepted) {
            continue;
        }
        if caption.secondary {
            if !caption.interacting {
                caption.top = None;
            }
            continue;
        }
        // A nearest free interval starts or ends at an occupied band's boundary. Searching those
        // boundaries finds the closest slot without a pixel loop or an arbitrary displacement cap.
        let mut best = None;
        let mut distance = f32::INFINITY;
        let mut best_inside = None;
        let mut distance_inside = f32::INFINITY;
        for other in accepted.iter().filter(|other| competes(caption, other)) {
            let Some(y) = other.top else { continue };
            for candidate in [y - caption.height, y + other.height] {
                let delta = (candidate - top).abs();
                if delta < distance && !occupied(caption, candidate, accepted) {
                    best = Some(candidate);
                    distance = delta;
                }
                if candidate >= visible[0]
                    && candidate + caption.height <= visible[1]
                    && delta < distance_inside
                    && !occupied(caption, candidate, accepted)
                {
                    best_inside = Some(candidate);
                    distance_inside = delta;
                }
            }
        }
        caption.top = best_inside.or(best).or(Some(top));
    }
}

#[cfg(test)]
mod tests;
