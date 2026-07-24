//! News panel: the window group's news feed, merged across the scoped cores and deduplicated by
//! `meta.id`.
//!
//! moonproto delivers news per core; the same item arrives from every subscribed core with the same
//! `meta.id`, so this panel merges the scoped cores' logical items by id (preferring a translated
//! copy) and shows one row each, newest first. `CoreData::news_rev` gates the rebuild so the panel
//! repaints only when the reduced set changes.
//!
//! Toolbar: a Coin filter and a Tags filter (both checkbox multi-select dropdowns, matching the
//! cores selector), a Translate toggle (Russian translation vs the delivered original — display
//! only, since moonproto has no terminal→core translate command), and a text search. The Tags
//! dropdown is filled from the service tag CATALOG (every known topic), not just the tags present on
//! the currently loaded items, because entity tags are sparse. This module owns data, filters, and
//! lifecycle; [`render`] owns card rendering.

mod render;

use std::collections::HashSet;
use std::rc::Rc;

use gpui::*;
use moon_ui::{
    DockArea, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonDropdown,
    MoonInput, MoonInputEvent, MoonInputState, MoonMenuItem, MoonMenuSize, MoonPalette, Panel,
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

/// Parse a service-supplied tag colour (`#RRGGBB`) into a `0xRRGGBB` value, or `None` if it is not a
/// 6-digit hex. The news service ships each entity tag with its own colour, so the terminal tints
/// tags from the feed instead of a hand-picked palette.
pub(super) fn parse_hex(s: &str) -> Option<u32> {
    let h = s.trim().strip_prefix('#').unwrap_or(s.trim());
    (h.len() == 6 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(h, 16).ok())
        .flatten()
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
        || item.tags.iter().any(|t| t.text.to_lowercase().contains(q))
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
    /// Tags the user unchecked in the Tags dropdown, by case-folded key ([`NewsTag::key`]). A card is
    /// hidden only if it has tags and every one of them is hidden, so tagless news always shows.
    hidden_tags: HashSet<String>,
    /// Card ids whose latency chain is currently expanded.
    expanded: HashSet<String>,
    /// The merged service tag catalog across the scoped cores (labels without a leading `#`) — the
    /// full topic vocabulary the Tags dropdown lists, rebuilt with the news set.
    catalog: Vec<String>,
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
            catalog: Vec::new(),
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

    /// Fold the scoped cores' id + news revision into a change signature. `news_rev` bumps whenever a
    /// core's reduced snapshot (items OR tag catalog) changes, so this alone covers every repaint.
    fn news_sig(&self, b: &Backend) -> u64 {
        let store = b.session.store();
        self.scope_cores(b).iter().fold(0u64, |a, (id, _)| {
            let rev = store.core(*id).map(|c| c.news_rev).unwrap_or(0);
            a.wrapping_mul(31)
                .wrapping_add(*id)
                .wrapping_mul(31)
                .wrapping_add(rev)
        })
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

    /// Union the scoped cores' tag catalogs (labels without `#`), deduped case-insensitively in
    /// first-seen (catalog) order. The vocabulary is identical across cores, so it dedups to one list.
    fn collect_catalog(&self, b: &Backend) -> Vec<String> {
        let store = b.session.store();
        let mut out: Vec<String> = Vec::new();
        for (id, _name) in self.scope_cores(b) {
            let Some(core) = store.core(id) else {
                continue;
            };
            for name in &core.news.catalog {
                if !out.iter().any(|x| x.eq_ignore_ascii_case(name)) {
                    out.push(name.clone());
                }
            }
        }
        out
    }

    fn rebuild(&mut self, b: &Backend) {
        self.cache_sig = Some(self.news_sig(b));
        self.cached = Rc::new(self.collect(b));
        self.catalog = self.collect_catalog(b);
        // Drop hidden-tag keys that are no longer selectable (an item-only tag that left the ring and
        // is absent from the catalog), so the master toggle's "all shown" state cannot be stuck by a
        // key with no visible row. Catalog tags persist, so a real topic is never forgotten.
        let keys: HashSet<String> = self.tag_rows().into_iter().map(|(k, _)| k).collect();
        self.hidden_tags.retain(|k| keys.contains(k));
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
        item.tags.is_empty() || item.tags.iter().any(|t| !self.hidden_tags.contains(&t.key()))
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

    /// The Tags dropdown row set: the service catalog (every known topic) plus any tag present on a
    /// loaded item but missing from the catalog. Returns `(key, label)` deduped by case-folded key,
    /// catalog first; `label` is shown as `#label`.
    fn tag_rows(&self) -> Vec<(String, String)> {
        let mut rows: Vec<(String, String)> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for name in &self.catalog {
            let key = name.to_lowercase();
            if seen.insert(key.clone()) {
                rows.push((key, name.clone()));
            }
        }
        for item in self.cached.iter() {
            for tag in &item.tags {
                let key = tag.key();
                if seen.insert(key.clone()) {
                    rows.push((key, tag.text.clone()));
                }
            }
        }
        rows
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

    /// Show all tags (clear the filter) or hide all — the Tags dropdown's master toggle.
    fn set_all_tags_hidden(&mut self, hidden: bool, cx: &mut Context<Self>) {
        if hidden {
            self.hidden_tags = self.tag_rows().into_iter().map(|(k, _)| k).collect();
        } else {
            self.hidden_tags.clear();
        }
        cx.notify();
    }

    // ---- dropdowns (cores-style checkbox multi-select) ----------------------------------------

    /// The Coin filter: a `MoonDropdown` with an "all coins" item plus a checkbox per coin present,
    /// mirroring the cores selector (`close_on_select(false)` keeps it open across ticks).
    fn coins_dropdown(&self, cx: &mut Context<Self>) -> MoonDropdown {
        let coins = self.coin_catalog();
        let all_label = t!("news.coin.all").to_string();
        let all_on = self.coin_filter.is_empty();
        let cur = match self.coin_filter.len() {
            0 => all_label.clone(),
            1 => self.coin_filter.iter().next().cloned().unwrap_or_default(),
            n => t!("news.coin.n", n = n).to_string(),
        };
        let (trigger_label, trigger_w, menu_w) = design::dropdown_content_widths(
            cx,
            &cur,
            std::iter::once(all_label.as_str()).chain(coins.iter().map(String::as_str)),
            design::CORES_TRIGGER_MIN_W,
            140.0,
        );
        let entity = cx.entity();
        let ent_all = entity.clone();
        let mut menu = MoonDropdown::new("news-coins")
            .label(trigger_label)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(trigger_w)
            .menu_width(menu_w)
            .menu_max_height(360.0)
            .menu_size(MoonMenuSize::Compact)
            .close_on_select(false)
            .item(
                MoonMenuItem::with_key("news-coins-all", all_label)
                    .checked(all_on)
                    .selected(all_on)
                    .on_click(move |_, _, app| ent_all.update(app, |t, c| t.clear_coins(c))),
            );
        for coin in coins {
            let on = self.coin_filter.contains(&coin);
            let ent = entity.clone();
            // Key by the coin value (not an enumerate index) so GPUI element identity is stable when
            // the sorted list reorders across renders.
            menu = menu.item(
                MoonMenuItem::with_key(format!("news-coin-{coin}"), coin.clone())
                    .checked(on)
                    .selected(on)
                    .on_click(move |_, _, app| {
                        ent.update(app, |t, c| {
                            let on = !t.coin_filter.contains(&coin);
                            t.toggle_coin(&coin, on, c);
                        })
                    }),
            );
        }
        menu
    }

    /// The Tags filter: a `MoonDropdown` filled from the service catalog (every topic) plus any
    /// item-only tag. The first item is a master show-all/hide-all toggle; each tag item's checkbox
    /// hides or shows that tag. `MoonMenuItem` can only hold a checkbox + label, so the retired manual
    /// colour picker has no place here — tags are tinted from the service's own per-tag colour.
    fn tags_dropdown(&self, cx: &mut Context<Self>) -> MoonDropdown {
        let rows = self.tag_rows();
        let label = t!("news.tags").to_string();
        let all_shown = self.hidden_tags.is_empty();
        let labels: Vec<String> = rows.iter().map(|(_, l)| format!("#{l}")).collect();
        let (trigger_label, trigger_w, menu_w) = design::dropdown_content_widths(
            cx,
            &label,
            std::iter::once(label.as_str()).chain(labels.iter().map(String::as_str)),
            design::CORES_TRIGGER_MIN_W,
            160.0,
        );
        let entity = cx.entity();
        let ent_all = entity.clone();
        let mut menu = MoonDropdown::new("news-tags")
            .label(trigger_label)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(trigger_w)
            .menu_width(menu_w)
            .menu_max_height(360.0)
            .menu_size(MoonMenuSize::Compact)
            .close_on_select(false)
            .item(
                // Master toggle: all shown -> hide all; otherwise -> show all.
                MoonMenuItem::with_key("news-tags-all", t!("news.tags.all").to_string())
                    .checked(all_shown)
                    .selected(all_shown)
                    .on_click(move |_, _, app| {
                        ent_all.update(app, |t, c| {
                            let all_shown = t.hidden_tags.is_empty();
                            t.set_all_tags_hidden(all_shown, c);
                        })
                    }),
            );
        for ((key, _), label) in rows.into_iter().zip(labels) {
            let on = !self.hidden_tags.contains(&key);
            let ent = entity.clone();
            // Key by the tag key (not an enumerate index) so element identity survives catalog/item
            // reordering across renders.
            menu = menu.item(
                MoonMenuItem::with_key(format!("news-tag-{key}"), label)
                    .checked(on)
                    .selected(on)
                    .on_click(move |_, _, app| {
                        ent.update(app, |t, c| {
                            let hidden = t.hidden_tags.contains(&key);
                            t.toggle_tag_hidden(&key, !hidden, c);
                        })
                    }),
            );
        }
        menu
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
            .child(self.coins_dropdown(cx))
            .child(self.tags_dropdown(cx))
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
                    render::news_card(it, translate, now, exp, p, cx)
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
