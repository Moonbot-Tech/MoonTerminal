//! Window-lifetime numeric conditions, evaluated from the strategy cells' displayed values.

use gpui::*;
use moon_core::db::analytics::GroupStat;
use moon_ui::{
    MoonBadge, MoonButton, MoonButtonSegment, MoonButtonVariant, MoonDropdown, MoonInput,
    MoonInputEvent, MoonInputState, MoonMenuItem, MoonPalette, MoonPopover, MoonPopoverPlacement,
    MoonTone, h_flex, v_flex,
};
use rust_i18n::t;
use std::collections::HashMap;

use super::super::super::AnalyticsView;
use super::METRIC_COLS;
use crate::design::{self, moon};

/// Only the six metrics shared by the strategy-list modes are offered.
const COLUMNS: [usize; 6] = [0, 1, 2, 3, 5, 6];

/// Numeric comparison chosen by the user; equality compares the displayed precision.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::analytics) enum Operator {
    #[default]
    Greater,
    AtLeast,
    Less,
    AtMost,
    Equal,
}

impl Operator {
    /// Mathematical labels are independent of the UI language.
    fn label(self) -> &'static str {
        match self {
            Self::Greater => ">",
            Self::AtLeast => "≥",
            Self::Less => "<",
            Self::AtMost => "≤",
            Self::Equal => "=",
        }
    }

    /// Apply the operator without a tolerance that could disagree with the visible number.
    fn keeps(self, value: f64, threshold: f64) -> bool {
        match self {
            Self::Greater => value > threshold,
            Self::AtLeast => value >= threshold,
            Self::Less => value < threshold,
            Self::AtMost => value <= threshold,
            Self::Equal => value == threshold,
        }
    }
}

/// The memo key contains editable text too, so invalid-to-valid edits always invalidate it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::analytics) struct Condition {
    pub(super) id: u64,
    pub(super) column: usize,
    pub(super) operator: Operator,
    pub(super) number: String,
}

impl Condition {
    /// Empty drafts stay neutral; only a non-empty malformed number needs correction.
    fn invalid(&self) -> bool {
        !self.number.trim().is_empty() && number(&self.number).is_none()
    }
}

/// Parse a decimal with either separator; partial edits and non-finite numbers are invalid.
pub(super) fn number(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty()
        || !text
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | ','))
    {
        return None;
    }
    text.replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
}

/// Read the exact cell formatter, including its rounding, quote denomination and percent lens.
/// Unknown money remains unavailable rather than being mistaken for zero.
fn cell_value(group: &GroupStat, column: usize) -> Option<f64> {
    let text = (METRIC_COLS[column].text)(group);
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if compact == "inf" {
        return Some(f64::INFINITY);
    }
    let prefix: String = compact
        .chars()
        .take_while(|c| c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | ','))
        .collect();
    number(&prefix)
}

/// Invalid conditions are ignored individually; every valid condition must match.
pub(super) fn keeps(group: &GroupStat, conditions: &[Condition]) -> bool {
    conditions.iter().all(|condition| {
        number(&condition.number).is_none_or(|threshold| {
            cell_value(group, condition.column)
                .is_some_and(|value| condition.operator.keeps(value, threshold))
        })
    })
}

/// Shared by all four modes and deliberately absent from layout/config serialization.
#[derive(Default)]
pub(in crate::analytics) struct Conditions {
    pub(super) rows: Vec<Condition>,
    inputs: HashMap<u64, Entity<MoonInputState>>,
    open: bool,
    next_id: u64,
}

impl Conditions {
    /// Forget retained input entities when their rows disappear, without reusing element ids.
    fn remove(&mut self, id: u64) {
        self.rows.retain(|row| row.id != id);
        self.inputs.remove(&id);
    }

    /// Clear the filter while keeping the editor's open state and monotonic ids.
    fn clear(&mut self) {
        self.rows.clear();
        self.inputs.clear();
    }

    /// Count only conditions that currently affect filtering.
    pub(super) fn applied(&self) -> usize {
        self.active().count()
    }

    /// Applied captions and the count share the same valid-number set as filtering.
    fn active(&self) -> impl Iterator<Item = &Condition> {
        self.rows.iter().filter(|row| number(&row.number).is_some())
    }

    /// A valid condition marks its own metric header even when another row is invalid.
    pub(super) fn marks(&self, key: &str) -> bool {
        self.active().any(|row| METRIC_COLS[row.column].key == key)
    }
}

/// Shared editor geometry: the probe draws these exact controls and the invalid-number hint.
fn condition_row(
    column: impl IntoElement,
    operator: impl IntoElement,
    input: impl IntoElement,
    remove: impl IntoElement,
    invalid: bool,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    v_flex()
        .debug_selector(|| "an-condition-editor-row".into())
        .gap(design::ui_px(cx, 2.0))
        .child(
            h_flex()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .child(column)
                .child(operator)
                .child(div().w(design::ui_px(cx, 100.0)).child(input))
                .child(remove),
        )
        .children(invalid.then(|| {
            div()
                .debug_selector(|| "an-condition-invalid".into())
                .whitespace_nowrap()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.red_text))
                .child(t!("analytics.conditions.invalid").to_string())
        }))
        .into_any_element()
}

/// The live count stays readable between the reset and completion controls.
fn conditions_footer(
    reset: impl IntoElement,
    caption: String,
    done: impl IntoElement,
    cx: &App,
) -> AnyElement {
    h_flex()
        .debug_selector(|| "an-condition-footer".into())
        .gap(design::ui_px(cx, 10.0))
        .items_center()
        .child(reset)
        .child(
            div()
                .debug_selector(|| "condition-count".into())
                .whitespace_nowrap()
                .child(caption),
        )
        .child(done)
        .into_any_element()
}

/// Keep arbitrary numeric drafts and currency lists inside a chip; its tooltip retains all text.
fn condition_chip(
    id: u64,
    caption: String,
    edit: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    remove: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let (label, _) = design::fit_text(&caption, f32::from(design::font_w_px(cx, 240.0)), |text| {
        design::ui_body_text_width(cx, text, 400.0)
    });
    h_flex()
        .debug_selector(move || format!("condition-chip-{id}"))
        .min_w_0()
        .child(
            MoonButton::new(SharedString::from(format!("condition-chip-edit-{id}")))
                .label(label)
                .tooltip(caption)
                .variant(MoonButtonVariant::Soft)
                .on_click(edit)
                .render(),
        )
        .child(
            MoonButton::new(SharedString::from(format!("condition-chip-remove-{id}")))
                .label("×")
                .tooltip(t!("analytics.conditions.remove").to_string())
                .on_click(remove)
                .render(),
        )
        .into_any_element()
}

/// Chips wrap independently so adding a condition does not create a horizontal scrollbar.
fn chips_row(chips: Vec<AnyElement>, reset: impl IntoElement, cx: &App) -> AnyElement {
    h_flex()
        .debug_selector(|| "an-condition-chips".into())
        .w_full()
        .flex_none()
        .flex_wrap()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .px(design::ui_px(cx, 8.0))
        .py(design::ui_px(cx, 4.0))
        .children(chips)
        .child(reset)
        .into_any_element()
}

impl AnalyticsView {
    /// One caption for both the chip and empty result, with the cell's current currency unit.
    fn condition_caption(&self, row: &Condition) -> String {
        let units = if matches!(row.column, 1 | 3) {
            let mut units: Vec<String> = self
                .strategy_data
                .data()
                .into_iter()
                .flat_map(|data| data.strategies.iter())
                .filter_map(|g| {
                    let text = (METRIC_COLS[row.column].text)(g);
                    if text.ends_with('%') {
                        Some("%".to_string())
                    } else {
                        text.split_whitespace()
                            .last()
                            .filter(|v| v.chars().all(|c| c.is_ascii_alphabetic()))
                            .map(str::to_string)
                    }
                })
                .collect();
            units.sort();
            units.dedup();
            units.join("/")
        } else {
            String::new()
        };
        format!(
            "{} {} {}{}{}",
            t!(METRIC_COLS[row.column].key),
            row.operator.label(),
            row.number,
            if units.is_empty() { "" } else { " " },
            units
        )
    }

    /// Only applied conditions make chips; drafts remain available in the editor.
    pub(super) fn condition_chips(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if self.strat_conditions.applied() == 0 {
            return None;
        }
        let mut chips = Vec::new();
        for row in self.strat_conditions.active() {
            let id = row.id;
            chips.push(condition_chip(
                id,
                self.condition_caption(row),
                cx.listener(|this, _, _, cx| {
                    this.strat_conditions.open = true;
                    cx.notify();
                }),
                cx.listener(move |this, _, _, cx| {
                    this.strat_conditions.remove(id);
                    cx.notify();
                }),
                cx,
            ));
        }
        Some(chips_row(
            chips,
            MoonButton::new("condition-chips-reset")
                .label(t!("analytics.conditions.reset").to_string())
                .variant(MoonButtonVariant::Ghost)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.strat_conditions.clear();
                    cx.notify();
                }))
                .render(),
            cx,
        ))
    }

    /// Explain the empty condition result using the same captions as the applied chips.
    pub(super) fn conditions_empty(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        let text = self
            .strat_conditions
            .active()
            .map(|r| self.condition_caption(r))
            .collect::<Vec<_>>()
            .join(" · ");
        v_flex()
            .w_full()
            .p(design::ui_px(cx, 18.0))
            .gap(design::ui_px(cx, 8.0))
            .items_center()
            .font_family(design::ui_font())
            .text_color(moon(p.text_muted))
            .child(t!("analytics.conditions.empty").to_string())
            .child(
                div()
                    .id("condition-empty-summary")
                    .w_full()
                    .truncate()
                    .tooltip(crate::panels::common::text_tooltip(text.clone()))
                    .child(text),
            )
            .child(
                MoonButton::new("condition-empty-reset")
                    .label(t!("analytics.conditions.reset_conditions").to_string())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.strat_conditions.clear();
                        cx.notify();
                    }))
                    .render(),
            )
            .into_any_element()
    }

    /// Controlled MoonUI popover; nested dropdowns keep it open until Done or the trigger.
    pub(super) fn conditions_control(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.strat_conditions.open;
        let applied = self.strat_conditions.applied();
        let weak = cx.entity().downgrade();
        let mut popup = MoonPopover::new("an-conditions-popup")
            .placement(MoonPopoverPlacement::BottomStart)
            .fit_content()
            .open(open)
            .close_on_content_click(false)
            .overlay_closable(false)
            .on_open_change(move |open, _, app| {
                let _ = weak.update(app, |this, cx| {
                    this.strat_conditions.open = open;
                    cx.notify();
                });
            })
            .trigger(
                h_flex()
                    .items_center()
                    .gap(design::ui_px(cx, 3.0))
                    .child(
                        MoonButton::new("an-conditions-trigger")
                            .variant(if applied > 0 {
                                MoonButtonVariant::Amber
                            } else {
                                MoonButtonVariant::Soft
                            })
                            .selected(applied > 0)
                            .label(t!("analytics.conditions.title").to_string())
                            .render(),
                    )
                    .children((applied > 0).then(|| MoonBadge::new(applied.to_string()).render())),
            );
        if open {
            popup = popup.content(self.conditions_content(p, window, cx));
        }
        popup.into_any_element()
    }

    /// Build retained numeric inputs and live filter counts inside MoonUI's popup chrome.
    fn conditions_content(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut body = v_flex()
            .id("an-conditions-body")
            .debug_selector(|| "an-conditions-body".into())
            .min_w_0()
            .gap(design::ui_px(cx, 8.0))
            .font_family(design::ui_font())
            .child(t!("analytics.conditions.title").to_string())
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .child(t!("analytics.conditions.all").to_string()),
            );
        let mut rows = v_flex()
            .id("an-condition-editor-rows")
            .max_h(design::ui_px(cx, 300.0))
            .overflow_y_scroll()
            .gap(design::ui_px(cx, 6.0));
        for row in self.strat_conditions.rows.clone() {
            let id = row.id;
            let input = if let Some(state) = self.strat_conditions.inputs.get(&id) {
                state.clone()
            } else {
                let value = row.number.clone();
                let state = cx.new(|cx| MoonInputState::new(window, cx).default_value(value));
                cx.subscribe(&state, move |this, state, event: &MoonInputEvent, cx| {
                    if matches!(event, MoonInputEvent::Change)
                        && let Some(row) =
                            this.strat_conditions.rows.iter_mut().find(|r| r.id == id)
                    {
                        row.number = state.read(cx).value().to_string();
                        cx.notify();
                    }
                })
                .detach();
                self.strat_conditions.inputs.insert(id, state.clone());
                state
            };
            let mut columns = MoonDropdown::new(format!("condition-column-{id}"))
                .label(t!(METRIC_COLS[row.column].key).to_string())
                .trigger_variant(MoonButtonVariant::Soft)
                .fit_menu_width(100.0, 420.0);
            for column in COLUMNS {
                columns = columns.item(
                    MoonMenuItem::with_key(
                        format!("column-{column}"),
                        t!(METRIC_COLS[column].key).to_string(),
                    )
                    .checked(row.column == column)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(row) =
                            this.strat_conditions.rows.iter_mut().find(|r| r.id == id)
                        {
                            row.column = column;
                        }
                        cx.notify();
                    })),
                );
            }
            let mut operators = MoonDropdown::new(format!("condition-operator-{id}"))
                .label(row.operator.label())
                .trigger_variant(MoonButtonVariant::Soft);
            for operator in [
                Operator::Greater,
                Operator::AtLeast,
                Operator::Less,
                Operator::AtMost,
                Operator::Equal,
            ] {
                operators = operators.item(
                    MoonMenuItem::with_key(operator.label(), operator.label())
                        .checked(row.operator == operator)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(row) =
                                this.strat_conditions.rows.iter_mut().find(|r| r.id == id)
                            {
                                row.operator = operator;
                            }
                            cx.notify();
                        })),
                );
            }
            let invalid = row.invalid();
            rows = rows.child(condition_row(
                columns,
                operators,
                MoonInput::new(format!("condition-number-{id}"))
                    .state(&input)
                    .size(design::INPUT_SIZE)
                    .tone(if invalid {
                        MoonTone::Danger
                    } else {
                        MoonTone::Default
                    }),
                MoonButton::new(SharedString::from(format!("condition-remove-{id}")))
                    .label("×")
                    .tooltip(t!("analytics.conditions.remove").to_string())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.strat_conditions.remove(id);
                        cx.notify();
                    }))
                    .render(),
                invalid,
                p,
                cx,
            ));
        }
        body = body.child(rows).child(
            MoonButton::new("condition-add")
                .segment(MoonButtonSegment::new("+"))
                .label(t!("analytics.conditions.add").to_string())
                .variant(MoonButtonVariant::Soft)
                .on_click(cx.listener(|this, _, _, cx| {
                    let state = &mut this.strat_conditions;
                    let id = state.next_id;
                    state.next_id += 1;
                    state.rows.push(Condition {
                        id,
                        column: 0,
                        operator: Operator::Greater,
                        number: String::new(),
                    });
                    cx.notify();
                }))
                .render(),
        );
        let (matched, total) = self
            .strategy_data
            .data()
            .map(|data| {
                let key = self.strat_visible_key(&data.strategies);
                let matched = super::filter_sort_indices(&data.strategies, &key).0.len();
                let mut without = key;
                without.conditions.clear();
                (
                    matched,
                    super::filter_sort_indices(&data.strategies, &without)
                        .0
                        .len(),
                )
            })
            .unwrap_or_default();
        body.child(conditions_footer(
            MoonButton::new("condition-reset-all")
                .label(t!("analytics.conditions.reset_all").to_string())
                .variant(MoonButtonVariant::Ghost)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.strat_conditions.clear();
                    cx.notify();
                }))
                .render(),
            t!("analytics.conditions.matches", n = matched, m = total).to_string(),
            MoonButton::new("condition-done")
                .label(t!("analytics.conditions.done").to_string())
                .on_click(cx.listener(|this, _, window, cx| {
                    window.blur();
                    this.strat_conditions.open = false;
                    cx.notify();
                }))
                .render(),
            cx,
        ))
        .into_any_element()
    }
}

#[cfg(test)]
mod tests;
