//! Health strip at the top of Core Status.
//!
//! Counts, the verdict, and the scope-marker tail are computed by the caller. Click handlers
//! are handed in, so this function never reads the view entity.

use std::rc::Rc;

use gpui::*;
use moon_ui::{MoonPalette, MoonTag, MoonTagRadius, MoonTagVariant, MoonTone, h_flex};
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
    let offline = on_filter.clone();
    let ping = on_filter.clone();
    let version = on_filter.clone();
    let api = on_filter.clone();
    let facts = chip_facts(counts, ping_factor);
    let tip = chips_tooltip(&facts);
    let [f_off, f_ping, f_ver, f_api, f_alarms] = facts;
    // One row, clipped at the right: the order is the clip priority (offline first, alarms
    // last), and the tooltip names every chip so a clipped one stays reachable. The row never
    // wraps — a second line would push the toolbar down whenever the dock narrows.
    let chips = h_flex()
        .id("cs-health-chips")
        .debug_selector(|| "cs-health-chips".into())
        .min_w(px(0.0))
        .flex_shrink(1.0)
        .overflow_hidden()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .font_family(design::ui_font())
        .tooltip(crate::panels::common::text_tooltip(tip))
        .child(health_chip(
            cx,
            "core-status-health-offline",
            f_off.label,
            f_off.count,
            MoonTone::Danger,
            active == Some(HealthFilter::Offline),
            move |_, _, app| offline(HealthFilter::Offline, app),
        ))
        .child(health_chip(
            cx,
            "core-status-health-ping",
            f_ping.label,
            f_ping.count,
            MoonTone::Warning,
            active == Some(HealthFilter::Ping),
            move |_, _, app| ping(HealthFilter::Ping, app),
        ))
        .child(health_chip(
            cx,
            "core-status-health-version",
            f_ver.label,
            f_ver.count,
            MoonTone::Warning,
            active == Some(HealthFilter::Version),
            move |_, _, app| version(HealthFilter::Version, app),
        ))
        .child(health_chip(
            cx,
            "core-status-health-api",
            f_api.label,
            f_api.count,
            MoonTone::Warning,
            active == Some(HealthFilter::ApiExpiry),
            move |_, _, app| api(HealthFilter::ApiExpiry, app),
        ))
        .child(health_chip(
            cx,
            "core-status-health-alarms",
            f_alarms.label,
            f_alarms.count,
            MoonTone::Warning,
            false,
            move |_, _, app| on_alarms(app),
        ));

    h_flex()
        .id("core-status-health")
        .debug_selector(|| "cs-health".into())
        .w_full()
        .min_w(px(0.0))
        .flex_none()
        .items_center()
        .gap_x(design::ui_px(cx, 14.0))
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 10.0))
        .border_b_1()
        .border_color(rgb(p.border))
        .child(
            h_flex()
                .debug_selector(|| "cs-health-verdict".into())
                .min_w(design::ui_px(cx, VERDICT_MIN_W))
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

/// Narrowest the verdict gets while chips are clipped, in design pixels: the mark plus the
/// start of the sentence, so the verdict never collapses to its mark alone.
const VERDICT_MIN_W: f32 = 180.0;

/// One chip's text, in strip order. The same facts build the chips and their tooltip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ChipFact {
    /// Translated chip name. A sentence, so it draws in the interface face.
    pub(super) label: String,
    /// Members of the chip. Drawn in the mono face.
    pub(super) count: usize,
}

/// The five chips in clip-priority order: offline, ping, version, API key, alarms.
///
/// Args:
///     counts: Chip counts.
///     ping_factor: `LatWarn.yellow`, printed in the ping label.
///
/// Returns:
///     One fact per chip. GPUI-free, so the order and the tooltip are testable without a window.
pub(super) fn chip_facts(counts: &health::HealthCounts, ping_factor: u8) -> [ChipFact; 5] {
    let fact = |label: String, count: usize| ChipFact { label, count };
    [
        fact(
            t!("core_status.health.chip_offline").to_string(),
            counts.offline_failed,
        ),
        fact(
            t!("core_status.health.chip_ping", k = ping_factor).to_string(),
            counts.ping_high,
        ),
        fact(
            t!("core_status.health.chip_version").to_string(),
            counts.version_behind,
        ),
        fact(
            t!("core_status.health.chip_api").to_string(),
            counts.api_expiring,
        ),
        fact(
            t!("core_status.health.chip_alarms").to_string(),
            counts.alarms_24h,
        ),
    ]
}

/// Tooltip over the chip row: every chip on its own line, so a clipped chip stays readable.
///
/// Args:
///     facts: The facts the chips were built from.
///
/// Returns:
///     `label: count` lines in strip order.
pub(super) fn chips_tooltip(facts: &[ChipFact]) -> String {
    facts
        .iter()
        .map(|fact| format!("{}: {}", fact.label, fact.count))
        .collect::<Vec<_>>()
        .join("\n")
}

/// One chip: the tinted severity tag, a label plus a count that is never the part that gets cut.
///
/// Args:
///     cx: Application context used for the palette.
///     id: Element id, unique in the strip. Also the layout selector.
///     label: Translated chip name, drawn in the interface font.
///     count: Members of the chip. Zero draws a neutral, dimmed chip; the count is mono.
///     tone: Severity of a non-zero count: wash, border and count colour.
///     selected: Whether this chip is the active filter. The tag draws the accent ring.
///     on_click: Click handler. An empty chip still fires it.
///
/// Returns:
///     The tag, content-sized, so a layout test can see the chip box.
#[allow(clippy::too_many_arguments)]
fn health_chip(
    cx: &App,
    id: &'static str,
    label: String,
    count: usize,
    tone: MoonTone,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let selector = id;
    let measure_label = label.clone();
    let measure_count = count.to_string();
    let inner_id = format!("{id}-inner");
    let label_id = format!("{id}-label");
    let count_id = format!("{id}-count");
    // The tag's own face: 11 px on a 14 px line, scaled by the theme.
    let face = moon_ui::MoonTheme::active_tokens(cx).text(11.0, 14.0);
    // The tag cannot carry a debug id on its count. A zero-opacity copy behind it gives the
    // layout test the label and count boxes without changing the control the user clicks.
    div()
        .debug_selector(move || selector.into())
        .relative()
        .flex_none()
        .child(
            h_flex()
                .absolute()
                .inset_0()
                .h_full()
                .overflow_hidden()
                .opacity(0.0)
                .text_size(px(face.font_size))
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
                        .font_family(design::mono())
                        .debug_selector(move || count_id)
                        .child(measure_count),
                ),
        )
        .child(
            MoonTag::new()
                .variant(MoonTagVariant::Tinted)
                .radius(MoonTagRadius::Small)
                .mono(false)
                .severity_count(count, tone)
                .selected(selected)
                .on_click(id, on_click)
                .child(label),
        )
}

#[cfg(test)]
mod tests;
