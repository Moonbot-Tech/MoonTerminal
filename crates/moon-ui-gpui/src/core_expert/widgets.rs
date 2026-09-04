//! Controls the expert window's pages are built from.
//!
//! Moonbot's pages are dense: a checkbox whose own caption carries the value, a slider under it,
//! groups drawn as titled frames. These helpers reproduce that shape once so a page reads as a list
//! of Moonbot rows rather than as layout.
//!
//! A page draws EVERY row Moonbot has — including the ones whose values this terminal does not
//! carry. A row it cannot fill is drawn in full and disabled, never hidden: a trader compares this
//! window against Moonbot's, and a missing row reads as a bug where a dead one reads as a limit. So
//! most helpers take an `enabled` flag. The `_live` ones do not: they exist for the rows this
//! terminal CAN fill. Where a page needs the same control dead it reaches for the plain helper
//! beside it — and where no plain helper is left, that is because no page needs one.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonDropdown,
    MoonGroupBox, MoonInput, MoonLink, MoonMenuItem, MoonMenuSize, MoonPalette, MoonRadio,
    MoonRadioSize, MoonSlider, MoonStepper, MoonStepperSize, MoonText, h_flex, v_flex,
};

use moon_core::feed::CoreConfig;

use crate::design;
use crate::panels::popup_group;
use crate::shell::editors::EditorStore;

use super::CoreExpertView;

/// A titled frame, as Moonbot draws its groups.
///
/// The settings-popup frame, not one of this window's own: both faces of the gear draw the same
/// Moonbot groups, and a second builder is how their padding and fill would come to differ.
pub(super) fn group(id: &'static str, title: String) -> MoonGroupBox {
    popup_group(id, title)
}

/// One checkbox row that stages into the page.
///
/// Moonbot puts the VALUE in the caption of the row that controls it ("Take Profit: [buy] +5.0%"),
/// so the caller formats the whole line; this only owns the box and the write.
pub(super) fn flag(
    id: &'static str,
    label: String,
    checked: bool,
    enabled: bool,
    view: &Entity<CoreExpertView>,
    set: fn(&mut CoreConfig, bool),
) -> impl IntoElement {
    let view = view.clone();
    MoonCheckbox::new(SharedString::from(id))
        .label(label)
        .checked(checked)
        .disabled(!enabled)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| {
            let on = *ch;
            view.update(app, |this, cx| {
                this.edit_draft(|draft| set(draft, on), cx);
            });
        })
}

/// One label, sized to its own text.
///
/// Deliberately NOT full width: Moonbot writes a sentence with its boxes inside it ("Стоп если …
/// [0.10$] за [5] сделок"), and a label that claimed the whole row would push every box in that
/// sentence to the far edge.
pub(super) fn caption(text: String, enabled: bool, p: MoonPalette, cx: &App) -> impl IntoElement {
    text_at(
        text,
        if enabled { p.text } else { p.text_muted },
        design::t_body(cx),
        false,
        cx,
    )
}

/// A smaller, quieter caption — Moonbot's explanatory lines under a group's title.
pub(super) fn hint(text: String, p: MoonPalette, cx: &App) -> impl IntoElement {
    text_at(text, p.text_soft, design::t_caption(cx), false, cx)
}

/// One label's worth of text, at one of the theme's steps.
///
/// `flex_none` so a row of label-box-label keeps Moonbot's spacing instead of the labels absorbing
/// the row; paragraphs that DO want the width go through [`text_block`].
fn text_at(text: String, color: u32, size: Pixels, bold: bool, cx: &App) -> impl IntoElement {
    div().flex_none().child(
        MoonText::new(text)
            .color(color)
            .font_size(f32::from(size))
            .line_height(f32::from(design::line_px(cx, 15.0)))
            .weight(if bold { 600.0 } else { 400.0 })
            .uppercase(false)
            .render(),
    )
}

/// A paragraph: the running text Moonbot prints under a group or beside a warning, which wraps and
/// takes the width it is given.
pub(super) fn text_block(text: String, color: u32, bold: bool, cx: &App) -> impl IntoElement {
    div().w_full().min_w_0().child(
        MoonText::new(text)
            .color(color)
            .font_size(f32::from(design::t_body(cx)))
            .line_height(f32::from(design::line_px(cx, 15.0)))
            .weight(if bold { 600.0 } else { 400.0 })
            .uppercase(false)
            .wrap()
            .render(),
    )
}

/// The slider under a row, or nothing when the page never declared it.
///
/// The id is given ONCE and answers for both the store key and the element identity: written twice,
/// a typo in either would silently drop the row's control — and on a dead row nobody would see it.
pub(super) fn slider(
    store: &EditorStore,
    id: &'static str,
    enabled: bool,
) -> Option<impl IntoElement> {
    let state = store.slider(id)?;
    Some(
        div().w_full().child(
            MoonSlider::new(&state)
                .id(id)
                .height(18.0)
                .disabled(!enabled),
        ),
    )
}

/// A full-width text field, or nothing when the page never declared it.
pub(super) fn field(
    store: &EditorStore,
    id: &'static str,
    enabled: bool,
) -> Option<impl IntoElement> {
    field_masked(store, id, enabled, false)
}

/// The same field, optionally masked — Moonbot's password boxes show dots, and a mirrored page that
/// showed characters would suggest it holds the secret it cannot have.
pub(super) fn field_masked(
    store: &EditorStore,
    id: &'static str,
    enabled: bool,
    masked: bool,
) -> Option<impl IntoElement> {
    let state = store.input(id)?;
    Some(
        div().w_full().child(
            MoonInput::new(id)
                .state(&state)
                .small()
                .disabled(!enabled)
                .when(masked, |input| input.mask_toggle()),
        ),
    )
}

/// One of Moonbot's action buttons. Every one on a page this terminal only mirrors is dead: the
/// action belongs to that process, not to a value we could send.
pub(super) fn action(id: &'static str, label: String, enabled: bool) -> impl IntoElement {
    MoonButton::new(id)
        .label(label)
        .size(MoonButtonSize::Action)
        .variant(MoonButtonVariant::Soft)
        .disabled(!enabled)
        .padding_x(14.0)
        .render()
}

/// The two-column body Moonbot's wider pages use, with its columns kept independent so a long left
/// column cannot push the right one off the page.
pub(super) fn columns(
    left: impl IntoElement,
    right: impl IntoElement,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .items_start()
        .gap(design::ui_px(cx, 12.0))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(design::ui_px(cx, 8.0))
                .child(left),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(design::ui_px(cx, 8.0))
                .child(right),
        )
}

/// A column of rows at the page's own vertical rhythm.
pub(super) fn rows(cx: &App) -> Div {
    v_flex().w_full().gap(design::ui_px(cx, 6.0))
}

/// A control under its own label, the shape Moonbot uses wherever a field is not self-describing.
///
/// The control is optional for the same reason [`field`] returns an option: a page that asks for a
/// control it never declared draws the label alone rather than panicking mid-frame.
pub(super) fn labeled(
    label: String,
    control: Option<impl IntoElement>,
    enabled: bool,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 3.0))
        .child(caption(label, enabled, p, cx))
        .children(control)
}

/// One line of running text on a page — Moonbot's coloured status lines and the short sentences it
/// sets beside a control. Sized to its text like [`caption`], so a row can hold it next to a box;
/// the paragraphs that need the width and wrap are [`text_block`].
pub(super) fn text_line(text: String, color: u32, bold: bool, cx: &App) -> impl IntoElement {
    text_at(text, color, design::t_body(cx), bold, cx)
}

/// One of Moonbot's links. Dead like the rest of a mirrored page: the destination belongs to that
/// process, and this window has no browser of its own to open it with.
pub(super) fn link(id: &'static str, label: String, enabled: bool) -> impl IntoElement {
    MoonLink::new(id, label).disabled(!enabled).underline(true)
}

/// A selector showing one value. Moonbot fills these from its own machine state, so a mirrored page
/// shows the value and refuses the menu.
pub(super) fn dropdown(id: &'static str, current: String, enabled: bool) -> impl IntoElement {
    MoonDropdown::new(id)
        .label(current)
        .items(Vec::<MoonMenuItem>::new())
        .trigger_variant(MoonButtonVariant::Soft)
        .trigger_size(MoonButtonSize::Action)
        .disabled(!enabled)
}

/// One option of a Moonbot radio group that stages into the page.
///
/// The write takes the whole GROUP, not this option's own flag: Moonbot's control is exclusive
/// while the wire stores the choice as several independent bools, so picking one has to set the
/// others in the same packet — the shape `apply_leverage` uses for isolated-versus-cross.
///
/// A click on the option ALREADY selected stages nothing. `MoonRadio` fires on every click, and
/// without this the packet would rewrite flags the trader never touched: the wire's factory default
/// holds a combination the exclusive control cannot express, and re-picking the option on screen
/// would silently normalise it.
pub(super) fn radio_live(
    id: &'static str,
    label: String,
    selected: bool,
    view: &Entity<CoreExpertView>,
    set: fn(&mut CoreConfig),
) -> impl IntoElement {
    let view = view.clone();
    MoonRadio::new(id)
        .label(label)
        .checked(selected)
        .size(MoonRadioSize::Compact)
        .on_change(move |_, _w, app| {
            if selected {
                return;
            }
            view.update(app, |this, cx| {
                this.edit_draft(set, cx);
            });
        })
}

/// Moonbot's `< n >` spinner, for the counts it does not give a slider, staging into the page.
///
/// Deliberately given NO range: the component clamps the value it DISPLAYS into the range, while
/// the draft keeps the core's own number, so a range here would show one number and send another —
/// and the arrows would then step from the displayed one, so a single click on a core value outside
/// it would discard that value. The wire states no bound for either of these widths, and inventing
/// one is what produced the mismatch. `floor` is the one thing that is not a guess: every count
/// drawn this way — a pixel width, a number of words, an alert level — is meaningless below zero,
/// and the wire states no upper bound for any of them.
pub(super) fn stepper_live(
    id: &'static str,
    value: i32,
    floor: i32,
    view: &Entity<CoreExpertView>,
    set: fn(&mut CoreConfig, i32),
) -> impl IntoElement {
    let view = view.clone();
    MoonStepper::new(id)
        .value(value as f32)
        .step(1.0)
        .precision(0)
        .size(MoonStepperSize::Compact)
        .on_change(move |v, _w, app| {
            // `as i32` saturates rather than wrapping.
            let next = (v.round() as i32).max(floor);
            // Without a range the component never dims its "−", so pressing it at the floor arrives
            // here as a change to the value already held. Staging that would mark the page edited
            // and stop the window following the core, for a press that moved nothing.
            if next == value {
                return;
            }
            view.update(app, |this, cx| {
                this.edit_draft(|draft| set(draft, next), cx);
            });
        })
}

/// A bordered list of plain lines — Moonbot's channel box and the other places it shows a set it
/// owns.
///
/// A framed column rather than `MoonList`: that component carries a delegate, a selection model and
/// virtualization for lists the user drives, and none of it applies to a box this window can only
/// mirror. An empty box says so in words instead of leaving a blank frame.
pub(super) fn list_box(
    id: &'static str,
    lines: Vec<String>,
    empty_note: String,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    let empty = lines.is_empty();
    v_flex()
        .id(id)
        .w_full()
        .flex_1()
        .min_h(design::ui_px(cx, 120.0))
        .gap(design::ui_px(cx, 2.0))
        .p(design::ui_px(cx, 6.0))
        .rounded(design::r_button(cx))
        .border_1()
        .border_color(rgb(p.border))
        .overflow_y_scroll()
        .when(empty, |this| this.child(hint(empty_note, p, cx)))
        .children(lines.into_iter().map(|line| caption(line, false, p, cx)))
}

/// The same framed list, with one row pickable.
///
/// Still not `MoonList`, for the reason [`list_box`] gives: this is a handful of lines, not a
/// virtualized list with a delegate. The pick lives in the WINDOW — a page is rebuilt every render
/// — so the row hands the index straight to it.
pub(super) fn list_box_select(
    id: &'static str,
    lines: Vec<String>,
    selected: Option<usize>,
    empty_note: String,
    view: &Entity<CoreExpertView>,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    let empty = lines.is_empty();
    v_flex()
        .id(id)
        .w_full()
        .flex_1()
        .min_h(design::ui_px(cx, 120.0))
        .gap(design::ui_px(cx, 2.0))
        .p(design::ui_px(cx, 6.0))
        .rounded(design::r_button(cx))
        .border_1()
        .border_color(rgb(p.border))
        .overflow_y_scroll()
        .when(empty, |this| this.child(hint(empty_note, p, cx)))
        .children(lines.into_iter().enumerate().map(|(row, line)| {
            let view = view.clone();
            let picked = selected == Some(row);
            // Keyed by BOTH the row and the line: the line alone collides when the core carries
            // the same channel twice, and the row alone would let GPUI carry one id's interaction
            // state onto a different channel after a removal.
            div()
                .id(SharedString::from(format!("{id}-{row}-{line}")))
                .w_full()
                .px(design::ui_px(cx, 4.0))
                .rounded(design::r_button(cx))
                .overflow_hidden()
                .cursor_pointer()
                // The theme's own selected-row fill: `row_alt` is the zebra stripe, a shade off the
                // panel, and would leave the pick invisible in the light theme.
                .when(picked, |this| this.bg(rgb(p.table_selected)))
                .child(caption(line.clone(), true, p, cx))
                .on_click(move |_, _w, app| {
                    view.update(app, |this, cx| {
                        // Clicking the picked row clears the pick, which is how a list with no
                        // other way to deselect gives one back.
                        this.set_selected_channel((!picked).then_some(row), cx);
                    });
                })
        }))
}

/// One of Moonbot's action buttons, with something behind it.
pub(super) fn action_live(
    id: &'static str,
    label: String,
    enabled: bool,
    on_click: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    MoonButton::new(id)
        .label(label)
        .size(MoonButtonSize::Action)
        .variant(MoonButtonVariant::Soft)
        .disabled(!enabled)
        .padding_x(14.0)
        .on_click(move |_, _w, app| on_click(app))
        .render()
}

/// Side of the sound-preview square, in font-scaled units: the height of the dropdown beside it, so
/// the pair reads as one control. The same value the compact popup uses.
const SOUND_PLAY_SIDE: f32 = 22.0;

/// A narrow numeric box, the shape Moonbot puts inline in a sentence ("... за [5] сделок").
pub(super) fn num(
    store: &EditorStore,
    id: &'static str,
    width: f32,
    enabled: bool,
    cx: &App,
) -> Option<impl IntoElement> {
    let state = store.input(id)?;
    Some(
        div()
            .flex_none()
            .w(design::ui_px(cx, width))
            .child(MoonInput::new(id).state(&state).small().disabled(!enabled)),
    )
}

/// Moonbot's alert-sound picker: its own list in a dropdown, with the square button that plays what
/// is selected.
///
/// The same pair the compact popup and the core-warning settings draw, from the same two sources,
/// so the sound pickers in this application cannot drift apart. Picking does NOT play; the preview
/// button does.
pub(super) fn sound_cell(
    id: &'static str,
    current: i32,
    enabled: bool,
    view: &Entity<CoreExpertView>,
    set: fn(&mut CoreConfig, i32),
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    let name = crate::media::sound::mb_sound_name(current);
    // A core holding an ordinal this build has no name for shows that NUMBER rather than a guess.
    let label = name.map_or_else(|| format!("#{current}"), str::to_string);
    let view = view.clone();
    let options = crate::media::sound::MB_SOUNDS
        .iter()
        .enumerate()
        .map(|(index, sound)| {
            // The wire ordinal is 1-based; see `media::sound::MB_SOUNDS`.
            let ordinal = index as i32 + 1;
            (
                ordinal,
                SharedString::from(format!("{id}-{ordinal}")),
                SharedString::from(*sound),
            )
        });
    let items = crate::panels::common::radio_items(
        options,
        current,
        crate::panels::common::RadioMark::Check,
        move |app, ordinal| {
            view.update(app, |this, cx| {
                this.edit_draft(|draft| set(draft, ordinal), cx);
            });
        },
    );
    h_flex()
        .items_center()
        .gap(design::ui_px(cx, 4.0))
        .child(
            MoonDropdown::new(id)
                .label(label)
                .trigger_caret(true)
                .trigger_variant(MoonButtonVariant::Soft)
                .trigger_size(MoonButtonSize::Action)
                .trigger_width_scaled(94.0)
                .menu_width_scaled(128.0)
                .menu_size(MoonMenuSize::Compact)
                .items(items)
                .disabled(!enabled),
        )
        .child(crate::panels::common::sound_preview_button(
            SharedString::from(format!("{id}-play")),
            name,
            design::ui_px(cx, SOUND_PLAY_SIDE),
            p,
            cx,
        ))
}
