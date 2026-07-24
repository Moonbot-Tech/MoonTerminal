//! Single registry of detachable dock panels — the one place that knows how to build each panel
//! docked and detached, whether it detaches, and where it sits in the default bottom strip.
//!
//! Before this module the same panel identity was spread across parallel lists that had to be
//! edited in lockstep to add a panel: `window::detached::{supports_panel, build_panel, spawn}`,
//! `persistence::dock_persist::register_panels`, `shell::docks::DOCK_TAB_ORDER`, and the
//! default-layout push in `shell::init`. A name added to one and forgotten in another failed
//! silently (a panel that would not detach, restored as a `StubPanel`, or shown with an English
//! label). All of those now derive from [`DOCK_PANELS`].
//!
//! ChartTabs and Detects are intentionally NOT here. They are docked-only, constructed
//! differently (chart theme/epoch/focus, and Detects takes no `Window`), and never duplicated
//! across the detach machinery, so each appears once as an explicit `register_panel` call in
//! `dock_persist` and once in the default layout. Folding them in would add a parallel pattern
//! for no deduplication.

use std::rc::Rc;
use std::sync::LazyLock;

use gpui::{AnyView, App, AppContext, Entity, Window};
use moon_ui::{MoonDataTableState, PanelInfo, PanelView};

use crate::Backend;
use crate::panels::{
    AlertsPanel, AssetsView, CoreStatusView, LogPanel, NewsView, OrdersPanel, ReportPanel,
};

/// A detached-window panel plus the optional auto-width reset binding its window header exposes.
///
/// `widths_reset` is `Some((button_id, table_state))` only for panels whose detached header shows
/// the shared reset-column-widths button; `None` for panels without a resizable table (or, like
/// Assets, that deliberately omit the button in the detached header).
pub(crate) struct DetachedContent {
    pub(crate) view: AnyView,
    pub(crate) widths_reset: Option<(&'static str, Entity<MoonDataTableState>)>,
}

/// Factory that builds a panel for a DOCK: registry restore (`info` = `Some`), repin, or default
/// layout (`info` = `None`). See [`DockPanelKind::build_docked`].
type MkDocked =
    fn(&Entity<Backend>, &str, Option<&PanelInfo>, &mut Window, &mut App) -> Rc<dyn PanelView>;

/// Factory that builds a panel for its own detached OS window. See [`DockPanelKind::build_detached`].
type MkDetached = fn(&Entity<Backend>, &str, &mut Window, &mut App) -> DetachedContent;

/// Construction and placement facts for one detachable dock panel.
pub(crate) struct DockPanelKind {
    /// Stable persistence identity: equals [`moon_ui::Panel::panel_name`] and the key used by
    /// `docks.json`, `detached.json`, and [`crate::persistence::panel_meta::tab_label`].
    pub(crate) name: &'static str,
    /// Position in the default bottom home strip, or `None` for a panel that is not a default home
    /// tab (CoreStatus). Drives both the default-layout push order (`shell::init`) and the restore
    /// priority plus home-strip identification (`shell::docks`) via [`home_ordered_names`].
    pub(crate) home_order: Option<u8>,
    /// Build the panel for a dock. `info` = `Some` on registry restore lets Orders reapply its saved
    /// sort/kind/filter/columns; `info` = `None` (repin or default layout) starts it fresh, because
    /// its view state was not persisted in the dock JSON while it lived detached.
    mk_docked: MkDocked,
    /// Build the panel for its own detached OS window. Some panels use a distinct constructor here
    /// (Assets and CoreStatus use `detached_group`, not `restored_group`) and mark their tables as
    /// detached so they share the `:win` width context.
    mk_detached: MkDetached,
}

impl DockPanelKind {
    /// Build this panel docked. `info` carries the persisted `PanelInfo` during registry restore
    /// and is `None` for a repin or a fresh default layout.
    pub(crate) fn build_docked(
        &self,
        backend: &Entity<Backend>,
        group: &str,
        info: Option<&PanelInfo>,
        window: &mut Window,
        cx: &mut App,
    ) -> Rc<dyn PanelView> {
        (self.mk_docked)(backend, group, info, window, cx)
    }

    /// Build this panel for a detached OS window, returning its view and any width-reset binding.
    pub(crate) fn build_detached(
        &self,
        backend: &Entity<Backend>,
        group: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> DetachedContent {
        (self.mk_detached)(backend, group, window, cx)
    }
}

/// Every detachable dock panel. The single source of truth; all other panel-name dispatch derives
/// from it. Home tabs come first in strip order, then the non-home CoreStatus.
pub(crate) const DOCK_PANELS: &[DockPanelKind] = &[
    DockPanelKind {
        name: "Orders",
        home_order: Some(0),
        mk_docked: |b, g, info, w, cx| {
            let panel = cx.new(|cx| match info {
                // Restore replays saved view state; a repin or fresh layout starts from defaults.
                Some(info) => OrdersPanel::restored(b.clone(), g.to_string(), info, w, cx),
                None => OrdersPanel::new(b.clone(), g.to_string(), w, cx),
            });
            Rc::new(panel)
        },
        mk_detached: |b, g, w, cx| {
            let p = cx.new(|cx| OrdersPanel::new(b.clone(), g.to_string(), w, cx));
            // Detached tables share the `:win` detached-mode width context and keys across windows.
            p.update(cx, |this, cx| this.mark_table_detached(cx));
            let state = p.read(cx).table_state();
            DetachedContent {
                view: p.into(),
                widths_reset: Some(("orders-reset-widths-win", state)),
            }
        },
    },
    DockPanelKind {
        name: "Assets",
        home_order: Some(1),
        // Docked Assets shows its group's live grouped table only (`restored_group`).
        mk_docked: |b, g, _info, w, cx| {
            Rc::new(cx.new(|cx| AssetsView::restored_group(b.clone(), g.to_string(), w, cx)))
        },
        // A detached Assets window uses `detached_group` and exposes no header width-reset button.
        mk_detached: |b, g, w, cx| DetachedContent {
            view: cx
                .new(|cx| AssetsView::detached_group(b.clone(), g.to_string(), w, cx))
                .into(),
            widths_reset: None,
        },
    },
    DockPanelKind {
        name: "Report",
        home_order: Some(2),
        mk_docked: |b, g, _info, w, cx| {
            Rc::new(cx.new(|cx| ReportPanel::new(b.clone(), g.to_string(), w, cx)))
        },
        mk_detached: |b, g, w, cx| {
            let p = cx.new(|cx| ReportPanel::new(b.clone(), g.to_string(), w, cx));
            p.update(cx, |this, cx| this.mark_table_detached(cx));
            let state = p.read(cx).table_state();
            DetachedContent {
                view: p.into(),
                widths_reset: Some(("report-reset-widths-win", state)),
            }
        },
    },
    DockPanelKind {
        name: "Alerts",
        home_order: Some(3),
        mk_docked: |b, g, _info, w, cx| {
            Rc::new(cx.new(|cx| AlertsPanel::new(b.clone(), g.to_string(), w, cx)))
        },
        mk_detached: |b, g, w, cx| DetachedContent {
            view: cx
                .new(|cx| AlertsPanel::new(b.clone(), g.to_string(), w, cx))
                .into(),
            widths_reset: None,
        },
    },
    DockPanelKind {
        name: "Log",
        home_order: Some(4),
        mk_docked: |b, g, _info, w, cx| {
            Rc::new(cx.new(|cx| LogPanel::new(b.clone(), g.to_string(), w, cx)))
        },
        mk_detached: |b, g, w, cx| DetachedContent {
            view: cx
                .new(|cx| LogPanel::new(b.clone(), g.to_string(), w, cx))
                .into(),
            widths_reset: None,
        },
    },
    DockPanelKind {
        name: "News",
        home_order: Some(5),
        // News has no persisted view state and no resizable table, so both builders start fresh and
        // expose no width-reset button, mirroring Alerts/Log.
        mk_docked: |b, g, _info, w, cx| {
            Rc::new(cx.new(|cx| NewsView::new(b.clone(), g.to_string(), w, cx)))
        },
        mk_detached: |b, g, w, cx| DetachedContent {
            view: cx
                .new(|cx| NewsView::new(b.clone(), g.to_string(), w, cx))
                .into(),
            widths_reset: None,
        },
    },
    DockPanelKind {
        name: "CoreStatus",
        // Deliberately not a default bottom tab; see the home-strip note in `shell::docks`.
        home_order: None,
        mk_docked: |b, g, _info, w, cx| {
            Rc::new(cx.new(|cx| CoreStatusView::restored_group(b.clone(), g.to_string(), w, cx)))
        },
        mk_detached: |b, g, w, cx| {
            let p = cx.new(|cx| CoreStatusView::detached_group(b.clone(), g.to_string(), w, cx));
            let state = p.read(cx).table_state();
            DetachedContent {
                view: p.into(),
                widths_reset: Some(("core-status-reset-widths-win", state)),
            }
        },
    },
];

/// Find a panel kind by its stable [`DockPanelKind::name`].
pub(crate) fn find(name: &str) -> Option<&'static DockPanelKind> {
    DOCK_PANELS.iter().find(|k| k.name == name)
}

/// True for panels that can move into a detached OS window — every registry entry qualifies.
pub(crate) fn supports(name: &str) -> bool {
    find(name).is_some()
}

/// Default bottom home-strip names in left-to-right tab order, derived once from `home_order`.
///
/// This list both identifies the home strip and supplies the final fallback insertion priority
/// during restore. It must contain ALL default bottom-row tabs: `shell::docks` identifies the
/// "home" strip by the presence of any of these names, so a default tab missing from here would
/// make its strip unfindable and send a returning panel into a second bottom zone. CoreStatus is
/// excluded because it is not part of the default layout (`home_order: None`).
static HOME_ORDER: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let mut ordered: Vec<(u8, &'static str)> = DOCK_PANELS
        .iter()
        .filter_map(|k| k.home_order.map(|o| (o, k.name)))
        .collect();
    ordered.sort_by_key(|(order, _)| *order);
    ordered.into_iter().map(|(_, name)| name).collect()
});

/// The default bottom home-strip names in tab order. See [`HOME_ORDER`].
pub(crate) fn home_ordered_names() -> &'static [&'static str] {
    &HOME_ORDER
}

#[cfg(test)]
mod tests;
