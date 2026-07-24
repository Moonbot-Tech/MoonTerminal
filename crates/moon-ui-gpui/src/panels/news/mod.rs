//! News panel: the window group's news feed, merged across the scoped cores and deduplicated by
//! `meta.id`.
//!
//! moonproto delivers news per core; the same item arrives from every subscribed core with the same
//! `meta.id`, so this panel merges the scoped cores' logical items by id (preferring a translated
//! copy) and shows one row each, newest first. `CoreData::news_rev` gates the rebuild so the panel
//! repaints only when the reduced set changes.
//!
//! Like the other group panels it holds `backend` + `group`, lives in a dock tab or a detached
//! window, and routes its caption through [`crate::persistence::panel_meta`]. This module owns data
//! and lifecycle; [`render`] owns card rendering.

mod render;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    DockArea, MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize,
    MoonDropdown, MoonInput, MoonInputEvent, MoonInputState, MoonMenuItem, MoonMenuSize, MoonPalette,
    MoonPopover, MoonPopoverPlacement, Panel, PanelEvent, PanelState, h_flex, v_flex,
};
use rust_i18n::t;

use crate::Backend;
use crate::core_order::{CoreOrder, OrderedCores};
use crate::design;
use moon_core::feed::NewsItem;

/// Ceiling on logical items shown after the cross-core merge. The per-core ring is 50, so this is a
/// generous cap across a multi-core group.
const MAX_NEWS_DISPLAY: usize = 200;

/// Selected display language for news bodies. English is always the fallback.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum NewsLang {
    Ru,
    En,
    Es,
}

impl NewsLang {
    fn all() -> [NewsLang; 3] {
        [NewsLang::Ru, NewsLang::En, NewsLang::Es]
    }

    fn label(self) -> String {
        match self {
            NewsLang::Ru => t!("news.lang.ru"),
            NewsLang::En => t!("news.lang.en"),
            NewsLang::Es => t!("news.lang.es"),
        }
        .to_string()
    }

    /// Body text in this language, falling back to English when the translation is absent.
    pub(super) fn text(self, it: &NewsItem) -> &str {
        let s = match self {
            NewsLang::Ru => &it.ru,
            NewsLang::En => &it.en,
            NewsLang::Es => &it.es,
        };
        if s.is_empty() {
            &it.en
        } else {
            s
        }
    }

    /// Whether the selected non-English translation is still missing (drives the "pending" badge).
    pub(super) fn missing(self, it: &NewsItem) -> bool {
        match self {
            NewsLang::En => false,
            NewsLang::Ru => it.ru.is_empty(),
            NewsLang::Es => it.es.is_empty(),
        }
    }
}

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

/// Format Unix ms as a `DD.MM.YYYY` UTC date for the subscription pill and day separators.
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

/// Build a sticky-styled day header for the feed. `day`/`today` are UTC day numbers (ms / 86.4M).
fn day_separator(day: i64, today: i64, p: MoonPalette, cx: &App) -> AnyElement {
    let label = if day == today {
        t!("news.day.today").to_string()
    } else if day == today - 1 {
        t!("news.day.yesterday").to_string()
    } else {
        fmt_date(day.saturating_mul(86_400_000))
    };
    div()
        .w_full()
        .flex_none()
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 4.0))
        .text_size(design::t_caption(cx))
        .text_color(rgb(p.text_faint))
        .child(label)
        .into_any_element()
}

/// Group-scoped News panel for a dock tab or detached window.
pub struct NewsView {
    backend: Entity<Backend>,
    group: String,
    lang: NewsLang,
    /// Selected coin filter; `None` shows every coin.
    coin_filter: Option<String>,
    /// Free-text search over body/tickers/tags.
    query: Entity<MoonInputState>,
    /// Tags the user unchecked in the Tags popover. A card is hidden only if it has tags and every
    /// one of them is hidden, so tagless news always shows.
    hidden_tags: HashSet<String>,
    /// Card ids whose latency chain is currently expanded.
    expanded: HashSet<String>,
    /// Whether the Tags popover is open.
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
            lang: NewsLang::Ru,
            coin_filter: None,
            query,
            hidden_tags: HashSet::new(),
            expanded: HashSet::new(),
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
        base.wrapping_mul(31)
            .wrapping_add(b.news_tag_colors.rev())
    }

    /// Merge the scoped cores' logical news by `meta.id` (preferring a translated copy), newest first.
    ///
    /// Peak allocation is bounded by `scoped_cores × 50` (the per-core ring cap) before the
    /// `MAX_NEWS_DISPLAY` truncation. In practice the same news carries the same `meta.id` across
    /// cores, so the map dedups back to ~one ring; the transient upper bound only matters under a
    /// many-core group whose cores report entirely distinct news, and even then it is freed after
    /// each (infrequent) news-change rebuild.
    fn collect(&self, b: &Backend) -> Vec<NewsItem> {
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

    fn set_lang(&mut self, lang: NewsLang, cx: &mut Context<Self>) {
        if self.lang != lang {
            self.lang = lang;
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

    /// Whether a card passes the tag-visibility filter: tagless cards always show; a tagged card
    /// shows unless every one of its tags is hidden.
    fn tag_visible(&self, item: &NewsItem) -> bool {
        item.tags.is_empty() || item.tags.iter().any(|t| !self.hidden_tags.contains(t))
    }

    /// Whether a card passes ALL active filters: tag visibility, the coin filter, and the search
    /// query (already lowercased).
    fn passes(&self, item: &NewsItem, q: &str) -> bool {
        self.tag_visible(item)
            && self
                .coin_filter
                .as_ref()
                .is_none_or(|c| item.coins.iter().any(|x| x == c))
            && (q.is_empty() || item_matches_query(item, q))
    }

    /// Unique coins across the merged items, sorted — the coin filter's options.
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

    fn set_coin_filter(&mut self, coin: Option<String>, cx: &mut Context<Self>) {
        if self.coin_filter != coin {
            self.coin_filter = coin;
            cx.notify();
        }
    }

    /// Build the coin filter dropdown ("Монета: все" plus one entry per coin present).
    fn coin_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let coins = self.coin_catalog();
        let all = t!("news.coin.all").to_string();
        let sel = self.coin_filter.clone();
        let cur = format!(
            "{}: {}",
            t!("news.coin"),
            sel.clone().unwrap_or_else(|| all.clone())
        );
        let mut labels: Vec<String> = vec![all.clone()];
        labels.extend(coins.iter().cloned());
        let (label, trigger_w, menu_w) =
            design::dropdown_content_widths(cx, &cur, labels.iter().map(String::as_str), 120.0, 130.0);
        let view = cx.entity();
        let mut items: Vec<MoonMenuItem> = Vec::new();
        {
            let view = view.clone();
            items.push(
                MoonMenuItem::with_key("nc-all", all)
                    .selected(sel.is_none())
                    .on_click(move |_, _, app| {
                        view.update(app, |t, c| t.set_coin_filter(None, c));
                    }),
            );
        }
        for (i, coin) in coins.into_iter().enumerate() {
            let view = view.clone();
            let selected = sel.as_deref() == Some(coin.as_str());
            let coin_for_click = coin.clone();
            items.push(
                MoonMenuItem::with_key(format!("nc-{i}"), coin)
                    .selected(selected)
                    .on_click(move |_, _, app| {
                        let c = coin_for_click.clone();
                        view.update(app, |t, cx| t.set_coin_filter(Some(c), cx));
                    }),
            );
        }
        MoonDropdown::new("news-coin")
            .label(label)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(trigger_w)
            .menu_width(menu_w)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
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

    /// Toggle one tag's visibility in the filter.
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

    /// Show all tags (clear the filter) or hide all — the popover's "show all / hide all" actions.
    fn set_all_tags_hidden(&mut self, hidden: bool, cx: &mut Context<Self>) {
        if hidden {
            self.hidden_tags = self.tag_catalog().into_iter().collect();
        } else {
            self.hidden_tags.clear();
        }
        cx.notify();
    }

    /// Build the language selector dropdown.
    fn lang_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let items: Vec<(NewsLang, String)> =
            NewsLang::all().iter().map(|&l| (l, l.label())).collect();
        let cur = self.lang.label();
        let (label, trigger_w, menu_w) = design::dropdown_content_widths(
            cx,
            &cur,
            items.iter().map(|(_, s)| s.as_str()),
            90.0,
            110.0,
        );
        let view = cx.entity();
        let sel = self.lang;
        MoonDropdown::new("news-lang")
            .label(label)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(trigger_w)
            .menu_width(menu_w)
            .menu_size(MoonMenuSize::Compact)
            .items(items.into_iter().enumerate().map(move |(i, (lang, disp))| {
                let view = view.clone();
                MoonMenuItem::with_key(format!("nl-{i}"), disp)
                    .selected(lang == sel)
                    .on_click(move |_, _, app| {
                        view.update(app, |t, c| t.set_lang(lang, c));
                    })
            }))
    }

    /// The "Теги" popover: a trigger button plus a deferred overlay listing every tag with a
    /// visibility checkbox and a colour-swatch picker.
    fn tags_popover(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let entity = cx.entity();
        let trigger = MoonButton::new("news-tags-trigger")
            .label(format!("{} ▾", t!("news.tags")))
            .variant(MoonButtonVariant::Soft)
            .size(MoonButtonSize::Action)
            .render();
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
            .children(rows)
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
        let lang = self.lang;
        let colors = self.backend.read(cx).news_tag_colors.clone();
        let expanded = self.expanded.clone();
        let q = self.query.read(cx).value().trim().to_lowercase();
        // Apply the tag / coin / search filters (small N, cheap to materialize).
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
            .child(self.coin_combo(cx))
            .child(self.lang_combo(cx))
            .child(self.tags_popover(cx))
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
            // Distinguish "no news at all" from "all news hidden by the tag filter".
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
            // Interleave day separators (Today / Yesterday / date) as the day changes.
            let today = now / 86_400_000;
            let mut children: Vec<AnyElement> = Vec::new();
            let mut last_day: Option<i64> = None;
            for it in &visible {
                // Group by publication day — the SAME key the list is sorted by — so day headers
                // stay monotonic and mark when the news happened, not when this session received it.
                let day = if it.time_ms > 0 {
                    it.time_ms / 86_400_000
                } else {
                    i64::MIN
                };
                if last_day != Some(day) {
                    last_day = Some(day);
                    children.push(day_separator(day, today, p, cx));
                }
                let exp = expanded.contains(&it.id);
                children.push(render::news_card(it, lang, &colors, now, exp, p, cx));
            }
            v_flex()
                .id("news-feed")
                .flex_1()
                .w_full()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .children(children)
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
