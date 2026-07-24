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

use std::collections::HashMap;
use std::rc::Rc;

use gpui::*;
use moon_ui::{
    DockArea, MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonMenuItem, MoonMenuSize,
    MoonPalette, Panel, PanelEvent, PanelState, h_flex, v_flex,
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

/// Group-scoped News panel for a dock tab or detached window.
pub struct NewsView {
    backend: Entity<Backend>,
    group: String,
    lang: NewsLang,
    cache_sig: Option<u64>,
    cached: Rc<Vec<NewsItem>>,
    dock: Option<WeakEntity<DockArea>>,
    focus: FocusHandle,
}

impl NewsView {
    pub fn new(
        backend: Entity<Backend>,
        group: String,
        _window: &mut Window,
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

        let mut this = Self {
            backend,
            group,
            lang: NewsLang::Ru,
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

    /// Fold scoped cores' id + news revision into a change signature.
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
        let items = self.cached.clone();
        let now = moon_chart::paint::now_unix_ms() as i64;
        let lang = self.lang;

        let controls = h_flex()
            .w_full()
            .flex_none()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(self.lang_combo(cx))
            .child(div().flex_1())
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_muted))
                    .child(format!("{}", items.len())),
            );

        let body: AnyElement = if items.is_empty() {
            div()
                .flex_1()
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(p.text_soft))
                .child(t!("news.empty").to_string())
                .into_any_element()
        } else {
            v_flex()
                .id("news-feed")
                .flex_1()
                .w_full()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .children(
                    items
                        .iter()
                        .map(|it| render::news_card(it, lang, now, p, cx)),
                )
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
    }
}
