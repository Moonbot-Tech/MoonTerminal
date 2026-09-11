//! Builds the Hotkeys tab in a Moonbot-style layout: an always-visible block of hard-coded
//! built-in hotkeys, a group sub-tab switcher (`SettingsView.hotkeys_group`), and the active
//! group's rows — read off [`super::registry`], which is the one place that says what the page
//! shows and in what order. The row editors (`slot_row`, `split_parts_row`, `same_move_checkbox`)
//! update the draft.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::config::moonbot_import::shortcut;
use moon_core::config::{
    GestureSlot, HotkeysConfig, KeySlot, MouseGestureBinding, MoveKind, MoveKindSlot,
    SPLIT_PARTS_MAX, SPLIT_PARTS_MIN,
};
use moon_core::feed::CoreConfigState;
use moon_core::session::CoreId;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonDropdown,
    MoonHotkeyInput, MoonKbd, MoonKbdSize, MoonMenuItem, MoonMenuSize, MoonPalette, MoonTabItem,
    MoonTabStrip, MoonTag, MoonText, MoonTooltipView, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::clash::{Clash, Clashes, Severity};
use super::pull::{PullRow, PullVerdict, apply_core_hotkeys, preview_core_hotkeys};
use super::pull_gestures::{self, GesturePullRow, apply_core_gestures, preview_core_gestures};
use super::registry::{self, HotkeyGroup, Row, SlotSpec};
use super::set_gesture_mirrored;
use crate::design;
use crate::hotkeys::meta::{self, Origin, SlotMeta};
use crate::settings::SettingsView;

/// Logical width of the `?` cell that opens a row's description on hover.
///
/// A cell of its own, first in the row, so the titles start at one x and the glyph is where the
/// eye goes for "what does this do" — a description printed on every row was the column that made
/// the page long, and the one thing nobody re-reads once they know the action.
const ROW_HINT_WIDTH: f32 = 14.0;

/// Logical width reserved for every hotkey row title.
const ROW_TITLE_WIDTH: f32 = 160.0;

/// Maximum readable width of a hotkey row's notes — the conflict captions — before its editor
/// column begins.
const ROW_DESCRIPTION_MAX_WIDTH: f32 = 640.0;

/// Readable width of the description tooltip, in rendered pixels.
const HINT_TOOLTIP_MAX_WIDTH: f32 = 380.0;

/// Logical width reserved for the marks that say where a binding acts and where its binding came
/// from.
///
/// A column of its own rather than an inline pair, and reserved on the rows that carry no marks
/// too: the marks are read DOWN the page ("which of these is mine to keep"), which only works while
/// they start at the same x on every row.
const ROW_MARKS_WIDTH: f32 = 150.0;

/// Width of the surface half inside that column.
///
/// Fixed so the provenance tag beside it starts at the same x on every row — the tag is what the
/// page is read down, and a tag that moves with the length of the words before it is not a column.
const ROW_SCOPE_WIDTH: f32 = 104.0;

/// Wraps one trailing control so it keeps its own width instead of stretching into the row.
fn control_cell(child: impl IntoElement, cx: &App) -> gpui::Div {
    sized_cell(child, ROW_CONTROL_WIDTH, cx)
}

/// The same, at a stated width.
fn sized_cell(child: impl IntoElement, width: f32, cx: &App) -> gpui::Div {
    div().flex_none().w(design::ui_px(cx, width)).child(child)
}

/// A cell reserved and left empty, so the column after it starts at the same x on every row.
fn empty_cell(width: f32, cx: &App) -> gpui::Div {
    div().flex_none().w(design::ui_px(cx, width))
}

/// One line of muted body text, the style this tab's descriptions, hints and marks all share.
fn muted_line(text: String, p: &MoonPalette) -> impl IntoElement {
    MoonText::new(text)
        .uppercase(false)
        .mono(false)
        .wrap()
        .line_height(12.0)
        .color(p.text_muted)
        .render()
}

/// The rendered width every row editor shares — the hotkey field and both dropdowns.
///
/// It has to be applied through `trigger_width` (rendered pixels) rather than
/// `trigger_width_scaled`, because the two components scale differently: `MoonHotkeyInput::width`
/// goes through the UI scale, while a scaled trigger width goes through the FONT scale. Left to
/// their own defaults they agree only at one setting of the font slider and drift apart at every
/// other, which is exactly how a field and the dropdown beside it ended up different widths.
const ROW_EDITOR_WIDTH: f32 = 176.0;

/// Extra width the "move kind" cell gets over the gesture cell beside it.
///
/// Its labels are phrases rather than a keystroke — "Параллельно к курсору" against "Alt+Middle" —
/// so the same width truncated them under the caret.
const ROW_KIND_EXTRA: f32 = 56.0;

/// Width of every trailing control cell.
///
/// Each control sits in a `flex_none` box of this width rather than straight in the row. Without
/// the box a trigger grows into whatever space is left, which is how one dropdown ended up twice
/// the width of the row above it with the description running underneath it.
const ROW_CONTROL_WIDTH: f32 = 184.0;

impl SettingsView {
    /// Builds the Settings Hotkeys tab, including its lifted-contrast group strip.
    ///
    /// The strip needs `window` because it is rendered through a lifted palette rather than the
    /// active one: MoonUI keys an inactive tab label off `text_muted`, which sits under the body
    /// contrast floor in both stock themes, and `render_with_theme` is the only way to hand it a
    /// different palette.
    ///
    /// Args:
    ///     window: Window that owns the strip's persistent overflow state.
    ///     cx: Settings context used to read the hotkey draft and build callbacks.
    ///
    /// Returns:
    ///     The complete Hotkeys tab content.
    pub(in crate::settings) fn hotkeys_tab(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let hotkeys = {
            let b = self.backend.read(cx);
            b.preview.as_ref().unwrap_or(&b.config).hotkeys.clone()
        };
        let p = MoonPalette::active(cx);

        // Match Moonbot: fixed built-ins stay at the top, the group sub-tabs follow, and only the
        // active group's rows appear below.
        let builtin = v_flex()
            .w_full()
            .gap(design::ui_px(cx, 3.0))
            .child(
                MoonText::new(t!("hotkeys.group.builtin").to_string())
                    .uppercase(false)
                    .mono(false)
                    .font_size(11.0)
                    .line_height(14.0)
                    .color(p.text)
                    .render(),
            )
            .child(muted_line(t!("hotkeys.group.builtin_hint").to_string(), &p))
            .children([
                self.builtin_row(t!("hotkeys.builtin.wheel_zoom").to_string(), cx),
                self.builtin_row(t!("hotkeys.builtin.wheel_pan").to_string(), cx),
                self.builtin_row(t!("hotkeys.builtin.x_sync").to_string(), cx),
                self.builtin_row(t!("hotkeys.builtin.cancel_hover").to_string(), cx),
                self.builtin_row(t!("hotkeys.builtin.esc_close").to_string(), cx),
                self.builtin_row(t!("hotkeys.builtin.close_all").to_string(), cx),
                self.builtin_row(t!("hotkeys.builtin.reset_windows").to_string(), cx),
            ]);

        // Reuse the main window's chart-tab control (`MoonTabStrip` + `MoonTabItem`) for
        // normal-case labels. Overflow-menu defaults off, so a short group list stays chevron-free.
        let entity = cx.entity();
        let strip_h = design::tab_strip_h(cx);
        let items: Vec<MoonTabItem> = HotkeyGroup::ALL
            .iter()
            .map(|g| MoonTabItem::new(g.title()).selected(self.hotkeys_group == *g))
            .collect();
        let strip = MoonTabStrip::new("hotkeys-group-strip")
            .gap(4.0)
            .items(items)
            .on_click(move |ix, _event, _window, app| {
                let Some(g) = HotkeyGroup::ALL.get(ix).copied() else {
                    return;
                };
                entity.update(app, |this, c| {
                    if this.hotkeys_group != g {
                        this.hotkeys_group = g;
                        c.notify();
                    }
                });
            });
        let strip = design::chrome_tab_strip(strip, p, window, cx);
        let switcher = div().w_full().h(strip_h).child(strip);

        let body = v_flex()
            .w_full()
            .gap(design::ui_px(cx, 3.0))
            .child(muted_line(self.hotkeys_group.hint(), &p))
            // Said once for the whole page instead of in every row's own description: the marks
            // repeat on every line, so their meaning does not have to.
            .child(muted_line(t!("hotkeys.marks_legend").to_string(), &p))
            .children(self.group_rows(self.hotkeys_group, &hotkeys, cx));

        v_flex()
            .w_full()
            .gap(design::ui_px(cx, 10.0))
            .child(builtin)
            .child(switcher)
            .child(body)
    }

    /// Builds the active group's sub-tab rows.
    ///
    /// The registry says what is on the page and in what order; this only draws it. The three rows
    /// that are not slots are placed by the same list, so no group has an order of its own here.
    fn group_rows(
        &self,
        group: HotkeyGroup,
        hotkeys: &HotkeysConfig,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        // Built once for the whole group rather than per row: it is an index over every slot, and
        // asking it forty-six times to rebuild itself forty-six times would be the same answer at
        // forty-six times the price.
        let clashes = Clashes::build(hotkeys);
        let mut out = Vec::new();
        for row in registry::rows().iter().filter(|row| row.group() == group) {
            match *row {
                Row::Slot(spec) => out.push(self.slot_row(spec, hotkeys, &clashes, cx)),
                Row::SplitParts => out.push(self.split_parts_row(hotkeys, cx)),
                Row::SameForMove => out.push(self.same_move_checkbox(hotkeys, cx)),
                Row::CorePull => out.extend(self.core_pull_section(hotkeys, cx)),
            }
        }
        out
    }

    /// Builds a text-only row for a hard-coded, non-configurable hotkey, matching Moonbot's
    /// built-in hotkey reference page.
    fn builtin_row(&self, line: impl Into<String>, cx: &Context<Self>) -> AnyElement {
        let p = MoonPalette::active(cx);
        h_flex()
            .w_full()
            .min_h(design::fit_h_px(cx, 22.0, 11.0, 5.0))
            .items_center()
            .child(
                MoonText::new(line.into())
                    .uppercase(false)
                    .mono(false)
                    .wrap()
                    .font_size(11.0)
                    .line_height(14.0)
                    .color(p.text_muted)
                    .render(),
            )
            .into_any_element()
    }

    /// The caption a row shows when something else answers its binding.
    ///
    /// In the row's middle column rather than beside the editor: it is a sentence, it names other
    /// rows, and it has to be able to wrap. Red when one of the two never fires, amber when both do
    /// and the user simply ought to know.
    fn clash_line(&self, clash: &Clash, p: &MoonPalette) -> AnyElement {
        let color = match clash.severity {
            Severity::Shadowed => p.red_text,
            Severity::Shares => p.amber,
        };
        MoonText::new(clash.text.clone())
            .uppercase(false)
            .mono(false)
            .wrap()
            .line_height(12.0)
            .color(color)
            .render()
            .into_any_element()
    }

    /// The marks column: WHERE this binding acts, and what Moonbot has of it.
    ///
    /// Two facts, because a user asks two different questions of this page — "why did my key do
    /// nothing over there" and "what will pasting a Moonbot configuration overwrite" — and neither
    /// was answerable from the row before. `None` renders the column empty, keeping a row that owns
    /// no slot (the part count, the mirroring checkbox) aligned with the rows that do.
    ///
    /// Args:
    ///     marks: The slot's two facts, or `None` for a row that is not a slot.
    ///     p: Active palette, already read by every caller.
    ///     cx: Settings context used for scaled geometry and the zone setting.
    ///
    /// Returns:
    ///     The fixed-width marks column.
    fn slot_marks(
        &self,
        marks: Option<SlotMeta>,
        p: &MoonPalette,
        cx: &Context<Self>,
    ) -> gpui::Div {
        let column = div().flex_none().w(design::ui_px(cx, ROW_MARKS_WIDTH));
        let Some(marks) = marks else {
            return column;
        };
        // The surface a row's two-way set resolves to right now, rather than the disjunction: the
        // reader wants the answer for the terminal in front of them.
        let separate_zones = {
            let b = self.backend.read(cx);
            b.preview
                .as_ref()
                .unwrap_or(&b.config)
                .separate_control_zones
        };
        column.child(
            h_flex()
                .gap(design::ui_px(cx, 6.0))
                .items_center()
                // The surface as plain muted text in a fixed sub-column, not a second pill and not
                // a wrapping pair. Two pills of the same tone would say nothing about which of them
                // is the one that changes; and letting the two share one wrapping box made the
                // whole point of the column collapse — a long surface pushed the tag onto its own
                // line, so no two rows started their tag at the same x.
                .child(
                    div()
                        .flex_none()
                        .w(design::ui_px(cx, ROW_SCOPE_WIDTH))
                        .child(muted_line(marks.scope.resolved(separate_zones).label(), p)),
                )
                .child(
                    match marks.origin {
                        // Two tones, because the two answers matter equally at a glance: blue for
                        // a binding a paste or a pull will overwrite, green for one nothing can.
                        Origin::Shared => MoonTag::info(),
                        Origin::Local => MoonTag::positive(),
                    }
                    .mono(false)
                    // `.child` rather than `.label`: the latter uppercases, and the legend that
                    // explains these marks quotes them as written.
                    .child(marks.origin.label())
                    .render(),
                ),
        )
    }

    /// Builds one editor row: title, marks, description, and the editors the row carries — a
    /// hotkey field, a gesture dropdown with its kind selector, or both.
    ///
    /// One builder for every row, because a row is defined by WHICH editors it has and not by which
    /// kind: almost every keyboard row carries a click half too. The trailing controls sit in a
    /// block of fixed-width cells, reserved on every row, empty ones included — that is what keeps
    /// the columns aligned when one row carries a kind dropdown and the next does not.
    ///
    /// Args:
    ///     spec: The row, from the registry.
    ///     hotkeys: Draft configuration used to show the current bindings and conflicts.
    ///     clashes: The group's conflict index.
    ///     cx: Settings context used for palette, scaling, and input events.
    ///
    /// Returns:
    ///     The rendered row.
    fn slot_row(
        &self,
        spec: SlotSpec,
        hotkeys: &HotkeysConfig,
        clashes: &Clashes,
        cx: &Context<Self>,
    ) -> AnyElement {
        // A greyed short row follows its long twin, so a clash reported on it would name a binding
        // the row does not own.
        let disabled = spec.follows_mirror() && hotkeys.same_hotkeys_for_move;
        // Every note this row prints goes in the middle column, beside the title, rather than in a
        // column of its own on the far right — which put it a screen away from the row it is about.
        let mut notes: Vec<Clash> = Vec::new();
        if let Some(key) = spec.key() {
            notes.extend(clashes.key(hotkeys, key));
        }
        if let Some(mouse) = spec.mouse()
            && !disabled
        {
            notes.extend(clashes.mouse(hotkeys, mouse));
        }

        let id = match (spec.key(), spec.mouse()) {
            (Some(key), _) => registry::key_id(key),
            (None, Some(mouse)) => registry::gesture_id(mouse),
            (None, None) => unreachable!("a row edits at least one slot"),
        };
        let row = self.row_head(
            id,
            spec.title(),
            spec.hint(),
            Some(spec.meta()),
            spec.title_is_identity(),
            disabled,
            notes,
            cx,
        );
        // The key cell is reserved on every row, empty on the four placement rows and the eight
        // move rows: the gesture column has to start at one x down the whole page now that almost
        // every row has one.
        row.child(match spec.key() {
            Some(key) => control_cell(self.hotkey_input(key, hotkeys, cx), cx),
            None => empty_cell(ROW_CONTROL_WIDTH, cx),
        })
        .when_some(spec.mouse(), |row, mouse| {
            row.child(
                h_flex()
                    .flex_none()
                    .gap(design::ui_px(cx, 10.0))
                    .items_center()
                    .child(control_cell(
                        self.gesture_dropdown(mouse, hotkeys, disabled, cx),
                        cx,
                    ))
                    .child(match spec.kind() {
                        Some(kind) => sized_cell(
                            self.move_kind_dropdown(kind, hotkeys, disabled, cx),
                            ROW_CONTROL_WIDTH + ROW_KIND_EXTRA,
                            cx,
                        ),
                        None => empty_cell(ROW_CONTROL_WIDTH + ROW_KIND_EXTRA, cx),
                    }),
            )
        })
        .into_any_element()
    }

    /// The keystroke editor of one row.
    fn hotkey_input(
        &self,
        slot: KeySlot,
        hotkeys: &HotkeysConfig,
        cx: &Context<Self>,
    ) -> MoonHotkeyInput {
        let raw = hotkeys.key(slot);
        let parsed = crate::hotkeys::parse_binding(raw);
        let invalid = !raw.trim().is_empty() && parsed.is_none();
        MoonHotkeyInput::new(format!("hotkey-{}", registry::key_id(slot)))
            .value(parsed)
            .placeholder(t!("hotkeys.unassigned").to_string())
            .recording_placeholder(t!("hotkeys.recording").to_string())
            .invalid(invalid)
            // No `.conflict()`: the component paints that frame AMBER and stamps an unlocalized
            // "conflict" badge inside the field, and amber is this page's "both work" tone. The
            // caption below the description carries the whole answer, in the right colour and in
            // words.
            .compact()
            .width(ROW_EDITOR_WIDTH)
            .on_change(
                cx.processor(move |this, value: Option<Keystroke>, _window, cx| {
                    // Store the PHYSICAL key: a letter recorded under a Cyrillic layout would
                    // otherwise be saved as that layout's character.
                    let value = value
                        .map(|k| crate::hotkeys::recorded_keystroke(k).unparse())
                        .unwrap_or_default();
                    this.set_hotkey(slot, value, cx);
                }),
            )
    }

    /// The gesture editor of one row.
    fn gesture_dropdown(
        &self,
        slot: GestureSlot,
        hotkeys: &HotkeysConfig,
        disabled: bool,
        cx: &App,
    ) -> MoonDropdown {
        let current = hotkeys.gesture(slot);
        let backend = self.backend.clone();
        let items = MouseGestureBinding::ALL.into_iter().map(move |gesture| {
            let backend = backend.clone();
            MoonMenuItem::with_key(gesture.config_value(), gesture.menu_label())
                .checked(gesture == current)
                .on_click(move |_, _, cx| {
                    backend.update(cx, |b, bcx| {
                        if let Some(p) = b.preview.as_mut()
                            && set_gesture_mirrored(&mut p.hotkeys, slot, gesture)
                        {
                            bcx.notify();
                        }
                    });
                })
        });
        Self::row_dropdown(
            format!("mouse-{}", registry::gesture_id(slot)),
            current.label(),
            cx,
        )
        .trigger_variant(if current == MouseGestureBinding::None {
            MoonButtonVariant::Neutral
        } else {
            MoonButtonVariant::Blue
        })
        .menu_width_scaled(228.0)
        .disabled(disabled)
        .items(items)
    }

    /// The "Move kind" selector of one move row — Moonbot's column of the same name.
    ///
    /// The gesture says WHERE (the clicked price); this says which orders go there and how the core
    /// arranges them. `None` leaves the gesture recognised and inert, which is Moonbot's own way of
    /// switching one off without clearing the binding.
    fn move_kind_dropdown(
        &self,
        slot: MoveKindSlot,
        hotkeys: &HotkeysConfig,
        disabled: bool,
        cx: &App,
    ) -> impl IntoElement {
        let current = hotkeys.move_kind(slot);
        let backend = self.backend.clone();
        let items = MoveKind::ALL.into_iter().map(move |kind| {
            let backend = backend.clone();
            let label_key = kind.locale_key();
            MoonMenuItem::with_key(kind.id(), t!(&label_key).to_string())
                .checked(kind == current)
                .on_click(move |_, _, cx| {
                    backend.update(cx, |b, bcx| {
                        if let Some(p) = b.preview.as_mut()
                            && p.hotkeys.set_move_kind(slot, kind)
                        {
                            bcx.notify();
                        }
                    });
                })
        });
        let current_key = current.locale_key();
        Self::row_dropdown(
            // The kind is named after the gesture row it sits on.
            format!("move-kind-{}", registry::gesture_id(slot.half(false))),
            t!(&current_key).to_string(),
            cx,
        )
        // The trigger shows the chosen kind, so the menu carries the name of the setting — the
        // "Move kind" column heading Moonbot puts above the same list.
        .header(18.0, |_, cx| {
            let p = MoonPalette::active(cx);
            MoonText::new(t!("hotkeys.move_kind.title").to_string())
                .uppercase(false)
                .mono(false)
                .font_size(9.0)
                .line_height(12.0)
                .color(p.text_muted)
                .render()
                .into_any_element()
        })
        .trigger_variant(if current == MoveKind::None {
            MoonButtonVariant::Neutral
        } else {
            MoonButtonVariant::Blue
        })
        .menu_width_scaled(228.0)
        .disabled(disabled)
        .items(items)
    }

    /// The `?` cell of a row: a glyph that brightens under the pointer and opens the description as
    /// a tooltip. Nothing to click — it only answers hover.
    ///
    /// Args:
    ///     id: Stable element id, which the tooltip state is keyed on.
    ///     hint: The description, already localized.
    ///     p: Active palette.
    ///     cx: Settings context used for scaled geometry.
    ///
    /// Returns:
    ///     The fixed-width hint cell.
    fn hint_glyph(
        &self,
        id: String,
        hint: String,
        p: &MoonPalette,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let hover = p.text;
        div()
            .id(SharedString::from(id))
            .flex_none()
            .w(design::ui_px(cx, ROW_HINT_WIDTH))
            .cursor_default()
            .text_size(design::t_caption(cx))
            .text_color(design::moon(p.text_muted))
            .hover(move |s| s.text_color(design::moon(hover)))
            .tooltip(move |_w, cx| {
                cx.new(|_| MoonTooltipView::new(hint.clone()).max_width(HINT_TOOLTIP_MAX_WIDTH))
                    .into()
            })
            .child("?")
    }

    /// Builds the shared leading half of an editor row: the `?` with the description behind it,
    /// the title, the marks, then the notes.
    ///
    /// Every row on this tab is that prefix plus one or two controls. The row wraps trailing
    /// controls at narrow widths instead of clipping them. The description used to be printed on
    /// every row and made the page twice as long; it now sits behind the glyph, and the middle
    /// column carries only what has to be seen without asking — the conflict captions.
    ///
    /// Args:
    ///     id: Stable element id for the row's hover state.
    ///     title: Label displayed in the shared fixed-width title column.
    ///     desc: Description shown as the `?` tooltip.
    ///     marks: The slot's two facts, or `None` for a row that owns no slot.
    ///     mono_title: Whether the title is an identity like `F3` rather than a phrase.
    ///     muted: Whether the title is greyed because the row is inert — the four short move rows
    ///         while the mirror switch owns them. Lost in a signature change on 2026-09-09, which
    ///         left those rows shouting a full-contrast title beside a dropdown that does nothing.
    ///     notes: Lines printed under the description, in order. A gesture row can carry two:
    ///         which row it takes its binding from, and which layer it shares it with.
    ///     cx: Settings context used for palette and scaled layout.
    ///
    /// Returns:
    ///     The row prefix to which callers append one or two controls.
    #[allow(clippy::too_many_arguments)]
    fn row_head(
        &self,
        id: String,
        title: String,
        desc: String,
        marks: Option<SlotMeta>,
        mono_title: bool,
        muted: bool,
        notes: Vec<Clash>,
        cx: &Context<Self>,
    ) -> gpui::Div {
        let p = MoonPalette::active(cx);
        h_flex()
            .w_full()
            .flex_wrap()
            .min_h(design::fit_h_px(cx, 24.0, 12.0, 6.0))
            .gap(design::ui_px(cx, 10.0))
            .items_center()
            .child(self.hint_glyph(format!("hint-{id}"), desc, &p, cx))
            .child(
                div()
                    .flex_none()
                    .w(design::ui_px(cx, ROW_TITLE_WIDTH))
                    .child(
                        MoonText::new(title)
                            .uppercase(false)
                            .mono(mono_title)
                            .wrap()
                            .font_size(11.0)
                            .line_height(14.0)
                            .color(if muted { p.text_muted } else { p.text })
                            .render(),
                    ),
            )
            .child(self.slot_marks(marks, &p, cx))
            .child(
                // The notes column: the captions that must be seen without asking, wrapping within
                // the window. Empty on most rows, and kept as the flexible spacer that pushes the
                // controls to the right regardless.
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(design::ui_px(cx, 2.0))
                    .max_w(design::ui_px(cx, ROW_DESCRIPTION_MAX_WIDTH))
                    .children(notes.iter().map(|note| self.clash_line(note, &p))),
            )
    }

    /// Builds a row's trailing dropdown with the trigger geometry shared by this tab.
    fn row_dropdown(id: String, label: impl Into<SharedString>, cx: &App) -> MoonDropdown {
        MoonDropdown::new(SharedString::from(id))
            .label(label)
            .trigger_caret(true)
            .trigger_size(MoonButtonSize::Micro)
            .trigger_width(design::ui_value(cx, ROW_EDITOR_WIDTH))
            .menu_size(MoonMenuSize::Compact)
    }

    /// Builds the part-count selector for `Split N` (Moonbot `Hotkeys.SplitParts`).
    ///
    /// A dropdown over the allowed range rather than a text field: the value goes straight into a
    /// live split command, and a picker cannot leave a half-typed number in the draft.
    fn split_parts_row(&self, hotkeys: &HotkeysConfig, cx: &Context<Self>) -> AnyElement {
        let current = hotkeys.split_n_parts();
        let items = (SPLIT_PARTS_MIN..=SPLIT_PARTS_MAX).map(|parts| {
            let backend = self.backend.clone();
            MoonMenuItem::with_key(format!("split-parts-{parts}"), parts.to_string())
                .checked(i32::from(parts) == current)
                .on_click(move |_, _, cx| {
                    backend.update(cx, |b, bcx| {
                        if let Some(preview) = b.preview.as_mut()
                            && preview.hotkeys.split_parts != parts
                        {
                            preview.hotkeys.split_parts = parts;
                            bcx.notify();
                        }
                    });
                })
        });

        self.row_head(
            "split-parts".to_string(),
            t!("hotkeys.split_parts").to_string(),
            t!("hotkeys.split_parts_hint").to_string(),
            Some(meta::SPLIT_PARTS),
            false,
            false,
            Vec::new(),
            cx,
        )
        .child(control_cell(
            Self::row_dropdown("hotkey-split-parts".into(), current.to_string(), cx)
                .trigger_variant(MoonButtonVariant::Blue)
                .menu_width_scaled(120.0)
                .items(items),
            cx,
        ))
        .into_any_element()
    }

    /// Builds the move-mirroring checkbox in the same control column as the gesture editors.
    ///
    /// Args:
    ///     hotkeys: Draft configuration that supplies the checkbox state.
    ///     cx: Settings context used for scaled layout and change events.
    ///
    /// Returns:
    ///     The aligned move-mirroring checkbox row.
    fn same_move_checkbox(&self, hotkeys: &HotkeysConfig, cx: &Context<Self>) -> AnyElement {
        let backend = self.backend.clone();

        h_flex()
            .w_full()
            .min_h(design::fit_h_px(cx, 30.0, 12.0, 6.0))
            .gap(design::ui_px(cx, 10.0))
            .items_center()
            .child(empty_cell(ROW_HINT_WIDTH, cx))
            .child(div().flex_none().w(design::ui_px(cx, ROW_TITLE_WIDTH)))
            .child(self.slot_marks(Some(meta::SAME_FOR_MOVE), &MoonPalette::active(cx), cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .max_w(design::ui_px(cx, ROW_DESCRIPTION_MAX_WIDTH)),
            )
            .child(
                MoonCheckbox::new("same-hotkeys-for-move")
                    .checked(hotkeys.same_hotkeys_for_move)
                    .size(MoonCheckboxSize::Compact)
                    .label(t!("hotkeys.mouse.same_move").to_string())
                    .on_change(move |value, _window, cx| {
                        backend.update(cx, |b, bcx| {
                            if let Some(p) = b.preview.as_mut() {
                                let changed = p.hotkeys.same_hotkeys_for_move != *value;
                                p.hotkeys.same_hotkeys_for_move = *value;
                                if *value {
                                    // Mirrors only on the way ON, which is what Moonbot's own
                                    // dialog does. The pull re-aims in both directions, because
                                    // there a short row can go live with a value nothing wrote.
                                    for row in MoveKindSlot::ALL {
                                        let long = p.hotkeys.gesture(row.half(false));
                                        p.hotkeys.set_gesture(row.half(true), long);
                                    }
                                }
                                if changed {
                                    bcx.notify();
                                }
                            }
                        });
                    }),
            )
            .into_any_element()
    }

    fn set_hotkey(&mut self, slot: KeySlot, value: String, cx: &mut Context<Self>) {
        let changed = self.backend.update(cx, |b, bcx| {
            let mut changed = false;
            if let Some(p) = b.preview.as_mut() {
                changed = p.hotkeys.set_key(slot, value);
                if changed {
                    bcx.notify();
                }
            }
            changed
        });
        if changed {
            cx.notify();
        }
    }

    /// Resolves the core whose layout the "pull" button addresses: the group's active trade
    /// core, the same resolution the header's manual-strategy cluster already uses.
    ///
    /// The Hotkeys tab has no owning window group of its own (unlike the toolbar or header, which
    /// render inside one group's window) — Settings is one shared window. `Backend::
    /// singleton_workspace` is the existing resolver for exactly this situation: it is the same
    /// "last focused live Auto group" the Strategies and Analytics windows already use to answer
    /// group-shaped questions from an unscoped window (`strategies/window.rs`,
    /// `analytics/tuner/mod.rs`).
    fn core_pull_target(&self, cx: &Context<Self>) -> Option<CoreId> {
        let b = self.backend.read(cx);
        let group = b.singleton_workspace()?.group;
        b.active_trade_core(&group)
    }

    /// Requests an on-purpose refresh and arms the `Pending` state for `core`.
    ///
    /// Fire-and-forget: completion arrives as a `SharedConfigUpdated` -> `FeedMsg::CoreConfig`,
    /// bumping `core_config_recv_rev` unconditionally even when the arriving config is
    /// byte-identical to what is already retained — which is exactly why `Pending` polls
    /// `core_config_recv_rev` here rather than the compare-then-bump `core_config_rev`; the
    /// latter would never clear on an identical echo.
    fn request_core_pull(&mut self, core: CoreId, cx: &mut Context<Self>) {
        let baseline = self
            .backend
            .read(cx)
            .session
            .store()
            .core(core)
            .map(|d| d.core_config_recv_rev)
            .unwrap_or(0);
        if let Err(error) = self.backend.read(cx).session.refresh_shared_config(core) {
            self.status = Some((crate::settings::StatusMsg::Text(error.to_string()), true));
        }
        self.core_pull = Some((core, baseline));
        cx.notify();
    }

    /// Applies every `WillApply` row of the CURRENT preview (rebuilt fresh here, not reused from
    /// render) and writes `hotkeys.toml` immediately.
    ///
    /// Rebuilding cannot disagree with what was drawn over the DRAFT — same computation, same
    /// values. It can over the CORE's block: that is re-read from the store here, so a
    /// configuration arriving between the frame the user read and the click they made is what gets
    /// applied. The window is an eye-to-click one and the alternative — applying rows captured at
    /// render time — would write a layout the core has already replaced, which is worse.
    ///
    /// This bypasses the tab's usual preview/Save cycle on purpose: `HotkeysConfig::save()` is a
    /// separate file with its own saver, no `config_dirty` involved. Writing both
    /// `config.hotkeys` and `preview.hotkeys` keeps them in sync so a LATER "Settings > Save"
    /// click (which starts from `preview`) cannot silently roll the pull back to what the draft
    /// looked like when the window opened.
    fn confirm_core_pull(&mut self, core: CoreId, cx: &mut Context<Self>) {
        let outcome = self.backend.update(cx, |b, bcx| {
            let (layout, manual_strategy_keys, gestures) = b
                .session
                .store()
                .core(core)
                .and_then(|d| d.core_config.as_ref())
                .map(|c| {
                    (
                        c.manual.core_hotkeys.clone(),
                        c.manual.strat_buttons.hot_keys,
                        c.gestures,
                    )
                })?;
            let base = b
                .preview
                .as_ref()
                .map(|p| p.hotkeys.clone())
                .unwrap_or_else(|| b.config.hotkeys.clone());
            let rows = preview_core_hotkeys(&base, &layout, &manual_strategy_keys);
            let gesture_rows = preview_core_gestures(&base, &gestures);
            let mut hotkeys = base;
            let mut changed = apply_core_hotkeys(&mut hotkeys, &rows);
            changed |= apply_core_gestures(&mut hotkeys, &gesture_rows);
            if changed {
                b.config.hotkeys = hotkeys.clone();
                if let Some(p) = b.preview.as_mut() {
                    p.hotkeys = hotkeys.clone();
                }
                bcx.notify();
            }
            Some((changed, hotkeys))
        });
        match outcome {
            Some((true, hotkeys)) => match hotkeys.save() {
                Ok(()) => {
                    self.status = Some((
                        crate::settings::StatusMsg::Key("hotkeys.pull.applied"),
                        false,
                    ))
                }
                Err(e) => {
                    self.status = Some((crate::settings::StatusMsg::Text(e.to_string()), true))
                }
            },
            Some((false, _)) => {
                self.status = Some((
                    crate::settings::StatusMsg::Key("hotkeys.pull.nothing_to_apply"),
                    false,
                ))
            }
            None => {}
        }
        self.core_pull = None;
        cx.notify();
    }

    /// One preview row: the slot's own identity label (without it, two visually identical `F1 ->
    /// F2 will apply` rows give no indication of what they each change), the terminal's current
    /// key (`MoonHotkeyInput`, read-only), the core's incoming key (`MoonKbd`), and the verdict.
    fn core_pull_row(&self, row: &PullRow, cx: &Context<Self>) -> AnyElement {
        let p = MoonPalette::active(cx);
        let id = format!("core-pull-{}", registry::key_id(row.slot));
        let (verdict_text, verdict_color): (String, u32) = match row.verdict {
            PullVerdict::Empty => (t!("hotkeys.pull.verdict.empty").to_string(), p.text_muted),
            PullVerdict::Unsupported => {
                (t!("hotkeys.pull.verdict.unsupported").to_string(), p.amber)
            }
            PullVerdict::Unchanged => (
                t!("hotkeys.pull.verdict.unchanged").to_string(),
                p.text_muted,
            ),
            PullVerdict::WillApply => (
                t!("hotkeys.pull.verdict.will_apply").to_string(),
                p.green_text,
            ),
            PullVerdict::Conflict => (t!("hotkeys.pull.verdict.conflict").to_string(), p.red_text),
        };

        h_flex()
            .w_full()
            .min_h(design::fit_h_px(cx, 24.0, 12.0, 6.0))
            .gap(design::ui_px(cx, 10.0))
            .items_center()
            .child(
                div()
                    .flex_none()
                    .w(design::ui_px(cx, 96.0))
                    // A slot identity (F1, S1) is a value, and the key columns it is compared
                    // against on this same row -- MoonHotkeyInput, the arrow, MoonKbd -- all stay
                    // mono. Without this pin it would be the only column of the comparison that
                    // changed face.
                    .font_family(design::mono())
                    .text_size(design::t_caption(cx))
                    .text_color(rgba_from(p.text, 1.0))
                    .child(registry::key_title(row.slot)),
            )
            .child(
                MoonHotkeyInput::new(format!("{id}-current"))
                    .value(crate::hotkeys::parse_binding(&row.current))
                    .placeholder(t!("hotkeys.unassigned").to_string())
                    .disabled(true)
                    .compact()
                    .width(140.0),
            )
            .child(
                MoonText::new("->")
                    .uppercase(false)
                    .mono(true)
                    .font_size(11.0)
                    .line_height(14.0)
                    .color(p.text_muted)
                    .render(),
            )
            .child(
                MoonKbd::new(shortcut::display(row.core_decoded))
                    .size(MoonKbdSize::Compact)
                    .outline(matches!(
                        row.verdict,
                        PullVerdict::Empty | PullVerdict::Unsupported
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(design::t_caption(cx))
                    .text_color(rgba_from(verdict_color, 1.0))
                    .child(verdict_text),
            )
            .into_any_element()
    }

    /// One gesture row of the pull preview.
    ///
    /// Text on both sides rather than the key row's `MoonHotkeyInput` and `MoonKbd`: a gesture is a
    /// phrase ("Ctrl+Left (CTRL_Click)"), not a keystroke, and those two controls draw keystrokes.
    fn core_pull_gesture_row(&self, row: &GesturePullRow, cx: &Context<Self>) -> AnyElement {
        let p = MoonPalette::active(cx);
        let (verdict_text, verdict_color): (String, u32) = match row.verdict {
            // Not produced for gestures — a zero ordinal is a value there — but the verdict enum
            // is shared with the keyboard half, so the arm has to exist. It borrows that half's
            // wording rather than carrying three translations nothing can render.
            PullVerdict::Empty => (t!("hotkeys.pull.verdict.empty").to_string(), p.text_muted),
            PullVerdict::Unsupported => (
                t!("hotkeys.pull.verdict.gesture_unsupported").to_string(),
                p.amber,
            ),
            PullVerdict::Unchanged => (
                t!("hotkeys.pull.verdict.unchanged").to_string(),
                p.text_muted,
            ),
            PullVerdict::WillApply => (
                t!("hotkeys.pull.verdict.will_apply").to_string(),
                p.green_text,
            ),
            // Never produced for gestures — see `pull_gestures::preview_core_gestures`.
            PullVerdict::Conflict => (t!("hotkeys.pull.verdict.conflict").to_string(), p.red_text),
        };
        let dim = matches!(
            row.verdict,
            PullVerdict::Empty | PullVerdict::Unsupported | PullVerdict::Unchanged
        );

        h_flex()
            .w_full()
            .min_h(design::fit_h_px(cx, 22.0, 12.0, 5.0))
            .gap(design::ui_px(cx, 10.0))
            .items_center()
            .child(
                div()
                    .flex_none()
                    .w(design::ui_px(cx, 200.0))
                    .text_size(design::t_caption(cx))
                    .text_color(rgba_from(p.text, 1.0))
                    .child(pull_gestures::target_label(row.target)),
            )
            .child(
                div()
                    .flex_none()
                    .w(design::ui_px(cx, 170.0))
                    .text_size(design::t_caption(cx))
                    .text_color(rgba_from(p.text_muted, 1.0))
                    .child(row.current.clone()),
            )
            .child(
                MoonText::new("->")
                    .uppercase(false)
                    .mono(true)
                    .font_size(11.0)
                    .line_height(14.0)
                    .color(p.text_muted)
                    .render(),
            )
            .child(
                div()
                    .flex_none()
                    .w(design::ui_px(cx, 170.0))
                    .text_size(design::t_caption(cx))
                    .text_color(rgba_from(if dim { p.text_muted } else { p.text }, 1.0))
                    .child(row.incoming.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(design::t_caption(cx))
                    .text_color(rgba_from(verdict_color, 1.0))
                    .child(verdict_text),
            )
            .into_any_element()
    }

    /// The "pull layout from core" button and, once a layout has arrived, its preview diff.
    /// Placed after the ManualStrategy rows and gated on nothing else: it is always visible on
    /// this sub-tab, which is what lets a resolved core's Live/Stale/Awaiting state stay legible
    /// without the user having to click anything first.
    fn core_pull_section(&self, hotkeys: &HotkeysConfig, cx: &Context<Self>) -> Vec<AnyElement> {
        let p = MoonPalette::active(cx);
        let mut out: Vec<AnyElement> = vec![
            div()
                .w_full()
                .h(design::ui_px(cx, 1.0))
                .bg(rgba_from(p.border, 1.0))
                .into_any_element(),
            MoonText::new(t!("hotkeys.pull.title").to_string())
                .uppercase(false)
                .mono(false)
                .font_size(11.0)
                .line_height(14.0)
                .color(p.text)
                .render()
                .into_any_element(),
        ];

        let Some(core) = self.core_pull_target(cx) else {
            out.push(self.pull_hint(t!("hotkeys.pull.no_core").to_string(), &p, cx));
            return out;
        };

        let b = self.backend.read(cx);
        let core_data = b.session.store().core(core);
        let state = core_data.map(|d| d.core_config_state());
        let manual = core_data.and_then(|d| d.core_config.as_ref()).map(|c| {
            (
                c.manual.core_hotkeys.clone(),
                c.manual.strat_buttons.hot_keys,
                c.gestures,
            )
        });

        let pending = self.core_pull.is_some_and(|(pending_core, baseline)| {
            pending_core == core
                && self
                    .backend
                    .read(cx)
                    .session
                    .store()
                    .core(core)
                    .map(|d| d.core_config_recv_rev)
                    == Some(baseline)
        });

        let freshness = match state {
            Some(CoreConfigState::Live) => {
                Some((t!("hotkeys.pull.live").to_string(), p.green_text))
            }
            Some(CoreConfigState::Stale) => Some((t!("hotkeys.pull.stale").to_string(), p.amber)),
            _ => None,
        };

        let mut header = h_flex()
            .w_full()
            .items_center()
            .gap(design::ui_px(cx, 10.0))
            .child(
                MoonButton::new("hotkeys-pull-request")
                    .outline()
                    .small()
                    .width(180.0)
                    .loading(pending)
                    .label(t!("hotkeys.pull.button").to_string())
                    .on_click(cx.listener(move |this, _, _, cx| this.request_core_pull(core, cx))),
            );
        if let Some((text, color)) = freshness {
            header = header.child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(rgba_from(color, 1.0))
                    .child(text),
            );
        }
        out.push(header.into_any_element());

        if pending {
            out.push(self.pull_hint(t!("hotkeys.pull.pending").to_string(), &p, cx));
            return out;
        }

        let Some((layout, manual_strategy_keys, gestures)) = manual else {
            out.push(self.pull_hint(t!("hotkeys.pull.empty").to_string(), &p, cx));
            return out;
        };

        let rows = preview_core_hotkeys(hotkeys, &layout, &manual_strategy_keys);
        let gesture_rows = preview_core_gestures(hotkeys, &gestures);
        let any_will_apply = rows
            .iter()
            .map(|r| r.verdict)
            .chain(gesture_rows.iter().map(|r| r.verdict))
            .any(|v| v == PullVerdict::WillApply);
        for row in &rows {
            out.push(self.core_pull_row(row, cx));
        }
        out.push(
            MoonText::new(t!("hotkeys.pull.gestures").to_string())
                .uppercase(false)
                .mono(false)
                .font_size(11.0)
                .line_height(14.0)
                .color(p.text)
                .render()
                .into_any_element(),
        );
        for row in &gesture_rows {
            out.push(self.core_pull_gesture_row(row, cx));
        }
        out.push(
            h_flex()
                .w_full()
                .gap(design::ui_px(cx, 8.0))
                .child(
                    MoonButton::new("hotkeys-pull-confirm")
                        .primary()
                        .small()
                        .width(130.0)
                        .disabled(!any_will_apply)
                        .label(t!("hotkeys.pull.confirm").to_string())
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.confirm_core_pull(core, cx)),
                        ),
                )
                .child(
                    MoonButton::new("hotkeys-pull-cancel")
                        .outline()
                        .small()
                        .width(110.0)
                        .label(t!("hotkeys.pull.cancel").to_string())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.core_pull = None;
                            cx.notify();
                        })),
                )
                .into_any_element(),
        );
        out
    }

    /// Small muted status line shared by the "no core" / "empty" / "pending" states.
    fn pull_hint(&self, text: String, p: &MoonPalette, cx: &Context<Self>) -> AnyElement {
        div()
            .text_size(design::t_caption(cx))
            .text_color(rgba_from(p.text_muted, 1.0))
            .child(text)
            .into_any_element()
    }
}
