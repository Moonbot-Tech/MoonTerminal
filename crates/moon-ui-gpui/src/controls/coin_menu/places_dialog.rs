//! Where one coin is blacklisted, and the one-click removal of each place.
//!
//! Rows are read from the live store on every render. A removal that the core accepts is hidden
//! at once; the row comes back if that place is still listed after 15 seconds, or disappears for
//! good once the store drops it. A refusal leaves the row and says so.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;
use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext as _, Context, Entity, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement as _, Styled, Subscription,
    Window, div, px, svg,
};
use moon_core::feed::FIELD_COINS_BLACK_LIST;
use moon_core::session::CoreId;
use moon_ui::{
    MoonButton, MoonDialog, MoonPalette, MoonTag, MoonTagVariant, MoonTone, MoonWindowExt as _,
    h_flex, v_flex,
};
use rust_i18n::t;

use crate::Backend;
use crate::design;
use crate::display_text::plural_key;
use crate::strategies::{RevealField, open_goto_field};

use super::places::{self, CoinPlace};
use super::{write_core_blacklist, write_strategy_blacklist};

/// How long an accepted removal stays hidden before a still-listed place is shown again.
pub(super) const HIDE_MS: i64 = 15_000;
/// Horizontal slack around one button label. The button draws its own padding on top of the text.
const BUTTON_CHROME: f32 = 56.0;
/// Horizontal slack around one tag label.
const TAG_CHROME: f32 = 48.0;
/// Gap between the tag, the text, and the buttons, in design pixels.
const GAP: f32 = 8.0;
/// Room for the dialog's close control, which sits over the header's trailing edge.
const HEADER_CLOSE_SLACK: f32 = 28.0;
/// Extra pixels so a glyph-advance estimate cannot clip the last character.
const SLACK: f32 = 16.0;
/// Dialog padding and border outside the column. The content box is this much narrower than `.w`.
const DIALOG_CHROME: f32 = 48.0;
/// Slack between the dialog and the viewport edge.
const VIEWPORT_MARGIN: f32 = 24.0;
/// Narrowest column. A viewport smaller than the chrome still leaves a readable box.
const COLUMN_FLOOR: f32 = 160.0;
const DIALOG_ID: &str = "coin-bl-places";

/// Identity of one hidden place. The market or strategy id keeps two rows on one core apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PlaceKey {
    Core(CoreId),
    Temp { core: CoreId, market: String },
    Strategy { core: CoreId, sid: u64 },
}

/// One optimistic hide and the instant it was accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct HiddenPlace {
    pub key: PlaceKey,
    pub at_ms: i64,
}

/// Which confirmation sentence to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ConfirmKind {
    RemovedCore,
    RemovedStrategy,
    LiftedTemp,
    Refused,
}

/// Drop hides whose place is gone, or whose accept is older than [`HIDE_MS`].
///
/// An age of exactly [`HIDE_MS`] stays. A place that is still listed after the timeout is no
/// longer hidden, so the next render shows it again.
///
/// Args:
///     hidden: Hides recorded by accepted removals, newest or oldest.
///     current: Places the store lists right now.
///     now: Current unix time in milliseconds.
pub(super) fn reconcile_hidden(hidden: &mut Vec<HiddenPlace>, current: &[CoinPlace], now: i64) {
    hidden.retain(|entry| {
        let age = now.saturating_sub(entry.at_ms);
        age <= HIDE_MS && current.iter().any(|place| place_key(place) == entry.key)
    });
}

/// Record a hide only when the core accepted the removal.
///
/// Args:
///     hidden: Hides already recorded.
///     key: Place the click addressed.
///     accepted: Whether the row should hide. `Ok(true)` and a lifted temporary ban hide it.
///         `Ok(false)` hides it too, but the caller shows no confirmation. `Err` does not.
///     now: Current unix time in milliseconds.
///
/// Returns:
///     `true` when a new hide was inserted. A refusal inserts nothing, so the row stays.
pub(super) fn note_removal(
    hidden: &mut Vec<HiddenPlace>,
    key: PlaceKey,
    accepted: bool,
    now: i64,
) -> bool {
    if !accepted || hidden.iter().any(|entry| entry.key == key) {
        return false;
    }
    hidden.push(HiddenPlace { key, at_ms: now });
    true
}

/// Places that are not currently hidden.
pub(super) fn visible_places<'a>(
    places: &'a [CoinPlace],
    hidden: &[HiddenPlace],
) -> Vec<&'a CoinPlace> {
    places
        .iter()
        .filter(|place| !hidden.iter().any(|entry| entry.key == place_key(place)))
        .collect()
}

pub(super) fn place_key(place: &CoinPlace) -> PlaceKey {
    match place {
        CoinPlace::Core { core, .. } => PlaceKey::Core(*core),
        CoinPlace::Temp { core, market, .. } => PlaceKey::Temp {
            core: *core,
            market: market.clone(),
        },
        CoinPlace::Strategy { core, sid, .. } => PlaceKey::Strategy {
            core: *core,
            sid: *sid,
        },
    }
}

/// Confirmation sentence for one removal outcome.
///
/// The check glyph is drawn beside this text. The string itself has none.
///
/// Args:
///     kind: Which outcome happened.
///     coin: Coin token the click named.
///     name: Core display name, or the strategy name. Ignored for a refusal.
///
/// Returns:
///     The localized sentence.
pub(super) fn confirm_text(kind: ConfirmKind, coin: &str, name: &str) -> String {
    match kind {
        ConfirmKind::RemovedCore => {
            t!("coin_bl_places.removed_core", coin = coin, core = name).to_string()
        }
        ConfirmKind::RemovedStrategy => {
            t!("coin_bl_places.removed_strategy", coin = coin, name = name).to_string()
        }
        ConfirmKind::LiftedTemp => {
            t!("coin_bl_places.lifted_temp", coin = coin, core = name).to_string()
        }
        ConfirmKind::Refused => t!("coin_bl_places.remove_failed", coin = coin).to_string(),
    }
}

/// How a blacklist write shows up in the places dialog.
///
/// `Ok(true)` hid a token that was there. `Ok(false)` means the text was already without it, so
/// the row still leaves and no sentence is owed. `Err` leaves the row and says the write failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RemovalUi {
    HideWithConfirm,
    HideSilent,
    KeepFailed,
}

/// Map a core or strategy blacklist write onto [`RemovalUi`].
///
/// Args:
///     outcome: `Ok(true)` when the text changed, `Ok(false)` when it did not, `Err` when refused.
///
/// Returns:
///     The dialog's hide and confirm choice.
pub(super) fn removal_ui(outcome: Result<bool, ()>) -> RemovalUi {
    match outcome {
        Ok(true) => RemovalUi::HideWithConfirm,
        Ok(false) => RemovalUi::HideSilent,
        Err(()) => RemovalUi::KeepFailed,
    }
}

/// Glyph-advance width of one raw text run at the body or caption tier.
///
/// The run is drawn with [`design::t_body`] / [`design::t_caption`], which follow the UI zoom
/// and not the legacy font-delta channel. [`design::ui_text_width_zoomed`] is the matching measure.
pub(super) fn measure_text(cx: &App, text: &str, caption: bool, weight: f32, mono: bool) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    let size = if caption {
        design::BODY_TEXT - 2.0
    } else {
        design::BODY_TEXT
    };
    design::ui_text_width_zoomed(cx, text, size, weight, mono)
}

/// One button on a place row.
#[derive(Clone)]
pub(super) struct FaceAction {
    pub label: String,
    pub enabled: bool,
    pub tip: Option<String>,
    pub on_click: Rc<dyn Fn(&mut Window, &mut App)>,
}

/// One drawn place.
pub(super) struct FaceRow {
    pub title: String,
    pub sub: String,
    pub tag: String,
    pub tag_tone: MoonTone,
    pub open: Option<FaceAction>,
    pub action: FaceAction,
}

/// Green or red line under the summary.
pub(super) struct FaceConfirm {
    pub text: String,
    pub ok: bool,
}

/// Everything the column draws. The layout test builds this without a backend.
pub(super) struct PlacesFace {
    pub coin: String,
    pub title: String,
    pub summary: String,
    pub rows: Vec<FaceRow>,
    pub empty: Option<String>,
    pub footer: String,
    pub confirm: Option<FaceConfirm>,
}

/// Density-scaled contribution of one strategy path to the dialog width.
///
/// Longer paths wrap at the available row width instead of widening the dialog.
///
/// Args:
///     cx: Application context that carries the font scale.
///
/// Returns:
///     The maximum title contribution to dialog sizing, in pixels.
pub(super) fn title_cap(cx: &App) -> f32 {
    design::font_w(cx, 600.0)
}

/// Width of the column, in pixels, from the strings it will draw.
///
/// A strategy path contributes at most [`title_cap`]. The sub line, the tag, and the buttons
/// still take their full measure, and the dialog grows to the widest of those.
pub(super) fn column_width(face: &PlacesFace, cx: &App) -> f32 {
    let cap = title_cap(cx);
    let mut width = measure_text(cx, &face.coin, false, 700.0, true)
        + GAP
        + measure_text(cx, &face.title, true, 400.0, false)
        + GAP
        + measure_text(cx, &face.summary, true, 400.0, false)
        + HEADER_CLOSE_SLACK;
    width = width.max(measure_text(cx, &face.footer, true, 400.0, false));
    if let Some(empty) = &face.empty {
        width = width.max(measure_text(cx, empty, false, 400.0, false));
    }
    if let Some(confirm) = &face.confirm {
        let icon = if confirm.ok { 20.0 + GAP } else { 0.0 };
        width = width.max(measure_text(cx, &confirm.text, false, 400.0, false) + icon);
    }
    let tag = tag_column_width(face, cx);
    for row in &face.rows {
        let text = measure_text(cx, &row.title, false, 400.0, true)
            .min(cap)
            .max(measure_text(cx, &row.sub, true, 400.0, false));
        let mut buttons = measure_text(cx, &row.action.label, false, 400.0, false) + BUTTON_CHROME;
        if let Some(open) = &row.open {
            buttons += GAP + measure_text(cx, &open.label, false, 400.0, false) + BUTTON_CHROME;
        }
        width = width.max(tag + GAP + text + GAP + buttons);
    }
    width + SLACK
}

/// Width of the tag column, in pixels: the widest tag, so every path starts at one edge.
fn tag_column_width(face: &PlacesFace, cx: &App) -> f32 {
    face.rows
        .iter()
        .map(|row| measure_text(cx, &row.tag, true, 400.0, false) + TAG_CHROME)
        .fold(0.0, f32::max)
}

/// Widest the column may be for this viewport, in pixels.
///
/// The dialog's own padding sits outside the column, and a margin stays clear of the window edge.
///
/// Args:
///     viewport_w: Window viewport width, in pixels.
///
/// Returns:
///     The cap. It is never below [`COLUMN_FLOOR`].
pub(super) fn column_width_cap(viewport_w: f32) -> f32 {
    (viewport_w - VIEWPORT_MARGIN - DIALOG_CHROME).max(COLUMN_FLOOR)
}

/// Tallest the place-row scroller may be for this viewport, in pixels.
///
/// Args:
///     viewport_h: Window viewport height, in pixels.
///
/// Returns:
///     55% of the viewport, and at least 96.
pub(super) fn rows_height_cap(viewport_h: f32) -> f32 {
    (viewport_h * 0.55).max(96.0)
}

/// The column the dialog body and the layout test both draw.
///
/// Args:
///     face: Strings and rows to draw.
///     viewport: Window the dialog is fitting into.
///     cx: Application context for the type scale and the palette.
///
/// Returns:
///     The column. Long sentences wrap inside the capped width. Rows scroll; the header and the
///     footer stay outside that scroller.
pub(super) fn places_column(
    face: &PlacesFace,
    viewport: gpui::Size<gpui::Pixels>,
    cx: &App,
) -> impl IntoElement + use<> {
    let p = MoonPalette::active(cx);
    let width = column_width(face, cx).min(column_width_cap(f32::from(viewport.width)));
    let mut column = v_flex()
        .w(px(width))
        .max_w(px(width))
        .flex_none()
        .overflow_hidden()
        .gap(px(GAP))
        .debug_selector(|| "coin-bl-dialog".into())
        .child(header_row(face, p, cx));
    if let Some(confirm) = &face.confirm {
        column = column.child(confirm_line(confirm, p, cx));
    }
    if face.rows.is_empty() {
        if let Some(empty) = &face.empty {
            column = column.child(sentence(
                "coin-bl-empty",
                empty,
                design::t_body(cx),
                FontWeight::NORMAL,
                design::moon(p.text),
            ));
        }
    } else {
        let tag_w = tag_column_width(face, cx);
        let mut rows = div()
            .id("coin-bl-rows")
            .flex()
            .flex_col()
            .w_full()
            .flex_none()
            .max_h(px(rows_height_cap(f32::from(viewport.height))))
            .overflow_y_scroll()
            .gap(px(GAP))
            .debug_selector(|| "coin-bl-rows".into());
        for (index, row) in face.rows.iter().enumerate() {
            rows = rows.child(place_row(index, row, tag_w, p, cx));
        }
        column = column.child(rows);
    }
    column.child(sentence(
        "coin-bl-footer",
        &face.footer,
        design::t_caption(cx),
        FontWeight::NORMAL,
        design::moon(p.text_muted),
    ))
}

/// Coin, caption and summary on one line; it wraps only when the viewport is narrower.
fn header_row(face: &PlacesFace, p: MoonPalette, cx: &App) -> impl IntoElement + use<> {
    h_flex()
        .w_full()
        .flex_none()
        .flex_wrap()
        .items_baseline()
        .gap(px(GAP))
        .pr(px(HEADER_CLOSE_SLACK))
        .child(text_run(
            "coin-bl-coin",
            &face.coin,
            design::mono(),
            design::t_body(cx),
            FontWeight::BOLD,
            design::moon(p.text),
        ))
        .child(text_run(
            "coin-bl-title",
            &face.title,
            design::ui_font(),
            design::t_caption(cx),
            FontWeight::NORMAL,
            design::moon(p.text_muted),
        ))
        .child(text_run(
            "coin-bl-summary",
            &face.summary,
            design::ui_font(),
            design::t_caption(cx),
            FontWeight::NORMAL,
            design::moon(p.text_muted),
        ))
}

fn confirm_line(confirm: &FaceConfirm, p: MoonPalette, cx: &App) -> impl IntoElement + use<> {
    let color = if confirm.ok {
        design::moon(design::positive_color(p))
    } else {
        design::moon(design::danger_color(p))
    };
    h_flex()
        .w_full()
        .flex_none()
        .items_start()
        .gap(px(GAP))
        .debug_selector(|| "coin-bl-confirm".into())
        .when(confirm.ok, |row| {
            row.child(
                svg()
                    .path(moon_ui::MOON_ICON_CHECK)
                    .size(design::t_body(cx))
                    .text_color(color)
                    .flex_none(),
            )
        })
        .child({
            let text = confirm.text.clone();
            div()
                .flex_1()
                .min_w_0()
                .whitespace_normal()
                .font_family(design::ui_font())
                .text_size(design::t_body(cx))
                .line_height(design::t_body(cx) + px(3.0))
                .font_weight(FontWeight::NORMAL)
                .text_color(color)
                .debug_selector(|| "coin-bl-confirm-text".into())
                .child(text)
        })
}

/// Lay out a wrapping path and caption between fixed-size, top-aligned tags and actions.
fn place_row(
    index: usize,
    row: &FaceRow,
    tag_w: f32,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement + use<> {
    let mut buttons = h_flex().flex_none().ml_auto().items_center().gap(px(GAP));
    if let Some(open) = &row.open {
        buttons = buttons.child(action_button(
            format!("coin-bl-open-{index}"),
            format!("coin-bl-row-{index}-open"),
            open,
        ));
    }
    buttons = buttons.child(action_button(
        format!("coin-bl-act-{index}"),
        format!("coin-bl-row-{index}-remove"),
        &row.action,
    ));
    h_flex()
        .w_full()
        .flex_none()
        .items_start()
        .gap(px(GAP))
        .child(
            div()
                .w(px(tag_w))
                .flex_none()
                .debug_selector(move || format!("coin-bl-row-{index}-tag"))
                .child(
                    MoonTag::new()
                        .tone(row.tag_tone)
                        .variant(MoonTagVariant::Soft)
                        .compact(true)
                        .mono(false)
                        .label(row.tag.clone()),
                ),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .items_start()
                .child(title_run(
                    &format!("coin-bl-row-{index}-title"),
                    &row.title,
                    cx,
                    design::moon(p.text),
                ))
                .child(sentence(
                    &format!("coin-bl-row-{index}-sub"),
                    &row.sub,
                    design::t_caption(cx),
                    FontWeight::NORMAL,
                    design::moon(p.text_muted),
                )),
        )
        .child(buttons)
}

fn action_button(id: String, selector: String, action: &FaceAction) -> impl IntoElement + use<> {
    let mut button = MoonButton::new(SharedString::from(id))
        .label(action.label.clone())
        .size(design::CONTROL_TIER)
        .disabled(!action.enabled);
    if let Some(tip) = &action.tip {
        button = button.tooltip(tip.clone());
    }
    let on_click = action.on_click.clone();
    div()
        .flex_none()
        .debug_selector(move || selector.clone())
        .child(button.on_click(move |_, window, cx| on_click(window, cx)))
}

/// Draw the complete path at the row's available width, including long unbroken names.
fn title_run(selector: &str, text: &str, cx: &App, color: Hsla) -> impl IntoElement + use<> {
    let selector = selector.to_string();
    let text = text.to_string();
    div()
        .w_full()
        .min_w_0()
        .whitespace_normal()
        .font_family(design::mono())
        .text_size(design::t_body(cx))
        .line_height(design::t_body(cx) + px(3.0))
        .font_weight(FontWeight::NORMAL)
        .text_color(color)
        .debug_selector(move || selector)
        .child(text)
}

fn text_run(
    selector: &str,
    text: &str,
    font: SharedString,
    size: gpui::Pixels,
    weight: FontWeight,
    color: Hsla,
) -> impl IntoElement + use<> {
    let selector = selector.to_string();
    let text = text.to_string();
    div()
        .flex_none()
        .whitespace_nowrap()
        .font_family(font)
        .text_size(size)
        .line_height(size + px(3.0))
        .font_weight(weight)
        .text_color(color)
        .debug_selector(move || selector.clone())
        .child(text)
}

/// A sentence that wraps inside the column instead of widening it.
///
/// Args:
///     selector: Debug selector the layout test reads.
///     text: The sentence.
///     size: Font size.
///     weight: Font weight.
///     color: Text colour.
///
/// Returns:
///     A full-width run with normal wrapping.
fn sentence(
    selector: &str,
    text: &str,
    size: gpui::Pixels,
    weight: FontWeight,
    color: Hsla,
) -> impl IntoElement + use<> {
    let selector = selector.to_string();
    let text = text.to_string();
    div()
        .w_full()
        .min_w_0()
        .whitespace_normal()
        .font_family(design::ui_font())
        .text_size(size)
        .line_height(size + px(3.0))
        .font_weight(weight)
        .text_color(color)
        .debug_selector(move || selector)
        .child(text)
}

/// What a click should do once the dialog has checked access.
enum PendingClose {
    /// Core settings were queued. Close the dialog.
    Dialog,
    /// Open the strategy field, then close. Done outside the dialog update so it can use the window.
    Strategy {
        core: CoreId,
        sid: u64,
        group: String,
        coin: String,
    },
}

/// One row click, captured while the row was built.
#[derive(Clone)]
enum RowAction {
    OpenCore {
        core: CoreId,
    },
    OpenStrategy {
        core: CoreId,
        sid: u64,
        group: Option<String>,
    },
    RemoveCore {
        core: CoreId,
        name: String,
    },
    RemoveStrategy {
        core: CoreId,
        sid: u64,
        name: String,
    },
    LiftTemp {
        core: CoreId,
        market: String,
        name: String,
    },
}

struct ConfirmState {
    kind: ConfirmKind,
    name: String,
}

pub(crate) struct PlacesDialog {
    backend: Entity<Backend>,
    coin: String,
    hidden: Vec<HiddenPlace>,
    confirm: Option<ConfirmState>,
    /// Temp-ban market resolved to its coin. An empty label, or one equal to the raw market, is absent.
    base_cache: HashMap<(CoreId, String), String>,
    /// Temp-ban markets the cache was last built from. A matching set skips `market_label`.
    cached_markets: BTreeSet<(CoreId, String)>,
    /// Per-core catalogue stamps the cache was built from.
    ///
    /// `None` is a real stamp: a provider that appears later re-queries every market.
    cached_revisions: BTreeMap<CoreId, Option<(CoreId, u64, u64)>>,
    _store_echo: Subscription,
}

impl PlacesDialog {
    fn new(backend: Entity<Backend>, coin: String, cx: &mut Context<Self>) -> Self {
        let _store_echo = cx.observe(&backend, |this, _, cx| {
            this.refresh_base_cache(cx);
            cx.notify();
        });
        let mut dialog = Self {
            backend,
            coin,
            hidden: Vec::new(),
            confirm: None,
            base_cache: HashMap::new(),
            cached_markets: BTreeSet::new(),
            cached_revisions: BTreeMap::new(),
            _store_echo,
        };
        dialog.refresh_base_cache(cx);
        dialog
    }

    /// Temp-ban markets currently on the connected cores.
    ///
    /// Args:
    ///     cx: Application context the backend is read through.
    ///
    /// Returns:
    ///     One `(core, market)` pair per temporary ban.
    fn temp_ban_markets(&self, cx: &App) -> BTreeSet<(CoreId, String)> {
        let backend = self.backend.read(cx);
        let ids: Vec<CoreId> = backend
            .session
            .sessions()
            .iter()
            .map(|session| session.id)
            .collect();
        let store = backend.session.store();
        let mut markets = BTreeSet::new();
        for id in ids {
            let Some(data) = store.core(id) else {
                continue;
            };
            for (market, _) in data.temp_bans() {
                markets.insert((id, market.to_string()));
            }
        }
        markets
    }

    /// Fill [`Self::base_cache`] when the temp-ban set or a catalogue stamp has changed.
    ///
    /// A market already resolved is copied only while its core's catalog revision
    /// is the stamp stored last time, and only when that label is a real coin. An empty label, or a
    /// label equal to the raw market (`@206`), is left out. A moved stamp re-queries every market
    /// instead of copying the previous cache.
    ///
    /// Args:
    ///     cx: Application context the backend is read through.
    ///
    /// Returns:
    ///     Whether the market set or a catalogue stamp changed.
    fn refresh_base_cache(&mut self, cx: &App) -> bool {
        let markets = self.temp_ban_markets(cx);
        let revisions = {
            let source = self.backend.read(cx).session.market_source();
            let mut revisions = BTreeMap::new();
            for (core, _) in &markets {
                revisions
                    .entry(*core)
                    .or_insert_with(|| source.catalog_revision(*core));
            }
            revisions
        };
        if markets == self.cached_markets && revisions == self.cached_revisions {
            return false;
        }
        let moved = revisions != self.cached_revisions;
        let source = self.backend.read(cx).session.market_source();
        let mut next = HashMap::new();
        for key in &markets {
            let market = key.1.as_str();
            if !moved
                && let Some(base) = self.base_cache.get(key)
                && !base.is_empty()
                && base != market
            {
                next.insert(key.clone(), base.clone());
                continue;
            }
            let label = source.market_label(key.0, market).coin;
            if !label.is_empty() && label != market {
                next.insert(key.clone(), label);
            }
        }
        self.base_cache = next;
        self.cached_markets = markets;
        self.cached_revisions = revisions;
        true
    }

    /// Places for this coin, resolving temp bans from the cache.
    ///
    /// Args:
    ///     cx: Application context the backend is read through.
    ///
    /// Returns:
    ///     Rows from [`places::find_coin_places`].
    fn places_from_cache(&self, cx: &App) -> Vec<CoinPlace> {
        let coin = self.coin.clone();
        let cache = self.base_cache.clone();
        let backend = self.backend.read(cx);
        let ids: Vec<CoreId> = backend
            .session
            .sessions()
            .iter()
            .map(|session| session.id)
            .collect();
        let store = backend.session.store();
        let cores = ids
            .iter()
            .filter_map(|id| store.core(*id).map(|data| (*id, data)));
        places::find_coin_places(cores, &coin, |core, market| {
            cache.get(&(core, market.to_string())).cloned()
        })
    }

    fn outer_width(&self, viewport: gpui::Size<gpui::Pixels>, cx: &App) -> f32 {
        let now = now_ms();
        let places = self.places_from_cache(cx);
        let hidden = self.preview_hidden(&places, now);
        let face = self.present(cx, &places, &hidden, &|_| Rc::new(|_, _| {}));
        let content = column_width(&face, cx).min(column_width_cap(f32::from(viewport.width)));
        content + DIALOG_CHROME
    }

    fn preview_hidden(&self, places: &[CoinPlace], now: i64) -> Vec<HiddenPlace> {
        let mut hidden = self.hidden.clone();
        reconcile_hidden(&mut hidden, places, now);
        hidden
    }

    fn present(
        &self,
        cx: &App,
        places: &[CoinPlace],
        hidden: &[HiddenPlace],
        bind: &dyn Fn(RowAction) -> Rc<dyn Fn(&mut Window, &mut App)>,
    ) -> PlacesFace {
        let backend = self.backend.read(cx);
        let shown = visible_places(places, hidden);
        let (place_count, core_count) = places::places_summary(shown.iter().copied());
        let mut rows = Vec::with_capacity(shown.len());
        for place in shown {
            rows.push(self.face_row(backend, place, bind));
        }
        let confirm = self.confirm.as_ref().map(|state| FaceConfirm {
            text: confirm_text(state.kind, &self.coin, &state.name),
            ok: state.kind != ConfirmKind::Refused,
        });
        let empty = rows
            .is_empty()
            .then(|| t!("coin_bl_places.empty", coin = self.coin.clone()).to_string());
        PlacesFace {
            coin: self.coin.clone(),
            title: t!("coin_bl_places.title").to_string(),
            summary: summary_line(place_count, core_count),
            rows,
            empty,
            footer: t!("coin_bl_places.footer").to_string(),
            confirm,
        }
    }

    fn face_row(
        &self,
        backend: &Backend,
        place: &CoinPlace,
        bind: &dyn Fn(RowAction) -> Rc<dyn Fn(&mut Window, &mut App)>,
    ) -> FaceRow {
        let core = place.core();
        let name = super::core_ref::core_session_name(backend, core);
        let allowed = super::core_ref::core_action_allowed(backend, core);
        let group = super::core_ref::core_action_group(backend, core);
        let tip = (!allowed).then(|| t!("coin_bl_places.no_access").to_string());
        let open_label = t!("coin_bl_places.open").to_string();
        let remove_label = t!("coin_bl_places.remove").to_string();
        let lift_label = t!("coin_bl_places.lift").to_string();
        match place {
            CoinPlace::Core { list_enabled, .. } => {
                let mut sub = t!("coin_bl_places.core_path").to_string();
                if !list_enabled {
                    sub.push(' ');
                    sub.push_str(&t!("coin_bl_places.list_off"));
                }
                FaceRow {
                    title: name.clone(),
                    sub,
                    tag: t!("coin_bl_places.tag_core").to_string(),
                    tag_tone: MoonTone::Danger,
                    open: Some(action(
                        open_label,
                        allowed,
                        tip.clone(),
                        bind(RowAction::OpenCore { core }),
                    )),
                    action: action(
                        remove_label,
                        allowed,
                        tip,
                        bind(RowAction::RemoveCore { core, name }),
                    ),
                }
            }
            CoinPlace::Temp {
                market, until_ms, ..
            } => {
                let left = crate::display_text::fmt_ban_left(until_ms - now_ms());
                FaceRow {
                    title: name.clone(),
                    sub: t!("coin_bl_places.temp_left", time = left).to_string(),
                    tag: t!("coin_bl_places.tag_temp").to_string(),
                    tag_tone: MoonTone::Warning,
                    open: None,
                    action: action(
                        lift_label,
                        allowed,
                        tip,
                        bind(RowAction::LiftTemp {
                            core,
                            market: market.clone(),
                            name,
                        }),
                    ),
                }
            }
            CoinPlace::Strategy {
                sid,
                folder,
                strategy,
                ..
            } => FaceRow {
                title: strategy_title(&name, folder, strategy),
                sub: t!("coin_bl_places.strategy_field").to_string(),
                tag: t!("coin_bl_places.tag_strategy").to_string(),
                tag_tone: MoonTone::Info,
                open: Some(action(
                    open_label,
                    allowed,
                    tip.clone(),
                    bind(RowAction::OpenStrategy {
                        core,
                        sid: *sid,
                        group: group.clone(),
                    }),
                )),
                action: action(
                    remove_label,
                    allowed,
                    tip,
                    bind(RowAction::RemoveStrategy {
                        core,
                        sid: *sid,
                        name: strategy.clone(),
                    }),
                ),
            },
        }
    }

    fn allows(&self, core: CoreId, cx: &App) -> bool {
        super::core_ref::core_action_allowed(self.backend.read(cx), core)
    }

    fn apply(&mut self, action: RowAction, cx: &mut Context<Self>) -> Option<PendingClose> {
        match action {
            RowAction::OpenCore { core } => {
                if !self.allows(core, cx) {
                    return None;
                }
                let coin = self.coin.clone();
                self.backend.update(cx, |backend, bcx| {
                    backend.core_settings_goto =
                        Some(crate::shell::CoreSettingsReveal { core, coin });
                    bcx.notify();
                });
                Some(PendingClose::Dialog)
            }
            RowAction::OpenStrategy { core, sid, group } => {
                if !self.allows(core, cx) {
                    return None;
                }
                Some(PendingClose::Strategy {
                    core,
                    sid,
                    group: group?,
                    coin: self.coin.clone(),
                })
            }
            RowAction::RemoveCore { core, name } => {
                if self.allows(core, cx) {
                    let coin = self.coin.clone();
                    let outcome = self.backend.update(cx, |backend, _| {
                        write_core_blacklist(backend, core, &coin, true)
                    });
                    self.settle_list_removal(
                        PlaceKey::Core(core),
                        outcome,
                        ConfirmKind::RemovedCore,
                        &name,
                        cx,
                    );
                }
                None
            }
            RowAction::RemoveStrategy { core, sid, name } => {
                if self.allows(core, cx) {
                    let coin = self.coin.clone();
                    let outcome = self.backend.update(cx, |backend, _| {
                        write_strategy_blacklist(backend, core, sid, &coin, true)
                    });
                    self.settle_list_removal(
                        PlaceKey::Strategy { core, sid },
                        outcome,
                        ConfirmKind::RemovedStrategy,
                        &name,
                        cx,
                    );
                }
                None
            }
            RowAction::LiftTemp { core, market, name } => {
                if self.allows(core, cx) {
                    let coin = self.coin.clone();
                    let market_key = market.clone();
                    let accepted = self.backend.update(cx, |backend, _| {
                        match backend.session.set_temp_ban(core, market, None) {
                            Ok(()) => true,
                            Err(err) => {
                                log::warn!(
                                    "coin_menu: lift {coin} from temp ban on core {core} failed: {err:#}"
                                );
                                false
                            }
                        }
                    });
                    self.finish(
                        PlaceKey::Temp {
                            core,
                            market: market_key,
                        },
                        accepted,
                        ConfirmKind::LiftedTemp,
                        &name,
                        cx,
                    );
                }
                None
            }
        }
    }

    fn finish(
        &mut self,
        key: PlaceKey,
        accepted: bool,
        kind: ConfirmKind,
        name: &str,
        cx: &mut Context<Self>,
    ) {
        let now = now_ms();
        if note_removal(&mut self.hidden, key, accepted, now) {
            wake_after_hide(cx);
        }
        self.confirm = Some(ConfirmState {
            kind: if accepted { kind } else { ConfirmKind::Refused },
            name: name.to_string(),
        });
        cx.notify();
    }

    /// Hide or keep a core/strategy blacklist row from the write's three outcomes.
    ///
    /// Args:
    ///     key: Place the click addressed.
    ///     outcome: What [`write_core_blacklist`] or [`write_strategy_blacklist`] returned.
    ///     kind: Confirmation used only when the text actually changed.
    ///     name: Core or strategy name that sentence names.
    ///     cx: View context used to repaint and to arm the hide timeout.
    fn settle_list_removal(
        &mut self,
        key: PlaceKey,
        outcome: anyhow::Result<bool>,
        kind: ConfirmKind,
        name: &str,
        cx: &mut Context<Self>,
    ) {
        match removal_ui(outcome.map_err(|_| ())) {
            RemovalUi::HideWithConfirm => self.finish(key, true, kind, name, cx),
            RemovalUi::HideSilent => {
                if note_removal(&mut self.hidden, key, true, now_ms()) {
                    wake_after_hide(cx);
                }
                self.confirm = None;
                cx.notify();
            }
            RemovalUi::KeepFailed => self.finish(key, false, kind, name, cx),
        }
    }
}

impl Render for PlacesDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.refresh_base_cache(cx) {
            cx.notify();
        }
        let now = now_ms();
        let places = self.places_from_cache(cx);
        reconcile_hidden(&mut self.hidden, &places, now);
        let this = cx.entity();
        let backend = self.backend.clone();
        let face = self.present(cx, &places, &self.hidden, &|action| {
            let this = this.clone();
            let backend = backend.clone();
            Rc::new(move |window, app| {
                let outcome = this.update(app, |dialog, cx| dialog.apply(action.clone(), cx));
                match outcome {
                    Some(PendingClose::Dialog) => window.close_dialog(app),
                    Some(PendingClose::Strategy {
                        core,
                        sid,
                        group,
                        coin,
                    }) => {
                        open_goto_field(
                            backend.clone(),
                            core,
                            sid,
                            RevealField {
                                key: FIELD_COINS_BLACK_LIST,
                                coin,
                            },
                            Some(group),
                            Some(window.window_handle()),
                            window.display(app).map(|display| display.id()),
                            app,
                        );
                        window.close_dialog(app);
                    }
                    None => {}
                }
            })
        });
        places_column(&face, window.viewport_size(), cx)
    }
}

/// Open the places dialog for `coin`.
///
/// Args:
///     backend: Live session the rows and the removals use.
///     coin: Coin token, as the caller already resolved it.
///     window: Window whose dialog layer hosts the modal.
///     cx: Application context.
pub(crate) fn open(backend: Entity<Backend>, coin: String, window: &mut Window, cx: &mut App) {
    let state = cx.new(|cx| PlacesDialog::new(backend, coin, cx));
    window.open_unique_moon_dialog(DIALOG_ID, cx, move |dialog, window, cx| {
        let width = state.read(cx).outer_width(window.viewport_size(), cx);
        places_chrome(dialog, width, cx).content({
            let state = state.clone();
            move |content, _window, _cx| content.child(state.clone())
        })
    });
}

/// Shared modal chrome. The header is drawn by the body: the dialog header slot is not the
/// live view, and the coin plus the counts change every render.
fn places_chrome(dialog: MoonDialog, width: f32, cx: &App) -> MoonDialog {
    let p = MoonPalette::active(cx);
    dialog
        .w(px(width))
        .close_button(true)
        .overlay(true)
        .overlay_closable(true)
        .bg(design::moon(p.shell_high))
        .border_color(design::moon(p.border))
        .rounded(design::r_container(cx))
        .text_color(design::moon(p.text))
}

fn action(
    label: String,
    enabled: bool,
    tip: Option<String>,
    on_click: Rc<dyn Fn(&mut Window, &mut App)>,
) -> FaceAction {
    FaceAction {
        label,
        enabled,
        tip: (!enabled).then_some(tip).flatten(),
        on_click,
    }
}

fn summary_line(places: usize, cores: usize) -> String {
    let places_n = places as u64;
    let cores_n = cores as u64;
    let places_key = plural_key("coin_bl_places.places", places_n);
    let cores_key = plural_key("coin_bl_places.cores", cores_n);
    let places_text = t!(&places_key, n = places_n).to_string();
    let cores_text = t!(&cores_key, n = cores_n).to_string();
    t!(
        "coin_bl_places.summary",
        places = places_text,
        cores = cores_text
    )
    .to_string()
}

/// `CORE / folder / strategy`, with an empty folder left out so the title is not `CORE /  / name`.
fn strategy_title(core_name: &str, folder: &str, strategy: &str) -> String {
    let mut parts = Vec::with_capacity(3);
    parts.push(core_name);
    if !folder.is_empty() {
        parts.push(folder);
    }
    parts.push(strategy);
    parts.join(" / ")
}

fn now_ms() -> i64 {
    moon_core::util::now_unix_ms_i64()
}

fn wake_after_hide(cx: &mut Context<PlacesDialog>) {
    cx.spawn(async move |this, cx| {
        cx.background_executor()
            .timer(Duration::from_millis(HIDE_MS as u64 + 50))
            .await;
        let _ = this.update(cx, |_, cx| cx.notify());
    })
    .detach();
}

#[cfg(test)]
mod tests;
