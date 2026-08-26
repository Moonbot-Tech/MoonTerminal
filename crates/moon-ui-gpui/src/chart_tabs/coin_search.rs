//! Per-tab/window coin input (search plus the "COIN - Server" dropdown), implemented as
//! a SHIM over the shared [`crate::controls::coin_search`] widget.
//!
//! The widget itself (market-universe search and list rendering) moved to
//! `controls/coin_search.rs`. It is shared by the tab strip, detached windows, the header price
//! ticker, and the Report coin filter. Only chart-specific behavior ([`MANUAL_COIN_TTL_MS`]) and
//! re-exports remain here, allowing `coin_search::search` / `coin_search::render_popup` calls in
//! `chart_tabs` (and `shell/ticker.rs`) to keep working unchanged.
//!
//! Search covers the market universe of cores belonging to the tab (Main/detached window):
//! Main and `Shared` use every core in the group, `Core(id)` uses one core, and `Bundle` uses the
//! bundle's cores. A selection opens the coin on the ACTIVE tab (Main → fullscreen chart;
//! Add/Custom → stack), as defined by the host through `on_pick` (see
//! [`super::common::coin_pick_handler`]).

/// Maximum number of MoonProto search matches to retrieve per core.
pub(super) use crate::controls::coin_search::COIN_MATCH_LIMIT;
/// Shared coin search and dropdown renderer (`pub(crate)` because the header price ticker in
/// `shell/ticker.rs` also reuses it).
pub(crate) use crate::controls::coin_search::{render_popup, search};

/// TTL of a manually added coin in an Add stack: none, so a coin opened by hand does not expire
/// under the detections' automatic TTL. Main opens without a TTL (`open_or_focus`). This
/// chart-specific policy does not belong in the shared widget.
///
/// It used to be "about a year" — the same eternity, but as a FINITE TTL, which armed a real
/// background close timer a year out for every such pane.
///
/// Still an AddToChart pane with no deadline rather than a `PaneSource::Manual` one, which would
/// say the same thing more directly: `Container::is_pinnable` accepts only AddToChart, so the swap
/// would quietly make every hand-opened coin in an Add stack impossible to pin.
pub(super) const MANUAL_COIN_TTL_MS: f64 = f64::INFINITY;
