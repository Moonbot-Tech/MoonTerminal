//! The search ranges of the "Entry/Exit" grid: "from", "to" and "step" beside each number knob,
//! and the square reset that takes a row, a section or the whole grid back to automatic.
//!
//! An empty cell is automatic, and shows what the search takes there greyed in as its
//! placeholder: the range the live strategies and the selection give the field
//! (`moon_core::db::tuner::ticks::params::range`), or — where another cell of the row is typed —
//! what that typed cell makes of it (a typed "to" cuts a new step out of the same steps per field).
//! A typed value is the user's and stays until the reset; the typed ranges persist with the
//! axis' settings (`WindowLayout::analytics_ticks`). A typed range the search cannot use —
//! "from" above "to", a step of zero, more points than a field may have, a step alone on a field
//! with no automatic range to cut it over — has its row framed
//! red, and the search takes the automatic range and says so in its status line.
//!
//! Nothing here is scored: a range moves only what the NEXT search tries (`variants.rs` resolves
//! the grids at its start), never a column or the table.

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonIconSlot, MoonButtonVariant, MoonInput, MoonInputEvent, MoonInputState,
    MoonPalette, h_flex,
};
use rust_i18n::t;

use super::super::super::AnalyticsView;
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::tuner::ticks::params::range::{
    Grids, RangeError, Resolved, TickRange, resolve, spell_number,
};
use moon_core::db::tuner::ticks::{ParamKind, TICK_PARAMS};

/// Width of one range cell, font-scaled px: five characters of a caption-sized mono figure
/// (`-0.75`, `0.001`, `1800`).
const RANGE_W: f32 = 40.0;
/// Gap between the range cells, ui px — tighter than the row's, so the three read as one group.
const RANGE_GAP: f32 = 3.0;
/// Id prefix of the range cells' boxes in `TicksState::inputs`.
const RANGE_INPUT_PREFIX: &str = "r:";
/// The reset's icon: the undo arrow of the embedded MoonUI set — "back to what it was".
const RESET_ICON: &str = "icons/undo-2.svg";

/// One cell of a range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    From,
    To,
    Step,
}

impl Slot {
    const ALL: [Slot; 3] = [Slot::From, Slot::To, Slot::Step];

    fn id(self) -> &'static str {
        match self {
            Slot::From => "from",
            Slot::To => "to",
            Slot::Step => "step",
        }
    }

    fn of(self, range: &TickRange) -> Option<f64> {
        match self {
            Slot::From => range.from,
            Slot::To => range.to,
            Slot::Step => range.step,
        }
    }

    fn set(self, range: &mut TickRange, value: Option<f64>) {
        match self {
            Slot::From => range.from = value,
            Slot::To => range.to = value,
            Slot::Step => range.step = value,
        }
    }

    /// The heading of the cell's column.
    fn title(self) -> String {
        match self {
            Slot::From => t!("analytics.ticks.range_from"),
            Slot::To => t!("analytics.ticks.range_to"),
            Slot::Step => t!("analytics.ticks.range_step"),
        }
        .to_string()
    }
}

/// A typed cell's number: empty is automatic, a comma reads as the decimal point; `Err` for text
/// that is not a finite number.
fn parse_cell(text: &str) -> Result<Option<f64>, ()> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    text.replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .map(Some)
        .ok_or(())
}

/// Width of the whole range block — three cells, their gaps and the reset — so a row without a
/// range and a heading keep the columns in line.
fn block_w(cx: &App) -> Pixels {
    design::font_w_px(cx, RANGE_W) * 3.0
        + design::ui_px(cx, RANGE_GAP) * 3.0
        + px(design::dense_glyph_btn_w(cx))
}

impl AnalyticsView {
    /// One field's grid as the search would take it now: its typed range over the automatic
    /// one, under the steps-per-field setting.
    fn ticks_resolved(&self, key: &str) -> Resolved {
        let data = self.ticks.data.data();
        let typed = self.ticks.ranges.get(key).copied().unwrap_or_default();
        resolve(
            data.and_then(|d| d.spans.get(key)),
            &typed,
            data.is_some_and(|d| d.integers.contains(key)),
            self.ticks.steps_per_param(),
        )
    }

    /// Every number knob's grid for a search, resolved from the ranges as they stand, and the
    /// keys whose typed range was set aside for the automatic one. A field with no point — one
    /// nothing is known of — gets no grid, and the search does not vary it.
    pub(super) fn ticks_search_grids(&self) -> (Grids, Vec<&'static str>) {
        let mut grids = Grids::default();
        let mut set_aside = Vec::new();
        for field in TICK_PARAMS.iter().filter(|f| f.kind == ParamKind::Num) {
            let resolved = self.ticks_resolved(field.key);
            if resolved.error.is_some() && !set_aside.contains(&field.key) {
                set_aside.push(field.key);
            }
            if !resolved.points.is_empty() {
                grids.insert(field.key, resolved.points);
            }
        }
        (grids, set_aside)
    }

    /// A number knob's range: the three cells, framed red while the typed range is set aside,
    /// and the row's reset.
    pub(super) fn ticks_range_cells(
        &mut self,
        key: &'static str,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let resolved = self.ticks_resolved(key);
        let typed = self.ticks.ranges.get(key).copied().unwrap_or_default();
        let shown = resolved.shown;
        let mut cells = h_flex()
            .id(SharedString::from(format!("an-ticks-range-{key}")))
            .flex_none()
            .items_center()
            .gap(design::ui_px(cx, RANGE_GAP))
            .tooltip(crate::panels::common::text_tooltip(range_tip(
                &resolved,
                self.ticks
                    .data
                    .data()
                    .is_some_and(|d| d.integers.contains(key)),
            )));
        for slot in Slot::ALL {
            // What an empty cell stands for: the value the search takes there.
            let placeholder = match (shown, slot) {
                (None, _) => "—".to_string(),
                (Some(s), Slot::From) => spell_number(s.from),
                (Some(s), Slot::To) => spell_number(s.to),
                (Some(s), Slot::Step) if s.step > 0.0 => spell_number(s.step),
                (Some(_), Slot::Step) => "—".to_string(),
            };
            let input = self.ticks_range_input(key, slot, &typed, placeholder, window, cx);
            let bad = parse_cell(input.read(cx).value().as_ref()).is_err();
            cells = cells.child(
                div()
                    .w(design::font_w_px(cx, RANGE_W))
                    .flex_none()
                    .font_family(design::mono())
                    .rounded(design::ui_px(cx, 3.0))
                    .border_1()
                    .border_color(if bad || resolved.error.is_some() {
                        moon(p.red)
                    } else {
                        moon_alpha(p.border, 0.0)
                    })
                    .child(
                        MoonInput::new(SharedString::from(format!(
                            "an-ticks-range-in-{}-{key}",
                            slot.id()
                        )))
                        .state(&input)
                        .size(design::dense_input_size(cx)),
                    ),
            );
        }
        cells
            .child(self.ticks_reset_button(
                SharedString::from(format!("an-ticks-range-reset-{key}")),
                vec![key],
                t!("analytics.ticks.range_reset").to_string(),
                cx,
            ))
            .into_any_element()
    }

    /// The blank a row without a range keeps where the range block stands.
    pub(super) fn ticks_range_blank(&self, cx: &App) -> AnyElement {
        div().w(block_w(cx)).flex_none().into_any_element()
    }

    /// The range columns' headings and the reset of every range, for the grid's header row.
    pub(super) fn ticks_range_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let keys: Vec<&'static str> = TICK_PARAMS
            .iter()
            .filter(|f| f.kind == ParamKind::Num)
            .map(|f| f.key)
            .collect();
        let mut head = h_flex()
            .flex_none()
            .items_center()
            .gap(design::ui_px(cx, RANGE_GAP));
        for slot in Slot::ALL {
            head = head.child(
                div()
                    .w(design::font_w_px(cx, RANGE_W))
                    .flex_none()
                    .text_center()
                    .truncate()
                    .child(slot.title()),
            );
        }
        head.child(self.ticks_reset_button(
            "an-ticks-range-reset-all".into(),
            keys,
            t!("analytics.ticks.range_reset_all").to_string(),
            cx,
        ))
        .into_any_element()
    }

    /// A section heading's reset of its knobs' ranges, right-aligned under the range column.
    pub(super) fn ticks_range_section_reset(
        &self,
        id: &str,
        keys: Vec<&'static str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .w(block_w(cx))
            .flex_none()
            .justify_end()
            .child(self.ticks_reset_button(
                SharedString::from(format!("an-ticks-range-reset-sec-{id}")),
                keys,
                t!("analytics.ticks.range_reset_section").to_string(),
                cx,
            ))
            .into_any_element()
    }

    /// The square reset of `keys`' ranges, muted and inert while none of them is typed.
    fn ticks_reset_button(
        &self,
        id: SharedString,
        keys: Vec<&'static str>,
        tip: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let typed = keys.iter().any(|k| self.ticks.ranges.contains_key(*k));
        MoonButton::new(id)
            .size(design::dense_glyph_btn_size())
            .width(design::dense_glyph_btn_w(cx))
            .variant(MoonButtonVariant::Ghost)
            .leading_icon(MoonButtonIconSlot::new(RESET_ICON))
            .tooltip(tip)
            .disabled(!typed)
            .on_click(cx.listener(move |this, _, _, cx| this.ticks_reset_ranges(&keys, cx)))
            .render()
            .into_any_element()
    }

    /// Take `keys` back to their automatic ranges.
    fn ticks_reset_ranges(&mut self, keys: &[&'static str], cx: &mut Context<Self>) {
        let mut changed = false;
        for key in keys {
            changed |= self.ticks.ranges.remove(*key).is_some();
            for slot in Slot::ALL {
                let id = range_input_id(key, slot);
                self.ticks.inputs.remove(&id);
                self.ticks.placeholders.remove(&id);
            }
        }
        if changed {
            self.persist_ticks_settings(cx);
        }
        cx.notify();
    }

    /// The box of one range cell, created on first use from the typed value and kept across
    /// repaints; its placeholder follows what the search takes there. A change stores the value
    /// at once — a search started before the box loses focus takes it — and the box's leaving or
    /// Enter writes the settings.
    fn ticks_range_input(
        &mut self,
        key: &'static str,
        slot: Slot,
        typed: &TickRange,
        placeholder: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        let id = range_input_id(key, slot);
        let state = match self.ticks.inputs.get(&id) {
            Some(state) => state.clone(),
            None => {
                let value = slot.of(typed).map(spell_number).unwrap_or_default();
                let state = cx.new(|cx| MoonInputState::new(window, cx).default_value(value));
                cx.subscribe_in(
                    &state,
                    window,
                    move |this, state, ev: &MoonInputEvent, _window, cx| {
                        if !matches!(
                            ev,
                            MoonInputEvent::Change
                                | MoonInputEvent::Blur
                                | MoonInputEvent::PressEnter { .. }
                        ) {
                            return;
                        }
                        // Text that is not a number keeps the slot as it was: the red frame says
                        // so, and the search does not read half a keystroke.
                        if let Ok(value) = parse_cell(state.read(cx).value().as_ref()) {
                            let mut range = this.ticks.ranges.get(key).copied().unwrap_or_default();
                            slot.set(&mut range, value);
                            if range.is_auto() {
                                this.ticks.ranges.remove(key);
                            } else {
                                this.ticks.ranges.insert(key.to_string(), range);
                            }
                        }
                        if !matches!(ev, MoonInputEvent::Change) {
                            this.persist_ticks_settings(cx);
                        }
                        cx.notify();
                    },
                )
                .detach();
                self.ticks.inputs.insert(id.clone(), state.clone());
                state
            }
        };
        // Set only when it moved: the box repaints on every set.
        if self.ticks.placeholders.get(&id).map(String::as_str) != Some(placeholder.as_str()) {
            state.update(cx, |state, cx| {
                state.set_placeholder(placeholder.clone(), window, cx)
            });
            self.ticks.placeholders.insert(id, placeholder);
        }
        state
    }
}

/// The key a range cell's box is kept under in `TicksState::inputs`.
fn range_input_id(key: &str, slot: Slot) -> String {
    format!("{RANGE_INPUT_PREFIX}{}:{key}", slot.id())
}

/// The tooltip over a row's range: how many values the search tries, why a typed range is set
/// aside when it is, and — on a field the schema types as an integer — that typed values are
/// rounded to whole ones, so a box showing 1.5 is not taken for what the search tries.
fn range_tip(resolved: &Resolved, integer: bool) -> String {
    let mut count = t!("analytics.ticks.range_points", n = resolved.points.len()).to_string();
    if integer {
        count = format!("{count} · {}", t!("analytics.ticks.range_integer"));
    }
    let why = resolved.error.map(|error| match error {
        RangeError::Inverted => t!("analytics.ticks.range_err_inverted"),
        RangeError::BadStep => t!("analytics.ticks.range_err_step"),
        RangeError::TooMany => t!(
            "analytics.ticks.range_err_many",
            n = moon_core::db::tuner::ticks::params::range::MAX_STEPS
        ),
        RangeError::NoEdges => t!("analytics.ticks.range_err_edges"),
    });
    match (resolved.shown, why) {
        (None, Some(why)) => format!("{why} · {}", t!("analytics.ticks.range_no_data")),
        (None, None) => t!("analytics.ticks.range_no_data").to_string(),
        (Some(_), Some(why)) => format!("{why} · {count}"),
        (Some(_), None) => format!("{count} · {}", t!("analytics.ticks.range_hint")),
    }
}

#[cfg(test)]
mod tests;
