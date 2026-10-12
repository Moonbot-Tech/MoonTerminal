//! Terminal-specific chrome composition over MoonPalette primitives.
//!
//! This is an adapter layer, not a reusable MoonPalette control: it knows about
//! Backend actions and MoonTerminal header content, while generic visuals still
//! come from MoonPalette tokens/components.
//!
//! The header is built in two steps. [`HeaderModel::read`] takes everything the row shows out of
//! the backend in one borrow; [`header_widths`] measures every element group of that model with the
//! arithmetic the row renders with; [`render_header`] draws the model at the collapse level those
//! widths choose. The renderer takes its actions as an `Option`, so a layout test can draw the same
//! geometry from a synthetic model with no backend at all.

use std::time::Instant;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonAccent, MoonButton, MoonButtonIconSlot, MoonButtonVariant, MoonDisclosure, MoonMenuItem,
    MoonPalette, MoonPopover, MoonPopoverPlacement, MoonPopupMenu, MoonSegmentItem,
    MoonSegmentSelection, MoonSegmentedControl, MoonSize, MoonTag, MoonWindowFrame, h_flex,
    rgba_from,
};
use rust_i18n::t;

use moon_core::config::WorkspaceMode;
use moon_core::feed::ConnStatus;
use moon_core::session::balances::{BalanceAggregate, BalanceFigures, aggregate_account_figures};
use moon_core::session::{BalanceState, CoreId};
use moon_core::util::fmt;

use crate::controls::launch::{LaunchTarget, launcher_label_width, open_window_button};
use crate::design::{HeaderLevel, HeaderWidths};
use crate::shell::Shell;
use crate::{Backend, design};

/// The header gear's trigger, built once for both faces it can open.
///
/// The two branches that use it are structurally different — one is a `MoonPopover`'s trigger, the
/// other a plain button — but the button itself must be the SAME: an icon that changed with the
/// preference would read as a different control rather than the same one behaving differently. The
/// tooltip names the thing it opens (core settings), not the face it opens it in — that stays
/// right whichever branch is live.
fn gear_trigger() -> MoonButton {
    MoonButton::new("core-gear")
        .leading_icon(MoonButtonIconSlot::new("icons/settings-2.svg"))
        // A flat icon button beside the core card, as the mockup's.
        .variant(MoonButtonVariant::Ghost)
        .tooltip(t!("core_settings.title").to_string())
}

/// Connection state the core card's dot reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CoreDot {
    /// The named core answers.
    Ready,
    /// The named core is connecting, down, or unknown.
    Down,
    /// Auto Overview: the row names the whole group, not one core.
    Overview,
}

/// What the core slot of the header shows.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CoreCard {
    /// The group has no cores: a button that opens Settings to add one.
    NoCores,
    /// One named core, or Auto's Overview.
    Named {
        /// Full core name, or the Overview label.
        name: String,
        /// Connection state.
        dot: CoreDot,
        /// Auto workspace: a passive scope indicator, never a selector.
        auto: bool,
        /// Emulator (`true`) or real (`false`) trading; `None` without a settings snapshot.
        emu: Option<bool>,
    },
}

/// Which face the core gear opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GearFace {
    /// The anchored core-settings popover.
    Popover,
    /// The full Moonbot settings window (`crate::core_expert`).
    Expert,
}

/// What the balance slot of the header shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum BalanceView {
    /// The group has no core yet.
    NoCore,
    /// A core is named but its balance is unknown (no snapshot, no valuation).
    Unknown,
    /// A usable figure: `total` is the headline, `free` the sub-figure, `stale` a pre-outage value,
    /// `missing` the scoped cores a group total could not count (always 0 for one core).
    Value {
        total: f64,
        free: f64,
        stale: bool,
        missing: u32,
    },
}

impl BalanceView {
    /// Map the store's balance state and figures onto what the header shows.
    ///
    /// Args:
    ///     state: The core's balance state, or `None` with no core named.
    ///     free: Free USDT.
    ///     total: Total USDT.
    ///
    /// Returns:
    ///     `Value` for a usable figure (stale when not current), `Unknown` otherwise.
    pub(crate) fn of(state: Option<BalanceState>, free: f64, total: f64) -> Self {
        match state {
            None => Self::NoCore,
            Some(state) if state.has_value() => Self::Value {
                total,
                free,
                stale: !state.is_current(),
                missing: 0,
            },
            Some(_) => Self::Unknown,
        }
    }

    /// Map a group's folded account aggregate onto the overview's balance slot.
    ///
    /// The sum is the Assets footer's ([`aggregate_account_figures`]): awaiting and unpriced
    /// cores are left out, never counted as zero, and a shared account is counted once. A total
    /// that left cores out says how many; one that counted a pre-outage figure is stale.
    ///
    /// Args:
    ///     sum: The folded aggregate over the group's displayed cores.
    ///     cores: How many cores the group displays.
    ///
    /// Returns:
    ///     `NoCore` for an empty group, `Unknown` when nothing could be counted, else `Value`.
    pub(crate) fn of_group(sum: &BalanceAggregate, cores: usize) -> Self {
        if cores == 0 {
            return Self::NoCore;
        }
        match (sum.total, sum.free) {
            (Some(total), Some(free)) if sum.counted > 0 => Self::Value {
                total,
                free,
                stale: sum.stale > 0,
                missing: sum.excluded,
            },
            _ => Self::Unknown,
        }
    }

    /// Whether the figure carries a trust tag (stale or partial) in place of its free sub-figure.
    pub(crate) fn tagged(self) -> bool {
        matches!(self, Self::Value { stale, missing, .. } if stale || missing > 0)
    }
}

/// The ticker's last price and its two deltas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TickerQuote {
    pub last: f64,
    pub delta_1h_pct: f64,
    pub delta_24h_pct: f64,
}

/// What the updater slot shows, when an update is on offer or under way.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct UpdateView {
    pub label: String,
    pub tooltip: String,
    /// `Available`: the one call to action the header ever grows.
    pub available: bool,
    pub busy: bool,
}

/// Everything the header shows, read out of the backend in one borrow.
#[derive(Clone, Debug)]
pub(crate) struct HeaderModel {
    pub group: String,
    pub mode: WorkspaceMode,
    pub card: CoreCard,
    /// The core gear, or `None` when the row names no single core.
    pub gear: Option<GearFace>,
    pub balance: BalanceView,
    /// Ticker coin (`BTC` when no source is resolved yet).
    pub ticker_base: String,
    pub ticker: Option<TickerQuote>,
    pub sleeping: bool,
    pub clock_zone: chrono_tz::Tz,
    pub update: Option<UpdateView>,
    /// Whether any core was ever saved; the first-run Settings hint stops once one was.
    pub core_ever_configured: bool,
}

impl HeaderModel {
    /// Read the header's model for `group`.
    ///
    /// `overview` is resolved ONCE here and gates every per-core element — the gear and the
    /// trade-mode tag — because both describe ONE account and mean nothing for the whole group.
    /// `active_trade_core` falls through to the group's first core in the Auto workspace Overview,
    /// so the balance there is never that core's: it is the group's folded total
    /// ([`BalanceView::of_group`]), beside a card that reads "the whole group". `expert_settings`
    /// is resolved in the same borrow because it decides the same element as `overview`.
    ///
    /// Args:
    ///     group: Group whose header is being rendered.
    ///     backend: Backend providing core, balance, and ticker state.
    ///     updater: Process-wide stable update state.
    ///     ticker_sel: Core and market selected for the ticker readout, if resolved.
    ///     cx: Application context used to read state.
    ///
    /// Returns:
    ///     The header model.
    pub(crate) fn read(
        group: &str,
        backend: &Entity<Backend>,
        updater: &Entity<crate::update::UpdateController>,
        ticker_sel: Option<&(CoreId, String)>,
        cx: &App,
    ) -> Self {
        let b = backend.read(cx);
        let overview = b.is_auto_overview_scope(group);
        let mode = b.workspace_mode(group);
        let card = core_card(group, b);
        let scoped_core = if overview {
            None
        } else {
            b.active_trade_core(group)
        };
        let core = scoped_core.and_then(|c| b.session.store().core(c));
        let emu = core
            .and_then(|cd| cd.client_settings.as_ref())
            .map(|cs| cs.emu_mode);
        let card = match card {
            CoreCard::Named {
                name, dot, auto, ..
            } => CoreCard::Named {
                name,
                dot,
                auto,
                emu,
            },
            other => other,
        };
        let has_cores = !matches!(card, CoreCard::NoCores);
        let balance = if overview {
            // The whole group's money, through the Assets footer's trust model.
            let store = b.session.store();
            let rows: Vec<(CoreId, String, BalanceFigures)> = displayed_cores(group, b)
                .into_iter()
                .map(|(id, name)| {
                    let figures = match store.core(id) {
                        Some(cd) => BalanceFigures {
                            state: cd.balance_state(),
                            free: cd.assets.global.free_usdt,
                            total: cd.assets.global.total_usdt,
                        },
                        None => BalanceFigures {
                            state: BalanceState::Awaiting,
                            free: 0.0,
                            total: 0.0,
                        },
                    };
                    (id, name, figures)
                })
                .collect();
            let agg = aggregate_account_figures(&b.session, &b.config.servers, &rows);
            BalanceView::of_group(&agg.sum, rows.len())
        } else {
            match core {
                Some(cd) => BalanceView::of(
                    Some(cd.balance_state()),
                    cd.assets.global.free_usdt,
                    cd.assets.global.total_usdt,
                ),
                None if has_cores => BalanceView::Unknown,
                None => BalanceView::NoCore,
            }
        };
        let gear = (!overview && has_cores).then(|| {
            if b.core_settings_expert() {
                GearFace::Expert
            } else {
                GearFace::Popover
            }
        });
        let ticker = ticker_sel.and_then(|(core, market)| {
            let t = b.session.market_source().market_ticker(*core, market)?;
            Some(TickerQuote {
                last: t.last,
                delta_1h_pct: t.delta_1h_pct,
                delta_24h_pct: t.delta_24h_pct,
            })
        });
        let ticker_base = ticker_sel
            .map(|(core, market)| b.session.market_source().market_label(*core, market).coin)
            .unwrap_or_else(|| "BTC".to_string());
        let sleeping = b.quiet_sleeping();
        let core_ever_configured = b.config.core_ever_configured();
        Self {
            group: group.to_string(),
            mode,
            card,
            gear,
            balance,
            ticker_base,
            ticker,
            sleeping,
            clock_zone: crate::chrome::clock::selected_zone(backend, cx),
            update: update_view(&updater.read(cx).state()),
            core_ever_configured,
        }
    }

    /// Whether the Auto workspace is active.
    fn auto(&self) -> bool {
        self.mode == WorkspaceMode::AutoTrading
    }
}

/// Describe the updater button for `state`, or `None` while there is nothing to offer.
fn update_view(state: &crate::update::UpdateState) -> Option<UpdateView> {
    if !state.visible() {
        return None;
    }
    let version = state
        .version()
        .map(|version| version.to_string())
        .unwrap_or_default();
    let label = if state.busy() {
        t!("update.installing").to_string()
    } else if matches!(state, crate::update::UpdateState::Failed { .. }) {
        t!("update.retry").to_string()
    } else if matches!(state, crate::update::UpdateState::Opened(_)) {
        t!("update.opened").to_string()
    } else {
        t!("update.available", version = version).to_string()
    };
    let tooltip = match state {
        crate::update::UpdateState::Failed { message, .. } => {
            t!("update.failed", error = message).to_string()
        }
        crate::update::UpdateState::Opened(_) => t!("update.opened_tooltip").to_string(),
        _ if cfg!(target_os = "macos") => t!("update.tooltip_image").to_string(),
        _ => t!("update.tooltip").to_string(),
    };
    Some(UpdateView {
        label,
        tooltip,
        available: matches!(state, crate::update::UpdateState::Available(_)),
        busy: state.busy(),
    })
}

/// Everything the header's controls act on. `None` at [`render_header`] draws the same geometry
/// with inert controls.
pub(crate) struct HeaderActions {
    pub backend: Entity<Backend>,
    pub updater: Entity<crate::update::UpdateController>,
    pub shell: Entity<Shell>,
    pub ticker_sel: Option<(CoreId, String)>,
    pub core_selector_open: bool,
    pub core_settings_open: bool,
    pub core_settings_content: Option<AnyElement>,
    pub quiet_settings_open: bool,
    pub quiet_settings_content: Option<AnyElement>,
    /// Whether the zone picker is held open under the header's "More" (the clock is hidden).
    pub clock_picker_open: bool,
    /// When the first-run Settings hint was armed, if it still is.
    pub settings_hint_at: Option<Instant>,
}

// ---- measurement ----------------------------------------------------------------------------

/// Horizontal inset of the core card before its dot.
const CARD_PAD_L: f32 = 9.0;
/// Horizontal inset of the core card after its caret.
const CARD_PAD_R: f32 = 6.0;
/// Gap between the core card's parts.
const CARD_GAP: f32 = 8.0;
/// Box the card's status dot is centred in.
const CARD_DOT_BOX: f32 = 8.0;
/// Diameter of the card's status dot.
const CARD_DOT: f32 = 6.0;
/// Two raw one-pixel borders around a bordered chrome group.
const BORDER_W: f32 = 2.0;
/// Core-name ceiling at header level 5, before the font scale.
const CORE_NAME_CAP_L5: f32 = 110.0;
/// Core-name ceiling at header level 6, before the font scale.
const CORE_NAME_CAP_L6: f32 = 70.0;
/// Narrowest the core name is ever fitted to, before the font scale: an ellipsis and a glyph.
const CORE_NAME_FLOOR: f32 = 24.0;
/// Gap between the ticker's coin, price and delta groups.
const TICKER_GAP: f32 = 10.0;
/// Gap between a ticker delta's window label and its value.
const TICKER_DELTA_GAP: f32 = 3.0;
/// Gap between the "free" caption and its figure.
const FREE_GAP: f32 = 4.0;
/// Horizontal inset of the stale-balance tag.
const STALE_PAD_X: f32 = 5.0;
/// Widest an AUTO/MANUAL cell is fitted to, before the font scale.
const MODE_SEGMENT_MAX_W: f32 = 160.0;
/// Weight of the balance headline figure.
const BALANCE_WEIGHT: f32 = 600.0;
/// Weight of the ticker price.
const PRICE_WEIGHT: f32 = 600.0;
/// Weight of the core name.
const NAME_WEIGHT: f32 = 500.0;
/// Weight of the mode tag's label (`MoonTag` draws MEDIUM).
const TAG_WEIGHT: f32 = 500.0;
/// `MoonTag`'s horizontal inset, before the UI scale.
const TAG_PAD_X: f32 = 10.0;
/// `MoonTag`'s text size on the legacy font channel.
const TAG_FONT: f32 = 11.0;

/// The ticker's price string: trailing dollar, magnitude-based precision.
///
/// Values at least 1000 are rounded to an integer and grouped with spaces, values at least 1 use
/// two decimal places, and lower values use four decimal places.
fn fmt_ticker_price(v: f64) -> String {
    if v >= 1000.0 {
        let mut s = fmt::group_thousands(&format!("{v:.0}"));
        s.push_str(" $");
        s
    } else if v >= 1.0 {
        format!("{v:.2} $")
    } else {
        format!("{v:.4} $")
    }
}

/// The text of one ticker delta, or a dash with no movement to report.
fn delta_text(v: f64) -> (String, Option<fmt::DeltaSign>) {
    match fmt::signed_pct(v, 1) {
        Some((text, sign)) => (text, Some(sign)),
        None => ("—".to_string(), None),
    }
}

/// Label of the emulator/real tag.
fn mode_tag_label(emu: bool) -> String {
    if emu {
        t!("header.trade_emu").to_string()
    } else {
        t!("header.trade_real").to_string()
    }
}

/// Rendered width of a `MoonTag` carrying `label`.
fn mode_tag_width(cx: &App, label: &str) -> f32 {
    design::ui_text_width(cx, label, TAG_FONT, TAG_WEIGHT, true)
        + design::ui_value(cx, TAG_PAD_X) * 2.0
        + BORDER_W
}

/// One AUTO/MANUAL cell, fitted to its label. The ONE builder both the header budget and the
/// rendered control read, so the measured and the drawn cell cannot disagree.
fn mode_segment_item(cx: &App, label: &str) -> MoonSegmentItem {
    MoonSegmentItem::new("", label.to_string()).fit_width(cx, 0.0, MODE_SEGMENT_MAX_W)
}

/// Rendered width of one AUTO/MANUAL cell, from the widget's own fitted geometry.
fn mode_segment_width(cx: &App, label: &str) -> f32 {
    mode_segment_item(cx, label).resolved_width()
}

/// The two mode labels, active first.
fn mode_labels(auto: bool) -> (String, String) {
    let auto_label = t!("workspace.mode.auto").to_string();
    let manual_label = t!("workspace.mode.classic").to_string();
    if auto {
        (auto_label, manual_label)
    } else {
        (manual_label, auto_label)
    }
}

/// Measure the core name the card would draw at full width.
fn core_name_width(cx: &App, name: &str) -> f32 {
    design::mono_body_text_width(cx, name, NAME_WEIGHT)
}

/// Rendered width of the core card without its name.
fn core_card_chrome(cx: &App, auto: bool, emu: Option<bool>) -> f32 {
    let gap = design::ui_value(cx, CARD_GAP);
    BORDER_W
        + design::ui_value(cx, CARD_PAD_L)
        + design::ui_value(cx, CARD_DOT_BOX)
        + gap
        + emu.map_or(0.0, |emu| gap + mode_tag_width(cx, &mode_tag_label(emu)))
        + if auto {
            0.0
        } else {
            gap + design::ui_value(cx, design::DISCLOSURE_BOX)
        }
        + design::ui_value(cx, CARD_PAD_R)
}

/// The labels of the three header launchers, in row order.
fn launcher_labels() -> [String; 3] {
    [
        t!("toolbar.strategies").to_string(),
        t!("toolbar.analytics").to_string(),
        t!("shell.settings_btn").to_string(),
    ]
}

/// Rendered width of the updater button.
fn update_width(cx: &App, view: &UpdateView) -> f32 {
    crate::controls::launch::labeled_button_width(
        cx,
        &view.label,
        crate::controls::launch::LAUNCHER_TEXT_WEIGHT,
        view.busy,
    )
}

/// A live figure as the budget measures it: every ASCII digit replaced by `8`.
///
/// Figures render in the mono face, where every digit has one advance, so the template measures
/// exactly what is drawn while staying constant as the figure ticks: the header's level then moves
/// only when the window or a figure's digit count changes, never on every price update.
fn digit_template(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_ascii_digit() { '8' } else { c })
        .collect()
}

/// Width a text element of measured advance `w` is laid out at: GPUI rounds a text run's box up
/// to a whole pixel, so a row of several runs outgrows the plain sum of their advances.
pub(crate) fn text_box(w: f32) -> f32 {
    w.ceil()
}

/// Widths of the balance's parts: headline (always), caption, unit, free sub-figure, and the
/// part that stands in its place when stale or unknown.
struct BalanceParts {
    fixed: f32,
    caption: f32,
    unit: f32,
    free: f32,
}

/// Measure the balance slot.
fn balance_parts(cx: &App, view: BalanceView) -> BalanceParts {
    let gap = design::ui_value(cx, design::CHROME_GAP);
    let caption = || {
        text_box(design::ui_caption_text_width(
            cx,
            &t!("header.balance_caption"),
            400.0,
        )) + gap
    };
    match view {
        BalanceView::NoCore => BalanceParts {
            fixed: text_box(design::ui_caption_text_width(
                cx,
                &t!("header.balance_none"),
                400.0,
            )),
            caption: 0.0,
            unit: 0.0,
            free: 0.0,
        },
        BalanceView::Unknown => BalanceParts {
            fixed: text_box(design::mono_body_text_width(cx, "—", BALANCE_WEIGHT)),
            caption: caption(),
            unit: 0.0,
            free: 0.0,
        },
        BalanceView::Value {
            total,
            free,
            stale,
            missing,
        } => {
            let tagged = view.tagged();
            let headline = text_box(design::mono_body_text_width(
                cx,
                &digit_template(&fmt::usd_grouped(total)),
                BALANCE_WEIGHT,
            ));
            let stale_w = if tagged {
                gap + stale_tag_width(cx, stale, missing)
            } else {
                0.0
            };
            BalanceParts {
                fixed: headline + stale_w,
                caption: caption(),
                unit: gap + text_box(design::mono_caption_text_width(cx, "USDT", 400.0)),
                free: if tagged {
                    0.0
                } else {
                    gap + free_width(cx, free)
                },
            }
        }
    }
}

/// Rendered width of the "free 19 932.94" sub-figure.
fn free_width(cx: &App, free: f64) -> f32 {
    text_box(design::ui_caption_text_width(
        cx,
        &t!("header.balance_free"),
        400.0,
    )) + design::ui_value(cx, FREE_GAP)
        + text_box(design::mono_caption_text_width(
            cx,
            &digit_template(&fmt::usd_grouped(free)),
            400.0,
        ))
}

/// Text of the balance trust tag: every reason the figure is not current and complete, so a
/// partial total that also counted a pre-outage figure says both.
fn balance_tag_text(stale: bool, missing: u32) -> String {
    let partial = (missing > 0).then(|| t!("header.balance_partial", n = missing).to_string());
    let stale = stale.then(|| t!("assets.balance_stale").to_string());
    partial
        .into_iter()
        .chain(stale)
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Rendered width of the balance trust tag.
fn stale_tag_width(cx: &App, stale: bool, missing: u32) -> f32 {
    text_box(design::ui_caption_text_width(
        cx,
        &balance_tag_text(stale, missing),
        500.0,
    )) + design::ui_value(cx, STALE_PAD_X) * 2.0
}

/// Widths of the ticker: the always-drawn part and the 1h delta group with its gap.
fn ticker_parts(cx: &App, model: &HeaderModel) -> (f32, f32) {
    let gap = design::ui_value(cx, TICKER_GAP);
    let base = text_box(design::ui_caption_text_width(cx, &model.ticker_base, 400.0));
    let delta = |label: &str, v: f64| {
        text_box(design::ui_caption_text_width(cx, label, 400.0))
            + design::ui_value(cx, TICKER_DELTA_GAP)
            + text_box(design::mono_body_text_width(
                cx,
                &digit_template(&delta_text(v).0),
                400.0,
            ))
    };
    match model.ticker {
        Some(q) => (
            base + gap
                + text_box(design::mono_body_text_width(
                    cx,
                    &digit_template(&fmt_ticker_price(q.last)),
                    PRICE_WEIGHT,
                ))
                + gap
                + delta(&t!("header.delta_24h"), q.delta_24h_pct),
            gap + delta(&t!("header.delta_1h"), q.delta_1h_pct),
        ),
        None => (
            base + gap + text_box(design::mono_body_text_width(cx, "—", 400.0)),
            0.0,
        ),
    }
}

/// Rendered width of the custom window controls, or zero where the platform draws its own.
fn window_controls_width(cx: &App) -> f32 {
    if design::show_custom_window_controls() {
        design::ui_value(
            cx,
            MoonWindowFrame::main("header-controls-metrics", 0.0)
                .show_controls(true)
                .controls_width(),
        )
    } else {
        0.0
    }
}

/// Width of the three header launchers drawn as icons, the gaps between them included — the
/// arithmetic [`header_widths`] counts them by.
fn launch_icons_width(cx: &App) -> f32 {
    let n = launcher_labels().len() as f32;
    design::glyph_btn_w(cx) * n + design::ui_value(cx, design::CHROME_GAP) * (n - 1.0)
}

/// Measure every header element group of `model` with the arithmetic [`render_header`] draws with.
///
/// The row is a flat run of root children separated by [`design::CHROME_GAP`]; each width below
/// carries the gap IN FRONT of its child, so a group that goes takes its gap with it. Rules are a
/// raw pixel, deliberately not font-scaled (`design::vline`). Nothing is read back from layout.
///
/// Args:
///     model: The header model.
///     cx: Application context used to measure the active typography and scales.
///
/// Returns:
///     The widths the collapse level is chosen from.
pub(crate) fn header_widths(model: &HeaderModel, cx: &App) -> HeaderWidths {
    let gap = design::ui_value(cx, design::CHROME_GAP);
    let rule = gap + 1.0;
    let glyph = design::glyph_btn_w(cx);
    let (active_mode, inactive_mode) = mode_labels(model.auto());
    let (card_chrome, name_full) = match &model.card {
        CoreCard::NoCores => (launcher_label_width(cx, &t!("header.add_core")), 0.0),
        CoreCard::Named {
            name, auto, emu, ..
        } => (core_card_chrome(cx, *auto, *emu), core_name_width(cx, name)),
    };
    // The name at a level cap is measured as the cut the card draws there, not the cap: an
    // ellipsis lands on a glyph boundary short of it.
    let name_fit = |cap: f32| match &model.card {
        CoreCard::Named { name, .. } => {
            design::fit_text(name, design::font_w(cx, cap), |text| {
                core_name_width(cx, text)
            })
            .1
        }
        CoreCard::NoCores => 0.0,
    };
    let balance = balance_parts(cx, model.balance);
    let (ticker, delta_1h) = ticker_parts(cx, model);
    let (clock_time, clock_city) =
        crate::chrome::clock::header_clock_part_widths(model.clock_zone, cx);
    let labels = launcher_labels();
    let launch_icons = glyph * labels.len() as f32 + gap * (labels.len() - 1) as f32;
    let launch_labels: f32 = labels
        .iter()
        .map(|label| launcher_label_width(cx, label) - glyph)
        .sum();
    let controls = window_controls_width(cx);
    let update = model
        .update
        .as_ref()
        .map_or(0.0, |view| gap + update_width(cx, view) + rule);

    let fixed = design::ui_value(cx, design::titlebar_leading_inset())
        // Mode control: the joined strip's hairline frame, the active cell, and the gap in front.
        + gap
        + design::joined_strip_width(cx, mode_segment_width(cx, &active_mode))
        // Core section and the card's chrome around its name.
        + gap
        + card_chrome
        // Balance group and the spacer's gap.
        + gap
        + balance.fixed
        + gap
        + update;
    let right_fixed = design::ui_value(cx, design::HEADER_PAD_X)
        + gap
        + crate::chrome::quiet::header_quiet_width(cx, model.sleeping, false)
        + gap
        + clock_time
        + gap
        + launch_icons
        + if controls > 0.0 { gap + controls } else { 0.0 };
    HeaderWidths {
        fixed,
        right_fixed,
        delta_1h,
        free_sub: balance.free,
        sleep_label: crate::chrome::quiet::header_quiet_label_width(cx, model.sleeping),
        ticker: gap + ticker,
        logo: design::ui_value(cx, design::HEADER_LOGO_W) + rule,
        launcher_labels: launch_labels,
        balance_caption: balance.caption,
        clock_city,
        separators: rule * 2.0,
        right_separators: rule,
        usdt_unit: balance.unit,
        inactive_mode_seg: mode_segment_width(cx, &inactive_mode),
        core_name_full: name_full,
        core_name_cap_l5: name_fit(CORE_NAME_CAP_L5),
        core_name_cap_l6: name_fit(CORE_NAME_CAP_L6),
        core_gear: if model.gear.is_some() {
            gap + glyph
        } else {
            0.0
        },
    }
}

/// How the clock is drawn once the narrowest header still does not fit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClockFit {
    Seconds,
    Minutes,
    Hidden,
}

/// What a header that outgrows its window can still give up, in rendered pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SqueezeRoom {
    /// The core name's width at the level ([`HeaderLevel::core_name_cap`]).
    pub name_cap: f32,
    /// Narrowest name worth drawing; below it the name is hidden whole.
    pub name_floor: f32,
    /// The card's gap in front of the name, which goes with a hidden name.
    pub name_gap: f32,
    /// What dropping the clock's seconds saves.
    pub clock_seconds: f32,
    /// What dropping the minute clock with its gap saves.
    pub clock_minutes: f32,
    /// The updater's full width, zero when none is offered.
    pub update: f32,
    /// The updater's narrowest width (its icon cell).
    pub update_min: f32,
    /// What folding the three launchers into one "More" glyph saves.
    pub launchers: f32,
    /// What folding the updater (its icon cell, rule and gap) into that "More" saves; zero when
    /// none is offered.
    pub update_fold: f32,
}

/// What the header draws at its level in a window `over` pixels too narrow for it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HeaderSqueeze {
    /// The core name's width budget; zero hides the name (dot, tag and caret stay).
    pub name: f32,
    pub clock: ClockFit,
    /// The updater's width budget.
    pub update: f32,
    /// Whether the launchers fold into one "More" glyph.
    pub launchers_folded: bool,
    /// Whether the updater folds into that "More" as its first row.
    pub update_folded: bool,
    /// Whether the right cluster still has to pin itself over the row's right edge.
    pub pinned: bool,
}

/// Give up `over` pixels in the fixed order: the core name down to its floor, then the name whole,
/// then the clock's seconds, then the updater's label, then the launchers fold into "More", then
/// the updater does, and only then the clock — whose zone picker that "More" then lists. Only what
/// is still short after all of that pins the right cluster. A row that fits gives up nothing.
///
/// The collapse level is always the one the window fits, so `over` is positive only at the
/// narrowest level; the core name is touched before it only through its level cap.
pub(crate) fn header_squeeze(over: f32, room: &SqueezeRoom) -> HeaderSqueeze {
    let mut over = over;
    let floor = room.name_floor.min(room.name_cap);
    let mut name = room.name_cap;
    if over > 0.0 {
        let cut = over.min(name - floor);
        name -= cut;
        over -= cut;
    }
    if over > 0.0 && name > 0.0 {
        over -= name + room.name_gap;
        name = 0.0;
    }
    let mut clock = ClockFit::Seconds;
    if over > 0.0 {
        over -= room.clock_seconds;
        clock = ClockFit::Minutes;
    }
    let mut update = room.update;
    if over > 0.0 {
        let cut = over.min((room.update - room.update_min).max(0.0));
        update -= cut;
        over -= cut;
    }
    let launchers_folded = over > 0.0;
    if launchers_folded {
        over -= room.launchers;
    }
    let update_folded = over > 0.0 && room.update_fold > 0.0;
    if update_folded {
        over -= room.update_fold;
    }
    // The clock goes last: its zone picker is reachable from the "More" the launchers have
    // already folded into, so hiding it costs no control.
    if over > 0.0 {
        over -= room.clock_minutes;
        clock = ClockFit::Hidden;
    }
    HeaderSqueeze {
        name,
        clock,
        update,
        launchers_folded,
        update_folded,
        pinned: over > 0.0,
    }
}

/// The squeeze of `model`'s header at `level` in a row `available` wide.
pub(crate) fn header_fit(
    model: &HeaderModel,
    widths: &HeaderWidths,
    level: HeaderLevel,
    available: f32,
    cx: &App,
) -> HeaderSqueeze {
    let gap = design::ui_value(cx, design::CHROME_GAP);
    let (seconds, _) = crate::chrome::clock::header_clock_part_widths(model.clock_zone, cx);
    let minutes = crate::chrome::clock::header_clock_minutes_width(model.clock_zone, cx);
    let room = SqueezeRoom {
        name_cap: level.core_name_cap(widths),
        name_floor: design::font_w(cx, CORE_NAME_FLOOR),
        name_gap: if matches!(model.card, CoreCard::Named { .. }) {
            design::ui_value(cx, CARD_GAP)
        } else {
            0.0
        },
        clock_seconds: (seconds - minutes).max(0.0),
        clock_minutes: gap + minutes,
        update: model
            .update
            .as_ref()
            .map_or(0.0, |view| update_width(cx, view)),
        update_min: design::glyph_btn_w(cx),
        launchers: launch_icons_width(cx) - design::glyph_btn_w(cx),
        // `header_widths` counts an offered updater as gap + button + rule (gap + 1).
        update_fold: if model.update.is_some() {
            gap * 2.0 + 1.0 + design::glyph_btn_w(cx)
        } else {
            0.0
        },
    };
    header_squeeze(widths.width_at(level) - available, &room)
}

/// Fit a header core label and return the card's content-driven width.
///
/// Args:
///     text: Full active-core label.
///     max_label_w: Maximum rendered width available to the label.
///     chrome_w: Rendered card width outside the label.
///     measure: Function returning rendered width for arbitrary label fragments.
///
/// Returns:
///     The fitted label and its measured width plus card chrome, rounded to a whole pixel.
fn fit_header_core_trigger(
    text: &str,
    max_label_w: f32,
    chrome_w: f32,
    measure: impl Fn(&str) -> f32,
) -> (String, f32) {
    // A zero budget hides the name whole: even an ellipsis would outgrow it.
    if max_label_w <= 0.0 {
        return (String::new(), chrome_w.ceil());
    }
    let (label, label_w) = design::fit_text(text, max_label_w, measure);
    (label, (label_w + chrome_w).ceil())
}

// ---- rendering --------------------------------------------------------------------------------

/// Compose the terminal header from its model at one collapse level.
///
/// Mockup order: logo · AUTO|MANUAL · core card and gear · balance · drag spacer · updater ·
/// ticker · sleep · clock · launchers · window controls. Each group named by `level` goes with the
/// gap in front of it, exactly as [`header_widths`] counts it.
///
/// Args:
///     model: What the row shows.
///     widths: The model's measured widths.
///     level: Collapse level chosen from `widths` for this window.
///     available: Window width, for the name fit past the narrowest level.
///     actions: Backend and Shell the controls act on; `None` draws them inert.
///     p: Active palette.
///     cx: Application context used to build elements.
///
/// Returns:
///     The complete terminal header element.
pub(crate) fn render_header(
    model: &HeaderModel,
    widths: &HeaderWidths,
    level: HeaderLevel,
    available: f32,
    actions: Option<HeaderActions>,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement + use<> {
    let (backend, updater, shell, ticker_sel) = match &actions {
        Some(a) => (
            Some(a.backend.clone()),
            Some(a.updater.clone()),
            Some(a.shell.clone()),
            a.ticker_sel.clone(),
        ),
        None => (None, None, None, None),
    };
    let (core_selector_open, core_settings_open, quiet_open, settings_hint_at, clock_picker_open) =
        actions
            .as_ref()
            .map_or((false, false, false, None, false), |a| {
                (
                    a.core_selector_open,
                    a.core_settings_open,
                    a.quiet_settings_open,
                    a.settings_hint_at,
                    a.clock_picker_open,
                )
            });
    let (mut core_settings_content, quiet_content) = match actions {
        Some(a) => (a.core_settings_content, a.quiet_settings_content),
        None => (None, None),
    };
    let squeeze = header_fit(model, widths, level, available, cx);
    // Unsqueezed, the name is cut against the level's own cap — the cut `header_widths` measured;
    // re-cutting against that cut's width could land one glyph shorter.
    let name_budget = if squeeze.name < level.core_name_cap(widths) {
        squeeze.name
    } else {
        match level.0 {
            0..=4 => f32::INFINITY,
            5 => design::font_w(cx, CORE_NAME_CAP_L5),
            _ => design::font_w(cx, CORE_NAME_CAP_L6),
        }
    };
    let pinned = squeeze.pinned;
    // Pinned, the updater shares the cluster with everything right of the ticker and leaves the
    // mode control and the core card their room; its label gives up the rest, as the core name
    // does in an unpinned row.
    let update_max = pinned.then(|| {
        let gap = design::ui_value(cx, design::CHROME_GAP);
        available
            - left_controls_width(model, cx)
            - widths.right_of_ticker(level)
            // The cluster's border and leading inset, and the updater's own rule.
            - (gap + 1.0) * 2.0
            - gap
    });
    let update_max = update_max.or_else(|| {
        model
            .update
            .as_ref()
            .filter(|view| squeeze.update < update_width(cx, view))
            .map(|_| squeeze.update)
    });
    // Folded, the updater is the first row of the header's "More" instead of a button.
    let update_row = model
        .update
        .as_ref()
        .filter(|_| squeeze.update_folded)
        .map(|view| update_menu_row(view, updater.clone()));
    let update = model
        .update
        .as_ref()
        .filter(|_| !squeeze.update_folded)
        .map(|view| update_button(view, updater.clone(), update_max, p, cx));
    let (left_update, pinned_update) = if pinned {
        (None, update)
    } else {
        (update, None)
    };
    let right_cluster = header_right_cluster(
        model,
        level,
        (squeeze.clock, squeeze.launchers_folded, update_row),
        pinned,
        backend.clone(),
        shell.clone(),
        (quiet_open, quiet_content),
        clock_picker_open,
        settings_hint_at,
        pinned_update,
        p,
        cx,
    );
    h_flex()
        .id("header")
        // Anchors a pinned right cluster.
        .relative()
        .w_full()
        // Keep the drawn chrome on the same tier-derived height as the window drag overlay at all
        // three densities; the old agreement existed only at Compact.
        .h(design::header_height_px(cx))
        .flex_none()
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        // One spacing rule across BOTH chrome strips — see `design::CHROME_GAP`.
        .gap(design::ui_px(cx, design::CHROME_GAP))
        .bg(rgb(p.shell_high))
        // The brand carries its OWN trailing rule. It goes at level 2; the drag region stays, so a
        // narrow window keeps a titlebar the operator can grab.
        //
        // Assembled here rather than through `MoonWindowFrame::brand_cluster`, which draws MoonUI's
        // own Moonbot lockup: the terminal ships the MoonTerminal one from `assets/brand`. Only the
        // drag handle comes from the frame.
        .child(
            MoonWindowFrame::main("terminal-header-brand-drag", 0.0)
                .drag_handle()
                .flex()
                .items_center()
                .gap(design::ui_px(cx, design::CHROME_GAP))
                .when(level.logo(), |cluster| {
                    cluster
                        .child(
                            div()
                                .id("header-logo")
                                .debug_selector(|| "header-logo".into())
                                .child(design::header_logo(cx)),
                        )
                        .child(design::chrome_divider(cx, p))
                })
                .flex_none()
                .h_full(),
        )
        // Workspace mode is fixed chrome rather than dock content, so it remains reachable while
        // Auto independently controls which dock operations are allowed.
        .child(workspace_mode_segments(
            &model.group,
            model.mode,
            level.inactive_mode_seg(),
            backend.clone(),
            cx,
        ))
        .when(level.separators(), |row| {
            row.child(design::chrome_divider(cx, p))
        })
        // Active trade core leads the trading context after the workspace preset. Balance and
        // ticker read through it. Interactive widgets are never a drag zone.
        .child(
            design::chrome_section(cx)
                .child(
                    // The card carries the core-settings popup's anchor at the level that hides
                    // the gear, so an open popup stays up on a narrowing window.
                    div()
                        .relative()
                        .flex_none()
                        .child(core_selector(
                            model,
                            name_budget,
                            core_selector_open,
                            model.gear.filter(|_| !level.core_gear()),
                            backend.clone().zip(shell.clone()),
                            p,
                            cx,
                        ))
                        .children(
                            (core_settings_anchor(level) == CoreSettingsAnchor::Card
                                && model.gear == Some(GearFace::Popover))
                            .then(|| shell.clone())
                            .flatten()
                            .map(|shell| {
                                div()
                                    .absolute()
                                    .left_0()
                                    .bottom_0()
                                    .size_0()
                                    .debug_selector(|| "header-core-settings-anchor".into())
                                    .child(core_settings_popover(
                                        "core-card-settings",
                                        core_settings_open,
                                        core_settings_content.take(),
                                        div().size_0(),
                                        shell,
                                    ))
                            }),
                        ),
                )
                .children(
                    level
                        .core_gear()
                        .then(|| {
                            core_gear(
                                &model.group,
                                model.gear,
                                core_settings_open,
                                core_settings_content.take(),
                                backend.clone().zip(shell.clone()),
                            )
                        })
                        .flatten()
                        .map(|gear| {
                            div()
                                .flex_none()
                                .debug_selector(|| "header-gear".into())
                                .child(gear)
                        }),
                ),
        )
        .when(level.separators(), |row| {
            row.child(design::chrome_divider(cx, p))
        })
        .child(balance_label(
            model.group.clone(),
            backend.clone(),
            model.balance,
            level,
            p,
            cx,
        ))
        .child(
            // A dual role, both required: this is BOTH the `flex_1` spacer that pins the cluster
            // after it to the right edge, AND the region that drags the window by the empty part
            // of the header. Swapping it for a plain `div().flex_1()` silently removes dragging.
            MoonWindowFrame::main("terminal-header-spacer-drag", 0.0)
                .drag_handle()
                .h_full()
                .flex_1()
                .min_w_0()
                .flex(),
        )
        // The updater is a separate pre-ticker cluster: its coupled rule appears only with the
        // button, so it is part of the left budget and never shifts the ticker popup. A pinned
        // right cluster carries it instead, so the cluster cannot cover it (no ticker is drawn
        // then).
        .children(left_update)
        // Rate ticker. Its popup is positioned by hand in `shell::ticker` from the window's right
        // edge inward by `HeaderWidths::right_of_ticker`, so EVERYTHING to its right must be in
        // that sum. One predicate for the ticker and its popup: `design::ticker_visible`.
        .when(design::ticker_visible(level), |row| {
            row.child(ticker_readout(
                model,
                ticker_sel,
                design::ticker_deltas_visible(level),
                backend.clone().zip(shell.clone()),
                p,
                cx,
            ))
        })
        .child(right_cluster)
}

/// Width of the left groups a pinned right cluster must leave uncovered: the leading inset, the
/// active mode segment and the core card at its narrowest name.
fn left_controls_width(model: &HeaderModel, cx: &App) -> f32 {
    let gap = design::ui_value(cx, design::CHROME_GAP);
    let card = match &model.card {
        CoreCard::NoCores => launcher_label_width(cx, &t!("header.add_core")),
        CoreCard::Named { auto, emu, .. } => {
            core_card_chrome(cx, *auto, *emu) + design::font_w(cx, CORE_NAME_FLOOR)
        }
    };
    design::ui_value(cx, design::titlebar_leading_inset())
        + gap
        + design::joined_strip_width(cx, mode_segment_width(cx, &mode_labels(model.auto()).0))
        + gap
        + card
}

/// The header's right cluster: quiet mode, the clock, the launchers and the window controls,
/// led by the updater while pinned.
#[allow(clippy::too_many_arguments)]
fn header_right_cluster(
    model: &HeaderModel,
    level: HeaderLevel,
    (clock, launchers_folded, update_row): (ClockFit, bool, Option<MoonMenuItem>),
    pinned: bool,
    backend: Option<Entity<Backend>>,
    shell: Option<Entity<Shell>>,
    (quiet_open, quiet_content): (bool, Option<AnyElement>),
    clock_picker_open: bool,
    settings_hint_at: Option<Instant>,
    update: Option<AnyElement>,
    p: MoonPalette,
    cx: &App,
) -> Div {
    // The hidden clock's zone picker: a "More" row and the anchor it opens under, both drawn with
    // the clock gone (the launchers have folded by then, so "More" exists).
    let clock_hidden = clock == ClockFit::Hidden;
    let open_picker: Option<crate::chrome::clock::ClockPickerOpen> = shell.clone().map(|shell| {
        std::rc::Rc::new(move |open: bool, _: &mut Window, cx: &mut App| {
            shell.update(cx, |s, cx| s.set_header_clock_picker_open(open, cx));
        }) as crate::chrome::clock::ClockPickerOpen
    });
    let lead = header_more_lead(update_row, clock_hidden, open_picker.clone());
    let picker_anchor = clock_hidden.then(|| {
        crate::chrome::clock::header_clock_picker_anchor(
            backend.as_ref(),
            model.clock_zone,
            clock_picker_open,
            open_picker,
        )
    });
    design::chrome_section(cx)
        .debug_selector(|| "header-right".into())
        .when(pinned, |section| {
            section
                .absolute()
                .top_0()
                .bottom_0()
                .right(design::ui_px(cx, design::HEADER_PAD_X))
                .pl(design::ui_px(cx, design::CHROME_GAP))
                .bg(rgb(p.shell_high))
                .border_l_1()
                .border_color(rgb(p.border))
        })
        .children(update)
        // Quiet mode ("sleep"): the labelled button plus its settings gear. Rendered at an explicit
        // width (`chrome::quiet::header_quiet_width`) that `right_of_ticker` reuses.
        .child(crate::chrome::quiet::header_quiet_cluster(
            model.sleeping,
            level.sleep_label(),
            backend.clone().zip(shell.clone()).map(|(backend, shell)| {
                crate::chrome::quiet::QuietActions {
                    backend,
                    shell,
                    open: quiet_open,
                    content: quiet_content,
                }
            }),
            p,
            cx,
        ))
        // The selected zone's clock; clicking opens the picker anchored to it. The narrowest
        // header drops its seconds, then — once the launchers and the updater are in "More" — the
        // clock, before the right cluster pins.
        .children(match clock {
            ClockFit::Seconds => Some(crate::chrome::clock::header_clock_city(
                backend.as_ref(),
                model.clock_zone,
                level.clock_city(),
                p,
                cx,
            )),
            ClockFit::Minutes => Some(crate::chrome::clock::header_clock_minutes(
                backend.as_ref(),
                model.clock_zone,
                p,
                cx,
            )),
            ClockFit::Hidden => None,
        })
        .when(level.separators(), |row| {
            row.child(design::chrome_divider(cx, p))
        })
        .child(header_launchers(
            &model.group,
            level.launcher_labels(),
            launchers_folded.then_some((lead, picker_anchor)),
            backend.clone(),
            settings_hint_at.filter(|_| !model.core_ever_configured),
            p,
            cx,
        ))
        .when(design::show_custom_window_controls(), |this| {
            this.child(
                MoonWindowFrame::main("terminal-header-controls", 0.0)
                    .show_controls(true)
                    .visual_controls(cx),
            )
        })
}

/// The AUTO|MANUAL two-segment control.
///
/// AUTO and MANUAL are two full modes rather than the two positions of one switch, so both are
/// named and the current one is filled. The inactive segment goes at header level 4; the active
/// one then still names the mode, and a click on it does nothing. A pick writes through the same
/// `Backend::set_workspace_mode` authority the persisted mode has always used.
///
/// Args:
///     group: Group whose preset is selected.
///     mode: Current workspace mode.
///     show_inactive: Whether the inactive segment fits at the current level.
///     backend: Persisted workspace authority; `None` renders the segments inert.
///     cx: Application context used to fit the segments.
///
/// Returns:
///     The filled segmented control.
fn workspace_mode_segments(
    group: &str,
    mode: WorkspaceMode,
    show_inactive: bool,
    backend: Option<Entity<Backend>>,
    cx: &App,
) -> AnyElement {
    let segment = |target: WorkspaceMode, label: String, tip: String| {
        let active = mode == target;
        let item = mode_segment_item(cx, &label).selected(active).tooltip(tip);
        (target, active, item)
    };
    let segments = [
        segment(
            WorkspaceMode::AutoTrading,
            t!("workspace.mode.auto").to_string(),
            t!("workspace.mode.auto_tip").to_string(),
        ),
        segment(
            WorkspaceMode::Classic,
            t!("workspace.mode.classic").to_string(),
            t!("workspace.mode.classic_tip").to_string(),
        ),
    ];
    // The modes the control draws, in order: a click maps its cell index back through this list.
    let (targets, items): (Vec<WorkspaceMode>, Vec<MoonSegmentItem>) = segments
        .into_iter()
        .filter(|(_, active, _)| *active || show_inactive)
        .map(|(target, _, item)| (target, item))
        .unzip();
    let group = group.to_string();
    div()
        .id("header-mode")
        .debug_selector(|| "header-mode".into())
        .flex_none()
        .child(
            MoonSegmentedControl::new("header-mode-segments")
                .selection(MoonSegmentSelection::Filled)
                .accent(MoonAccent::Blue)
                .item_gap(0.0)
                .items(items)
                .on_click(move |ix, _, _, app| {
                    // The active cell names the mode; a click on it does nothing.
                    let (Some(&target), Some(backend)) = (targets.get(ix), backend.as_ref()) else {
                        return;
                    };
                    if target == mode {
                        return;
                    }
                    backend.update(app, |backend, backend_cx| {
                        backend.set_workspace_mode(&group, target, backend_cx);
                    });
                })
                .render(),
        )
        .into_any_element()
}

/// The core gear for one face, or `None` when the row names no single core.
///
/// Expert mode replaces the popover with a plain button opening the full Moonbot settings window
/// (`crate::core_expert`). The two faces cannot both be live: the expert window owns its own draft
/// of the same page, and a popup staged beside it would give one core two pending pages.
/// `Shell::gear_popup_unavailable` closes an already-open popup whenever the header draws no
/// core-settings popover — Overview or expert mode; at the level that hides the gear the popup
/// moves to the core card ([`core_settings_anchor`]).
fn core_gear(
    group: &str,
    face: Option<GearFace>,
    open: bool,
    content: Option<AnyElement>,
    actions: Option<(Entity<Backend>, Entity<Shell>)>,
) -> Option<AnyElement> {
    let face = face?;
    let Some((backend, shell)) = actions else {
        return Some(gear_trigger().render().into_any_element());
    };
    Some(match face {
        GearFace::Expert => {
            let group = group.to_string();
            gear_trigger()
                .on_click(move |_, window, cx| {
                    let owner_display = crate::window::windowing::window_display_id(window, cx);
                    crate::core_expert::open(
                        backend.clone(),
                        Some(window.window_handle()),
                        owner_display,
                        group.clone(),
                        cx,
                    );
                })
                .render()
                .into_any_element()
        }
        GearFace::Popover => {
            core_settings_popover("core-gear", open, content, gear_trigger().render(), shell)
                .into_any_element()
        }
    })
}

/// Where an open core-settings popup anchors at `level`: its gear, or the core card at the level
/// that hides the gear. Never a reason to hold the level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CoreSettingsAnchor {
    Gear,
    Card,
}

/// The core-settings popup's anchor at `level`.
pub(crate) fn core_settings_anchor(level: HeaderLevel) -> CoreSettingsAnchor {
    if level.core_gear() {
        CoreSettingsAnchor::Gear
    } else {
        CoreSettingsAnchor::Card
    }
}

/// The core-settings `MoonPopover` on `trigger` — the gear, or the core card's zero-size anchor.
fn core_settings_popover(
    id: &'static str,
    open: bool,
    content: Option<AnyElement>,
    trigger: impl IntoElement,
    shell: Entity<Shell>,
) -> MoonPopover {
    header_gear_popover(
        id,
        MoonPopoverPlacement::BottomStart,
        crate::shell::core_settings_popup::CONTENT_W,
        open,
        content,
        trigger,
        move |open, window, cx| {
            shell.update(cx, |s, cx| s.set_core_settings_open(open, window, cx));
        },
    )
    // The alert-sound pickers on the General tab are `MoonDropdown`s, whose menus paint in
    // their OWN deferred layers outside this popover's box; the bounds-based
    // `on_mouse_down_out` runs in the CAPTURE phase, so the click that picks a sound would read
    // as "outside" and shut the popup before the pick landed. Until MoonUI suppresses that (the
    // Popover entry in docs-internal/FORK_BUGS.md) outside-click dismissal has to be off here —
    // the same trade the detects, core-status, tuner and chart-label popups already make.
    // Cancel, OK and the gear itself are the dismissal paths, and this popup has all three.
    .close_on_content_click(false)
    .overlay_closable(false)
}

/// Resolve the core slot from the group's cores and workspace mode.
///
/// Classic choices come from the group's cores; [`Backend::active_trade_core`] prefers a still-valid
/// remembered Classic selection, the current trading target, and finally the group's first core.
/// Auto reads only the rail-owned workspace selection and names Overview when it is empty.
fn core_card(group: &str, b: &Backend) -> CoreCard {
    let cores: Vec<(CoreId, String)> = displayed_cores(group, b);
    let auto = b.workspace_mode(group) == WorkspaceMode::AutoTrading;
    if cores.is_empty() && !auto {
        return CoreCard::NoCores;
    }
    let active = if auto {
        b.valid_auto_workspace_core(group)
    } else {
        b.active_trade_core(group)
    };
    let ready = active
        .and_then(|id| b.session.store().core(id))
        .is_some_and(|c| c.status == ConnStatus::Ready);
    let dot = if auto && active.is_none() {
        CoreDot::Overview
    } else if ready {
        CoreDot::Ready
    } else {
        CoreDot::Down
    };
    let name = active
        .and_then(|id| cores.iter().find(|(cid, _)| *cid == id))
        .map(|(_, n)| n.clone())
        .unwrap_or_else(|| {
            if auto {
                t!("workspace.overview").to_string()
            } else {
                "—".to_string()
            }
        });
    CoreCard::Named {
        name,
        dot,
        auto,
        emu: None,
    }
}

/// The group's cores the header may name.
///
/// Filtered here rather than at `Backend::group_cores`, whose other callers include the
/// membership-authority functions themselves: dropping non-members here also keeps the card from
/// naming a hidden core, since it looks the active id up in this same filtered list.
fn displayed_cores(group: &str, b: &Backend) -> Vec<(CoreId, String)> {
    b.group_cores(group)
        .into_iter()
        .filter(|(id, _)| b.core_displayed_in_group(group, *id))
        .collect()
}

/// Build the core card: the Classic selector or the passive Auto scope indicator.
///
/// The card shows a status dot, the mono core name (fitted to `name_budget`), the emulator/real tag
/// and a caret. Auto renders the same card disabled and caret-free, never a popover: the rail owns
/// the Auto core. With no cores the slot is an "Add core" button opening Settings.
///
/// Args:
///     model: Header model holding the card.
///     name_budget: Width the name is fitted to at the current level.
///     open: Whether the selector popover is open.
///     settings_item: The gear's face when the level hides the gear: the menu then carries the
///         core-settings entry the gear would have offered.
///     actions: Backend and Shell for the selector; `None` renders the card inert.
///     p: Active palette.
///     cx: Application context used to measure the name.
///
/// Returns:
///     The interactive Classic selector, passive Auto indicator, or the "Add core" button.
#[allow(clippy::too_many_arguments)]
fn core_selector(
    model: &HeaderModel,
    name_budget: f32,
    open: bool,
    settings_item: Option<GearFace>,
    actions: Option<(Entity<Backend>, Entity<Shell>)>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let (raw_name, dot, auto, emu) = match &model.card {
        CoreCard::NoCores => {
            let label = t!("header.add_core").to_string();
            let button = open_window_button(
                LaunchTarget {
                    id: "header-add-core",
                    label: label.clone(),
                    icon: "icons/plus.svg",
                    labeled_width: Some(launcher_label_width(cx, &label)),
                    workspace_owner: None,
                    open: crate::settings::open,
                },
                actions.map(|(backend, _)| backend),
                p,
                cx,
            );
            return div()
                .id("header-core-card")
                .debug_selector(|| "header-core-card".into())
                .flex_none()
                .font_family(design::ui_font())
                .tooltip(crate::panels::common::text_tooltip(
                    t!("header.add_core_tip").to_string(),
                ))
                .child(button)
                .into_any_element();
        }
        CoreCard::Named {
            name,
            dot,
            auto,
            emu,
        } => (name.as_str(), *dot, *auto, *emu),
    };
    let dot_color = match dot {
        CoreDot::Overview => p.accent,
        CoreDot::Ready => design::positive_color(p),
        CoreDot::Down => design::danger_color(p),
    };
    // A zero budget hides the name, and the card's gap in front of it goes with it.
    let name_hidden = name_budget <= 0.0;
    let chrome_w = core_card_chrome(cx, auto, emu)
        - if name_hidden {
            design::ui_value(cx, CARD_GAP)
        } else {
            0.0
        };
    let (active_name, card_w) = fit_header_core_trigger(raw_name, name_budget, chrome_w, |text| {
        core_name_width(cx, text)
    });
    let name_w = card_w - chrome_w;
    // Auto's card opens a menu only for the core-settings entry at the level that hides the gear.
    let clickable = !auto || settings_item.is_some();
    // Final pixels into `px`, never scaled again: the card shares one top and bottom edge with the
    // ordinary controls beside it at every UI scale.
    let trigger_h = design::action_control_h_value(cx);
    let card = h_flex()
        .id("header-core-card")
        .debug_selector(|| "header-core-card".into())
        .flex_none()
        .items_center()
        .w(px(card_w))
        .h(px(trigger_h))
        .pl(design::ui_px(cx, CARD_PAD_L))
        .pr(design::ui_px(cx, CARD_PAD_R))
        .gap(design::ui_px(cx, CARD_GAP))
        .border_1()
        .border_color(rgb(p.border))
        .rounded(design::r_button(cx))
        .bg(rgb(p.panel))
        .when(clickable, |card| card.cursor_pointer())
        .tooltip(crate::panels::common::text_tooltip(
            t!("header.core_card_tip").to_string(),
        ))
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .w(design::ui_px(cx, CARD_DOT_BOX))
                .child(design::status_dot_sized(dot_color, CARD_DOT, cx)),
        )
        .when(!name_hidden, |card| {
            card.child(
                div()
                    .id("header-core-name")
                    .debug_selector(|| "header-core-name".into())
                    .flex_none()
                    .w(px(name_w.max(0.0)))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .font_family(design::mono())
                    .text_size(design::t_body(cx))
                    .font_weight(FontWeight(NAME_WEIGHT))
                    .text_color(rgb(p.text))
                    .child(active_name),
            )
        })
        .children(emu.map(header_trade_mode_tag))
        .when(!auto, |card| {
            card.child(
                div()
                    .flex_none()
                    .w(design::ui_px(cx, design::DISCLOSURE_BOX))
                    .child(MoonDisclosure::glyph(open).color(p.text_muted)),
            )
        });
    let Some((backend, shell)) = actions else {
        return card.into_any_element();
    };
    // The rail owns the Auto core, so the Auto card is a passive scope indicator — except at the
    // level that hides the gear, where its menu carries the core-settings entry alone.
    if auto && settings_item.is_none() {
        return card.into_any_element();
    }

    // The header renders continuously, but exchange discovery scans every client snapshot. Build
    // the hidden menu only after controlled open state triggers a repaint.
    let items = if open {
        let b = backend.read(cx);
        let cores = if auto {
            Vec::new()
        } else {
            displayed_cores(&model.group, b)
        };
        let active = b.active_trade_core(&model.group);
        let venues = b.session.core_venues();
        let sections = crate::controls::core_menu_sections(&cores, venues);
        let mut items = Vec::with_capacity(cores.len() + sections.len() + 1);
        for (venue, members) in sections {
            let exchange_label = crate::controls::venue_section_label(venue);
            items.push(MoonMenuItem::label(exchange_label));
            for (id, name) in members {
                let backend = backend.clone();
                let group = model.group.clone();
                let item_shell = shell.clone();
                items.push(
                    MoonMenuItem::with_key(format!("core-{id}"), name)
                        .selected(active == Some(id))
                        .checked(active == Some(id))
                        .on_click(move |_, _, cx| {
                            backend.update(cx, |b, bcx| {
                                if b.workspace_mode(&group) == WorkspaceMode::AutoTrading {
                                    return;
                                }
                                b.set_active_trade_core(&group, id);
                                bcx.notify();
                            });
                            item_shell.update(cx, |shell, cx| {
                                shell.set_header_core_selector_open(false, cx);
                            });
                        }),
                );
            }
        }
        // At the level that hides the gear, core settings stay one click away from the card.
        if let Some(face) = settings_item {
            let backend = backend.clone();
            let group = model.group.clone();
            let item_shell = shell.clone();
            items.push(
                MoonMenuItem::with_key("core-settings", t!("core_settings.title").to_string())
                    .on_click(move |_, window, cx| {
                        item_shell.update(cx, |shell, cx| {
                            shell.set_header_core_selector_open(false, cx);
                        });
                        match face {
                            GearFace::Popover => {
                                item_shell.update(cx, |shell, cx| {
                                    shell.set_core_settings_open(true, window, cx);
                                });
                            }
                            GearFace::Expert => {
                                let owner_display =
                                    crate::window::windowing::window_display_id(window, cx);
                                crate::core_expert::open(
                                    backend.clone(),
                                    Some(window.window_handle()),
                                    owner_display,
                                    group.clone(),
                                    cx,
                                );
                            }
                        }
                    }),
            );
        }
        items
    } else {
        Vec::new()
    };

    MoonPopover::new("header-core-selector")
        .placement(MoonPopoverPlacement::BottomStart)
        .fit_content()
        .open(open)
        .on_open_change(move |open, _, cx| {
            shell.update(cx, |shell, cx| {
                shell.set_header_core_selector_open(open, cx);
            });
        })
        .trigger(card)
        .content(
            MoonPopupMenu::new("header-core-menu")
                .fit_width(180.0, 560.0)
                .size(MoonSize::Xs)
                .max_height_ui(520.0)
                .items(items)
                .render(),
        )
        .into_any_element()
}

/// Header badge for the named core's emulator vs real trading mode.
///
/// A readout, not a toggle: changing `emu_mode` stays in the gear popover, next to its warning.
/// Real reads in the positive tint, the emulator in the bordered warning tag. `emu_mode` is already
/// Overview-gated by [`HeaderModel::read`]; `None` (no settings snapshot) draws no tag at all.
fn header_trade_mode_tag(emu: bool) -> AnyElement {
    let tip = if emu {
        t!("header.trade_emu_tip").to_string()
    } else {
        t!("header.trade_real_tip").to_string()
    };
    let tag = if emu {
        MoonTag::warning()
    } else {
        MoonTag::positive()
    }
    .label(mode_tag_label(emu));
    div()
        .id("header-trade-mode")
        .debug_selector(|| "header-trade-mode".into())
        .flex_none()
        .tooltip(crate::panels::common::text_tooltip(tip))
        .child(tag)
        .into_any_element()
}

/// The updater button and the rule after it.
///
/// An offered update is the only call to action this header ever grows, on a window whose every
/// other control is `Panel`. `Green` rather than `Blue`, for contrast: every tinted MoonUI variant
/// paints its label in the accent over a ~10% wash, and in the light theme only green has a
/// light-specific foreground (`green_text`) that clears 4.5:1 on `shell_high`. Every other state
/// keeps `Panel`: Installing reads as busy through `loading`, Failed through its retry label.
fn update_button(
    view: &UpdateView,
    updater: Option<Entity<crate::update::UpdateController>>,
    max_w: Option<f32>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let full = update_width(cx, view);
    let width = max_w.map_or(full, |max| full.min(max).max(design::glyph_btn_w(cx)));
    let measure = |text: &str| {
        design::ui_text_width_zoomed(
            cx,
            text,
            design::BODY_TEXT,
            crate::controls::launch::LAUNCHER_TEXT_WEIGHT,
            false,
        )
    };
    // A squeezed label keeps its whole text in the tooltip.
    let (label, tooltip) = if width < full {
        let chrome = full - measure(&view.label);
        (
            design::fit_text(&view.label, (width - chrome).max(0.0), measure).0,
            format!(
                "{}
{}",
                view.label, view.tooltip
            ),
        )
    } else {
        (view.label.clone(), view.tooltip.clone())
    };
    let variant = if view.available {
        MoonButtonVariant::Green
    } else {
        MoonButtonVariant::Panel
    };
    let fg = if view.available { p.green_text } else { p.text };
    let mut button = MoonButton::new("terminal-update")
        .width(width)
        .padding_x(crate::controls::launch::LAUNCHER_PAD_X)
        .text_segment(label, fg, crate::controls::launch::LAUNCHER_TEXT_WEIGHT)
        .variant(variant)
        .loading(view.busy);
    if let Some(updater) = updater {
        button = button.on_click(move |_, _window, cx| {
            crate::update::UpdateController::start_install(&updater, cx);
        });
    }
    design::chrome_section(cx)
        .font_family(design::ui_font())
        .child(
            div()
                .id("terminal-update-tooltip")
                .debug_selector(|| "terminal-update".into())
                .tooltip(crate::panels::common::text_tooltip(tooltip))
                .child(button.render()),
        )
        .child(design::chrome_divider(cx, p))
        .into_any_element()
}

/// The updater folded into the header's "More": its label, the same install action, disabled
/// while an install is already running.
fn update_menu_row(
    view: &UpdateView,
    updater: Option<Entity<crate::update::UpdateController>>,
) -> MoonMenuItem {
    let row = match updater {
        Some(updater) => crate::controls::launch::overflow_row(
            "header-update-menu",
            view.label.clone(),
            move |_, cx| crate::update::UpdateController::start_install(&updater, cx),
        ),
        None => MoonMenuItem::with_key("header-update-menu", view.label.clone()),
    };
    row.disabled(view.busy)
}

/// The rows the header's "More" lists before the launchers: the folded updater's, then — with the
/// clock hidden — «Time zone», opening the clock's own zone picker under "More".
///
/// Args:
///     update_row: The folded updater's row, if any.
///     clock_hidden: Whether the clock is off the row.
///     open_picker: The picker's open through `Shell`; `None` renders the row disabled.
///
/// Returns:
///     The lead rows in display order.
pub(crate) fn header_more_lead(
    update_row: Option<MoonMenuItem>,
    clock_hidden: bool,
    open_picker: Option<crate::chrome::clock::ClockPickerOpen>,
) -> Vec<MoonMenuItem> {
    // The picker's own caption names the row, so both read alike in every locale.
    let label = t!("header.clock_pick").to_string();
    let zone_row = clock_hidden.then(|| match open_picker {
        Some(open) => crate::controls::launch::overflow_row(
            "header-clock-zone-menu",
            label,
            move |window, app| open(true, window, app),
        ),
        None => MoonMenuItem::with_key("header-clock-zone-menu", label).disabled(true),
    });
    update_row.into_iter().chain(zone_row).collect()
}

/// Render the header ticker as `BTC 82 690 $ 1h +0.1% 24h +2.0%`.
///
/// An ambient readout, so it reads muted: the coin in the UI face, the price in mono. A click opens
/// the market on Main; a double-click opens the source popup hosted by [`Shell`]. When
/// `show_1h` is false the 1h delta is dropped.
///
/// Args:
///     model: Header model holding the ticker figures.
///     sel: Resolved ticker core and market, or `None` for the fallback source picker.
///     show_1h: Whether the 1h delta fits at the current level.
///     actions: Backend and Shell; `None` renders the readout inert.
///     p: Active Moon palette.
///     cx: Application context used to render scaled metrics.
///
/// Returns:
///     A ticker element whose delayed chart navigation cannot bypass the Auto rail.
fn ticker_readout(
    model: &HeaderModel,
    sel: Option<(CoreId, String)>,
    show_1h: bool,
    actions: Option<(Entity<Backend>, Entity<Shell>)>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    // Each delta carries its window as a label; without one the tooltip is the only place that
    // says which span a percentage covers. Colour goes through `design::delta_tone`, the ONE
    // sign-to-colour mapping, so this readout can never drift from every other money cell.
    let delta_span = |selector: &'static str, label: String, v: f64| {
        let (text, sign) = delta_text(v);
        // Zero renders neutral: colouring it would report movement that is not there.
        let color = sign.map_or(p.text_muted, |sign| design::delta_tone(sign).color(p));
        h_flex()
            .flex_none()
            .gap(design::ui_px(cx, TICKER_DELTA_GAP))
            .items_baseline()
            .child(
                div()
                    .font_family(design::ui_font())
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(p.text_muted))
                    .child(label),
            )
            .child(
                div()
                    .debug_selector(|| selector.into())
                    .font_family(design::mono())
                    .text_size(design::t_body(cx))
                    .text_color(rgb(color))
                    .child(text),
            )
            .into_any_element()
    };
    let mut row = h_flex()
        .id("header-ticker")
        .debug_selector(|| "header-ticker".into())
        .flex_none()
        .items_baseline()
        // Wider than the U+0020 inside a grouped price ("61 333"): with a narrower gap the eye
        // binds "333" to the next token and reads the thousands group as a separate number.
        .gap(design::ui_px(cx, TICKER_GAP))
        .child(
            div()
                .font_family(design::ui_font())
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(model.ticker_base.clone()),
        );
    match model.ticker {
        Some(t) => {
            row = row
                .child(
                    div()
                        .debug_selector(|| "header-ticker-price".into())
                        .font_family(design::mono())
                        .text_size(design::t_body(cx))
                        .text_color(rgb(p.text_soft))
                        .font_weight(FontWeight(PRICE_WEIGHT))
                        .child(fmt_ticker_price(t.last)),
                )
                .children(show_1h.then(|| {
                    delta_span(
                        "header-ticker-delta-1h",
                        t!("header.delta_1h").to_string(),
                        t.delta_1h_pct,
                    )
                }))
                .child(delta_span(
                    "header-ticker-delta-24h",
                    t!("header.delta_24h").to_string(),
                    t.delta_24h_pct,
                ));
        }
        None => {
            row = row.child(
                div()
                    .font_family(design::mono())
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_muted))
                    .child("—"),
            );
        }
    }
    let row = row.tooltip(|_w, cx| {
        cx.new(|_| moon_ui::MoonTooltipView::new(t!("header.ticker_tip").to_string()))
            .into()
    });
    let Some((backend, shell)) = actions else {
        return row.into_any_element();
    };
    let group = model.group.clone();
    // A single click opens the ticker coin's chart on Main, matching ticker clicks in Orders and
    // Assets; a double-click opens the coin/core picker. The first click of a double-click may
    // already queue a chart request, but it does not raise the window because `activate` is false.
    row.cursor_pointer()
        .on_click(move |ev: &ClickEvent, window, cx| {
            if ev.click_count() >= 2 {
                shell.update(cx, |s, cx| s.toggle_ticker_popup(window, cx));
                return;
            }
            let Some((core, market)) = sel.clone() else {
                // An unresolved source has no core or market to open, so show the picker instead.
                shell.update(cx, |s, cx| s.toggle_ticker_popup(window, cx));
                return;
            };
            backend.update(cx, |b, bcx| {
                if b.open_on_main_if_authorized(Some(&group), (core, market), false) {
                    bcx.notify();
                }
            });
        })
        .into_any_element()
}

/// The Strategies, Analytics and Settings launchers.
///
/// Labelled with their names while the row has room, icon-only from header level 3 (the tooltip
/// keeps the name). The first-run Settings hint rides the Settings launcher.
fn header_launchers(
    group: &str,
    labeled: bool,
    folded: Option<(Vec<MoonMenuItem>, Option<AnyElement>)>,
    backend: Option<Entity<Backend>>,
    settings_hint_at: Option<Instant>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let [strategies, analytics, settings] = launcher_labels();
    let width = |label: &str| labeled.then(|| launcher_label_width(cx, label));
    let targets = [
        LaunchTarget {
            id: "header-strategies",
            labeled_width: width(&strategies),
            label: strategies,
            icon: crate::controls::STRATEGIES_ICON,
            workspace_owner: Some(group.to_string()),
            open: crate::strategies::open,
        },
        LaunchTarget {
            id: "header-analytics",
            labeled_width: width(&analytics),
            label: analytics,
            icon: "icons/layout-dashboard.svg",
            workspace_owner: Some(group.to_string()),
            open: crate::analytics::open,
        },
        LaunchTarget {
            id: "header-settings",
            labeled_width: width(&settings),
            label: settings,
            icon: "icons/settings.svg",
            workspace_owner: None,
            open: crate::settings::open,
        },
    ];
    // The first-run hint. TWO conditions: the timer decides how long the ring breathes, and the
    // saved config (`HeaderModel::core_ever_configured`) decides whether it is relevant at all, so
    // the moment a core is saved the ring is gone on the NEXT FRAME rather than at the end of its
    // timer.
    let mut ring = settings_hint_at.and_then(|at| crate::pulse::attention_ring(p.accent, at));
    let section = design::chrome_section(cx)
        .id("header-launchers")
        .debug_selector(|| "header-launchers".into())
        // Launcher captions are control captions, so they read in the UI face; `MoonButton` can
        // only force MONO on its own segments, so the family is set here and inherited. Paired
        // with `launcher_label_width`, which measures the same family.
        .font_family(design::ui_font());
    // The narrowest header's last steps: the three launchers fold into one "More" glyph, in the
    // same order and opening the same windows, led by the folded updater's and the hidden
    // clock's rows when there are; the first-run ring moves onto the glyph, and the clock's zone
    // picker anchors under it.
    if let Some((lead, anchor)) = folded {
        return section
            .child(
                div()
                    .relative()
                    .child(crate::controls::launch::overflow_button(
                        "header-overflow",
                        lead,
                        targets.into(),
                        Vec::new(),
                        backend,
                        p,
                        cx,
                    ))
                    .children(anchor)
                    .children(ring.take()),
            )
            .into_any_element();
    }
    section
        .children(targets.into_iter().map(|target| {
            let is_settings = target.id == "header-settings";
            let button = open_window_button(target, backend.clone(), p, cx);
            if is_settings {
                div()
                    .relative()
                    .child(button)
                    // Declared AFTER the button so the ring paints on top of its chrome; it is a
                    // pointer-transparent overlay and takes no clicks from the control beneath.
                    .children(ring.take())
                    .into_any_element()
            } else {
                button
            }
        }))
        .into_any_element()
}

/// Render one header gear button and its anchored settings `MoonPopover`.
///
/// Shared by the core-settings gear and the quiet-mode gear: one place that pairs a header trigger
/// with a content-width popover and its controlled open state. Shell owns that state through
/// `on_open_change`, while the popover handles outside-click dismissal.
///
/// Returns the `MoonPopover` itself rather than an opaque element so a caller whose content carries
/// `MoonDropdown`s can switch that dismissal off — their menus paint in their own deferred layers
/// and would read as an outside click. See the core-settings gear above.
///
/// Args:
///     id: Stable element identity; the popover derives its own id from it.
///     placement: Which corner of the trigger the popup hangs from.
///     content_width: Font-scaled content width the popup declares.
///     open: Whether the popup is up.
///     content: Lazily built content, present only while `open`.
///     trigger: The button that opens it — the header core gear keeps its own Action-sized button,
///         while the quiet-mode gear reuses the panels' shared `popup_gear_trigger`.
///     on_open_change: Handler that records the requested open state on Shell.
///
/// Returns:
///     The given trigger with its popover attached.
#[allow(clippy::too_many_arguments)]
pub(crate) fn header_gear_popover(
    id: &'static str,
    placement: MoonPopoverPlacement,
    content_width: f32,
    open: bool,
    content: Option<AnyElement>,
    trigger: impl IntoElement,
    on_open_change: impl Fn(bool, &mut Window, &mut App) + 'static,
) -> MoonPopover {
    MoonPopover::new(SharedString::from(format!("{id}-popover")))
        .placement(placement)
        .content_width_font(content_width)
        .open(open)
        .on_open_change(on_open_change)
        .trigger(trigger)
        .content(content.unwrap_or_else(|| div().into_any_element()))
}

/// Header balance: caption, TOTAL headline, unit, and the FREE sub-figure.
///
/// "Balance" names the whole account (`total_usdt`); "free" the part not committed to orders
/// (`free_usdt`). A `Stale` figure keeps its numbers and trades the "free" sub-figure for an amber
/// tag, so a retained pre-outage number is not presented as current. Numbers go through the shared
/// grouped amount format, the one the Assets panel uses. The caption goes at level 3, the unit at
/// level 4, the sub-figure at level 1; the headline never.
///
/// Double-click reveals Assets: Auto requests the existing group-owned surface, Classic opens the
/// singleton window. A dash is still clickable so a first-time user with no snapshot yet can reach
/// the same panel.
fn balance_label(
    group: String,
    backend: Option<Entity<Backend>>,
    view: BalanceView,
    level: HeaderLevel,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let caption = || {
        div()
            .font_family(design::ui_font())
            .text_size(design::t_caption(cx))
            .text_color(rgb(p.text_muted))
            .child(t!("header.balance_caption").to_string())
    };
    let mut row = h_flex()
        .id("header-balance")
        .debug_selector(|| "header-balance".into())
        .flex_none()
        .items_baseline()
        .gap(design::ui_px(cx, design::CHROME_GAP))
        .whitespace_nowrap();
    row = match view {
        BalanceView::NoCore => row.child(
            div()
                .font_family(design::ui_font())
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(t!("header.balance_none").to_string()),
        ),
        BalanceView::Unknown => row
            .when(level.balance_caption(), |row| row.child(caption()))
            .child(
                div()
                    .font_family(design::mono())
                    .text_size(design::t_body(cx))
                    .font_weight(FontWeight(BALANCE_WEIGHT))
                    .text_color(rgb(p.text_muted))
                    .child("—"),
            ),
        BalanceView::Value {
            total,
            free,
            stale,
            missing,
        } => row
            .when(level.balance_caption(), |row| row.child(caption()))
            .child(
                div()
                    .id("header-balance-total")
                    .debug_selector(|| "header-balance-total".into())
                    .font_family(design::mono())
                    .text_size(design::t_body(cx))
                    .font_weight(FontWeight(BALANCE_WEIGHT))
                    .text_color(rgb(if view.tagged() { p.text_muted } else { p.text }))
                    .child(fmt::usd_grouped(total)),
            )
            .when(level.usdt_unit(), |row| {
                row.child(
                    div()
                        .font_family(design::mono())
                        .text_size(design::t_caption(cx))
                        .text_color(rgb(p.text_muted))
                        .child("USDT"),
                )
            })
            .when(view.tagged(), |row| {
                row.child(
                    div()
                        .id("header-balance-stale")
                        .debug_selector(|| "header-balance-stale".into())
                        .px(design::ui_px(cx, STALE_PAD_X))
                        .rounded(design::ui_px(cx, 3.0))
                        .bg(rgba_from(p.amber, 0.14))
                        .font_family(design::ui_font())
                        .text_size(design::t_caption(cx))
                        .font_weight(FontWeight(500.0))
                        .text_color(rgb(p.amber))
                        .child(balance_tag_text(stale, missing)),
                )
            })
            .when(!view.tagged() && level.free_sub(), |row| {
                row.child(
                    h_flex()
                        .id("header-balance-free")
                        .debug_selector(|| "header-balance-free".into())
                        .items_baseline()
                        .gap(design::ui_px(cx, FREE_GAP))
                        .text_size(design::t_caption(cx))
                        .text_color(rgb(p.text_muted))
                        .child(
                            div()
                                .font_family(design::ui_font())
                                .child(t!("header.balance_free").to_string()),
                        )
                        .child(
                            div()
                                .font_family(design::mono())
                                .child(fmt::usd_grouped(free)),
                        ),
                )
            }),
    };
    let row = row.tooltip(crate::panels::common::text_tooltip(
        t!("header.balance_tip").to_string(),
    ));
    let Some(backend) = backend else {
        return row.into_any_element();
    };
    row.cursor_pointer()
        .on_click(move |ev: &ClickEvent, window, cx| {
            if ev.click_count() < 2 {
                return;
            }
            let auto = backend.update(cx, |b, bcx| {
                if b.request_assets_surface(&group) {
                    bcx.notify();
                    true
                } else {
                    false
                }
            });
            if !auto {
                let owner_display = crate::window::windowing::window_display_id(window, cx);
                crate::panels::open_assets_window(
                    backend.clone(),
                    Some(window.window_handle()),
                    owner_display,
                    cx,
                );
            }
        })
        .into_any_element()
}

#[cfg(test)]
mod tests;
