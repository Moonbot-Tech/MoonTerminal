//! Terminal-specific chrome composition over MoonPalette primitives.
//!
//! This is an adapter layer, not a reusable MoonPalette control: it knows about
//! Backend actions and MoonTerminal header content, while generic visuals still
//! come from MoonPalette tokens/components.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonIconSlot, MoonButtonSize, MoonButtonVariant, MoonMenuItem, MoonMenuSize,
    MoonPalette, MoonPopover, MoonPopoverPlacement, MoonPopupMenu, MoonRect, MoonSelectorPill,
    MoonSelectorSegment, MoonTag, MoonToggle, MoonToggleLabelSide, MoonToggleSize, MoonWindowFrame,
    h_flex,
};
use rust_i18n::t;

use moon_core::config::WorkspaceMode;
use moon_core::feed::ConnStatus;
use moon_core::session::BalanceState;
use moon_core::util::fmt;

use crate::shell::Shell;
use crate::{Backend, design};

/// Compose the terminal header for one group from the current backend and shell state.
///
/// `chrome_width` is the window width and controls priority-based ticker collapse on narrow windows.
///
/// Args:
///     group: Group whose header is being rendered.
///     backend: Backend providing core, balance, and ticker state.
///     shell: Shell owning controlled header popovers and window actions.
///     ticker_sel: Optional core and market selected for the ticker readout.
///     core_selector_open: Whether the active-core selector is open.
///     core_settings_open: Whether the core-settings popover is open.
///     core_settings_content: Lazily built content for the open core-settings popover.
///     quiet_settings_open: Whether the quiet-mode ("sleep") settings popover is open.
///     quiet_settings_content: Lazily built content for the open quiet-mode popover.
///     chrome_width: Current window width used for responsive ticker visibility.
///     p: Active palette.
///     cx: Application context used to read state and build elements.
///
/// Returns:
///     The complete terminal header element.
#[allow(clippy::too_many_arguments)]
pub fn header(
    group: &str,
    backend: Entity<Backend>,
    shell: Entity<Shell>,
    ticker_sel: Option<(moon_core::session::CoreId, String)>,
    core_selector_open: bool,
    core_settings_open: bool,
    core_settings_content: Option<AnyElement>,
    quiet_settings_open: bool,
    quiet_settings_content: Option<AnyElement>,
    chrome_width: f32,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    // Balance of the group's active trade core (server-side USDT figures). Rendered only when
    // the store classifies it as usable: no core, no snapshot yet, or an invalid valuation all
    // mean the amount is unknown, and printing 0.00 there would state an empty account as fact.
    let balance = {
        let b = backend.read(cx);
        b.active_trade_core(group)
            .and_then(|c| b.session.store().core(c))
            .map(|cd| {
                (
                    cd.balance_state(),
                    cd.assets.global.free_usdt,
                    cd.assets.global.total_usdt,
                )
            })
            .filter(|(state, ..)| state.has_value())
    };
    // The manual-strategy cluster is absent when the group has no active trade core; its
    // preceding separator has to go with it rather than fence off empty space.
    let manual = crate::controls::manual_strategy_controls(group, &backend, p, cx);
    h_flex()
        .w_full()
        .h(design::header_height_px(cx))
        .flex_none()
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        // One spacing rule across BOTH chrome strips — see `design::CHROME_GAP`: 8px inside a
        // group, 8px + rule + 8px between groups. The brand cluster uses the same token
        // internally, so the seams line up.
        .gap(design::ui_px(cx, design::CHROME_GAP))
        .bg(rgb(p.shell_high))
        // Brand draws its OWN trailing separator (MoonWindowFrame::brand_cluster), so the
        // groups below add only the seams after them.
        .child(
            MoonWindowFrame::main("terminal-header-brand-drag", 0.0)
                .brand_cluster(cx)
                .flex_none()
                .h_full(),
        )
        // Workspace mode is fixed chrome rather than dock content, so it remains reachable while
        // Auto independently controls which dock operations are allowed.
        .child(design::chrome_section(cx).child(workspace_mode_selector(group, &backend, cx)))
        .child(design::chrome_divider(cx, p))
        // Active trade core leads the trading context after the workspace preset. Balance, manual
        // strategy, and ticker all read through it. Interactive widgets are never a drag zone.
        .child(
            design::chrome_section(cx)
                .child(core_selector(
                    group,
                    &backend,
                    shell.clone(),
                    core_selector_open,
                    p,
                    cx,
                ))
                .child({
                    let shell = shell.clone();
                    header_gear_popover(
                        "core-gear",
                        MoonPopoverPlacement::BottomStart,
                        crate::shell::core_settings_popup::CONTENT_W,
                        core_settings_open,
                        core_settings_content,
                        MoonButton::new("core-gear")
                            .leading_icon(MoonButtonIconSlot::new("icons/settings-2.svg"))
                            .size(MoonButtonSize::Action)
                            .variant(MoonButtonVariant::Panel)
                            .render(),
                        move |open, window, cx| {
                            shell.update(cx, |s, cx| s.set_core_settings_open(open, window, cx));
                        },
                    )
                }),
        )
        .child(design::chrome_divider(cx, p))
        .child(balance_label(balance, p, cx))
        // Shrinkable: the strategy summary inside truncates, so a long one yields space to the
        // right-hand readouts instead of pushing them off the window.
        .children(manual.map(|ms| {
            h_flex()
                .min_w_0()
                .gap(design::ui_px(cx, design::CHROME_GAP))
                .items_center()
                .child(design::chrome_divider(cx, p))
                .child(ms)
        }))
        .child(
            // A dual role, both required: this is BOTH the `flex_1` spacer that pins the cluster
            // after it (ticker, clock, window buttons) to the right edge, AND the region that
            // drags the window by the empty part of the header. Swapping it for a plain
            // `div().flex_1()` looks like a harmless simplification but silently removes dragging.
            MoonWindowFrame::main("terminal-header-spacer-drag", 0.0)
                .drag_handle()
                .h_full()
                .flex_1()
                .min_w_0()
                .flex(),
        )
        // Ambient readouts are right-aligned: rate ticker, then the clock, then the window controls.
        .child(
            design::chrome_section(cx)
                // Rate ticker (configurable): "1 BTC = 61 333$ 1h +0.1% 24h +2.0%". Its popup is
                // positioned by hand in `shell::ticker` from the window's right edge inward, so it
                // has to account for EVERYTHING standing to the ticker's right — the divider below,
                // the clock, the gaps, and the window controls. Move or resize anything in this
                // cluster and that offset must follow. Divider and readout share ONE predicate:
                // split, they could drift into a divider fencing off nothing.
                .children(design::ticker_visible(cx, chrome_width).then(|| {
                    design::chrome_section(cx)
                        .child(ticker_readout(
                            group.to_string(),
                            ticker_sel,
                            design::ticker_deltas_visible(cx, chrome_width),
                            &backend,
                            shell.clone(),
                            p,
                            cx,
                        ))
                        .child(design::chrome_divider(cx, p))
                }))
                // Quiet mode ("sleep"): the toggle plus its settings gear, between the rate and the
                // clock. It renders at an explicit width (`chrome::quiet::header_quiet_width`) that
                // the ticker popup's offset above reuses, so the two cannot drift; its trailing
                // divider is part of the same cluster for the same reason.
                .child(crate::chrome::quiet::header_quiet_cluster(
                    &backend,
                    shell,
                    quiet_settings_open,
                    quiet_settings_content,
                    p,
                    cx,
                ))
                .child(design::chrome_divider(cx, p))
                // The selected zone's clock with a city code or system abbreviation; clicking opens
                // the picker. Its MoonPopover is anchored to this trigger, so unlike the ticker it
                // needs no offset arithmetic.
                .child(crate::chrome::clock::header_clock(&backend, p, cx))
                .when(design::show_custom_window_controls(), |this| {
                    this.child(
                        MoonWindowFrame::main("terminal-header-controls", 0.0)
                            .show_controls(true)
                            .visual_controls(cx),
                    )
                }),
        )
}

/// Build the persisted workspace-mode control as a compact Auto toggle.
///
/// Args:
///     group: Group whose preset is selected.
///     backend: Shared persisted workspace authority.
///     cx: Application context used to read the controlled mode.
///
/// Returns:
///     A compact MoonUI toggle that publishes mode changes through `WorkspaceRevision`.
fn workspace_mode_selector(group: &str, backend: &Entity<Backend>, cx: &App) -> impl IntoElement {
    let mode = backend.read(cx).workspace_mode(group);
    let auto = mode == WorkspaceMode::AutoTrading;
    let tooltip = if auto {
        t!("workspace.mode.auto_tip").to_string()
    } else {
        t!("workspace.mode.classic_tip").to_string()
    };
    let backend = backend.clone();
    let group = group.to_string();
    div()
        .id("header-workspace-mode-tip")
        .tooltip(crate::panels::common::text_tooltip(tooltip))
        .child(
            MoonToggle::new("header-workspace-mode")
                .label(t!("workspace.mode.auto").to_string())
                .label_side(MoonToggleLabelSide::Left)
                .checked(auto)
                .size(MoonToggleSize::Compact)
                .on_change(move |checked, _, cx| {
                    let mode = if *checked {
                        WorkspaceMode::AutoTrading
                    } else {
                        WorkspaceMode::Classic
                    };
                    backend.update(cx, |backend, backend_cx| {
                        backend.set_workspace_mode(&group, mode, backend_cx);
                    });
                }),
        )
}

/// Render the header ticker as `1 BTC = 61 333$ 1h +0.1% 24h +2.0%`.
///
/// Price and signed deltas come from `MarketDataSource::market_ticker`. A click opens the market;
/// a double-click opens the source popup hosted by [`Shell`]. When `show_deltas` is false, only the
/// price remains.
///
/// Args:
///     group: Group whose current Auto authority owns the retained ticker callback.
///     sel: Resolved ticker core and market, or `None` for the fallback source picker.
///     show_deltas: Whether one-hour and one-day changes fit in the current chrome width.
///     backend: Shared market data and chart-request authority.
///     shell: Header owner used to toggle the ticker source popup.
///     p: Active Moon palette.
///     cx: Application context used to read data and render scaled metrics.
///
/// Returns:
///     A ticker element whose delayed chart navigation cannot bypass the Auto rail.
fn ticker_readout(
    group: String,
    sel: Option<(moon_core::session::CoreId, String)>,
    show_deltas: bool,
    backend: &Entity<Backend>,
    shell: Entity<Shell>,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    let data = sel.as_ref().and_then(|(core, market)| {
        let b = backend.read(cx);
        let t = b.session.market_source().market_ticker(*core, market)?;
        Some((market.clone(), t))
    });
    let base = sel
        .as_ref()
        .map(|(core, market)| {
            backend
                .read(cx)
                .session
                .market_source()
                .market_label(*core, market)
                .coin
        })
        .unwrap_or_else(|| "BTC".to_string());
    // Each delta carries its window as a label; without one the tooltip is the only place that
    // says which span a percentage covers.
    let delta_span = |label: String, v: f64| {
        let (text, color) = match fmt::signed_pct(v, 1) {
            // Zero renders neutral: colouring it would report movement that is not there.
            Some((text, sign)) => (
                text,
                sign.pick(
                    design::positive_color(p),
                    design::danger_color(p),
                    p.text_soft,
                ),
            ),
            None => ("—".to_string(), p.text_muted),
        };
        h_flex()
            .gap(design::ui_px(cx, 3.0))
            .items_center()
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(p.text_muted))
                    .child(label),
            )
            .child(div().text_color(rgb(color)).child(text))
            .into_any_element()
    };
    let mut row = h_flex()
        .id("header-ticker")
        .flex_none()
        .items_center()
        // Wider than the U+0020 inside a grouped price ("61 333"): with a narrower gap the eye
        // binds "333$" to the next token and reads the thousands group as a separate number.
        .gap(design::ui_px(cx, 10.0))
        .font_family(design::mono())
        .text_size(design::t_body(cx))
        .cursor_pointer()
        .child(
            div()
                .text_color(rgb(p.text_soft))
                .child(format!("1 {base} =")),
        );
    match data {
        Some((_, t)) => {
            row = row
                .child(
                    div()
                        .text_color(rgb(p.text))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(fmt_ticker_price(t.last)),
                )
                .children(
                    show_deltas
                        .then(|| delta_span(t!("header.delta_1h").to_string(), t.delta_1h_pct)),
                )
                .children(
                    show_deltas
                        .then(|| delta_span(t!("header.delta_24h").to_string(), t.delta_24h_pct)),
                );
        }
        None => {
            row = row.child(div().text_color(rgb(p.text_muted)).child("—"));
        }
    }
    let backend = backend.clone();
    row.tooltip(|_w, cx| {
        cx.new(|_| moon_ui::MoonTooltipView::new(t!("header.ticker_tip").to_string()))
            .into()
    })
    // A single click opens the ticker coin's chart on Main, matching ticker clicks in Orders and
    // Assets; a double-click opens the coin/core picker. The first click of a double-click may
    // already queue a chart request, but it does not raise the window because `activate` is false.
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
}

/// Format a ticker price with a trailing dollar sign and magnitude-based precision.
///
/// Values at least 1000 are rounded to an integer and grouped with spaces, values at least 1 use
/// two decimal places, and lower values use four decimal places.
///
/// # Arguments
///
/// * `v` - Price value to format.
///
/// # Returns
///
/// The formatted price string.
fn fmt_ticker_price(v: f64) -> String {
    if v >= 1000.0 {
        let mut s = fmt::group_thousands(&format!("{v:.0}"));
        s.push('$');
        s
    } else if v >= 1.0 {
        format!("{v:.2}$")
    } else {
        format!("{v:.4}$")
    }
}

/// Fit a header core label and return the selector pill's content-driven width.
///
/// Args:
///     text: Full active-core label.
///     max_label_w: Maximum rendered width available to the label.
///     chrome_w: Rendered pill width outside the label.
///     measure: Function returning rendered width for arbitrary label fragments.
///
/// Returns:
///     The fitted label and its measured width plus pill chrome, rounded to a whole pixel.
fn fit_header_core_trigger(
    text: &str,
    max_label_w: f32,
    chrome_w: f32,
    measure: impl Fn(&str) -> f32,
) -> (String, f32) {
    let (label, label_w) = design::fit_text(text, max_label_w, measure);
    (label, (label_w + chrome_w).ceil())
}

/// Build the active-core control for Classic or the passive Auto workspace scope indicator.
///
/// Classic choices come from the group's cores. [`Backend::active_trade_core`] prefers a still-valid
/// remembered Classic selection, the current trading target, and finally the group's first core.
/// Auto instead reads only the rail-owned workspace selection, displays Overview when it is empty,
/// and returns a disabled caret-free pill without building a popover.
/// The trading target can come from Main's active fullscreen chart or a locked comparison anchor
/// in an Add or Custom tab. All toolbar and header trading controls read the same active core.
///
/// # Arguments
///
/// * `group` - Group whose trading cores should be listed.
/// * `backend` - Backend that owns core state and the remembered group selection.
/// * `shell` - Shell that owns the selector's controlled open state.
/// * `open` - Whether the selector popover is currently open.
/// * `p` - Active palette used to render status and text colors.
/// * `cx` - Application context used to read state and measure labels.
///
/// # Returns
///
/// The interactive Classic selector, passive Auto indicator, or a Classic placeholder when the
/// group has no cores.
fn core_selector(
    group: &str,
    backend: &Entity<Backend>,
    shell: Entity<Shell>,
    open: bool,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    // The pill keeps a fixed height and full rounding; its content width is capped below so a long
    // user-defined name cannot displace the header's right-hand readouts.
    const SEL_H: f32 = 26.0;

    let b = backend.read(cx);
    let cores = b.group_cores(group);
    let auto = b.workspace_mode(group) == WorkspaceMode::AutoTrading;
    let active = if auto {
        b.valid_auto_workspace_core(group)
    } else {
        b.active_trade_core(group)
    };
    let store = b.session.store();

    // Render a static placeholder instead of an empty drop-down when the group has no cores.
    if cores.is_empty() && !auto {
        return MoonTag::new()
            .outline()
            .rounded_full()
            .child(design::status_dot(p.text_muted, cx))
            .label(t!("header.no_cores").to_string())
            .into_any_element();
    }

    let active_ready = active
        .and_then(|id| store.core(id))
        .map(|c| c.status == ConnStatus::Ready)
        .unwrap_or(false);
    let dot_color = if auto && active.is_none() {
        p.accent
    } else if active_ready {
        design::positive_color(p)
    } else {
        design::danger_color(p)
    };
    // Normal labels size the pill to their measured content, avoiding an empty tail. The text cap
    // still prevents an anomalously long configured name from displacing the rest of the header.
    let raw_active_name = active
        .and_then(|id| cores.iter().find(|(cid, _)| *cid == id))
        .map(|(_, n)| n.clone())
        .unwrap_or_else(|| {
            if auto {
                t!("workspace.overview").to_string()
            } else {
                "—".to_string()
            }
        });
    // Chrome outside the label: left/right padding, status dot, gap, and borders. The absolute
    // caret occupies the right-padding reservation instead of adding another flex child.
    let (active_name, trigger_w) = fit_header_core_trigger(
        &raw_active_name,
        design::font_w(cx, design::HEADER_LABEL_MAX_W),
        design::ui_value(cx, 44.0),
        |text| design::ui_text_width(cx, text, 10.5, 500.0, true),
    );
    let trigger_h = design::ui_value(cx, SEL_H);

    // The header renders continuously, but exchange discovery scans every client snapshot. Build
    // the hidden menu only after controlled open state triggers a repaint.
    let items = if open && !auto {
        let exchange_names = b.session.market_source().core_exchange_names();
        let unknown_exchange = t!("common.exchange_unknown").to_string();
        let sections = crate::controls::core_menu_sections(&cores, &exchange_names);
        let mut items = Vec::with_capacity(cores.len() + sections.len());
        for (exchange, members) in sections {
            let exchange_label = exchange
                .map(crate::controls::exchange_display_name)
                .unwrap_or_else(|| unknown_exchange.clone());
            items.push(MoonMenuItem::label(exchange_label));
            for (id, name) in members {
                let backend = backend.clone();
                let group = group.to_string();
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
        items
    } else {
        Vec::new()
    };

    // Use the canonical `MoonSelectorPill` visual, with a glowing status dot and caret icon, as
    // the `MoonPopover` trigger. The explicit bounds below give its absolute root a measurable,
    // content-driven anchor; the content remains a `MoonPopupMenu` listing the cores. Shell
    // controls open state so selecting a core closes the popover while exchange labels and scroll
    // interactions do not.
    //
    // The pill uses `p.panel` as its background and `p.border` as an explicit border, keeping the
    // shape legible against the `shell_high` header unlike the old borderless Panel variant.
    //
    let pill = div()
        .relative()
        .flex_none()
        .w(px(trigger_w))
        .h(px(trigger_h))
        .child(
            MoonSelectorPill::new("header-core-pill")
                .bounds(MoonRect::new(0.0, 0.0, trigger_w, trigger_h))
                .height(SEL_H)
                .radius(SEL_H / 2.0)
                .leading_dot(dot_color)
                .disabled(auto)
                .caret(!auto)
                .segment(
                    MoonSelectorSegment::new(active_name)
                        .color(p.text)
                        .weight(500.0),
                )
                .render(),
        );
    if auto {
        return pill.into_any_element();
    }

    MoonPopover::new("header-core-selector")
        .placement(MoonPopoverPlacement::BottomStart)
        .fit_content()
        .open(open)
        .on_open_change(move |open, _, cx| {
            shell.update(cx, |shell, cx| {
                shell.set_header_core_selector_open(open, cx);
            });
        })
        .trigger(
            // MoonSelectorPill::bounds is absolute. This explicit in-flow box therefore owns the
            // exact geometry MoonPopover measures and keeps BottomStart anchored to its left edge.
            pill,
        )
        .content(
            MoonPopupMenu::new("header-core-menu")
                .fit_width(180.0, 560.0)
                .size(MoonMenuSize::Compact)
                .max_height_ui(520.0)
                .items(items)
                .render(),
        )
        .into_any_element()
}

/// Render one header gear button and its anchored settings `MoonPopover`.
///
/// Shared by the core-settings gear and the quiet-mode gear: one place that pairs a header trigger
/// with a content-width popover and its controlled open state. Shell owns that state through
/// `on_open_change`, while the popover handles outside-click dismissal.
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
) -> impl IntoElement {
    MoonPopover::new(SharedString::from(format!("{id}-popover")))
        .placement(placement)
        .content_width_font(content_width)
        .open(open)
        .on_open_change(on_open_change)
        .trigger(trigger)
        .content(content.unwrap_or_else(|| div().into_any_element()))
}

/// Header balance.
///
/// The caller supplies `Some` only for states with a usable value. `None` therefore represents
/// no core, no snapshot, or no valid valuation and renders as a dash. A `Stale` figure is shown
/// muted and tagged so a retained pre-outage number is not presented as current. Usable figures
/// render in `free / total USDT` order with the shared grouped amount format.
fn balance_label(
    balance: Option<(BalanceState, f64, f64)>,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    let row = h_flex()
        .flex_none()
        .gap(px(0.0))
        .font_family(design::mono())
        .text_size(design::t_body(cx))
        .text_color(rgb(p.text_soft))
        .child("Balance: ");
    let Some((state, free, total)) = balance else {
        return row.child(div().text_color(rgb(p.text_muted)).child("—"));
    };
    let live = state.is_current();
    // The shared amount format, precision included: the Assets panel renders this same figure,
    // and a locally formatted string drifts from it on trailing zeros.
    let free_text = fmt::usd_grouped(free);
    row.child(
        div()
            .text_color(rgb(if live { p.text } else { p.text_muted }))
            .font_weight(FontWeight::SEMIBOLD)
            .child(free_text),
    )
    .child(
        // Spaces on BOTH sides of the slash: " /19983" reads as a fraction of the free amount.
        // Same format as the free figure and as Assets — a locally rounded total would disagree
        // with the very panel this shares a number with.
        div()
            .text_color(rgb(p.text_muted))
            .child(format!(" / {} USDT", fmt::usd_grouped(total))),
    )
    .when(!live, |el| {
        el.child(
            div()
                .text_color(rgb(p.text_muted))
                .child(format!(" {}", t!("assets.balance_stale"))),
        )
    })
}

#[cfg(test)]
mod tests;
