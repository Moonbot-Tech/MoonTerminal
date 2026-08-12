//! Header toggle and picker for Moonbot manual strategies.
//!
//! State lives in the core's `ClientSettings.use_manual_strategy` and `manual_strategy_id` fields.
//! Toggle and picker changes send `ClientSettingsEdit::ManualStrategy`; the process-lifetime local
//! override exposed by `Backend::manual_strat_state` provides immediate feedback and continues to
//! take precedence over ClientSettings snapshots until replaced. Core echoes and command failures do
//! not reconcile it. When enabled, the core derives sell and stop behavior for manual orders from
//! the strategy fields, so the toolbar's TP, S slots, and SL do not apply to new orders and are
//! disabled. `effective_strat_id` in moon-core already routes manual orders through the selected
//! strategy.

use gpui::*;
use rust_i18n::t;

use moon_ui::{
    MoonMenuItem, MoonMenuSize, MoonPalette, MoonPopover, MoonPopoverPlacement, MoonPopupMenu,
    MoonSelectorPill, MoonSelectorSegment, MoonToggle, MoonToggleLabelSide, MoonToggleSize, h_flex,
};

use moon_core::feed::{ClientSettingsEdit, StrategyRow, StrategySchemaModel};
use moon_core::session::CoreId;

use crate::backend::MANUAL_STRATEGY_KIND;
use crate::{Backend, design};

/// Pill height shared with the header's core selector; label width is capped separately.
const PILL_H: f32 = 26.0;

/// Header "manual strategy" cluster: the MS toggle, the picker pill, and a summary of the
/// selected strategy's parameters.
///
/// `None` when the group has no active trade core or that core has no Manual-kind strategies. The
/// caller owns the separator that precedes the cluster and must drop it on `None`, otherwise the
/// header keeps a rule with nothing behind it.
pub fn manual_strategy_controls(
    group: &str,
    backend: &Entity<Backend>,
    p: MoonPalette,
    cx: &App,
) -> Option<AnyElement> {
    let b = backend.read(cx);
    let core = b.active_trade_core(group)?;
    let core_data = b.session.store().core(core)?;
    let manuals = manual_strategy_options(&core_data.strategies)?;
    let (on, id) = b.manual_strat_state(core);
    let sel_row = core_data.strategies.iter().find(|s| s.id == id && id != 0);
    let schema = core_data.schema.as_ref();
    // Show the Moonbot-style Buy/Sell/SL/TS summary only while the mode is enabled.
    let summary = (on && sel_row.is_some())
        .then(|| sel_row.map(|r| strat_summary(r, schema)))
        .flatten();

    // The pill shows the selected strategy, the localized none marker when no id is selected, or
    // `?` when an id exists but the selected strategy was deleted.
    // Capped like the core selector beside it: a strategy name is arbitrary user text and this
    // pill sizes to its content, so an uncapped one pushes the header's right cluster off-window.
    let display: String = match (sel_row, id) {
        (Some(r), _) => {
            design::fit_label(cx, &r.name, design::font_w(cx, design::HEADER_LABEL_MAX_W))
        }
        (None, 0) => t!("header.ms_none").to_string(),
        (None, _) => "?".to_string(),
    };
    let dot_color = if on && sel_row.is_some() {
        design::positive_color(p)
    } else if on {
        // Signal an enabled strategy id that did not resolve by using the danger color.
        design::danger_color(p)
    } else {
        p.text_muted
    };

    let mut items = Vec::with_capacity(manuals.len());
    for (sid, name) in &manuals {
        let sid = *sid;
        let backend = backend.clone();
        items.push(
            MoonMenuItem::with_key(format!("ms-{sid}"), name.clone())
                .selected(id == sid)
                .checked(id == sid)
                // Selecting a strategy also enables the mode, matching the Moonbot menu.
                .on_click(move |_, _, cx| {
                    backend.update(cx, |b, bcx| {
                        send_manual(b, core, true, sid);
                        bcx.notify();
                    });
                }),
        );
    }

    let toggle_backend = backend.clone();
    let mut row = h_flex()
        .min_w_0()
        .gap(design::ui_px(cx, 8.0))
        .items_center()
        .child(
            MoonToggle::new("header-ms-toggle")
                .label("MS")
                .label_side(MoonToggleLabelSide::Left)
                .checked(on)
                .size(MoonToggleSize::Compact)
                // The mode cannot be enabled until the picker selects a strategy.
                .disabled(id == 0)
                .on_change(move |ch: &bool, _w, app| {
                    let v = *ch;
                    toggle_backend.update(app, |b, bcx| {
                        let (_, cur_id) = b.manual_strat_state(core);
                        if cur_id == 0 {
                            return;
                        }
                        // Disabling preserves the id so the next toggle restores the same strategy.
                        send_manual(b, core, v, cur_id);
                        bcx.notify();
                    });
                }),
        )
        .child({
            MoonPopover::new("header-ms-selector")
                .placement(MoonPopoverPlacement::BottomStart)
                .fit_content()
                .close_on_content_click(true)
                .trigger(
                    MoonSelectorPill::new("header-ms-pill")
                        .height(PILL_H)
                        .radius(PILL_H / 2.0)
                        .leading_dot(dot_color)
                        .segment(
                            MoonSelectorSegment::new(display)
                                .color(if on { p.text } else { p.text_soft })
                                .weight(500.0),
                        )
                        .render(),
                )
                .content(
                    MoonPopupMenu::new("header-ms-menu")
                        .fit_width(200.0, 560.0)
                        .size(MoonMenuSize::Compact)
                        .items(items)
                        .render(),
                )
        });
    if let Some(summary) = summary {
        row = row.child(
            div()
                // The longest thing in the left half of the header; truncating it here keeps a
                // long strategy summary from pushing the right-hand readouts off a narrow window.
                .min_w_0()
                .truncate()
                .text_size(design::t_caption(cx))
                .font_family(design::mono())
                .text_color(rgb(p.text_soft))
                .child(summary),
        );
    }
    Some(row.into_any_element())
}

/// Collect Manual-kind strategy ids and names in their snapshot order.
///
/// Args:
///     strategies: Current strategy snapshot for the active trade core.
///
/// Returns:
///     Ordered picker options, or `None` when the snapshot has no Manual-kind strategies.
fn manual_strategy_options(strategies: &[StrategyRow]) -> Option<Vec<(u64, String)>> {
    let options: Vec<_> = strategies
        .iter()
        .filter(|strategy| strategy.kind_ordinal == MANUAL_STRATEGY_KIND)
        .map(|strategy| (strategy.id, strategy.name.clone()))
        .collect();
    (!options.is_empty()).then_some(options)
}

#[cfg(test)]
mod tests;

/// Select the zero-based `ix`th manual strategy in picker order and enable manual-strategy mode.
///
/// This performs the same update as clicking the corresponding picker item.
///
/// Args:
///     b: Backend used to read strategies and send the settings edit.
///     core: Core whose manual strategy should be selected.
///     ix: Zero-based position among that core's Manual-kind strategies.
///
/// Returns:
///     `true` when that position exists; `false` to let the hotkey propagate otherwise.
pub(crate) fn select_manual_strategy(b: &mut Backend, core: CoreId, ix: usize) -> bool {
    let sid = b.session.store().core(core).and_then(|cd| {
        cd.strategies
            .iter()
            .filter(|s| s.kind_ordinal == MANUAL_STRATEGY_KIND)
            .nth(ix)
            .map(|s| s.id)
    });
    match sid {
        Some(sid) => {
            send_manual(b, core, true, sid);
            true
        }
        None => false,
    }
}

/// Store the process-lifetime local override and send a manual-strategy edit to the core.
///
/// The override remains authoritative until replaced or process exit; neither a core echo nor a
/// command failure reconciles it. Send failures are logged.
///
/// Args:
///     b: Backend whose local state and session are updated.
///     core: Target core.
///     on: Whether manual-strategy mode should be enabled.
///     id: Selected strategy id, retained even when the mode is disabled.
fn send_manual(b: &mut Backend, core: CoreId, on: bool, id: u64) {
    b.set_manual_strat_local(core, on, id);
    if let Err(e) = b
        .session
        .edit_client_settings(core, ClientSettingsEdit::ManualStrategy { on, id })
    {
        log::warn!("manual strategy edit failed: {e:#}");
    }
}

/// Build a Moonbot-style `Buy +0.00% Sell +0.50% SL ON TS OFF` parameter summary.
///
/// Values absent from the strategy snapshot fall back to defaults from its schema kind. Missing
/// Buy or Sell fields fall back to zero; SL and TS are always included.
///
/// Args:
///     row: Selected strategy snapshot.
///     schema: Optional schema providing defaults omitted from the snapshot.
///
/// Returns:
///     The four-part Buy, Sell, SL, and TS summary.
fn strat_summary(row: &StrategyRow, schema: Option<&StrategySchemaModel>) -> String {
    let field = |name: &str| strat_field(row, schema, name);
    let mut parts: Vec<String> = Vec::with_capacity(4);
    // Always show Buy and Sell, as Moonbot does. A default-valued field may be absent from both the
    // snapshot and some schema sections; zero then means the current price, matching the core's
    // `signal price +0%` diagnostic.
    let buy = field("BuyPrice").unwrap_or_else(|| "0".to_string());
    parts.push(format!("Buy {}", fmt_pct(&buy)));
    let sell = field("SellPrice").unwrap_or_else(|| "0".to_string());
    parts.push(format!("Sell {}", fmt_pct(&sell)));
    parts.push(format!("SL {}", on_off(field("UseStopLoss"))));
    parts.push(format!("TS {}", on_off(field("UseTrailing"))));
    parts.join(" · ")
}

/// Resolve a strategy field from the snapshot, then its schema-kind default.
///
/// Args:
///     row: Strategy snapshot whose explicit fields take priority.
///     schema: Optional schema containing defaults by strategy kind.
///     name: Exact field name to resolve.
///
/// Returns:
///     The explicit or default value, or `None` when neither is available.
fn strat_field(
    row: &StrategyRow,
    schema: Option<&StrategySchemaModel>,
    name: &str,
) -> Option<String> {
    if let Some((_, v)) = row.fields.iter().find(|(n, _)| n == name) {
        return Some(v.clone());
    }
    schema?
        .kinds
        .iter()
        .find(|k| k.ordinal == row.kind_ordinal)?
        .sections
        .iter()
        .flat_map(|s| s.fields.iter())
        .find(|f| f.name == name)?
        .default
        .clone()
}

/// Format a numeric percentage field with an explicit non-zero sign (`"0.5"` -> `"+0.50%"`).
///
/// A value rounding to zero prints `"0.00%"`. Non-numeric or non-finite input is returned unchanged.
fn fmt_pct(v: &str) -> String {
    let raw = v.trim();
    match raw.parse::<f64>() {
        Ok(f) => moon_core::util::fmt::signed_pct(f, 2)
            .map(|(text, _)| text)
            .unwrap_or_else(|| v.to_string()),
        Err(_) => v.to_string(),
    }
}

/// Convert a `Yes` or `No` field to `ON` or `OFF`, treating missing or other values as `OFF`.
///
/// Args:
///     v: Optional strategy field value.
///
/// Returns:
///     `ON` only for case-insensitive `Yes`; otherwise `OFF`.
fn on_off(v: Option<String>) -> &'static str {
    match v {
        Some(s) if s.trim().eq_ignore_ascii_case("yes") => "ON",
        _ => "OFF",
    }
}
