//! One editable coin-list field and the "Not saved" band, as FREE render functions over a
//! plain model plus handler closures.
//!
//! Free functions, not `AnalyticsView` methods, so a layout test can render them in a probe
//! view with no-op handlers; the view supplies handlers that edit its own state.
//!
//! MoonUI has no token input, so the field is composed of its parts: a chip per coin with a
//! bare `MoonButton` for remove / undo, and a trailing `MoonInput` for typing and pasting.

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonVariant, MoonInput, MoonInputState, MoonPalette, h_flex, v_flex,
};
use rust_i18n::t;

use super::state::PendingDelta;
use crate::design;
use crate::design::{moon, moon_alpha};

/// Ceiling on how many chips a field draws.
///
/// Counted in ENTRIES, not coins — a list may spell one coin several ways. Measured on the
/// real database: the biggest single list is 474 and the union over every strategy is 819,
/// so this clears both. Past it the field states how many it left out.
pub(in crate::analytics::tuner) const MAX_CHIPS: usize = 1000;

/// A handler acting on one coin token.
pub(in crate::analytics::tuner) type TokenAct = Rc<dyn Fn(&SharedString, &mut Window, &mut App)>;
/// A handler acting on a whole field.
pub(in crate::analytics::tuner) type Act = Rc<dyn Fn(&mut Window, &mut App)>;

/// How a chip relates to the saved list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::analytics::tuner) enum ChipKind {
    /// Saved and still listed.
    Saved,
    /// Listed but not saved yet.
    Added,
    /// Saved but removed by the edit — written out on Save.
    Removed,
}

/// One chip, built once per list revision.
#[derive(Clone, Debug)]
pub(in crate::analytics::tuner) struct FieldChip {
    /// The matching token the remove / undo handlers receive.
    pub(in crate::analytics::tuner) token: SharedString,
    /// The entry as written.
    pub(in crate::analytics::tuner) text: SharedString,
    pub(in crate::analytics::tuner) tip: SharedString,
    pub(in crate::analytics::tuner) id: SharedString,
    pub(in crate::analytics::tuner) kind: ChipKind,
    /// Text opacity of a saved chip: its age on the list's brightness ramp.
    pub(in crate::analytics::tuner) alpha: f32,
}

/// Everything one field draws.
pub(in crate::analytics::tuner) struct ListFieldModel {
    pub(in crate::analytics::tuner) black: bool,
    pub(in crate::analytics::tuner) title: String,
    /// The strategy parameter the field is written to.
    pub(in crate::analytics::tuner) key: &'static str,
    /// Coins in the working list; `None` without a strategy.
    pub(in crate::analytics::tuner) count: Option<usize>,
    pub(in crate::analytics::tuner) chips: Vec<FieldChip>,
    /// Chips left out past [`MAX_CHIPS`].
    pub(in crate::analytics::tuner) more: usize,
    /// The sentence shown instead of chips.
    pub(in crate::analytics::tuner) empty: Option<String>,
    pub(in crate::analytics::tuner) legend: Vec<String>,
    pub(in crate::analytics::tuner) can_copy: bool,
    pub(in crate::analytics::tuner) can_clear: bool,
}

impl ListFieldModel {
    /// Build one field's model.
    ///
    /// Args:
    ///     black: The blacklist field when `true`, the whitelist otherwise.
    ///     chips: The field's chips; `None` when no strategy is selected.
    ///
    /// Returns:
    ///     The model with its count, empty sentence, legend and button states.
    pub(in crate::analytics::tuner) fn new(black: bool, chips: Option<Vec<FieldChip>>) -> Self {
        let selected = chips.is_some();
        let mut chips = chips.unwrap_or_default();
        // Counted in COINS: `BTC, BTC_0626` are two chips of one listed coin.
        let count = selected.then(|| {
            chips
                .iter()
                .filter(|c| c.kind != ChipKind::Removed)
                .map(|c| c.token.as_ref())
                .collect::<std::collections::HashSet<_>>()
                .len()
        });
        let more = chips.len().saturating_sub(MAX_CHIPS);
        chips.truncate(MAX_CHIPS);
        let empty = if !selected {
            Some(t!("analytics.coins.no_strategy").to_string())
        } else if chips.is_empty() {
            Some(t!("analytics.coins.field_empty").to_string())
        } else {
            None
        };
        let legend = if black {
            vec![
                t!("analytics.coins.legend_bright").to_string(),
                t!("analytics.coins.legend_hover").to_string(),
                t!("analytics.coins.legend_paste").to_string(),
            ]
        } else {
            vec![t!("analytics.coins.white_empty_hint").to_string()]
        };
        let listed = count.unwrap_or(0) > 0;
        Self {
            black,
            title: if black {
                t!("analytics.coins.black_title").to_string()
            } else {
                t!("analytics.coins.white_title").to_string()
            },
            key: if black {
                super::save::FIELD
            } else {
                super::save::WHITE_FIELD
            },
            count,
            chips,
            more,
            empty,
            legend,
            can_copy: listed,
            can_clear: listed,
        }
    }
}

/// The handlers of one field.
pub(in crate::analytics::tuner) struct ListFieldActs {
    pub(in crate::analytics::tuner) on_remove: TokenAct,
    pub(in crate::analytics::tuner) on_restore: TokenAct,
    pub(in crate::analytics::tuner) on_copy: Act,
    pub(in crate::analytics::tuner) on_clear: Act,
}

/// The "Not saved" text: each side's additions and removals, signed and sorted.
///
/// Returns:
///     `None` when nothing is pending, so the band is not drawn.
pub(in crate::analytics::tuner) fn pending_text(d: &PendingDelta) -> Option<String> {
    let side = |label: String, added: &[String], removed: &[String]| {
        (!added.is_empty() || !removed.is_empty()).then(|| {
            let signed = added
                .iter()
                .map(|t| format!("+{t}"))
                .chain(removed.iter().map(|t| format!("\u{2212}{t}")))
                .collect::<Vec<_>>()
                .join(" ");
            format!("{label} {signed}")
        })
    };
    let parts: Vec<String> = [
        side(
            t!("analytics.coins.black").to_string(),
            &d.added_bl,
            &d.removed_bl,
        ),
        side(
            t!("analytics.coins.white").to_string(),
            &d.added_wl,
            &d.removed_wl,
        ),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// One field: header with title, parameter key + count, Copy and Clear; the chip well with
/// its typing input; the legend.
///
/// Args:
///     m: The field's model.
///     input: The typing field; `None` without a strategy (nothing to edit).
///     acts: Remove / restore / copy / clear handlers.
///     p: Theme palette.
///     cx: App, for scale and text metrics.
///
/// Returns:
///     The rendered field.
pub(in crate::analytics::tuner) fn list_field_el(
    m: &ListFieldModel,
    input: Option<&Entity<MoonInputState>>,
    acts: &ListFieldActs,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let side = if m.black { "bl" } else { "wl" };
    let key = match m.count {
        Some(n) => format!("{} · {n}", m.key),
        None => m.key.to_string(),
    };
    let button = |id: String, label: String, tip: String, enabled: bool, act: Act| {
        let el = MoonButton::new(SharedString::from(id.clone()))
            .variant(MoonButtonVariant::Soft)
            .xsmall()
            .label(label)
            .tooltip(tip)
            .disabled(!enabled)
            .on_click(move |_, window, app| act(window, app))
            .render();
        super::layout::tagged(id, el)
    };
    let header = h_flex()
        .w_full()
        .flex_wrap()
        .items_center()
        .gap(design::ui_px(cx, 8.0))
        .font_family(design::ui_font())
        .child(
            div()
                .flex_none()
                .text_size(design::t_body(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .debug_selector(move || format!("an-coin-{side}-title"))
                .child(m.title.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(design::mono())
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .child(key),
        )
        .child(button(
            format!("an-coin-{side}-copy"),
            t!("analytics.coins.copy").to_string(),
            t!("analytics.coins.tip_copy").to_string(),
            m.can_copy,
            acts.on_copy.clone(),
        ))
        .child(button(
            format!("an-coin-{side}-clear"),
            t!("analytics.coins.clear").to_string(),
            t!("analytics.coins.tip_clear").to_string(),
            m.can_clear,
            acts.on_clear.clone(),
        ));
    let mut items: Vec<AnyElement> = m.chips.iter().map(|c| chip_el(c, acts, p, cx)).collect();
    if m.more > 0 {
        items.push(
            div()
                .flex_none()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .child(t!("analytics.coins.more", n = m.more).to_string())
                .into_any_element(),
        );
    }
    if let Some(empty) = &m.empty {
        items.push(
            div()
                .w_full()
                .font_family(design::ui_font())
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .child(empty.clone())
                .into_any_element(),
        );
    }
    if let Some(input) = input {
        items.push(
            div()
                .id(SharedString::from(format!("an-coin-{side}-input")))
                .flex_1()
                .min_w(design::font_w_px(cx, 90.0))
                .tooltip(crate::panels::common::text_tooltip(
                    t!("analytics.coins.tip_input").to_string(),
                ))
                .child(
                    MoonInput::new(SharedString::from(format!("an-coin-{side}-in")))
                        .state(input)
                        .size(design::INPUT_SIZE),
                )
                .into_any_element(),
        );
    }
    // The well grows with its chips up to a cap, then scrolls inside; the column around it
    // scrolls as a whole.
    let well = div()
        .id(SharedString::from(format!("an-coin-{side}-well")))
        .w_full()
        .min_h(design::ui_px(cx, 78.0))
        .max_h(design::ui_px(cx, 180.0))
        .overflow_y_scroll()
        .rounded(design::ui_px(cx, 4.0))
        .bg(moon(p.shell))
        .border_1()
        .border_color(moon(p.border_soft))
        .p(design::ui_px(cx, 6.0))
        .child(
            h_flex()
                .w_full()
                .flex_wrap()
                .items_center()
                .gap(design::ui_px(cx, 4.0))
                .children(items),
        );
    let legend = h_flex()
        .w_full()
        .flex_wrap()
        .gap(design::ui_px(cx, 10.0))
        .font_family(design::ui_font())
        .text_size(design::t_caption(cx))
        .text_color(moon(p.text_muted))
        .children(m.legend.iter().map(|l| div().child(l.clone())));
    v_flex()
        .id(SharedString::from(format!("an-coin-{side}-field")))
        .debug_selector(move || format!("an-coin-{side}-field"))
        .w_full()
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 10.0))
        .gap(design::ui_px(cx, 6.0))
        .when(!m.black, |el| el.border_t_1().border_color(moon(p.border)))
        .child(header)
        .child(well)
        .child(legend)
        .into_any_element()
}

/// One chip with a measured text basis: native ellipsis has no reliable intrinsic flex width.
fn chip_el(c: &FieldChip, acts: &ListFieldActs, p: MoonPalette, cx: &App) -> AnyElement {
    let (bg, fg) = match c.kind {
        ChipKind::Saved => (moon(p.shell_high), moon_alpha(p.text, c.alpha)),
        ChipKind::Added => (moon_alpha(p.green, 0.16), moon(p.green_text)),
        ChipKind::Removed => (moon_alpha(p.red, 0.16), moon(p.red_text)),
    };
    let label = match c.kind {
        ChipKind::Added => format!("+{}", c.text),
        _ => c.text.to_string(),
    };
    let removed = c.kind == ChipKind::Removed;
    let token = c.token.clone();
    let act = if removed {
        acts.on_restore.clone()
    } else {
        acts.on_remove.clone()
    };
    // The whole entry leads the tooltip: a long one is cut in the chip itself.
    let tip = if c.tip.is_empty() {
        c.text.clone()
    } else if c.tip.contains(c.text.as_ref()) {
        c.tip.clone()
    } else {
        SharedString::from(format!("{}\n{}", c.text, c.tip))
    };
    h_flex()
        .flex_none()
        // Capped at the well: a long entry truncates instead of pushing its x out of the
        // field.
        .max_w_full()
        .min_w_0()
        .items_center()
        .gap(design::ui_px(cx, 2.0))
        .pl(design::ui_px(cx, 6.0))
        .rounded(design::ui_px(cx, 3.0))
        .bg(bg)
        .debug_selector({
            let sel = format!("{}-chip", c.id);
            move || sel
        })
        .child({
            let name_el = div()
                .id(c.id.clone())
                .w(px(design::mono_caption_text_width(cx, &label, 400.0).ceil()))
                .min_w_0()
                .truncate()
                .debug_selector({
                    let sel = c.id.to_string();
                    move || sel
                })
                .font_family(design::mono())
                .text_size(design::t_caption(cx))
                .text_color(fg)
                .when(removed, |el| el.line_through());
            // An empty `c.tip` is the coin token alone. Anything else is a sentence.
            let name_el = if c.tip.is_empty() {
                name_el.tooltip(crate::panels::common::text_tooltip_value(tip))
            } else {
                name_el.tooltip(crate::panels::common::text_tooltip(tip))
            };
            name_el.child(label)
        })
        .child(super::layout::tagged(
            format!("{}-x", c.id),
            MoonButton::new(SharedString::from(format!("{}-x", c.id)))
                .variant(MoonButtonVariant::Bare)
                .xsmall()
                .label(if removed { "\u{21ba}" } else { "\u{00d7}" })
                .tooltip(if removed {
                    t!("analytics.coins.tip_restore").to_string()
                } else {
                    t!("analytics.coins.tip_remove").to_string()
                })
                .on_click(move |_, window, app| act(&token, window, app))
                .render(),
        ))
        .into_any_element()
}

/// The "Not saved: ..." band with its Revert edits button.
///
/// Args:
///     text: The signed delta from [`pending_text`].
///     on_revert: Throws the edit away.
///     p: Theme palette.
///     cx: App, for scale.
///
/// Returns:
///     The rendered band.
pub(in crate::analytics::tuner) fn pending_band_el(
    text: String,
    on_revert: Act,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    h_flex()
        .id("an-coin-pending")
        .debug_selector(|| "an-coin-pending".into())
        .w_full()
        .flex_wrap()
        .items_center()
        .gap(design::ui_px(cx, 8.0))
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 8.0))
        .border_t_1()
        .border_color(moon(p.border))
        .bg(moon_alpha(p.amber, 0.12))
        .text_size(design::t_caption(cx))
        .text_color(moon(p.amber))
        .child(
            div()
                .flex_none()
                .font_family(design::ui_font())
                .debug_selector(|| "an-coin-pending-lbl".into())
                .child(t!("analytics.coins.pending_band").to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .font_family(design::mono())
                .child(text),
        )
        .child(super::layout::tagged(
            "an-coin-revert",
            MoonButton::new("an-coin-revert")
                .variant(MoonButtonVariant::Soft)
                .xsmall()
                .label(t!("analytics.coins.revert_edits").to_string())
                .tooltip(t!("analytics.coins.tip_revert_edits").to_string())
                .on_click(move |_, window, app| on_revert(window, app))
                .render(),
        ))
        .into_any_element()
}

// Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
// shadows the built-in attribute and makes `#[test]` expand recursively.
#[cfg(test)]
mod tests;
