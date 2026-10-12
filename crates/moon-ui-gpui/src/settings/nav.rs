//! Left navigation of the Settings window: the page filter, the sidebar, and the one-line page
//! picker that replaces the sidebar when the window is too narrow for both.
//!
//! [`nav_model`] is the pure part — which pages show, in what order, with which markers — so the
//! filter and the markers are testable without a window. The builders only draw a model.

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonInput, MoonInputState, MoonPalette, MoonSelect, MoonSelectItem,
    MoonSelectState, MoonSidebar, MoonSidebarGroup, MoonSidebarMenu, MoonSidebarMenuItem, MoonTag,
    MoonTooltipView, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::{NavGroup, Tab, TabSet};
use crate::design;

/// Sidebar width in logical pixels at 100% zoom.
pub(super) const NAV_W: f32 = 232.0;
/// Window width below which the sidebar gives way to [`nav_picker`].
pub(super) const NAV_BREAKPOINT: f32 = 760.0;

/// Number drawn after a page name in the navigation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NavCount {
    /// Informational, muted: how many of something the page holds.
    Plain(u32),
    /// Something on the page needs attention.
    Warn(u32),
}

/// Counts shown beside page names, recomputed only when the settings signature moves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct NavCounts {
    /// Cores in the draft.
    pub(super) cores: u32,
    /// Hotkey rows that never fire because another holder takes their binding first.
    pub(super) hotkey_conflicts: u32,
    /// Cores whose station copy differs from the terminal's; `None` while unknown.
    pub(super) station_diverging: Option<u32>,
}

impl NavCounts {
    /// The marker for `tab`, or `None` when the page shows no number.
    fn count(&self, tab: Tab) -> Option<NavCount> {
        match tab {
            Tab::Cores => Some(NavCount::Plain(self.cores)),
            Tab::Hotkeys => {
                (self.hotkey_conflicts > 0).then_some(NavCount::Warn(self.hotkey_conflicts))
            }
            Tab::StCores => self
                .station_diverging
                .filter(|n| *n > 0)
                .map(NavCount::Warn),
            _ => None,
        }
    }
}

/// One page entry of the navigation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct NavItem {
    pub(super) tab: Tab,
    pub(super) title: String,
    pub(super) group: NavGroup,
    /// The page has unsaved draft changes.
    pub(super) dirty: bool,
    pub(super) count: Option<NavCount>,
}

/// The pages the navigation shows for one filter, in [`Tab::ALL`] order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct NavModel {
    pub(super) query: String,
    pub(super) items: Vec<NavItem>,
}

/// Build the navigation for `query`.
///
/// A page shows when the trimmed query is empty, or when its title or its keywords contain the
/// query, ignoring case. Pages not mounted yet never show.
///
/// Args:
///     query: Text typed into the search box.
///     dirty: Pages with unsaved draft changes.
///     counts: Numbers drawn beside page names.
///
/// Returns:
///     The filtered pages with their markers.
pub(super) fn nav_model(query: &str, dirty: TabSet, counts: &NavCounts) -> NavModel {
    let q = query.trim().to_lowercase();
    let items = Tab::ALL
        .into_iter()
        .filter_map(|tab| {
            let title = tab.title();
            let hit = q.is_empty()
                || title.to_lowercase().contains(&q)
                || tab.keywords().to_lowercase().contains(&q);
            hit.then(|| NavItem {
                tab,
                title,
                group: tab.group(),
                dirty: dirty.contains(tab),
                count: counts.count(tab),
            })
        })
        .collect();
    NavModel { query: q, items }
}

/// Click handler shared by every navigation entry.
pub(super) type OnPick = Rc<dyn Fn(Tab, &mut Window, &mut App)>;

/// Draw the sidebar: search box, the model's pages under their group titles, and the MoonBot
/// import button at the bottom.
///
/// Args:
///     model: Pages to draw.
///     active: Page on screen, drawn selected.
///     query: Search box state; typing in it rebuilds the model.
///     on_pick: Called with the page a click selects.
///     on_import: Called by the footer button.
///     cx: Application context for theme and scale.
///
/// Returns:
///     The sidebar, [`NAV_W`] wide at 100% zoom.
pub(super) fn nav_sidebar(
    model: &NavModel,
    active: Tab,
    query: &Entity<MoonInputState>,
    on_pick: OnPick,
    on_import: Rc<dyn Fn(&mut Window, &mut App)>,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    let search = MoonInput::new("settings-nav-search")
        .state(query)
        .small()
        .cleanable(true)
        .prefix(
            svg()
                .path("icons/search.svg")
                .size(design::ui_px(cx, 14.0))
                .text_color(rgba_from(p.text_muted, 1.0)),
        );
    let header = v_flex()
        .w_full()
        .gap(design::ui_px(cx, 8.0))
        .child(search)
        .when(model.items.is_empty(), |el| {
            el.child(
                div()
                    .px(design::ui_px(cx, 8.0))
                    .text_color(rgba_from(p.text_muted, 1.0))
                    .child(t!("settings.search.empty").to_string()),
            )
        });
    let groups = NavGroup::ALL.into_iter().filter_map(|group| {
        let entries: Vec<MoonSidebarMenuItem> = model
            .items
            .iter()
            .filter(|item| item.group == group)
            .map(|item| menu_item(item, active, on_pick.clone()))
            .collect();
        (!entries.is_empty()).then(|| {
            MoonSidebarGroup::new(group.title()).child(MoonSidebarMenu::new().children(entries))
        })
    });
    let footer = MoonButton::new("settings-nav-import")
        .ghost()
        .label(t!("import.button").to_string())
        .on_click(move |_, window, cx| on_import(window, cx))
        .render()
        .into_any_element();
    let footer = tipped(
        "settings-nav-import-tip".into(),
        t!("import.hint").to_string(),
        footer,
    );
    MoonSidebar::new("settings-nav")
        .collapsible(false)
        .w(design::ui_px(cx, NAV_W))
        .h_full()
        .header(header)
        .children(groups)
        .footer(footer)
        .into_any_element()
}

/// One sidebar entry with its unsaved dot and count.
fn menu_item(item: &NavItem, active: Tab, on_pick: OnPick) -> MoonSidebarMenuItem {
    let tab = item.tab;
    let dirty = item.dirty;
    let count = item.count;
    MoonSidebarMenuItem::new(item.title.clone())
        .active(tab == active)
        .on_click(move |_, window, cx| on_pick(tab, window, cx))
        .when(dirty || count.is_some(), |entry| {
            entry.suffix(move |_, cx| suffix(tab, dirty, count, cx))
        })
}

/// The unsaved dot and the count after a page name, each explained by a tooltip.
fn suffix(tab: Tab, dirty: bool, count: Option<NavCount>, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    let dot = dirty.then(|| {
        tipped(
            SharedString::from(format!("settings-nav-dirty-{}", tab.id())),
            t!("settings.nav.unsaved").to_string(),
            div()
                .size(design::ui_px(cx, 6.0))
                .rounded_full()
                .bg(rgba_from(p.blue, 1.0))
                .into_any_element(),
        )
    });
    let number = count.map(|count| match count {
        NavCount::Plain(n) => div()
            .font_family(design::mono())
            .text_size(design::t_caption(cx))
            .text_color(rgba_from(p.text_muted, 1.0))
            .child(n.to_string())
            .into_any_element(),
        NavCount::Warn(n) => tipped(
            SharedString::from(format!("settings-nav-warn-{}", tab.id())),
            match tab {
                Tab::StCores => t!("settings.nav.diverging"),
                _ => t!("settings.nav.conflicts"),
            }
            .to_string(),
            MoonTag::warning()
                .mono(true)
                .label(n.to_string())
                .into_any_element(),
        ),
    });
    h_flex()
        .flex_none()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .children(dot)
        .children(number)
        .into_any_element()
}

/// `el` with a one-line tooltip.
pub(super) fn tipped(id: SharedString, text: String, el: AnyElement) -> AnyElement {
    div()
        .id(id)
        .flex_none()
        .tooltip(move |_window, cx| cx.new(|_| MoonTooltipView::new(text.clone())).into())
        .child(el)
        .into_any_element()
}

/// Picker caption for one page: "Group · Page", then " •" when unsaved and " (N)" for a count.
pub(super) fn picker_label(item: &NavItem) -> String {
    let mut label = format!("{} · {}", item.group.title(), item.title);
    if item.dirty {
        label.push_str(" •");
    }
    if let Some(NavCount::Plain(n) | NavCount::Warn(n)) = item.count {
        label.push_str(&format!(" ({n})"));
    }
    label
}

/// Picker entries for every page of `model`.
pub(super) fn picker_items(model: &NavModel) -> Vec<MoonSelectItem<Tab>> {
    model
        .items
        .iter()
        .map(|item| MoonSelectItem::new(item.tab, picker_label(item)))
        .collect()
}

/// Draw the narrow-window page picker: one full-width searchable select.
///
/// Args:
///     state: Picker state; its items come from [`picker_items`].
///     cx: Application context for scale.
///
/// Returns:
///     The picker row.
pub(super) fn nav_picker(state: &Entity<MoonSelectState<Tab>>, cx: &App) -> AnyElement {
    div()
        .w_full()
        .flex_none()
        .px(design::ui_px(cx, 18.0))
        .pt(design::ui_px(cx, 12.0))
        .child(
            MoonSelect::new(state)
                .id("settings-nav-picker")
                .searchable(true)
                .search_placeholder(t!("settings.search.placeholder").to_string()),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests;
