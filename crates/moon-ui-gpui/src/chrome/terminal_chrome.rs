//! Terminal-specific chrome composition over MoonPalette primitives.
//!
//! This is an adapter layer, not a reusable MoonPalette control: it knows about
//! Backend actions and MoonTerminal header content, while generic visuals still
//! come from MoonPalette tokens/components.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonIconSlot, MoonButtonSize, MoonButtonVariant, MoonMenuItem, MoonMenuSize,
    MoonPalette, MoonPopover, MoonPopoverPlacement, MoonPopupMenu, MoonSelectorPill,
    MoonSelectorSegment, MoonTag, MoonWindowFrame, h_flex,
};
use rust_i18n::t;

use moon_core::feed::ConnStatus;
use moon_core::session::BalanceState;
use moon_core::util::fmt;

use crate::shell::Shell;
use crate::{Backend, design};

/// Compose the terminal header for one group from the current backend and shell state.
///
/// `chrome_width` is the window width and controls priority-based ticker collapse on narrow windows.
pub fn header(
    group: &str,
    backend: Entity<Backend>,
    shell: Entity<Shell>,
    ticker_sel: Option<(moon_core::session::CoreId, String)>,
    core_settings_open: bool,
    core_settings_content: Option<AnyElement>,
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
        // Active trade core first: the balance, the manual strategy and even the ticker are all
        // read through it, so the control everything else depends on leads the row. Interactive
        // widgets, so NOT a drag zone (a click would otherwise drag the window).
        .child(
            design::chrome_section(cx)
                .child(core_selector(group, &backend, p, cx))
                .child(core_gear_button(
                    shell.clone(),
                    core_settings_open,
                    core_settings_content,
                    cx,
                )),
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
                            ticker_sel,
                            design::ticker_deltas_visible(cx, chrome_width),
                            &backend,
                            shell,
                            p,
                            cx,
                        ))
                        .child(design::chrome_divider(cx, p))
                }))
                // UTC clock with an optional offset label; clicking opens the timezone picker. Its
                // MoonPopover is anchored to this trigger, so unlike the ticker it needs no offset
                // arithmetic.
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

/// Render the header ticker as `1 BTC = 61 333$ 1h +0.1% 24h +2.0%`.
///
/// Price and signed deltas come from `MarketDataSource::market_ticker`. A click opens the market;
/// a double-click opens the source popup hosted by [`Shell`]. When `show_deltas` is false, only the
/// price remains.
fn ticker_readout(
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
        .map(|(_, market)| moon_core::symbol::coin_of_market(market).to_string())
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
            b.open_request = Some((core, market));
            b.open_request_rev = b.open_request_rev.wrapping_add(1);
            b.open_request_activate = false;
            bcx.notify();
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

/// Build the selector for a group's active trading core.
///
/// The choices come from the group's cores. [`Backend::active_trade_core`] prefers a still-valid
/// sticky manual override, then the current trading target, and finally the group's first core.
/// The trading target can come from Main's active fullscreen chart or a locked comparison anchor
/// in an Add or Custom tab. All toolbar and header trading controls read the same active core.
///
/// # Arguments
///
/// * `group` - Group whose trading cores should be listed.
/// * `backend` - Backend that owns core state and selection overrides.
/// * `p` - Active palette used to render status and text colors.
/// * `cx` - Application context used to read state and measure labels.
///
/// # Returns
///
/// The selector element, or a static placeholder when the group has no cores.
fn core_selector(group: &str, backend: &Entity<Backend>, p: MoonPalette, cx: &App) -> AnyElement {
    // The pill keeps a fixed height and full rounding; its content width is capped below so a long
    // user-defined name cannot displace the header's right-hand readouts.
    const SEL_H: f32 = 26.0;

    let b = backend.read(cx);
    let cores = b.group_cores(group);
    let active = b.active_trade_core(group);
    let store = b.session.store();

    // Render a static placeholder instead of an empty drop-down when the group has no cores.
    if cores.is_empty() {
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
    let dot_color = if active_ready {
        design::positive_color(p)
    } else {
        design::danger_color(p)
    };
    // Capped: a core name is arbitrary user text and this pill sizes to its content, so an
    // uncapped one pushes the clock and the window controls off the header. Full name stays in
    // the menu below.
    let active_name = design::fit_label(
        cx,
        &active
            .and_then(|id| cores.iter().find(|(cid, _)| *cid == id))
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| "—".to_string()),
        design::font_w(cx, design::HEADER_LABEL_MAX_W),
    );

    let mut items = Vec::with_capacity(cores.len());
    for (id, name) in cores.iter() {
        let id = *id;
        let backend = backend.clone();
        let group = group.to_string();
        items.push(
            MoonMenuItem::with_key(format!("core-{id}"), name.clone())
                .selected(active == Some(id))
                .checked(active == Some(id))
                .on_click(move |_, _, cx| {
                    backend.update(cx, |b, bcx| {
                        b.set_trade_core_override(&group, id);
                        bcx.notify();
                    });
                }),
        );
    }

    // Use the canonical `MoonSelectorPill` visual, with a glowing status dot and caret icon, as
    // the `MoonPopover` trigger. The content is a `MoonPopupMenu` listing the cores. These moonui
    // components need no manual trigger styling or size workaround. The popover owns its open
    // state through internal `use_keyed_state`, toggles on click, and closes after core selection.
    //
    // The pill uses `p.panel` as its background and `p.border` as an explicit border, keeping the
    // shape legible against the `shell_high` header unlike the old borderless Panel variant.
    //
    // Size the menu to the longest core name; a fixed width clipped long names.
    let menu_w = design::menu_fit_width(cx, cores.iter().map(|(_, n)| n.as_str()), 180.0);
    MoonPopover::new("header-core-selector")
        .placement(MoonPopoverPlacement::BottomStart)
        .width(design::popover_outer_width(cx, menu_w))
        .close_on_content_click(true)
        .trigger(
            MoonSelectorPill::new("header-core-pill")
                .height(SEL_H)
                .radius(SEL_H / 2.0)
                .leading_dot(dot_color)
                .segment(
                    MoonSelectorSegment::new(active_name)
                        .color(p.text)
                        .weight(500.0),
                )
                .render(),
        )
        .content(
            MoonPopupMenu::new("header-core-menu")
                .width(menu_w)
                .size(MoonMenuSize::Compact)
                .items(items)
                .render(),
        )
        .into_any_element()
}

/// Render the core-settings button and its anchored `MoonPopover`.
///
/// Shell controls the open state and seeds fields through `set_core_settings_open`; the popover
/// handles outside-click dismissal. The icon-only button keeps square padding around the glyph.
fn core_gear_button(
    shell: Entity<Shell>,
    open: bool,
    content: Option<AnyElement>,
    cx: &App,
) -> impl IntoElement {
    MoonPopover::new("core-gear-popover")
        .placement(MoonPopoverPlacement::BottomStart)
        // Use the content module's width basis so both boxes follow the same font scale.
        .width(design::popover_outer_width(
            cx,
            design::font_w(cx, crate::core_settings_popup::CONTENT_W),
        ))
        .open(open)
        .on_open_change(move |open, window, cx| {
            shell.update(cx, |s, cx| s.set_core_settings_open(open, window, cx));
        })
        .trigger(
            MoonButton::new("core-gear")
                .leading_icon(MoonButtonIconSlot::new("icons/settings-2.svg"))
                .size(MoonButtonSize::Action)
                .variant(MoonButtonVariant::Panel)
                .render(),
        )
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
