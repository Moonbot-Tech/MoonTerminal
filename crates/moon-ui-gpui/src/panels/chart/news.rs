//! News marks on the chart: a small tag-coloured gem on the plot's bottom edge at each news item for
//! this coin, and the Ctrl-hover card that reads it out (Moonbot parity).
//!
//! Split of responsibilities:
//! - WHICH news belong to this chart is `moon_core::feed::news_marks` (shared with the News panel's
//!   tag filter), cached here and rebuilt only when the news revision moves;
//! - the GEMS are own-pass geometry (`chartdx::news_sync`), because the chart scrolls with the live
//!   edge every frame without notifying GPUI — a GPUI-drawn shape would lag a frame behind;
//! - the CARD is a GPUI overlay: wrapping, theme colours and the font-size slider all come for free,
//!   and it appears only while Ctrl is held over a mark, which is a rare, user-driven repaint.
//!
//! COLOUR comes from the user's per-tag palette (`NewsTagSettings`, the same colours the News panel
//! paints its chips with): every DISTINCT colour among an item's VISIBLE tags takes one wedge of the
//! gem. A tag unticked in the News panel paints nothing here either, a colourless tag contributes
//! nothing, and an item with no coloured tag at all stays one neutral gem.
//!
//! Pointer cost: a move pays the Delphi movement threshold first (as `INPUT_HOTPATH_NORMS.md`
//! requires of chart hit-tests), then — only when this chart HAS marks — a pane lookup and its plot
//! mapping, and then one Y comparison ([`moon_chart::news_marks::in_mark_row`]) that fails for
//! everything above the marks' row. Only inside that row does anything scan the marks themselves.
//!
//! Crossing a mark grows it, which costs one userdata rebuild — the same price the order-line hover
//! already pays. It fires only when the NEAREST mark changes, never per pixel; a scrolling chart
//! under a resting cursor can trip it from `revalidate_news_hover`, but only as often as the mark
//! under the cursor actually changes, which is a scroll of a whole mark's width.

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonPalette, MoonSurface, MoonSurfaceVariant, h_flex, v_flex};
use rust_i18n::t;

use moon_chart::axes::fmt_clock_dated;
use moon_chart::news_marks::{MarkHit, NewsMark};
use moon_core::config::Language;
use moon_core::feed::NewsItem;
use moon_core::feed::news_marks;
use moon_core::util::now_unix_ms_i64;

use super::ChartPanel;
use crate::design;
use crate::panels::news::{news_badge, tag_color, tag_palette_sig};

/// Card width in logical pixels — wide enough for a two-line headline, narrow enough to sit over a
/// chart without covering it. It goes through `design::ui_px` like the card's padding, so the whole
/// card follows the UI-scale slider.
const CARD_W: f32 = 320.0;
/// Gap between the gem's top tip and the card's bottom edge.
const CARD_GAP: f32 = 8.0;
/// Gap kept above the card so it never touches the slot's top edge.
const CARD_TOP_INSET: f32 = 8.0;
/// Floor for the card's height, below which clipping it further shows nothing useful.
const CARD_MIN_H: f32 = 48.0;
/// How many stacked items the card spells out before collapsing the rest into a counter.
const CARD_MAX_ITEMS: usize = 3;
/// Longest body the card renders. News bodies are service-controlled and unbounded; beyond this the
/// card is a wall of text nobody reads and GPUI shapes glyphs that the slot then clips away.
const CARD_BODY_CHARS: usize = 320;

/// This chart's news marks and the hover they drive.
///
/// One struct rather than seven fields on `ChartPanel`, matching how the panel already keeps its
/// input state: everything here is owned and mutated by this module alone.
#[derive(Default)]
pub(super) struct NewsState {
    /// Marks oldest first: the item backs the hover card, the mark carries the time and tag colours
    /// the gem is drawn from. Rebuilt as ONE list so the two halves cannot drift apart.
    items: Rc<Vec<(NewsItem, NewsMark)>>,
    /// The marks alone, shared with the engine's own-pass geometry.
    marks: Rc<Vec<NewsMark>>,
    /// News/tag-filter/palette revision the marks were built from; `None` means none built yet.
    sig: Option<u64>,
    /// Market the marks belong to, so a slot reused for another coin rebuilds them.
    market: Option<String>,
    /// Marks under the cursor: the nearest one grows, the stack fills the card.
    hover: Option<MarkHit>,
    /// Point of the most recent hit-test, carrying the same Delphi movement threshold the order-line
    /// hover uses.
    probe: Option<(f32, f32)>,
    /// Whether the secondary modifier (Ctrl, Command on macOS) was held at the last pointer event.
    /// The card reads the LIVE state at render; this only decides whether a change must repaint.
    ctrl: bool,
}

/// Clamp a body to [`CARD_BODY_CHARS`] on a CHARACTER boundary (news is UTF-8 from a live service).
/// Borrows when it fits, which is the usual case.
fn clamp_body(text: &str) -> std::borrow::Cow<'_, str> {
    match text.char_indices().nth(CARD_BODY_CHARS) {
        Some((cut, _)) => std::borrow::Cow::Owned(format!("{}…", &text[..cut])),
        None => std::borrow::Cow::Borrowed(text),
    }
}

impl ChartPanel {
    /// Rebuild this chart's news marks when the news revision, the tag filter, or the palette moved,
    /// and publish them to the engine. Returns whether the engine's geometry changed.
    ///
    /// Called from the Backend observer, which fires far more often than news arrives; the signature
    /// gate is what keeps that cheap.
    pub(super) fn sync_news_marks(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(market) = self.chart.active_target().map(|(_, market)| market) else {
            // No market on this slot: drop the marks so a reused panel cannot show another coin's.
            return self.clear_news(cx);
        };
        let palette = MoonPalette::active(cx);
        let sig = {
            let b = self.backend.read(cx);
            news_marks::signature(b.session.store(), &b.news_tag_settings)
                // Tag colours are resolved into the instances here, so a theme switch rebuilds them.
                .wrapping_add(tag_palette_sig(palette))
        };
        // The market is compared by value, not folded into the hash: a collision there would show
        // another coin's news on this chart.
        if self.news.sig == Some(sig) && self.news.market.as_deref() == Some(market.as_str()) {
            return false;
        }
        self.news.sig = Some(sig);
        self.news.market = Some(market.clone());
        let now_ms = now_unix_ms_i64();
        let collected = {
            let b = self.backend.read(cx);
            let settings = &b.news_tag_settings;
            news_marks::collect(b.session.store(), &market, settings, now_ms)
                .into_iter()
                .map(|(item, time_ms)| {
                    // Every distinct palette colour among the item's VISIBLE tags takes a wedge; a
                    // tag the user unticked in the News panel paints nothing here either.
                    let colors = item.tags.iter().filter_map(|tag| {
                        (!settings.is_hidden(&tag.to_lowercase()))
                            .then(|| tag_color(tag, settings, palette))
                            .flatten()
                    });
                    let mark = NewsMark::new(time_ms, colors);
                    (item, mark)
                })
                .collect::<Vec<_>>()
        };
        self.news.marks = Rc::new(collected.iter().map(|(_, mark)| *mark).collect());
        self.news.items = Rc::new(collected);
        // Indices into the old list mean nothing now; re-derive the hover from the live cursor.
        self.news.probe = None;
        self.news.hover = self.news_hit_at_cursor();
        self.publish_news_marks(cx)
    }

    /// Push the current marks and the hovered one to the engine, forcing the userdata rebuild that
    /// owns them.
    fn publish_news_marks(&mut self, cx: &mut Context<Self>) -> bool {
        let hovered = self.news.hover.as_ref().map(|h| h.nearest);
        if !self.chart.set_news_marks(self.news.marks.clone(), hovered) {
            return false;
        }
        // Userdata rebuilds only through `sync_orders_*`; trigger it now so the marks appear with
        // the news instead of waiting for the next order revision.
        let b = self.backend.read(cx);
        self.chart.sync_orders_if_visible(&b.session, true);
        true
    }

    /// Forget every mark (slot emptied or reused for another market).
    fn clear_news(&mut self, cx: &mut Context<Self>) -> bool {
        if self.news.sig.is_none() {
            return false;
        }
        self.news = NewsState::default();
        self.publish_news_marks(cx)
    }

    /// Update the hovered marks from a pointer position.
    ///
    /// `within` is false when the pointer left the chart slot, which clears the hover — "move the
    /// mouse away and the text disappears". Returns whether the GPUI tree must repaint, which is
    /// ONLY while the card is (or was) on screen: the gem's own growth is own-pass and presents
    /// itself.
    pub(super) fn sync_news_hover(
        &mut self,
        pos: (f32, f32),
        within: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if !within {
            return self.clear_news_hover(cx);
        }
        // Delphi threshold: raw pointer events arrive with subpixel jitter, and rescanning for it is
        // wasted work (docs-internal/INPUT_HOTPATH_NORMS.md).
        if !super::trade::hover_probe_due(self.news.probe, pos) {
            return false;
        }
        self.news.probe = Some(pos);
        let hit = self.news_hit(pos);
        self.apply_news_hover(hit, cx)
    }

    /// Drop the hover when the pointer leaves the chart slot entirely.
    pub(super) fn clear_news_hover(&mut self, cx: &mut Context<Self>) -> bool {
        self.news.probe = None;
        self.apply_news_hover(None, cx)
    }

    /// Re-run the hit test from the last cursor position, without the movement threshold.
    ///
    /// The chart scrolls with the live edge between pointer events, so marks slide out from under a
    /// resting cursor; the render path calls this so neither a grown gem nor a card outlives the
    /// mark it belongs to.
    pub(super) fn revalidate_news_hover(&mut self, cx: &mut Context<Self>) {
        if self.news.hover.is_none() {
            return;
        }
        let hit = self.news_hit_at_cursor();
        // No notify: this runs INSIDE render, and the card is built from the fresh state right after.
        self.apply_news_hover(hit, cx);
    }

    /// Hit-test at the panel's last known cursor position, if it has one.
    fn news_hit_at_cursor(&self) -> Option<MarkHit> {
        self.news_hit(self.input.cursor?)
    }

    /// Hit-test the marks' row at `pos` (panel-local device pixels).
    fn news_hit(&self, pos: (f32, f32)) -> Option<MarkHit> {
        if self.news.marks.is_empty() {
            return None;
        }
        // Book-only (broom) mode leaves the plot 1 px wide and the engine skips the whole layer
        // there; the panel's layout helper does not model that mode, so without this guard it would
        // report hits on marks that were never drawn.
        if self.orderbook_only {
            return None;
        }
        let pane = self.input.pane_at(pos.0, pos.1)?;
        let map = self.pane_map(pane)?;
        if !moon_chart::news_marks::in_mark_row(pos.1, map.plot.y + map.plot.h, self.last_ppp) {
            return None;
        }
        // Horizontal reach matches the shader's own clip: marks exist only over the plot, never in
        // the price-axis gutter or the order-book/control zone beside it.
        if pos.0 < map.plot.x || pos.0 > map.plot.x + map.plot.w {
            return None;
        }
        moon_chart::news_marks::hit_marks(
            self.news
                .marks
                .iter()
                .map(|m| map.x_of_time(m.time_ms as f64)),
            pos.0,
            self.last_ppp,
        )
    }

    /// Apply a hit result and republish the grown mark. Returns whether the GPUI tree must repaint.
    fn apply_news_hover(&mut self, hit: Option<MarkHit>, cx: &mut Context<Self>) -> bool {
        if self.news.hover == hit {
            return false;
        }
        self.news.hover = hit;
        self.publish_news_marks(cx);
        self.news.ctrl
    }

    /// Track the secondary modifier and repaint when that changes what the card shows.
    ///
    /// Reached from pointer moves and from GPUI's modifier event, which is dispatched along the
    /// FOCUS path — an unfocused chart may never see it, so [`Self::news_card`] reads the live state
    /// at render time and this only decides whether to repaint sooner.
    pub(super) fn note_news_modifiers(&mut self, modifiers: Modifiers, cx: &mut Context<Self>) {
        let ctrl = modifiers.secondary();
        if self.news.ctrl == ctrl {
            return;
        }
        self.news.ctrl = ctrl;
        // Only a mark under the cursor can produce a card; a bare Ctrl hotkey must not repaint.
        if self.news.hover.is_some() {
            cx.notify();
        }
    }

    /// The hover card, or `None` when nothing is hovered or Ctrl (Command on macOS) is not held.
    ///
    /// Anchored at the CURSOR, not at the mark: the mark moves with the live edge every frame while
    /// GPUI repaints at a fraction of that, so a mark-anchored card would visibly lag behind it.
    pub(super) fn news_card(
        &self,
        ppp: f32,
        ctrl: bool,
        palette: MoonPalette,
        cx: &App,
    ) -> Option<AnyElement> {
        if !ctrl {
            return None;
        }
        let hover = self.news.hover.as_ref()?;
        let (cursor_x, _) = self.input.cursor?;
        let ppp = ppp.max(0.1);
        let slot = self.chart.slot_dev_size();
        let slot_w = slot.0 as f32 / ppp;
        let slot_h = slot.1 as f32 / ppp;
        let card_w = f32::from(design::ui_px(cx, CARD_W));
        // Device pixels → the slot's logical pixels the overlay is laid out in. Keep the card fully
        // inside the slot, centred on the cursor when there is room.
        let left = (cursor_x / ppp - card_w * 0.5).clamp(0.0, (slot_w - card_w).max(0.0));
        // Sit above the marks' row: the axis gutter, then the gem at its HOVERED size, then a gap.
        // Derived from the same constants the geometry uses, so the card cannot drift from the marks.
        let axis_h = if self.time_axis_visible {
            moon_chart::TIME_AXIS_H
        } else {
            0.0
        };
        let bottom = axis_h
            + moon_chart::news_marks::mark_center_offset(true) * 2.0
            + f32::from(design::ui_px(cx, CARD_GAP));
        let now_ms = now_unix_ms_i64();
        let shown = hover.stack.len().min(CARD_MAX_ITEMS);
        let hidden = hover.stack.len() - shown;
        let rows: Vec<AnyElement> = hover.stack[..shown]
            .iter()
            .filter_map(|&i| {
                let (item, mark) = self.news.items.get(i)?;
                Some(news_row(item, mark, now_ms, palette, cx))
            })
            .collect();
        if rows.is_empty() {
            return None;
        }
        let max_h = (slot_h - bottom - CARD_TOP_INSET).max(CARD_MIN_H);
        Some(
            div()
                .absolute()
                .left(px(left))
                .bottom(px(bottom))
                .w(px(card_w))
                // Never taller than the room above the marks; a long stack clips rather than
                // covering the whole chart.
                .max_h(px(max_h))
                .overflow_hidden()
                .shadow_md()
                .child(
                    MoonSurface::new()
                        .variant(MoonSurfaceVariant::Card)
                        // Opaque over a busy chart: the Card variant's 2% tint would be unreadable.
                        .bg_color(palette.card)
                        .bg_alpha(1.0)
                        .border_color(palette.border_card)
                        .child(
                            v_flex()
                                .w_full()
                                .gap(design::ui_px(cx, 6.0))
                                .p(design::ui_px(cx, 8.0))
                                .children(rows)
                                .when(hidden > 0, |this| {
                                    this.child(
                                        div()
                                            .text_size(design::t_caption(cx))
                                            .text_color(rgb(palette.text_muted))
                                            .child(t!("news.chart.more", n = hidden).to_string()),
                                    )
                                }),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// How much later than publication the mark sits, in ms, or `None` when there is nothing to say.
///
/// The mark is drawn at delivery — when the news became actionable — while the item keeps its
/// publication stamp. The difference is the delivery delay, the same figure the copied text puts in
/// brackets. Saturating: these stamps are service-controlled.
fn delivery_lag_ms(item: &NewsItem, mark_time_ms: i64) -> Option<i64> {
    let published = (item.time_ms > 0).then_some(item.time_ms)?;
    let lag = mark_time_ms.saturating_sub(published);
    (lag > 0).then_some(lag)
}

/// One news item inside the card: the clock, its tag-colour dots and source, then the body.
fn news_row(item: &NewsItem, mark: &NewsMark, now_ms: i64, p: MoonPalette, cx: &App) -> AnyElement {
    // The chart's own axis formatter, so the card and the axis under it cannot disagree about where
    // the gem sits — and it adds the date when the mark is not from today.
    let clock = fmt_clock_dated(
        mark.time_ms as f64,
        crate::chartdx::axes::local_offset_sec(),
        true,
        now_ms as f64,
    );
    let mut head = h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 5.0))
        .child(
            div()
                .flex_none()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .font_family(design::mono())
                .child(clock),
        );
    // The gem sits at DELIVERY; this says how much later than publication that was, so the gap
    // between the move and the mark has a number on it. Absent when the item carries no
    // publication stamp of its own, or when the two coincide.
    if let Some(lag) = delivery_lag_ms(item, mark.time_ms) {
        head = head.child(
            div()
                .flex_none()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.amber))
                .font_family(design::mono())
                .child(format!("+{lag} {}", t!("news.lat.unit"))),
        );
    }
    // The same colours the gem is cut into, so the card explains what a coloured mark meant.
    for &color in mark.colors() {
        head = head.child(design::status_dot(color, cx));
    }
    if !item.source.is_empty() {
        head = head.child(news_badge(item.source.to_uppercase(), p.text_muted));
    }
    let lang = Language::from_code(&rust_i18n::locale()).unwrap_or(Language::En);
    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 3.0))
        .child(head)
        .child(
            div()
                .w_full()
                .text_size(design::t_body(cx))
                .text_color(rgb(p.text))
                .child(clamp_body(item.body(lang)).into_owned()),
        )
        .into_any_element()
}
