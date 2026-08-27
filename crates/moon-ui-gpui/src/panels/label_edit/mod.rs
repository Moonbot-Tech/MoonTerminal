//! The caption-module editor: one dialog that owns everything ABOUT a module, while the labels
//! popup keeps only what is shown and where.
//!
//! Why a dialog and not another expander in the popup: a module holds up to eight captions and each
//! caption has a field, a colour, a size, a prefix, a plate and a PnL basis. Nested inside a popover
//! that is itself opened from a toolbar button, that reads as a list of lists — and the popup then
//! answers two different questions at once. Here the popup answers "what and where", and this window
//! answers "what exactly does this one print".
//!
//! It edits a COPY. Cancel and the overlay discard it; only OK hands the row back through the
//! callback the opener supplied — which is the same write door the popup uses, so the sanitize and
//! persistence path is unchanged.

use std::rc::Rc;

use gpui::*;
use moon_core::config::ChartLabelRow;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonInputState, MoonPalette, MoonWindowExt as _,
    h_flex,
};
use rust_i18n::t;

use crate::design::{self, moon};

mod body;

#[cfg(test)]
mod tests;

use body::dialog_body;

/// Working state of the open editor: the module being edited, and which of its captions is selected.
pub struct LabelEditState {
    /// The edited COPY. Nothing outside this dialog sees it until OK.
    row: ChartLabelRow,
    /// Timeframe of the chart the module came from, so the preview resolves an `Авто` countdown to
    /// the period that chart will actually print rather than to the sample's own.
    chart_tf_ms: i64,
    /// Caption whose settings the right-hand pane shows. Clamped to the used captions on every
    /// change, so removing the last one cannot leave the pane describing a caption that is gone.
    selected: usize,
    name_input: Entity<MoonInputState>,
    /// Where an accepted module goes. `Rc` because the dialog's content closure is rebuilt on every
    /// render and each rebuild needs its own handle.
    on_done: Rc<dyn Fn(ChartLabelRow, &mut App)>,
    /// Which field picker is open, by its element id.
    ///
    /// The picker is a CONTROLLED popover — see [`crate::controls::field_picker`] — because a pick
    /// has to outlive the close: the library's own close-on-click fires on mouse-down and takes the
    /// button out of the tree before the click lands on it. Keyed by id rather than a bare flag so
    /// a second picker can be added without the two fighting over one switch.
    picker_open: Option<SharedString>,
    /// Opens the arbitrage roster window, for a module whose caption prints that column.
    ///
    /// Handed in rather than opened here, for the reason this dialog takes `on_done` rather than
    /// writing the configuration itself: the roster is the BACKEND's, and this window knows nothing
    /// about backends. It is invoked like an OK — the module is applied first — because the two
    /// windows cannot stand on top of each other.
    on_open_arb: Rc<dyn Fn(&mut Window, &mut App)>,
    /// Run when the dialog goes away, whichever way it goes: OK, Cancel, the ✕, or the overlay.
    ///
    /// The opener uses it to bring back the popup it closed. A popover paints in its own deferred
    /// layer, ABOVE this dialog's overlay — so the list has to leave the screen while the editor is
    /// up, and come back with it.
    on_dismiss: Rc<dyn Fn(&mut App)>,
}

impl LabelEditState {
    /// The row as it stands, with the name field folded in.
    ///
    /// Read at OK rather than committed on every keystroke: this is a dialog with an explicit
    /// Cancel, and a name that reached the configuration while the user was still typing would
    /// survive that Cancel.
    fn accepted_row(&self, cx: &App) -> ChartLabelRow {
        let mut row = self.row.clone();
        row.name = self.name_input.read(cx).value().trim().to_string();
        // The checkbox is disabled while the module has no name AT ALL, so one left nameless must
        // not carry a switch that says it prints one — it would come back the moment a name was
        // typed, which is not what the user set. A preset module always has a name to print, even
        // with the field empty, and keeps its switch.
        row.show_name = row.show_name && (!row.name.is_empty() || row.preset.is_some());
        row
    }

    /// Open or close one field picker, by id.
    fn set_picker(state: &Entity<Self>, id: &str, open: bool, cx: &mut App) {
        let id = SharedString::from(id.to_string());
        state.update(cx, |s, cx| {
            s.picker_open = open.then_some(id);
            cx.notify();
        });
    }

    /// Keep the selection on a caption that exists.
    fn clamp_selection(&mut self) {
        self.selected = clamped(self.selected, self.row.used_parts());
    }
}

/// The selection a list of `used` captions can honour.
///
/// Its own function because it is the one piece of this dialog with a rule rather than a layout:
/// removing the last caption must not leave the settings pane describing a caption that is gone,
/// and an empty module selects the slot a first caption will land in.
fn clamped(selected: usize, used: usize) -> usize {
    selected.min(used.saturating_sub(1))
}

/// Open the editor for one module.
///
/// Args:
///     title: Dialog header — the module's name, or the "new module" wording.
///     row: The module to edit; a fresh one for "add".
///     on_done: Applied with the edited module when OK is pressed.
///     on_dismiss: Run when the dialog closes, whichever way.
pub(crate) fn open_label_edit(
    title: String,
    row: ChartLabelRow,
    // The timeframe of the chart this module belongs to, for previewing an `Авто` countdown as the
    // period it will really print. Everything else the sample shows is a sample; this is not.
    chart_tf_ms: i64,
    window: &mut Window,
    cx: &mut App,
    on_done: impl Fn(ChartLabelRow, &mut App) + 'static,
    on_dismiss: impl Fn(&mut App) + 'static,
    on_open_arb: impl Fn(&mut Window, &mut App) + 'static,
) {
    // A preset module shows ITS OWN name as the placeholder rather than the generic hint: the
    // field is empty because the name comes from the dictionary, and an empty box under a chart
    // that clearly prints "Позиция" otherwise reads as a name that got lost. Typing here overrides
    // it; clearing the field gives the translated name back.
    let hint = row
        .title_key()
        .map(|key| t!(key).to_string())
        .unwrap_or_else(|| t!("chart_labels.row_name_hint").to_string());
    let name_input = cx.new(|cx| MoonInputState::new(window, cx).placeholder(hint));
    name_input.update(cx, |st, c| st.set_value(row.name.clone(), window, c));
    let state = cx.new(|_| LabelEditState {
        row,
        chart_tf_ms,
        selected: 0,
        name_input,
        picker_open: None,
        on_done: Rc::new(on_done),
        on_dismiss: Rc::new(on_dismiss),
        on_open_arb: Rc::new(on_open_arb),
    });

    window.open_unique_moon_dialog("chart-label-edit", cx, move |dialog, _window, cx| {
        let p = MoonPalette::active(cx);
        let content_state = state.clone();
        let footer_state = state.clone();
        let dismiss = state.read(cx).on_dismiss.clone();
        dialog
            // Wide enough for the caption list beside a settings pane that holds a seven-segment
            // control: the two panes are what set this, not the text.
            .w(px(600.0))
            .close_button(true)
            .overlay(true)
            .overlay_closable(true)
            .bg(moon(p.shell_high))
            .border_color(moon(p.border))
            .rounded(design::r_container(cx))
            .text_color(moon(p.text))
            .header(
                div()
                    .w_full()
                    .py_2()
                    .border_b_1()
                    .border_color(moon(p.border))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title.clone()),
            )
            // Every dismissal that is not OK: the ✕, the overlay, Escape. The list the opener
            // closed comes back here, and OK brings it back on its own path.
            .on_cancel(move |_, _, cx: &mut App| {
                dismiss(cx);
                true
            })
            .content(move |content, _window, cx| content.child(dialog_body(&content_state, cx)))
            .footer(dialog_footer(footer_state, p))
    });
}

/// Cancel and OK. Cancel is the dialog's own dismissal; OK hands the edited module back.
fn dialog_footer(state: Entity<LabelEditState>, p: MoonPalette) -> AnyElement {
    h_flex()
        .w_full()
        .justify_end()
        .gap_2()
        .child(
            MoonButton::new("le-cancel")
                .ghost()
                .size(MoonButtonSize::Micro)
                .label(t!("dialogs.cancel").to_string())
                .on_click(move |_, window: &mut Window, cx: &mut App| {
                    window.close_dialog(cx);
                })
                .render(),
        )
        .child(
            MoonButton::new("le-ok")
                .size(MoonButtonSize::Micro)
                .variant(MoonButtonVariant::Blue)
                .label(t!("dialogs.done").to_string())
                .on_click(move |_, window: &mut Window, cx: &mut App| {
                    let (row, on_done, on_dismiss) = {
                        let s = state.read(cx);
                        (s.accepted_row(cx), s.on_done.clone(), s.on_dismiss.clone())
                    };
                    on_done(row, cx);
                    // Reopening is idempotent, so it does not matter whether the close below also
                    // reaches `on_cancel`.
                    on_dismiss(cx);
                    window.close_dialog(cx);
                })
                .render(),
        )
        .text_color(moon(p.text))
        .into_any_element()
}
