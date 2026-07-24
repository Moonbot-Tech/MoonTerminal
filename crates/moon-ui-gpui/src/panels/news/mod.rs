//! News panel: the window group's news feed, merged across the scoped cores and deduplicated by
//! `meta.id`.
//!
//! moonproto delivers news per core; the same item arrives from every subscribed core with the same
//! `meta.id`, so this panel merges the scoped cores' logical items by id (preferring a translated
//! copy) and shows one row each, newest first. `CoreData::news_rev` gates the rebuild so the panel
//! repaints only when the reduced set changes.
//!
//! Toolbar: a Translate toggle (Russian translation vs the delivered original — display only, since
//! moonproto has no terminal→core translate command), a Coin filter and a Tags filter (both
//! checkbox multi-select popovers), and a text search. This module owns data, filters, and
//! lifecycle; [`render`] owns card rendering.

mod render;

use std::collections::HashSet;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    DockArea, MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize,
    MoonInput, MoonInputEvent, MoonInputState, MoonPalette, MoonPopover, MoonPopoverPlacement, Panel,
    PanelEvent, PanelState, h_flex, v_flex,
};
use rust_i18n::t;

use crate::Backend;
use crate::core_order::{CoreOrder, OrderedCores};
use crate::design;
use moon_core::feed::NewsItem;

/// Ceiling on logical items shown after the cross-core merge. The per-core ring is 50, so this is a
/// generous cap across a multi-core group.
const MAX_NEWS_DISPLAY: usize = 200;

/// The fixed palette a user can assign to a tag. Keys persist in `news_tags.json` and resolve to
/// theme colours via [`key_color`]; storing keys (not RGB) keeps the colour theme-adaptive. Limited
/// to the distinct hues `MoonPalette` actually carries (no teal/violet token exists upstream).
pub(super) const TAG_PALETTE: [&str; 4] = ["red", "amber", "green", "blue"];

/// Resolve a palette colour key to the active theme colour, or `None` for an unknown/neutral key.
pub(super) fn key_color(key: &str, p: MoonPalette) -> Option<u32> {
    match key {
        "red" => Some(p.red),
        "amber" => Some(p.amber),
        "green" => Some(p.green),
        "blue" => Some(p.blue),
        _ => None,
    }
}

/// Format Unix ms as a `DD.MM.YYYY` UTC date for the subscription pill.
fn fmt_date(ms: i64) -> String {
    chrono::DateTime::from_timestamp(ms / 1000, 0)
        .map(|dt| dt.format("%d.%m.%Y").to_string())
        .unwrap_or_default()
}

/// Whether any of the item's text (all languages), tickers, or tags contain the lowercased query.
fn item_matches_query(item: &NewsItem, q: &str) -> bool {
    item.en.to_lowercase().contains(q)
        || item.ru.to_lowercase().contains(q)
        || item.es.to_lowercase().contains(q)
        || item.coins.iter().any(|c| c.to_lowercase().contains(q))
        || item.tags.iter().any(|t| t.to_lowercase().contains(q))
}

/// A compact, padding-free popover trigger — just the label and a caret, text flush to the edges.
/// `active` brightens it when the filter it opens is engaged.
fn filter_trigger(label: String, active: bool, p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .flex_none()
        .items_center()
        .gap(design::ui_px(cx, 2.0))
        .cursor_pointer()
        .text_size(design::t_body(cx))
        .text_color(rgb(if active { p.text } else { p.text_dim }))
        .child(label)
        .child(div().text_size(design::t_caption(cx)).child("▾"))
}

/// Group-scoped News panel for a dock tab or detached window.
pub struct NewsView {
    backend: Entity<Backend>,
    group: String,
    /// Show the Russian translation (English fallback) when on; the delivered original when off.
    translate: bool,
    /// Selected coins to show; empty shows every coin.
    coin_filter: HashSet<String>,
    /// Free-text search over body/tickers/tags.
    query: Entity<MoonInputState>,
    /// Tags the user unchecked in the Tags popover. A card is hidden only if it has tags and every
    /// one of them is hidden, so tagless news always shows.
    hidden_tags: HashSet<String>,
    /// Card ids whose latency chain is currently expanded.
    expanded: HashSet<String>,
    /// Whether the Coin / Tags popovers are open.
    coins_open: bool,
    tags_open: bool,
    cache_sig: Option<u64>,
    cached: Rc<Vec<NewsItem>>,
    dock: Option<WeakEntity<DockArea>>,
    focus: FocusHandle,
}

impl NewsView {
    pub fn new(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Rebuild + repaint only when the scoped cores' news revision changes. News is event-driven
        // and low-volume, so no periodic idle refresh is needed.
        cx.observe(&backend, |this, backend, cx| {
            let b = backend.read(cx);
            let sig = this.news_sig(b);
            if this.cache_sig != Some(sig) {
                this.rebuild(b);
                cx.notify();
            }
        })
        .detach();

        // Search input: a change only re-filters at render time, so just repaint.
        let query =
            cx.new(|cx| MoonInputState::new(window, cx).placeholder(t!("news.search").to_string()));
        cx.subscribe(&query, |_this, _e, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                cx.notify();
            }
        })
        .detach();

        let mut this = Self {
            backend,
            group,
            translate: true,
            coin_filter: HashSet::new(),
            query,
            hidden_tags: HashSet::new(),
            expanded: HashSet::new(),
            coins_open: false,
            tags_open: false,
            cache_sig: None,
            cached: Rc::new(Vec::new()),
            dock: None,
            focus: cx.focus_handle(),
        };
        let b = this.backend.clone();
        this.rebuild(b.read(cx));
        this
    }

    /// Return this panel group's cores in canonical order.
    fn scope_cores(&self, b: &Backend) -> OrderedCores {
        CoreOrder::new(&b.config).from_sessions(b.session.sessions(), |s| s.group == self.group)
    }

    /// Fold scoped cores' id + news revision, plus the global tag-colour revision, into a change
    /// signature. The colour rev makes a colour edit in one open News view repaint the others.
    fn news_sig(&self, b: &Backend) -> u64 {
        let store = b.session.store();
        let base = self.scope_cores(b).iter().fold(0u64, |a, (id, _)| {
            let rev = store.core(*id).map(|c| c.news_rev).unwrap_or(0);
            a.wrapping_mul(31)
                .wrapping_add(*id)
                .wrapping_mul(31)
                .wrapping_add(rev)
        });
        base.wrapping_mul(31).wrapping_add(b.news_tag_colors.rev())
    }

    /// Merge the scoped cores' logical news by `meta.id` (preferring a translated copy), newest first.
    ///
    /// Peak allocation is bounded by `scoped_cores × 50` (the per-core ring cap) before the
    /// `MAX_NEWS_DISPLAY` truncation. In practice the same news carries the same `meta.id` across
    /// cores, so the map dedups back to ~one ring; the transient upper bound only matters under a
    /// many-core group whose cores report entirely distinct news, and even then it is freed after
    /// each (infrequent) news-change rebuild.
    fn collect(&self, b: &Backend) -> Vec<NewsItem> {
        use std::collections::HashMap;
        let store = b.session.store();
        let mut order: Vec<String> = Vec::new();
        let mut map: HashMap<String, NewsItem> = HashMap::new();
        for (id, _name) in self.scope_cores(b) {
            let Some(core) = store.core(id) else {
                continue;
            };
            for item in &core.news.items {
                match map.get_mut(&item.id) {
                    None => {
                        order.push(item.id.clone());
                        map.insert(item.id.clone(), item.clone());
                    }
                    // Across cores the same news can be original on one and translated on another;
                    // keep the translated copy.
                    Some(existing) => {
                        if existing.is_original && !item.is_original {
                            *existing = item.clone();
                        }
                    }
                }
            }
        }
        let mut items: Vec<NewsItem> = order.into_iter().filter_map(|id| map.remove(&id)).collect();
        // Newest first; stable sort keeps first-seen order among equal times.
        items.sort_by_key(|it| std::cmp::Reverse(it.time_ms));
        items.truncate(MAX_NEWS_DISPLAY);
        items
    }

    fn rebuild(&mut self, b: &Backend) {
        self.cache_sig = Some(self.news_sig(b));
        self.cached = Rc::new(self.collect(b));
    }

    fn set_translate(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.translate != on {
            self.translate = on;
            cx.notify();
        }
    }

    /// Toggle a card's expanded latency chain.
    fn toggle_expand(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.expanded.remove(id) {
            self.expanded.insert(id.to_string());
        }
        cx.notify();
    }

    // ---- filters ------------------------------------------------------------------------------

    /// Whether a card passes ALL active filters: coin selection, tag visibility, and the search
    /// query (already lowercased).
    fn passes(&self, item: &NewsItem, q: &str) -> bool {
        self.coin_ok(item) && self.tag_visible(item) && (q.is_empty() || item_matches_query(item, q))
    }

    /// Coin filter: empty selection shows all; otherwise the item must carry a selected coin.
    fn coin_ok(&self, item: &NewsItem) -> bool {
        self.coin_filter.is_empty() || item.coins.iter().any(|c| self.coin_filter.contains(c))
    }

    /// Tag visibility: tagless cards always show; a tagged card shows unless every tag is hidden.
    fn tag_visible(&self, item: &NewsItem) -> bool {
        item.tags.is_empty() || item.tags.iter().any(|t| !self.hidden_tags.contains(t))
    }

    /// Unique coins across the merged items, sorted — the Coin popover's row set.
    fn coin_catalog(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for item in self.cached.iter() {
            for c in &item.coins {
                if !out.iter().any(|x| x == c) {
                    out.push(c.clone());
                }
            }
        }
        out.sort();
        out
    }

    fn toggle_coin(&mut self, coin: &str, on: bool, cx: &mut Context<Self>) {
        let changed = if on {
            self.coin_filter.insert(coin.to_string())
        } else {
            self.coin_filter.remove(coin)
        };
        if changed {
            cx.notify();
        }
    }

    /// Clear the coin selection (show every coin).
    fn clear_coins(&mut self, cx: &mut Context<Self>) {
        if !self.coin_filter.is_empty() {
            self.coin_filter.clear();
            cx.notify();
        }
    }

    /// Unique tags across the merged items, in first-seen order — the Tags popover's row set.
    fn tag_catalog(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for item in self.cached.iter() {
            for tag in &item.tags {
                if !out.iter().any(|t| t == tag) {
                    out.push(tag.clone());
                }
            }
        }
        out
    }

    fn toggle_tag_hidden(&mut self, tag: &str, hidden: bool, cx: &mut Context<Self>) {
        let changed = if hidden {
            self.hidden_tags.insert(tag.to_string())
        } else {
            self.hidden_tags.remove(tag)
        };
        if changed {
            cx.notify();
        }
    }

    /// Show all tags (clear the filter) or hide all — the Tags popover's "show all / hide all".
    fn set_all_tags_hidden(&mut self, hidden: bool, cx: &mut Context<Self>) {
        if hidden {
            self.hidden_tags = self.tag_catalog().into_iter().collect();
        } else {
            self.hidden_tags.clear();
        }
        cx.notify();
    }

    /// Assign or clear a tag's colour on the global config, saving on a real change. Notifies the
    /// Backend (not just self) so EVERY open News view repaints: each observe fires and sees the
    /// tag-colour rev change in its signature.
    fn set_tag_color(&mut self, tag: &str, key: Option<&str>, cx: &mut Context<Self>) {
        self.backend.update(cx, |b, bcx| {
            if b.news_tag_colors.set(tag, key) {
                b.news_tag_colors.save();
                bcx.notify();
            }
        });
    }

    // ---- popovers -----------------------------------------------------------------------------

    /// The Coin filter popover: a compact trigger plus a checkbox per coin present.
    fn coins_popover(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let entity = cx.entity();
        let trigger =
            filter_trigger(t!("news.coin").to_string(), !self.coin_filter.is_empty(), p, cx);
        MoonPopover::new("news-coins-popover")
            .placement(MoonPopoverPlacement::BottomStart)
            .width(200.0)
            .close_on_content_click(false)
            .overlay_closable(false)
            .open(self.coins_open)
            .on_open_change(move |open, _w, app| {
                entity.update(app, |t, cx| {
                    t.coins_open = open;
                    cx.notify();
                });
            })
            .trigger(trigger)
            .content(self.coins_content(p, cx))
    }

    fn coins_content(&self, p: MoonPalette, cx: &mut Context<Self>) -> AnyElement {
        let coins = self.coin_catalog();
        let empty = coins.is_empty();
        let head = h_flex()
            .w_full()
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .child(
                MoonButton::new("news-coins-all")
                    .label(t!("news.coin.all").to_string())
                    .size(MoonButtonSize::Micro)
                    .variant(MoonButtonVariant::Ghost)
                    .on_click(cx.listener(|this, _, _w, cx| this.clear_coins(cx)))
                    .render(),
            )
            .child(div().flex_1())
            .child(
                MoonButton::new("news-coins-close")
                    .label("✕")
                    .size(MoonButtonSize::Micro)
                    .variant(MoonButtonVariant::Ghost)
                    .on_click(cx.listener(|this, _, _w, cx| {
                        this.coins_open = false;
                        cx.notify();
                    }))
                    .render(),
            );
        let rows: Vec<AnyElement> = coins
            .into_iter()
            .map(|coin| {
                let checked = self.coin_filter.contains(&coin);
                let coin_c = coin.clone();
                MoonCheckbox::new(SharedString::from(format!("news-coin-{coin}")))
                    .label(coin.clone())
                    .checked(checked)
                    .size(MoonCheckboxSize::Compact)
                    .on_change(cx.listener(move |this, ch: &bool, _w, cx| {
                        this.toggle_coin(&coin_c, *ch, cx);
                    }))
                    .into_any_element()
            })
            .collect();
        v_flex()
            .id("news-coins-content")
            .w_full()
            .p(design::ui_px(cx, 8.0))
            .gap(design::ui_px(cx, 4.0))
            .bg(rgb(p.panel_high))
            .border_1()
            .border_color(rgb(p.border))
            .rounded(design::r_button(cx))
            .font_family(design::mono())
            .child(head)
            .when(empty, |this| {
                this.child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(rgb(p.text_muted))
                        .child(t!("news.coin.empty").to_string()),
                )
            })
            .child(
                v_flex()
                    .id("news-coins-list")
                    .w_full()
                    .gap(design::ui_px(cx, 2.0))
                    .max_h(design::ui_px(cx, 260.0))
                    .overflow_y_scroll()
                    .children(rows),
            )
            .into_any_element()
    }

    /// The Tags filter popover: a compact trigger plus a visibility checkbox and colour picker per tag.
    fn tags_popover(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let entity = cx.entity();
        // Active when any tag is hidden (a filter is engaged) or any tag has a colour assigned.
        let active = !self.hidden_tags.is_empty();
        let trigger = filter_trigger(t!("news.tags").to_string(), active, p, cx);
        MoonPopover::new("news-tags-popover")
            .placement(MoonPopoverPlacement::BottomStart)
            .width(248.0)
            .close_on_content_click(false)
            // Swatch clicks and the close button are the explicit dismissal paths; an outside-click
            // close would fight the picker interactions like the detects popover.
            .overlay_closable(false)
            .open(self.tags_open)
            .on_open_change(move |open, _w, app| {
                entity.update(app, |t, cx| {
                    t.tags_open = open;
                    cx.notify();
                });
            })
            .trigger(trigger)
            .content(self.tags_content(p, cx))
    }

    /// Build the popover body: show-all/hide-all header plus one row per tag.
    fn tags_content(&self, p: MoonPalette, cx: &mut Context<Self>) -> AnyElement {
        let catalog = self.tag_catalog();
        let empty = catalog.is_empty();
        let head = h_flex()
            .w_full()
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .child(
                MoonButton::new("news-tags-showall")
                    .label(t!("news.tags.show_all").to_string())
                    .size(MoonButtonSize::Micro)
                    .variant(MoonButtonVariant::Ghost)
                    .on_click(cx.listener(|this, _, _w, cx| this.set_all_tags_hidden(false, cx)))
                    .render(),
            )
            .child(
                MoonButton::new("news-tags-hideall")
                    .label(t!("news.tags.hide_all").to_string())
                    .size(MoonButtonSize::Micro)
                    .variant(MoonButtonVariant::Ghost)
                    .on_click(cx.listener(|this, _, _w, cx| this.set_all_tags_hidden(true, cx)))
                    .render(),
            )
            .child(div().flex_1())
            .child(
                MoonButton::new("news-tags-close")
                    .label("✕")
                    .size(MoonButtonSize::Micro)
                    .variant(MoonButtonVariant::Ghost)
                    .on_click(cx.listener(|this, _, _w, cx| {
                        this.tags_open = false;
                        cx.notify();
                    }))
                    .render(),
            );
        let colors = self.backend.read(cx).news_tag_colors.clone();
        let rows: Vec<AnyElement> = catalog
            .into_iter()
            .map(|tag| {
                let current = colors.color(&tag).map(str::to_string);
                self.tag_row(tag, current, p, cx)
            })
            .collect();
        v_flex()
            .id("news-tags-content")
            .w_full()
            .p(design::ui_px(cx, 8.0))
            .gap(design::ui_px(cx, 4.0))
            .bg(rgb(p.panel_high))
            .border_1()
            .border_color(rgb(p.border))
            .rounded(design::r_button(cx))
            .font_family(design::mono())
            .child(head)
            .when(empty, |this| {
                this.child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(rgb(p.text_muted))
                        .child(t!("news.tags.empty").to_string()),
                )
            })
            .child(
                v_flex()
                    .id("news-tags-list")
                    .w_full()
                    .gap(design::ui_px(cx, 2.0))
                    .max_h(design::ui_px(cx, 300.0))
                    .overflow_y_scroll()
                    .children(rows),
            )
            .into_any_element()
    }

    /// One tag row: visibility checkbox · name · palette swatches (+ "none").
    fn tag_row(
        &self,
        tag: String,
        current: Option<String>,
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hidden = self.hidden_tags.contains(&tag);
        let cb_tag = tag.clone();
        let checkbox = MoonCheckbox::new(SharedString::from(format!("news-tagvis-{tag}")))
            .checked(!hidden)
            .size(MoonCheckboxSize::Compact)
            .on_change(cx.listener(move |this, checked: &bool, _w, cx| {
                this.toggle_tag_hidden(&cb_tag, !*checked, cx);
            }));

        let mut swatches = h_flex().items_center().gap(design::ui_px(cx, 4.0));
        for key in TAG_PALETTE {
            let c = key_color(key, p).unwrap_or(p.text_muted);
            let selected = current.as_deref() == Some(key);
            let sw_tag = tag.clone();
            swatches = swatches.child(
                div()
                    .id(SharedString::from(format!("nt-{tag}-{key}")))
                    .w(design::ui_px(cx, 14.0))
                    .h(design::ui_px(cx, 14.0))
                    .rounded(design::ui_px(cx, 3.0))
                    .bg(rgb(c))
                    .cursor_pointer()
                    .border_color(rgb(if selected { p.text } else { p.border }))
                    .map(|d| if selected { d.border_2() } else { d.border_1() })
                    .on_click(cx.listener(move |this, _, _w, cx| {
                        this.set_tag_color(&sw_tag, Some(key), cx);
                    })),
            );
        }
        let none_selected = current.is_none();
        let none_tag = tag.clone();
        swatches = swatches.child(
            div()
                .id(SharedString::from(format!("nt-{tag}-none")))
                .w(design::ui_px(cx, 14.0))
                .h(design::ui_px(cx, 14.0))
                .rounded(design::ui_px(cx, 3.0))
                .bg(rgb(p.surface))
                .cursor_pointer()
                .border_color(rgb(if none_selected { p.text } else { p.border }))
                .map(|d| if none_selected { d.border_2() } else { d.border_1() })
                .flex()
                .items_center()
                .justify_center()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child("×")
                .on_click(cx.listener(move |this, _, _w, cx| {
                    this.set_tag_color(&none_tag, None, cx);
                })),
        );

        h_flex()
            .w_full()
            .items_center()
            .gap(design::ui_px(cx, 8.0))
            .py(design::ui_px(cx, 2.0))
            .child(checkbox)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text))
                    .child(format!("#{tag}")),
            )
            .child(swatches)
            .into_any_element()
    }

    // ---- footer -------------------------------------------------------------------------------

    /// Latest news-subscription validity (Unix ms) across the scoped cores, or `None` if no core
    /// reports one. News arrives from any subscribed core, so the longest-valid wins.
    fn subscription_until(&self, b: &Backend) -> Option<i64> {
        let store = b.session.store();
        self.scope_cores(b)
            .iter()
            .filter_map(|(id, _)| {
                store
                    .core(*id)
                    .and_then(|c| c.license.as_ref())
                    .and_then(|l| l.news_valid_until)
            })
            .max()
    }

    /// Render the footer: a live indicator and the news-subscription status pill.
    fn footer(&self, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        let now = moon_chart::paint::now_unix_ms() as i64;
        let (color, text) = match self.subscription_until(self.backend.read(cx)) {
            Some(ms) if ms > now => (
                design::positive_color(p),
                format!("{} {}", t!("news.sub.until"), fmt_date(ms)),
            ),
            Some(_) => (design::danger_color(p), t!("news.sub.expired").to_string()),
            None => (p.text_muted, t!("news.sub.none").to_string()),
        };
        let pill = div()
            .flex_none()
            .px(design::ui_px(cx, 8.0))
            .py(design::ui_px(cx, 1.0))
            .rounded(design::r_button(cx))
            .text_size(design::t_caption(cx))
            .text_color(rgb(color))
            .bg(design::moon_alpha(color, 0.10))
            .border_1()
            .border_color(design::moon_alpha(color, 0.22))
            .child(text);
        h_flex()
            .w_full()
            .flex_none()
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .px_2()
            .py_1()
            .child(design::status_dot(p.green, cx))
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(p.text_muted))
                    .child(t!("news.live").to_string()),
            )
            .child(div().flex_1())
            .child(pill)
    }
}

impl EventEmitter<PanelEvent> for NewsView {}
impl Focusable for NewsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for NewsView {
    fn panel_name(&self) -> &'static str {
        "News"
    }
    /// Visible tab caption. `panel_name` is the stable persistence key and stays untouched.
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        crate::persistence::panel_meta::tab_label(self.panel_name())
    }
    fn closable(&self, _cx: &App) -> bool {
        true
    }
    fn show_dock_header(&self, _cx: &App) -> bool {
        true
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::persistence::panel_meta::panel_title(self.panel_name())
    }
    fn dump(&self, _cx: &App) -> PanelState {
        crate::persistence::dock_persist::panel_state_with_group("News", &self.group)
    }
    fn on_added_to(
        &mut self,
        dock_area: WeakEntity<DockArea>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.dock = Some(dock_area);
    }
    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Vec<AnyElement>> {
        Some(vec![crate::panels::detach_button(
            "News",
            self.group.clone(),
            self.backend.clone(),
            self.dock.clone(),
        )])
    }
}

impl Render for NewsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let now = moon_chart::paint::now_unix_ms() as i64;
        let translate = self.translate;
        let colors = self.backend.read(cx).news_tag_colors.clone();
        let expanded = self.expanded.clone();
        let q = self.query.read(cx).value().trim().to_lowercase();
        // Apply the coin / tag / search filters (small N, cheap to materialize).
        let visible: Vec<NewsItem> = self
            .cached
            .iter()
            .filter(|it| self.passes(it, &q))
            .cloned()
            .collect();

        let controls = h_flex()
            .w_full()
            .flex_none()
            .flex_wrap()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(self.coins_popover(cx))
            .child(self.tags_popover(cx))
            .child(
                MoonCheckbox::new("news-translate")
                    .label(t!("news.translate").to_string())
                    .checked(self.translate)
                    .size(MoonCheckboxSize::Compact)
                    .on_change(cx.listener(|this, ch: &bool, _w, cx| this.set_translate(*ch, cx))),
            )
            .child(
                div().w(design::font_w_px(cx, 150.0)).flex_none().child(
                    MoonInput::new("news-search")
                        .state(&self.query)
                        .small()
                        .cleanable(true),
                ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_muted))
                    .child(format!("{}", visible.len())),
            );

        let body: AnyElement = if visible.is_empty() {
            // Distinguish "no news at all" from "everything filtered out".
            let msg = if self.cached.is_empty() {
                t!("news.empty")
            } else {
                t!("news.empty_filtered")
            };
            div()
                .flex_1()
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(p.text_soft))
                .child(msg.to_string())
                .into_any_element()
        } else {
            v_flex()
                .id("news-feed")
                .flex_1()
                .w_full()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .children(visible.iter().map(|it| {
                    let exp = expanded.contains(&it.id);
                    render::news_card(it, translate, &colors, now, exp, p, cx)
                }))
                .into_any_element()
        };

        v_flex()
            .id("news-panel")
            .size_full()
            .min_h(px(0.0))
            .overflow_hidden()
            .track_focus(&self.focus)
            .font_family(design::mono())
            .bg(rgb(p.table_body))
            .child(controls)
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)))
            .child(body)
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)))
            .child(self.footer(p, cx))
    }
}
