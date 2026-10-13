//! Health strip at the top of Core Status.
//!
//! Counts, the verdict, and the scope-marker tail are computed by the caller. Click handlers
//! are handed in, so this function never reads the view entity.

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonButton, MoonButtonSize, MoonButtonVariant, MoonPalette, h_flex};
use rust_i18n::t;

use super::health::{self, HealthFilter, Tone};
use crate::design;

/// Draw the verdict, the five chips, and the hidden-cores marker.
///
/// Args:
///     verdict: Tone and the figures the two sentences name.
///     counts: Chip counts. Zero stays visible and clickable.
///     active: Chip narrowing the rows, if one is on.
///     scope_tail: Marker for cores the preset hides. `None` when nothing is hidden.
///     ping_factor: `LatWarn.yellow`, the multiplier printed on the ping chip.
///     cx: Application context used to size text and read the palette.
///     on_filter: Toggles one chip. An empty chip still fires it.
///     on_alarms: Opens the alarms list. The alarms chip is not a row filter.
///
/// Returns:
///     The strip. The title and the secondary line ellipsize. A chip count does not.
pub(super) fn health_strip(
    verdict: &health::Verdict,
    counts: &health::HealthCounts,
    active: Option<HealthFilter>,
    scope_tail: Option<AnyElement>,
    ping_factor: u8,
    cx: &App,
    on_filter: Rc<dyn Fn(HealthFilter, &mut App)>,
    on_alarms: Rc<dyn Fn(&mut App)>,
) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    let (mark, mark_bg, mark_fg) = match verdict.tone {
        Tone::Bad => ("!", design::moon_alpha(p.red, 0.14), p.red),
        Tone::Warn => ("!", design::moon_alpha(p.amber, 0.18), p.amber),
        Tone::Ok => ("✓", design::moon_alpha(p.green, 0.18), p.green),
    };
    // n == 1 uses its own key. plural_key's Slavic `_one` also matches 21/31/101, which must
    // keep the count, so the uncounted sentence is not that form.
    let title = if verdict.tone == Tone::Ok {
        if verdict.total == 1 {
            t!("core_status.health.verdict_ok_single").to_string()
        } else {
            let key = super::presentation::plural_key(
                "core_status.health.verdict_ok",
                verdict.total as u64,
            );
            t!(key.as_str(), total = verdict.total).to_string()
        }
    } else {
        t!(
            "core_status.health.verdict_bad",
            n = verdict.attention,
            total = verdict.total
        )
        .to_string()
    };
    let online_key =
        super::presentation::plural_key("core_status.health.sub_online", verdict.online as u64);
    let servers_key =
        super::presentation::plural_key("core_status.health.sub_servers", verdict.servers as u64);
    let secondary = format!(
        "{} · {}",
        t!(online_key.as_str(), n = verdict.online),
        t!(servers_key.as_str(), n = verdict.servers),
    );
    let quiet = p.text_soft;
    let offline = on_filter.clone();
    let ping = on_filter.clone();
    let version = on_filter.clone();
    let api = on_filter.clone();
    let chips = h_flex()
        .debug_selector(|| "cs-health-chips".into())
        .min_w(px(0.0))
        .max_w_full()
        .flex_1()
        .flex_wrap()
        .justify_end()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .child(health_chip(
            cx,
            "core-status-health-offline",
            t!("core_status.health.chip_offline").to_string(),
            counts.offline_failed,
            p.red,
            quiet,
            active == Some(HealthFilter::Offline),
            move |_, _, app| offline(HealthFilter::Offline, app),
        ))
        .child(health_chip(
            cx,
            "core-status-health-ping",
            t!("core_status.health.chip_ping", k = ping_factor).to_string(),
            counts.ping_high,
            p.yellow,
            quiet,
            active == Some(HealthFilter::Ping),
            move |_, _, app| ping(HealthFilter::Ping, app),
        ))
        .child(health_chip(
            cx,
            "core-status-health-version",
            t!("core_status.health.chip_version").to_string(),
            counts.version_behind,
            p.yellow,
            quiet,
            active == Some(HealthFilter::Version),
            move |_, _, app| version(HealthFilter::Version, app),
        ))
        .child(health_chip(
            cx,
            "core-status-health-api",
            t!("core_status.health.chip_api").to_string(),
            counts.api_expiring,
            p.yellow,
            quiet,
            active == Some(HealthFilter::ApiExpiry),
            move |_, _, app| api(HealthFilter::ApiExpiry, app),
        ))
        .child(health_chip(
            cx,
            "core-status-health-alarms",
            t!("core_status.health.chip_alarms").to_string(),
            counts.alarms_24h,
            p.yellow,
            quiet,
            false,
            move |_, _, app| on_alarms(app),
        ));

    h_flex()
        .id("core-status-health")
        .debug_selector(|| "cs-health".into())
        .w_full()
        .min_w(px(0.0))
        .flex_none()
        .flex_wrap()
        .items_center()
        .gap_x(design::ui_px(cx, 14.0))
        .gap_y(design::ui_px(cx, 8.0))
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 10.0))
        .border_b_1()
        .border_color(rgb(p.border))
        .child(
            h_flex()
                .debug_selector(|| "cs-health-verdict".into())
                .min_w(px(0.0))
                .flex_1()
                .items_center()
                .gap(design::ui_px(cx, 10.0))
                .child(
                    div()
                        .flex_none()
                        .w(design::ui_px(cx, 30.0))
                        .h(design::ui_px(cx, 30.0))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(mark_bg)
                        .text_color(rgb(mark_fg))
                        .font_weight(FontWeight::BOLD)
                        .text_size(design::t_body(cx))
                        .child(mark),
                )
                .child(
                    div()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .child(
                            div()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .font_family(design::ui_font())
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(design::t_body(cx))
                                .text_color(rgb(p.text))
                                .child(title),
                        )
                        .child(
                            div()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .font_family(design::ui_font())
                                .text_size(design::t_caption(cx))
                                .text_color(rgb(p.text_soft))
                                .child(secondary),
                        ),
                ),
        )
        .child(chips)
        .children(scope_tail)
}

/// One chip: a label plus a count that is never the part that gets cut off.
///
/// Args:
///     cx: Application context used to size the button.
///     id: Element id, unique in the strip. Also the layout selector.
///     label: Translated chip name.
///     count: Members of the chip. Zero selects the dim bordered variant.
///     hot_color: Count colour when the chip has members.
///     quiet_color: Count colour when the chip is empty.
///     selected: Whether this chip is the active filter.
///     on_click: Click handler. An empty chip still fires it.
///
/// Returns:
///     The button, content-sized, so a layout test can see the chip box. The count is its own
///     segment, so the label ellipsizes before the number.
fn health_chip(
    cx: &App,
    id: &'static str,
    label: String,
    count: usize,
    hot_color: u32,
    quiet_color: u32,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let dim = count == 0;
    let p = MoonPalette::active(cx);
    let selector = id;
    let count_text = count.to_string();
    let measure_label = label.clone();
    let measure_count = count_text.clone();
    let inner_id = format!("{id}-inner");
    let label_id = format!("{id}-label");
    let count_id = format!("{id}-count");
    // The button cannot carry a debug id on its label and count. A zero-opacity copy behind it
    // gives the layout test those two boxes without changing the control the user clicks.
    div()
        .debug_selector(move || selector.into())
        .relative()
        .flex_none()
        .when(dim, |chip| chip.opacity(0.65))
        .when(selected, |chip| {
            chip.child(
                div()
                    .absolute()
                    .top(px(-2.0))
                    .left(px(-2.0))
                    .right(px(-2.0))
                    .bottom(px(-2.0))
                    .border_2()
                    .border_color(rgb(p.accent))
                    .rounded(px(6.0)),
            )
        })
        .child(
            h_flex()
                .absolute()
                .inset_0()
                .h_full()
                .overflow_hidden()
                .opacity(0.0)
                .debug_selector(move || inner_id)
                .child(
                    div()
                        .min_w(px(0.0))
                        .h_full()
                        .flex_1()
                        .overflow_hidden()
                        .debug_selector(move || label_id)
                        .child(measure_label),
                )
                .child(
                    div()
                        .h_full()
                        .flex_none()
                        .overflow_hidden()
                        .debug_selector(move || count_id)
                        .child(measure_count),
                ),
        )
        .child(
            MoonButton::new(id)
                .text_segment(label, quiet_color, 400.0)
                .text_segment(count_text, if dim { quiet_color } else { hot_color }, 600.0)
                .size(MoonButtonSize::density(cx))
                .variant(if dim {
                    MoonButtonVariant::Panel
                } else {
                    MoonButtonVariant::Neutral
                })
                .selected(selected)
                .on_click(on_click)
                .render(),
        )
}

#[cfg(test)]
mod tests;
