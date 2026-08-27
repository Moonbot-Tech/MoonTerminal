//! The chart-caption catalogue as a PICKER: every field a caption can print, in columns.
//!
//! Shared because two surfaces ask the same question and must not drift — "which figure does this
//! caption print" and "which figure does the caption I am adding print". What differs is what
//! happens to the pick, which is the callback; never the list.
//!
//! A grid rather than a menu, and that is a decision the list forced: the catalogue is fifty-one
//! figures in ten sections, and `MoonDropdown` draws ONE column with no hover-opened submenus —
//! its nested level opens only when the parent row is explicitly `selected`, which needs state the
//! menu does not keep. A column per section shows the whole catalogue at once, which is also how a
//! reader picks from it: they know the subject before they know the figure.

use gpui::*;
use moon_core::config::{ChartLabelField, ChartLabelGroup, ChartLabelRow};
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonListItem, MoonPalette, MoonPopover,
    MoonPopoverPlacement, MoonSeparator, h_flex, v_flex,
};
use rust_i18n::t;

use crate::design::{self, moon};

/// Width of the check column on a field row, in design pixels.
///
/// Held whether or not the field is checked, which is the point: a mark that pushed its own label
/// sideways would make the checked rows the only ones that do not line up with the rest.
const CHECK_W: f32 = 14.0;

/// Width of one column, in design pixels.
///
/// Sized on the longest localized field name rather than guessed: the names are the only thing in
/// the column, and a column narrower than its widest name truncates every row under it.
const COLUMN_W: f32 = 178.0;

/// Tallest a column may get, counted in ROWS — a section heading counts as one.
///
/// The picker is a popover over a chart, and a column taller than this turns it into a half-screen
/// wall: the first version put one section per column, and the section holding twenty-one figures
/// set the height of all five. Sections are packed into columns instead, several to a column, each
/// keeping its own heading — which is the only thing that makes a long list readable at all.
///
/// Sixteen is where the ten sections settle into five columns, the tallest of them exactly sixteen
/// rows; twelve splits them further, and twenty puts twenty rows in one. Worth re-checking whenever
/// a section is added — the number is a result, not a preference, and the tallest column is now
/// flush against it.
const MAX_COLUMN_ROWS: usize = 16;

/// Lay the sections out into columns, keeping each section whole and its heading with it.
///
/// Greedy and deliberately simple: sections are placed in catalogue order, and a section that does
/// not fit the current column opens the next one. Order is worth more than perfect balance here —
/// a reader looks for "the price ones" where the catalogue says they are, not where a packing
/// algorithm decided to move them.
///
/// A section taller than the limit takes a column of its own rather than being split: a heading
/// repeated over half a list names something the reader cannot see the rest of.
fn pack_columns(sections: &[(ChartLabelGroup, Vec<ChartLabelField>)]) -> Vec<Vec<usize>> {
    let mut columns: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut rows = 0usize;
    for (ix, (_, fields)) in sections.iter().enumerate() {
        let needed = fields.len() + 1;
        if !current.is_empty() && rows + needed > MAX_COLUMN_ROWS {
            columns.push(std::mem::take(&mut current));
            rows = 0;
        }
        current.push(ix);
        rows += needed;
    }
    if !current.is_empty() {
        columns.push(current);
    }
    columns
}

/// What a module is CALLED, in the reader's language.
///
/// One rule in one place, because three surfaces ask it and must not drift: the chart, when the
/// module prints its own name; the popup's module line; and the editor's title and preview. The
/// user's own name wins, a preset's name is looked up every time it is printed — which is what
/// makes it follow a live language switch — and a module that has neither has no title at all.
pub(crate) fn row_title(row: &ChartLabelRow) -> Option<String> {
    if !row.name.is_empty() {
        return Some(row.name.clone());
    }
    row.title_key().map(|key| t!(key).to_string())
}

/// What the module LIST calls this module: its title, or the captions it prints, WHOLE.
///
/// A module is not required to have a title — most never get one — so the list falls back to
/// naming it by what it does. An empty module with no title says so rather than showing a blank
/// line.
///
/// The composition is returned COMPLETE, and that is the contract: this function no longer decides
/// how much of it a reader sees. It used to keep two field names and append " · …", which was a
/// truncation taken blind — it counted PARTS while the thing that runs out is PIXELS, so it threw
/// away the third name on a line that had room for it and still overflowed on two long ones. What
/// fits is a question about the box the text is drawn in, so the caller that owns that box answers
/// it by measuring (`labels_popup::fit_row_name`), and a caller with no box — the editor's window
/// title — simply prints the whole thing.
pub(crate) fn row_display_name(row: &ChartLabelRow) -> String {
    if let Some(title) = row_title(row) {
        return title;
    }
    let used = row.used_parts();
    if used == 0 {
        return t!("chart_labels.row_empty").to_string();
    }
    let mut out = String::new();
    for part in &row.parts[..used] {
        if !out.is_empty() {
            out.push_str(" · ");
        }
        out.push_str(&t!(part.field.locale_key()));
    }
    out
}

/// The catalogue as a popover: a trigger showing `label`, and the whole grid under it.
///
/// Args:
///     id: Element-id prefix, so two pickers alive at once keep distinct identities.
///     label: What the trigger shows — the current field, or the "add a caption" wording.
///     marked: Whether a field is already configured, for its mark. The picker MARKS rather than
///         disables: the same figure on two modules is a legitimate layout, and the mark answers
///         "did I already add this?", not "you may not".
///     disabled: Whether the trigger refuses to open — a module with no room left, where the
///         catalogue would take a pick and silently drop it.
///     open: Whether the grid is up. CONTROLLED by the caller, because the pick has to survive the
///         close: `close_on_content_click` shuts the popover on mouse-DOWN through a deferred
///         update, and the button underneath is gone by the time the click would have fired — which
///         is why picking a field only closed the list.
///     on_open: Told when the trigger asks to open or the overlay asks to close.
///     on_pick: Receives the chosen field. The caller closes the picker from here.
#[allow(clippy::too_many_arguments)]
pub(crate) fn field_picker(
    id: &str,
    label: String,
    disabled: bool,
    open: bool,
    on_open: impl Fn(bool, &mut App) + 'static,
    marked: impl Fn(ChartLabelField) -> bool + 'static,
    on_pick: impl Fn(ChartLabelField, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    let sections: Vec<(ChartLabelGroup, Vec<ChartLabelField>)> = ChartLabelGroup::ALL
        .into_iter()
        .map(|group| {
            let fields = ChartLabelField::ALL
                .into_iter()
                .filter(|f| f.group() == group)
                .collect();
            (group, fields)
        })
        .collect();
    let mut grid = h_flex().gap(design::ui_px(cx, 12.0)).items_start();
    for column_sections in pack_columns(&sections) {
        let mut column = v_flex()
            .w(px(design::font_w(cx, COLUMN_W)))
            .gap(design::ui_px(cx, 1.0));
        for (n, section_ix) in column_sections.into_iter().enumerate() {
            let (group, fields) = &sections[section_ix];
            // A RULE above every heading but the column's first, and the heading itself in the
            // body colour rather than the muted one. Two sections in one column read as one long
            // list otherwise — a word in the middle of it is not a boundary anybody sees.
            if n > 0 {
                column = column.child(
                    div()
                        .pt(design::ui_px(cx, 8.0))
                        .pb(design::ui_px(cx, 4.0))
                        .child(MoonSeparator::horizontal().color(p.border)),
                );
            }
            column = column.child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text))
                    .font_weight(FontWeight::SEMIBOLD)
                    .pb(design::ui_px(cx, 2.0))
                    .pl(design::ui_px(cx, 4.0))
                    .child(t!(group.locale_key()).to_string()),
            );
            for field in fields.iter().copied() {
                let on_pick = on_pick.clone();
                let checked = marked(field);
                // A LIST ROW, not a button: the catalogue is a list, and the row has to put its
                // label at the same left edge whether or not it carries a mark. `MoonListItem`
                // brings the hover and selected surfaces with it, so nothing here paints its own.
                column = column.child(
                    MoonListItem::new(SharedString::from(format!("{id}-{field:?}")))
                        .selected(checked)
                        .on_click(move |_, window: &mut Window, app: &mut App| {
                            on_pick(field, window, app)
                        })
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .gap(design::ui_px(cx, 4.0))
                                .child(
                                    div()
                                        .w(px(design::font_w(cx, CHECK_W)))
                                        .flex_none()
                                        .text_color(moon(p.text_muted))
                                        .child(match checked {
                                            true => "✓",
                                            false => "",
                                        }),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(t!(field.locale_key()).to_string()),
                                ),
                        ),
                );
            }
        }
        grid = grid.child(column);
    }

    MoonPopover::new(SharedString::from(format!("{id}-picker")))
        .trigger(
            MoonButton::new(SharedString::from(format!("{id}-trigger")))
                // The caret is part of the LABEL: `MoonButton` has no disclosure of its own, and a
                // trigger with no hint that something opens under it reads as a dead button.
                .label(format!("{label}  ▾"))
                .size(MoonButtonSize::Micro)
                .variant(MoonButtonVariant::Soft)
                .disabled(disabled)
                .render(),
        )
        .disabled(disabled)
        .placement(MoonPopoverPlacement::BottomStart)
        .open(open)
        .on_open_change(move |open, _window, cx| on_open(open, cx))
        // Deliberately NOT `close_on_content_click`: that closes on mouse-down, before the click
        // reaches the field under the cursor. The pick itself closes the picker, from the caller.
        .close_on_content_click(false)
        .fit_content()
        .content(grid)
}
