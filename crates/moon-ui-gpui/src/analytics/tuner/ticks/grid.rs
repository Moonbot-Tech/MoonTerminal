//! The parameter grid of the "Entry/Exit" axis, laid out as the "By filter" grid is: a tick per
//! field that admits it to the search (the header's tick admits them all), the field's name —
//! a click selects it for "Search" on one field — the value the selected strategies hold, which
//! a click sends to В1, the variant column with its clear crosses, and the search range of each
//! number field — from, to, step and the reset (`ranges.rs`) — where the second variant column
//! stood until 2026-09-25.
//!
//! The rows come by the strategy editor's sections (`sections.rs`) — Strategy settings, Stops,
//! Sell order, SellShot, SellSpread, Delta Modifiers — each with every knob and every field a
//! strategy of the scope switches on, and a tick in its heading that admits all its knobs at
//! once. Only the knobs are live; a field the model reads but does not turn, or does not know at
//! all, is drawn greyed with the strategies' value, where Moonbot shows it. The sections the
//! model does not have at all (SellShot, SellSpread) are drawn muted, every row inactive, and
//! their heading says so; a section no row is left in is not drawn.
//!
//! The search still gates by group, Entry and Exit: a group the model does not reproduce well
//! enough (the share gate of the search settings) is not searched, and the first section holding
//! its knobs says so; where a kind in the scope has no entry model, the Entry knobs are drawn
//! fixed. A field the entry method does not read — the path-only fields under the shift — is
//! greyed out: varying it would move no column.

use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonCheckbox, MoonInput, MoonInputEvent, MoonInputState, MoonPalette, h_flex, v_flex,
};
use rust_i18n::t;

use super::super::super::AnalyticsView;
use super::super::shared::{TunerKind, collapse_caret, glyph_btn};
use super::sections::{GridSection, RowRole, layout};
use super::state::{NowValue, TicksData};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::tuner::ticks::params::{ParamGroup, ParamKind, ParamSection, TickParam};

/// Width of the strategy and variant cells, font-scaled px.
const CELL_W: f32 = 60.0;
/// Left inset of a field row, ui px: its tick sits under its section's tick, past the caret,
/// so the row reads as inside the section.
const ROW_INDENT: f32 = 30.0;
/// Preserve six caption-sized characters of the tooltip-backed field name beside ranges.
const NAME_MIN_W: f32 = 48.0;
/// The shipped row's clear control and trailing inset, in UI design pixels.
const CLEAR_W: f32 = 12.0;
const ROW_RIGHT: f32 = 8.0;
/// Five gaps separate the six cells of a row with editable ranges.
const ROW_GAP: f32 = 6.0;

/// Render-time range visibility and the active palette travel together through the grid rows.
#[derive(Clone, Copy)]
struct GridAppearance {
    ranges_visible: bool,
    palette: MoonPalette,
}
/// Id prefix of the variant cells' boxes in `TicksState::inputs`.
pub(super) const VARIANT_INPUT_PREFIX: &str = "v:";

/// Interactive field content; the renderer owns the widths of its numeric slots.
pub(super) struct FieldRowParts {
    pub checkbox: AnyElement,
    pub name: AnyElement,
    pub current: AnyElement,
    pub variant: AnyElement,
    pub clear: AnyElement,
}

/// Build the shipped field name, including its muted and selected states, without view state.
pub(super) fn field_name_el(
    key: &str,
    selected: bool,
    read: bool,
    odd: Option<ParamGroup>,
    p: MoonPalette,
) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!("an-ticks-name-{key}")))
        .flex_1()
        .min_w_0()
        .truncate()
        .cursor_pointer()
        .font_family(design::ui_font())
        .tooltip(crate::panels::common::text_tooltip(key.to_string()))
        .text_color(moon(if selected {
            p.amber
        } else if !read {
            p.text_muted
        } else if odd.is_some() {
            p.blue
        } else {
            p.text
        }))
        .when_some(odd, |el, group| {
            let locale_key = match group {
                ParamGroup::Entry => "analytics.ticks.row_odd_entry",
                ParamGroup::Exit => "analytics.ticks.row_odd_exit",
            };
            el.tooltip(crate::panels::common::text_tooltip(format!(
                "{key}\n\n{}",
                t!(locale_key)
            )))
        })
        .child(key.to_string())
}

/// Build the real current-value cell without clipping values or localized status text.
pub(super) fn current_cell_el(
    key: &str,
    read: bool,
    now: Option<&NowValue>,
    p: MoonPalette,
    cx: &App,
) -> Stateful<Div> {
    let clickable = read && matches!(now, Some(NowValue::Same(value)) if !value.is_empty());
    let (text, mono) = match now {
        _ if !read => (t!("analytics.ticks.not_read").to_string(), false),
        Some(NowValue::Same(value)) if !value.is_empty() => (value.clone(), true),
        Some(NowValue::Differs) => (t!("analytics.time.cur_varies").to_string(), false),
        _ => ("—".to_string(), true),
    };
    let measured = if mono {
        design::mono_caption_text_width(cx, &text, 400.0)
    } else {
        design::ui_caption_text_width(cx, &text, 400.0)
    };
    let selector = format!("an-ticks-current-cell-{key}");
    div()
        .id(SharedString::from(format!("an-ticks-chip-{key}")))
        .debug_selector(move || selector.clone())
        .w(px(f32::from(design::font_w_px(cx, CELL_W)).max(measured)))
        .flex_none()
        .whitespace_nowrap()
        .text_right()
        .font_family(if mono {
            design::mono()
        } else {
            design::ui_font()
        })
        .text_size(design::t_caption(cx))
        .text_color(moon(if clickable { p.amber } else { p.text_muted }))
        .when(clickable, |el| {
            el.cursor_pointer()
                .hover(move |st| st.text_color(moon(p.text)))
        })
        .child(text)
}

/// Build the shipped numeric input with its production font and density.
pub(super) fn variant_cell_el(key: &str, input: &Entity<MoonInputState>, cx: &App) -> Div {
    div()
        .w(design::font_w_px(cx, CELL_W))
        .flex_none()
        .font_family(design::mono())
        .child(
            MoonInput::new(SharedString::from(format!("an-ticks-in-v-{key}")))
                .state(input)
                .size(design::dense_input_size(cx)),
        )
}

/// Render production field slots and their optional range with a single numeric-width contract.
pub(super) fn field_row_el(
    key: &str,
    selected: bool,
    parts: FieldRowParts,
    range: Option<AnyElement>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let selector = format!("an-ticks-field-{key}");
    let cell = |role: &str, content: AnyElement| {
        let selector = format!("an-ticks-{role}-cell-{key}");
        div()
            .debug_selector(move || selector.clone())
            .w(design::font_w_px(cx, CELL_W))
            .flex_none()
            .font_family(design::mono())
            .child(content)
    };
    h_flex()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector.clone())
        .w_full()
        .px(design::ui_px(cx, ROW_RIGHT))
        .pl(design::ui_px(cx, ROW_INDENT))
        .py(design::ui_px(cx, 2.0))
        .items_center()
        .gap(design::ui_px(cx, ROW_GAP))
        .border_t_1()
        .border_color(moon_alpha(p.border, 0.5))
        .text_size(design::t_caption(cx))
        .when(selected, |row| row.bg(moon_alpha(p.amber, 0.08)))
        .child(parts.checkbox)
        .child(parts.name)
        .child(parts.current)
        .child(cell("variant", parts.variant))
        .child(parts.clear)
        .children(range)
        .into_any_element()
}

/// Minimum grid width preserving the same field slots before offering editable ranges.
pub(super) fn grid_range_min_width(cx: &App) -> Pixels {
    // MoonCheckbox's Sm box follows its 16px base, capped growth to the theme font delta.
    let tokens = moon_ui::MoonTheme::active_tokens(cx);
    let checkbox = px(tokens.ui(16.0).max(
        tokens
            .line_height(20.0)
            .min(tokens.ui(16.0) + tokens.scale.font_delta.max(0.0)),
    ));
    design::font_w_px(cx, CELL_W * 2.0 + NAME_MIN_W)
        + design::ui_px(cx, ROW_INDENT + ROW_RIGHT + ROW_GAP * 5.0 + CLEAR_W)
        + checkbox
        + super::ranges::block_w(cx)
}

/// Decide before layout whether the host preserves the field slots and editable ranges.
pub(super) fn ranges_fit(host_w: Pixels, cx: &App) -> bool {
    host_w >= grid_range_min_width(cx)
}

/// Draw the production section warning under its interactive heading, without clipping prose.
pub(super) fn section_body_el(
    header: AnyElement,
    id: &str,
    note: Option<(String, String)>,
    color: u32,
    cx: &App,
) -> AnyElement {
    v_flex()
        .w_full()
        .min_w_0()
        .child(header)
        .when_some(note, |column, (note, tip)| {
            let selector = format!("an-ticks-sec-note-{id}");
            column.child(
                div()
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector.clone())
                    .w_full()
                    .min_w_0()
                    .px(design::ui_px(cx, 12.0))
                    .py(design::ui_px(cx, 4.0))
                    .font_family(design::ui_font())
                    .text_size(design::t_caption(cx))
                    .text_color(moon(color))
                    .tooltip(crate::panels::common::text_tooltip(tip))
                    .child(note),
            )
        })
        .into_any_element()
}

impl AnalyticsView {
    /// The grid panel: its title, wrapping search row and expandable sections;
    /// sections; the side column owns their shared scrolling.
    pub(in crate::analytics::tuner) fn ticks_grid(
        &mut self,
        host_w: Pixels,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ranges_visible = ranges_fit(host_w - design::ui_px(cx, 2.0), cx);
        let appearance = GridAppearance {
            ranges_visible,
            palette: p,
        };
        let (header, summary) = self.ticks_parameters_parts(host_w, p, cx);
        let cfg_row = self.shell_config_row(TunerKind::Ticks, p, window, cx);
        let data = self.ticks.data.data().cloned();
        let schema_sig = super::sections::schema_signature(self.backend.read(cx).session.store());
        // A core's schema that arrived, changed or went after the latest load chose its keys
        // leaves the layout short of that core's fields and their values: ask the refresh gate
        // for the visible axis again, once per signature. Deferred — a load is not started from
        // inside a render.
        if self.ticks.keys_sig.is_some_and(|sig| sig != schema_sig)
            && self.ticks.schema_reload != Some(schema_sig)
        {
            self.ticks.schema_reload = Some(schema_sig);
            cx.defer_in(window, |this, _window, cx| {
                this.request_report_refresh(
                    crate::analytics::refresh::RefreshUrgency::User,
                    false,
                    cx,
                )
            });
        }
        let entry_on = data.as_ref().is_some_and(|d| d.entry_modelled());
        // Before the first load there is no layout; the bare headings stand in for it.
        let sections: Arc<[GridSection]> = match data.as_ref() {
            Some(d) if !d.grid.is_empty() => d.grid.clone(),
            _ => layout(&[], &[], &Default::default()).into(),
        };
        let live: Vec<&'static str> = sections
            .iter()
            .flat_map(GridSection::knobs)
            .filter(|k| knob_ticks(k, entry_on))
            .map(|k| k.key)
            .collect();
        let mut grid = v_flex().w_full().flex_none().child(self.ticks_grid_header(
            live,
            ranges_visible,
            p,
            cx,
        ));
        // A section left without a row — no knob of the scope's kinds, no field a strategy of it
        // switches on — is dropped once the scope is known; before that every heading stands,
        // the first carrying the scope's note.
        let scoped = data.as_ref().is_some_and(|d| !d.kinds.is_empty());
        let mut noted: Vec<ParamGroup> = Vec::new();
        let mut first = true;
        for section in sections.iter().filter(|s| !scoped || !s.rows.is_empty()) {
            grid = grid.child(self.ticks_section_header(
                section,
                data.as_deref(),
                first,
                &mut noted,
                appearance,
                cx,
            ));
            first = false;
            // A folded section keeps one row in sight: the field selected for "Search", so what
            // the button would search is never hidden.
            let open = self.ticks.open_sections.contains(&section.section);
            let sel = self.ticks.sel_field;
            // The knobs filed among the other group's are marked, so a search of one group does
            // not read as leaving the whole section alone.
            let odd = section.minority_group();
            for row in section
                .rows
                .iter()
                .filter(|r| open || sel == Some(r.key.as_str()))
            {
                let now = data.as_ref().and_then(|d| d.now.get(&row.key).cloned());
                grid = grid.child(match row.role {
                    RowRole::Knob(knob) if knob_live(knob, entry_on) => {
                        let odd = odd.filter(|group| *group == knob.group);
                        self.ticks_field_row(knob.key, odd, now, appearance, window, cx)
                    }
                    role => {
                        self.ticks_fixed_row(&row.key, role, now, data.as_deref(), appearance, cx)
                    }
                });
            }
        }
        // Coverage is useful before a search too: it explains which trades can teach it.
        let accuracy = self.ticks_accuracy_row(p, cx);
        v_flex()
            .id("an-ticks-grid")
            .w_full()
            .flex_none()
            .rounded(design::ui_px(cx, 8.0))
            .bg(moon(p.panel))
            .border_1()
            .border_color(moon(p.border))
            .overflow_hidden()
            .child(header)
            .child(cfg_row)
            .children(accuracy)
            .child(summary)
            .child(grid)
            .into_any_element()
    }

    /// Tick or untick every field the grid shows — the header's tick. Unticked is held by the
    /// search at В1's value where В1 has one, else at the strategy's, but for a value a switch
    /// the variant turns on needs (`search::deps`).
    fn ticks_set_all(&mut self, fields: &[&'static str], on: bool, cx: &mut Context<Self>) {
        for key in fields {
            if on {
                self.ticks.locked.remove(*key);
            } else {
                self.ticks.locked.insert((*key).to_string());
            }
        }
        self.persist_ticks_settings(cx);
        cx.notify();
    }

    /// Open or fold one section of the grid.
    fn ticks_toggle_section(&mut self, section: ParamSection, cx: &mut Context<Self>) {
        if !self.ticks.open_sections.remove(&section) {
            self.ticks.open_sections.insert(section);
        }
        cx.notify();
    }

    /// The state of a tick that admits all of `keys`: `(checked, indeterminate)` — every one
    /// admitted to the search, or some but not all. No keys, no tick.
    fn ticks_tick_state(&self, keys: &[&'static str]) -> (bool, bool) {
        let on = keys
            .iter()
            .filter(|k| !self.ticks.locked.contains(**k))
            .count();
        (
            !keys.is_empty() && on == keys.len(),
            on > 0 && on < keys.len(),
        )
    }

    /// The live master tick and plain field/current/V1/range headings share the field slots.
    fn ticks_grid_header(
        &self,
        keys: Vec<&'static str>,
        ranges_visible: bool,
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (all_on, some_on) = self.ticks_tick_state(&keys);
        let cell = |text: String| {
            div()
                .w(design::font_w_px(cx, CELL_W))
                .flex_none()
                .text_center()
                .truncate()
                .child(text)
        };
        let mut head = h_flex()
            .w_full()
            .px(design::ui_px(cx, 8.0))
            .h(design::fit_h_px(cx, 22.0, 12.0, 5.0))
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .text_size(design::t_caption(cx))
            .text_color(moon(p.text_soft))
            .bg(moon(p.table_head))
            .child(
                div().flex_none().child(
                    MoonCheckbox::new("an-ticks-en-all")
                        .checked(all_on)
                        .indeterminate(some_on)
                        .disabled(keys.is_empty())
                        .size(design::CONTROL_TIER)
                        .on_change({
                            let view = cx.entity();
                            let keys = keys.clone();
                            move |on: &bool, _w, app| {
                                let on = *on;
                                view.update(app, |this, cx| this.ticks_set_all(&keys, on, cx));
                            }
                        }),
                ),
            )
            // The caption toggles the lot too, as the filter grid's does — a click target rather
            // than the checkbox's own label, which would widen the checkbox and push every
            // heading after it out of line. It answers as the box does: a half-set box reads as
            // ticked, so a click on either unticks all.
            .child(
                div()
                    .id("an-ticks-en-all-lbl")
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(t!("analytics.ticks.pick.col_field").to_string())
                    .when(!keys.is_empty(), |el| {
                        el.cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.ticks_set_all(&keys, !(all_on || some_on), cx);
                            }))
                    }),
            )
            .child(cell(t!("analytics.ticks.pick.col_now").to_string()));
        head = head
            .child(cell(t!("analytics.ticks.pick.col_v1").to_string()))
            .child(
                glyph_btn(
                    "an-ticks-clr-col",
                    "✕",
                    t!("analytics.time.tip_clear_all").to_string(),
                    p.orange,
                    p,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.ticks_clear_variant(cx))),
            )
            .when(ranges_visible, |head| {
                head.child(self.ticks_range_header(cx))
            });
        head.into_any_element()
    }
}

/// What a search group's heading warns of, when it does — the kinds whose entry is taken from
/// the fact (not searched), or a share of reproduced trades under the gate (searched, but its
/// answer speaks for fewer trades) — with the colour to say it in. An unanswered tape read
/// stays neutral instead of reporting the temporary sample's reproduction share.
pub(super) fn group_note(
    state: &super::state::TicksState,
    group: ParamGroup,
    d: &TicksData,
    p: MoonPalette,
) -> Option<(String, u32)> {
    if group == ParamGroup::Entry && !d.entry_modelled() {
        return Some((
            t!(
                "analytics.ticks.entry_from_fact",
                kinds = d.unmodelled_kinds().join(", ")
            )
            .to_string(),
            p.text_muted,
        ));
    }
    let gate = state.gate();
    let (hits, n) = d.share_of(group);
    let name = match group {
        ParamGroup::Entry => t!("analytics.ticks.group_entry"),
        ParamGroup::Exit => t!("analytics.ticks.group_exit"),
    };
    if state.tape_reading {
        return Some((
            format!("{name}: {}", t!("analytics.ticks.fetch_reading")),
            p.text_muted,
        ));
    }
    // Answered, but no trade fit for the search — whatever the share and the gate: the search
    // has nothing to learn on and refuses the group (`TicksData::group_searchable`).
    if n > 0 && !d.group_searchable(group) {
        // An exit nobody could check is named for why, not left to read as every trade missed.
        let why = (group == ParamGroup::Exit)
            .then(|| d.accuracy.none_checked_note())
            .flatten()
            .map_or_else(String::new, |note| format!(" — {note}"));
        return Some((
            format!("{name}: {}{why}", t!("analytics.ticks.vary_none")),
            p.orange,
        ));
    }
    match d.group_passes(group, gate) {
        Some(false) => Some((
            format!(
                "{name}: {}",
                t!(
                    "analytics.ticks.vary_gated",
                    hits = hits,
                    n = n,
                    gate = (gate * 100.0).round() as i64
                )
            ),
            p.orange,
        )),
        None => Some((
            format!("{name}: {}", t!("analytics.ticks.vary_unknown")),
            p.text_muted,
        )),
        Some(true) => None,
    }
}

impl AnalyticsView {
    /// A section's heading: the tick admitting all its live knobs, its name as the Strategies
    /// window titles it (the human gloss under that window's own preference), and the notes of
    /// the search groups first met in it (`noted` carries the groups an earlier heading already
    /// spoke for).
    fn ticks_section_header(
        &self,
        section: &GridSection,
        data: Option<&TicksData>,
        first: bool,
        noted: &mut Vec<ParamGroup>,
        appearance: GridAppearance,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let GridAppearance {
            ranges_visible,
            palette: p,
        } = appearance;
        let title = crate::strategies::sections::section_display_title(
            section.section.schema_title(),
            crate::strategies::settings::human_labels(&self.backend.read(cx).layout),
        );
        let entry_on = data.is_some_and(|d| d.entry_modelled());
        let keys: Vec<&'static str> = section
            .knobs()
            .filter(|k| knob_ticks(k, entry_on))
            .map(|k| k.key)
            .collect();
        let (all_on, some_on) = self.ticks_tick_state(&keys);
        let mut notes: Vec<(String, u32)> = Vec::new();
        // What the heading's tooltip says in place of a note: the short "not modelled" on the
        // row, what follows from it on hover.
        let mut unmodelled_tip = None;
        let modelled = section.section.modelled();
        if !modelled {
            notes.push((
                t!("analytics.ticks.section_unmodelled").to_string(),
                p.text_muted,
            ));
            unmodelled_tip = Some(t!("analytics.ticks.section_unmodelled_tip").to_string());
        }
        match data {
            // Only a LOADED empty scope says so; a load in flight or a failed one has its own
            // note in the table.
            Some(d) if d.kinds.is_empty() => {
                if first {
                    notes.push((t!("analytics.ticks.no_deals").to_string(), p.text_muted));
                }
            }
            Some(d) => {
                for group in [ParamGroup::Entry, ParamGroup::Exit] {
                    if section.knobs().any(|k| k.group == group) && !noted.contains(&group) {
                        noted.push(group);
                        notes.extend(group_note(&self.ticks, group, d, p));
                    }
                }
            }
            None => {}
        }
        let color = if notes.iter().any(|(_, c)| *c == p.orange) {
            p.orange
        } else {
            p.text_muted
        };
        let note = (!notes.is_empty()).then(|| {
            let text = notes
                .iter()
                .map(|(text, _)| text.as_str())
                .collect::<Vec<_>>()
                .join(" · ");
            // The tooltip repeats the row with the unmodelled note spelled out: it is always
            // the first one pushed.
            let tip = match &unmodelled_tip {
                Some(long) => std::iter::once(long.as_str())
                    .chain(notes.iter().skip(1).map(|(text, _)| text.as_str()))
                    .collect::<Vec<_>>()
                    .join(" · "),
                None => text.clone(),
            };
            (text, tip)
        });
        let id = format!("{:?}", section.section);
        let which = section.section;
        let collapsed = !self.ticks.open_sections.contains(&which);
        // The section's reset takes every number knob of it back to its automatic range.
        let numbers: Vec<&'static str> = section
            .knobs()
            .filter(|k| k.kind == ParamKind::Num)
            .map(|k| k.key)
            .collect();
        let reset = self.ticks_range_section_reset(&id, numbers, cx);
        let live_keys: Vec<&str> = keys
            .iter()
            .copied()
            .filter(|key| !self.ticks.locked.contains(*key))
            .collect();
        let (changed, live) =
            super::sections::changed_in(section, &live_keys, &self.ticks.variant_changes());
        let status = if self.ticks.current_result().is_some() {
            t!("analytics.ticks.pick.sec_changed", k = changed, m = live).to_string()
        } else if live > 0 {
            t!("analytics.ticks.pick.sec_on").to_string()
        } else {
            t!("analytics.ticks.pick.sec_off").to_string()
        };
        let header = h_flex()
            .w_full()
            .px(design::ui_px(cx, 8.0))
            .py(design::ui_px(cx, 2.0))
            .gap(design::ui_px(cx, 6.0))
            .items_center()
            .bg(moon_alpha(p.table_head, 0.6))
            .border_t_1()
            .border_color(moon_alpha(p.border, 0.7))
            .text_size(design::t_caption(cx))
            .font_family(design::ui_font())
            .child(collapse_caret(
                SharedString::from(format!("an-ticks-sec-caret-{id}")),
                collapsed,
                t!("analytics.ticks.section_collapse").to_string(),
                t!("analytics.ticks.section_expand").to_string(),
                p,
                cx.listener(move |this, _, _, cx| this.ticks_toggle_section(which, cx)),
            ))
            .child(
                div().flex_none().child(
                    MoonCheckbox::new(SharedString::from(format!("an-ticks-sec-{id}")))
                        .checked(all_on)
                        .indeterminate(some_on)
                        .disabled(keys.is_empty())
                        .size(design::CONTROL_TIER)
                        .on_change({
                            let view = cx.entity();
                            let keys = keys.clone();
                            move |on: &bool, _w, app| {
                                let on = *on;
                                view.update(app, |this, cx| this.ticks_set_all(&keys, on, cx));
                            }
                        }),
                ),
            )
            // The name folds the section too, as a heading row does; the body step marks it
            // above the caption-sized rows it holds.
            .child(
                div()
                    .id(SharedString::from(format!("an-ticks-sec-title-{id}")))
                    .flex_1()
                    .min_w_0()
                    .cursor_pointer()
                    .text_size(design::t_body(cx))
                    .text_color(moon(if modelled { p.text } else { p.text_muted }))
                    .child(title)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.ticks_toggle_section(which, cx)),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(moon(p.text_muted))
                    .child(status),
            )
            .when(ranges_visible, |header| header.child(reset));
        section_body_el(header.into_any_element(), &id, note, color, cx)
    }

    /// A field the search does not turn: a greyed, disabled tick, the name with why in its
    /// tooltip, the strategies' value, and blanks where the variant cell and the range stand, so
    /// the columns stay in line.
    fn ticks_fixed_row(
        &self,
        key: &str,
        role: RowRole,
        now: Option<NowValue>,
        data: Option<&TicksData>,
        appearance: GridAppearance,
        cx: &Context<Self>,
    ) -> AnyElement {
        let GridAppearance {
            ranges_visible,
            palette: p,
        } = appearance;
        let (tip, name_color) = match role {
            RowRole::Fixed => (t!("analytics.ticks.row_fixed").to_string(), p.text_soft),
            RowRole::Outside => (t!("analytics.ticks.row_outside").to_string(), p.text_muted),
            RowRole::Unmodelled => (
                t!("analytics.ticks.row_unmodelled").to_string(),
                p.text_muted,
            ),
            // A knob of the entry group while a kind of the scope has no entry model.
            RowRole::Knob(_) => (
                t!(
                    "analytics.ticks.entry_from_fact",
                    kinds = data
                        .map(|d| d.unmodelled_kinds().join(", "))
                        .unwrap_or_default()
                )
                .to_string(),
                p.text_soft,
            ),
        };
        let value = match now {
            Some(NowValue::Same(value)) if !value.is_empty() => value,
            Some(NowValue::Differs) => t!("analytics.time.cur_varies").to_string(),
            _ => "—".to_string(),
        };
        let mut row = h_flex()
            .w_full()
            .px(design::ui_px(cx, 8.0))
            .pl(design::ui_px(cx, ROW_INDENT))
            .py(design::ui_px(cx, 2.0))
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .border_t_1()
            .border_color(moon_alpha(p.border, 0.5))
            .text_size(design::t_caption(cx))
            .child(
                div().flex_none().child(
                    MoonCheckbox::new(SharedString::from(format!("an-ticks-fx-en-{key}")))
                        .checked(false)
                        .disabled(true)
                        .size(design::CONTROL_TIER),
                ),
            )
            .child(
                div()
                    .id(SharedString::from(format!("an-ticks-fx-{key}")))
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(moon(name_color))
                    .tooltip(crate::panels::common::text_tooltip(format!(
                        "{key}\n\n{tip}"
                    )))
                    .child(key.to_string()),
            )
            .child(
                div()
                    .w(design::font_w_px(cx, CELL_W))
                    .flex_none()
                    .truncate()
                    .text_right()
                    .text_color(moon_alpha(p.text_muted, 0.8))
                    .child(value),
            );
        row = row
            .child(div().w(design::font_w_px(cx, CELL_W)).flex_none())
            .child(div().w(design::ui_px(cx, 12.0)).flex_none())
            .when(ranges_visible, |row| row.child(self.ticks_range_blank(cx)));
        row.into_any_element()
    }

    /// One field: its tick, its name, the strategies' value, the variant's input with its clear
    /// cross, and its search range — a number's; a switch or a list has no range, and a blank
    /// keeps the columns in line.
    fn ticks_field_row(
        &mut self,
        key: &'static str,
        odd: Option<ParamGroup>,
        now: Option<NowValue>,
        appearance: GridAppearance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let GridAppearance {
            ranges_visible,
            palette: p,
        } = appearance;
        let read = super::model_cfg::current().entry_method.reads(key);
        let selected = self.ticks.sel_field == Some(key);
        let on = !self.ticks.locked.contains(key);
        let input = self.ticks_cell_input(key, window, cx);
        let is_number = moon_core::db::tuner::ticks::TICK_PARAMS
            .iter()
            .any(|f| f.key == key && f.kind == ParamKind::Num);
        let parts = FieldRowParts {
            checkbox: (div().flex_none().child(
                MoonCheckbox::new(SharedString::from(format!("an-ticks-en-{key}")))
                    .checked(on && read)
                    .disabled(!read)
                    .size(design::CONTROL_TIER)
                    .on_change({
                        let view = cx.entity();
                        move |on: &bool, _w, app| {
                            let on = *on;
                            view.update(app, |this, cx| {
                                if on {
                                    this.ticks.locked.remove(key);
                                } else {
                                    this.ticks.locked.insert(key.to_string());
                                }
                                this.persist_ticks_settings(cx);
                                cx.notify();
                            });
                        }
                    }),
            ))
            .into_any_element(),
            name: (field_name_el(key, selected, read, odd, p).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.ticks.sel_field = Some(key);
                    cx.notify();
                },
            )))
            .into_any_element(),
            current: current_cell_el(key, read, now.as_ref(), p, cx)
                .when(read, |el| {
                    el.when_some(
                        now.and_then(|now| match now {
                            NowValue::Same(value) if !value.is_empty() => Some(value),
                            _ => None,
                        }),
                        |el, value| {
                            el.on_click(cx.listener(move |this, _, _, cx| {
                                this.ticks.locked.insert(key.to_string());
                                this.persist_ticks_settings(cx);
                                this.ticks_set_cell(key, value.clone(), cx);
                            }))
                        },
                    )
                })
                .into_any_element(),
            variant: variant_cell_el(key, &input, cx).into_any_element(),
            clear: (glyph_btn(
                SharedString::from(format!("an-ticks-clr-{key}")),
                "✕",
                t!("analytics.time.tip_clear").to_string(),
                p.orange,
                p,
                cx,
            )
            .on_click(
                cx.listener(move |this, _, _, cx| this.ticks_set_cell(key, String::new(), cx)),
            ))
            .into_any_element(),
        };
        let range = ranges_visible.then(|| {
            if is_number {
                self.ticks_range_cells(key, p, window, cx)
            } else {
                self.ticks_range_blank(cx)
            }
        });
        field_row_el(key, selected, parts, range, p, cx)
    }

    /// Set one variant cell from outside its box — the strategy chip, a row's cross — and have
    /// the box show it.
    fn ticks_set_cell(&mut self, key: &str, value: String, cx: &mut Context<Self>) {
        self.set_ticks_variant(key, value, cx);
        // The box is recreated from the stored value on the next frame.
        self.ticks
            .inputs
            .remove(&format!("{VARIANT_INPUT_PREFIX}{key}"));
        cx.notify();
    }

    /// The input box of one variant cell, created on first use from the stored value and
    /// kept across repaints; a change stores the value and rescores the columns.
    fn ticks_cell_input(
        &mut self,
        key: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        let id = format!("{VARIANT_INPUT_PREFIX}{key}");
        if let Some(state) = self.ticks.inputs.get(&id) {
            return state.clone();
        }
        let value = self.ticks.variant.get(key).cloned().unwrap_or_default();
        let state = cx.new(|cx| MoonInputState::new(window, cx).default_value(value));
        cx.subscribe_in(
            &state,
            window,
            move |this, state, ev: &MoonInputEvent, _window, cx| {
                if matches!(
                    ev,
                    MoonInputEvent::Change
                        | MoonInputEvent::Blur
                        | MoonInputEvent::PressEnter { .. }
                ) {
                    let value = state.read(cx).value().to_string();
                    if this.ticks.variant.get(key).map(String::as_str) != Some(value.as_str()) {
                        this.set_ticks_variant(key, value, cx);
                    }
                    if !matches!(ev, MoonInputEvent::Change) {
                        cx.notify();
                    }
                }
            },
        )
        .detach();
        self.ticks.inputs.insert(id, state.clone());
        state
    }
}

/// Whether a knob is drawn live: an Entry knob only while every kind of the scope has an entry
/// model, as the search varies it only then.
fn knob_live(knob: &TickParam, entry_on: bool) -> bool {
    knob.group != ParamGroup::Entry || entry_on
}

/// Whether a section's or the header's tick counts and toggles a knob: a live one the entry
/// method reads. A field it does not read is drawn unticked whatever `locked` says, and counting
/// it would leave the tick half-set with every visible box ticked.
pub(super) fn knob_ticks(knob: &TickParam, entry_on: bool) -> bool {
    knob_live(knob, entry_on) && super::model_cfg::current().entry_method.reads(knob.key)
}
