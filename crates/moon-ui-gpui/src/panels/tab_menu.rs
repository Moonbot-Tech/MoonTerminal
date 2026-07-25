//! The dock tab's right-click menu: the display switches for a panel's unread counters.
//!
//! MoonUI's dock owns no menu of its own — a right-click on a tab emits `DockEvent::TabContextMenu`
//! and the host decides what it offers. Everything here is display policy over `TabBadgeSettings`,
//! so the menu is built from the panel NAME rather than from the panel entity: a tab can be
//! right-clicked while its panel is hidden, and the settings outlive both.
//!
//! Which panels have a menu, and what it contains, is read from [`registry`] — the repo's single
//! source of truth for per-panel facts. A tab with no counters opens nothing at all: an empty menu
//! reads as a bug, and a switch for a counter that does not exist is a promise the panel does not
//! keep.

use gpui::*;
use moon_ui::{MoonContextMenuWindowExt as _, MoonMenuItem, MoonWindowExt as _};
use rust_i18n::t;

use crate::Backend;
use crate::panels::registry;
use moon_core::config::TabBadgeSettings;

/// Width of the tab menu, wide enough for the longest localized label without wrapping.
const MENU_WIDTH: f32 = 240.0;

/// Open the right-click menu for `panel` at `position`, or do nothing when it has no menu.
pub(crate) fn open(
    panel: &str,
    position: Point<Pixels>,
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut App,
) {
    let items = items(panel, backend, cx);
    if items.is_empty() {
        return;
    }
    window.open_moon_context_menu(
        cx,
        SharedString::from(format!("tab-menu-{panel}")),
        position,
        items,
        MENU_WIDTH,
    );
}

/// The switches `panel` offers, or nothing when its tab carries no counters.
fn items(panel: &str, backend: &Entity<Backend>, cx: &App) -> Vec<MoonMenuItem> {
    if !registry::find(panel).is_some_and(|kind| kind.tab_colour_counters) {
        return Vec::new();
    }
    vec![
        switch_item(
            panel,
            backend,
            cx,
            "counters",
            t!("dock.tab_menu.counters").to_string(),
            TabBadgeSettings::counters_visible,
            TabBadgeSettings::set_counters_visible,
        ),
        switch_item(
            panel,
            backend,
            cx,
            "merged",
            t!("dock.tab_menu.no_colour_split").to_string(),
            TabBadgeSettings::counters_merged,
            TabBadgeSettings::set_counters_merged,
        ),
    ]
}

/// One checkable switch over a `TabBadgeSettings` flag.
///
/// `get`/`set` are function pointers rather than a captured value because the menu overlay keeps
/// the items it was built with: the flag is read again at CLICK time, so a switch flipped
/// elsewhere (another window's tab menu) between opening and clicking cannot be written back.
fn switch_item(
    panel: &str,
    backend: &Entity<Backend>,
    cx: &App,
    key: &str,
    label: String,
    get: fn(&TabBadgeSettings, &str) -> bool,
    set: fn(&mut TabBadgeSettings, &str, bool) -> bool,
) -> MoonMenuItem {
    let checked = get(&backend.read(cx).tab_badges, panel);
    let backend = backend.clone();
    let panel = panel.to_string();
    MoonMenuItem::with_key(format!("tab-menu-{key}-{panel}"), label)
        .checked(checked)
        .on_click(move |_, window, app| {
            // Dismissed here: the overlay would otherwise stay open showing the old checkmark.
            window.close_context_menu(app);
            backend.update(app, |b, bcx| {
                let next = !get(&b.tab_badges, &panel);
                if set(&mut b.tab_badges, &panel, next) {
                    b.tab_badges_dirty = true;
                    bcx.notify();
                }
            });
        })
}
