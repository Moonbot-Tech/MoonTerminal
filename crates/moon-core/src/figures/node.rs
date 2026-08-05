//! The one point type every figure is built from.

use serde::{Deserialize, Serialize};

/// Figure node represented by a `(time, price)` point. Time is Unix milliseconds.
///
/// Price-only tools (a horizontal line, a horizontal channel) still use this type: they keep a
/// node whose time is meaningless and which their `move_handle`/`translate` ignore. That keeps
/// dragging, handle picking and geometry generic over "a figure is a list of nodes" instead of
/// carrying a second point type through every one of them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FigNode {
    pub time_ms: f64,
    pub price: f64,
}

impl FigNode {
    /// A node at a moment and a price.
    pub fn new(time_ms: f64, price: f64) -> Self {
        Self { time_ms, price }
    }

    /// Moves the node by a time and price delta.
    pub fn shifted(self, dt_ms: f64, dp: f64) -> Self {
        Self {
            time_ms: self.time_ms + dt_ms,
            price: self.price + dp,
        }
    }
}
