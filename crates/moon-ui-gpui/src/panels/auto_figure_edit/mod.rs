//! The auto-figure row editor: one dialog that owns everything ABOUT a row, while the auto-figures
//! popup keeps only the list — order, visibility, removal.
//!
//! A row is a detector with up to eleven numbers, a timeframe, a selection, a line style, a
//! caption and five event marks. Inside a popover that would be a list of expanders; here the
//! left pane answers "what does this row look for", the right one "how does it look", and the
//! preview at the bottom shows both at once on demo candles, through the chart's own detector and
//! figure builder (see [`preview`]).
//!
//! It edits a COPY, exactly like the captions editor (`panels::label_edit`): Cancel, the ✕ and
//! the overlay discard it, and only OK writes — through [`write_auto_figures`], the one door every
//! auto-figure edit takes, which sanitizes and persists.
//!
//! Storage is per tab KIND (`WindowLayout::auto_figures_for` / `set_auto_figures_default`), not
//! per tab, so the editor needs no host: a kind and a row index name the row completely. That is
//! what lets the chart's own right-click menu open it as well ([`open_auto_figure_row_editor`]).

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use moon_core::config::ChartTabKind;
use moon_core::config::ChartTheme;
use moon_core::config::auto_figures::{
    AF_ROWS_MAX, AutoFigureRow, AutoFiguresCfg, DetectorKind, DetectorParams, EventMarks,
    LineCaption, RowDetector,
};
use moon_ui::{
    MoonButton, MoonButtonVariant, MoonInputEvent, MoonInputState, MoonPalette, MoonWindowExt as _,
    h_flex,
};
use rust_i18n::t;

use crate::Backend;
use crate::design::{self, moon};

mod body;
mod fields;
mod preview;

#[cfg(test)]
mod tests;

use crate::chartdx::auto_figures_sync::RowTitle;
/// The detector-kind vocabulary is the chart's own, shared with the captions and the hover card.
pub(crate) use crate::chartdx::auto_figures_sync::caption::kind_key;
pub(crate) use body::tf_source_label;
use fields::{NumField, parse_number};
pub(crate) use preview::line_swatch;

/// Write one kind's auto-figure configuration: read it, change it, sanitize it, store it.
///
/// The single write door for the popup and the editor. Sanitizing on every write is what keeps an
/// impossible row from being persisted at all; storing through `set_auto_figures_default` keeps
/// the other kinds where they were (its separation pass), and `layout_dirty` hands the file to the
/// debounced save. The backend notification is what makes every chart panel re-read its kind's
/// rows — each one observes the backend and compares the configuration it last applied.
///
/// Args:
///     backend: Application state holding the layout.
///     kind: The kind of chart whose rows are edited.
///     cx: Application context.
///     f: The edit.
///
/// Returns:
///     Whether the stored configuration actually changed.
pub(crate) fn write_auto_figures(
    backend: &Entity<Backend>,
    kind: ChartTabKind,
    cx: &mut App,
    f: impl FnOnce(&mut AutoFiguresCfg),
) -> bool {
    backend.update(cx, |b, bcx| {
        let mut cfg = b.layout.auto_figures_for(kind).clone();
        // Sanitized BEFORE the edit as well: an edit locates rows by comparing them with what the
        // popup or the editor read, and those always read a sanitized copy. Idempotent, so a
        // stored set that is already clean is unchanged by it.
        cfg.sanitize();
        f(&mut cfg);
        cfg.sanitize();
        let moved = b.layout.set_auto_figures_default(kind, cfg);
        if moved {
            b.layout_dirty = true;
            bcx.notify();
        }
        moved
    })
}

/// The name a row is listed and titled by: the user's own, else its preset's in the active
/// language, else its detector kind's — the same rule the chart's captions and hover card print
/// by ([`RowTitle`]); a row of a detector this build lacks says so with the kind the newer build
/// wrote.
pub(crate) fn row_title(row: &AutoFigureRow) -> String {
    match &row.detector {
        RowDetector::Known(params) => RowTitle::of(row, params.kind()).text(),
        RowDetector::Unknown(_) if !row.name.is_empty() => row.name.clone(),
        RowDetector::Unknown(u) => match row.title_key() {
            Some(key) => t!(key).to_string(),
            None => t!("auto_figures.unknown", kind = u.kind()).to_string(),
        },
    }
}

/// Working state of the open editor.
pub struct AutoFigureEditState {
    /// The edited COPY, never sanitized while the dialog is up (see [`NumField::set`]); OK
    /// sanitizes what it hands back.
    row: AutoFigureRow,
    name_input: Entity<MoonInputState>,
    /// One input per numeric field, created up front: which ones are SHOWN depends on the detector
    /// kind, and switching kinds must not need a window to build new ones.
    nums: Vec<(NumField, Entity<MoonInputState>)>,
    /// The chart theme the dialog was opened under, for the preview's and the swatch's colours.
    theme: ChartTheme,
    /// The demo series' reference time, fixed at open so the preview does not drift while edited.
    demo_now_ms: f64,
    /// The last preview and the sanitized row it was computed for; recomputed only when an edit
    /// moved that row, never on a repaint.
    preview: Option<(AutoFigureRow, Rc<Option<preview::PreviewData>>)>,
    on_done: Rc<dyn Fn(AutoFigureRow, &mut App)>,
    /// Run when the dialog goes away, whichever way: the opener brings its popup back with it.
    on_dismiss: Rc<dyn Fn(&mut App)>,
}

impl AutoFigureEditState {
    /// The row as OK hands it back: the name folded in, then sanitized.
    fn accepted_row(&self, cx: &App) -> AutoFigureRow {
        let mut row = self.row.clone();
        row.name = self.name_input.read(cx).value().trim().to_string();
        row.sanitize();
        row
    }

    /// The preview for the row as it stands, computed at most once per distinct sanitized row.
    fn preview(&mut self) -> Rc<Option<preview::PreviewData>> {
        let mut key = self.row.clone();
        key.sanitize();
        // The preview draws the detector's lines on demo candles in the row's style, selected by
        // its timeframe and `take`: the name, the preset label, the caption and the event marks
        // change none of that, so an edit to them must not recompute it.
        key.name.clear();
        key.preset = None;
        key.caption = LineCaption::default();
        key.events = EventMarks::default();
        if let Some((cached, data)) = &self.preview
            && *cached == key
        {
            return data.clone();
        }
        let data = Rc::new(preview::compute(&key, self.demo_now_ms, &self.theme));
        self.preview = Some((key, data.clone()));
        data
    }
}

/// Put every numeric input back to `row`'s sanitized values, e.g. after the detector kind changed.
fn reseed_inputs(
    row: &AutoFigureRow,
    nums: &[(NumField, Entity<MoonInputState>)],
    window: &mut Window,
    cx: &mut App,
) {
    let mut sanitized = row.clone();
    sanitized.sanitize();
    for (field, input) in nums {
        let text = field
            .get(&sanitized)
            .map(|v| field.display(v))
            .unwrap_or_default();
        input.update(cx, |st, c| st.set_value(text, window, c));
    }
}

/// Open the editor on the row at `row_ix` of `kind`'s auto figures, and write back what OK
/// accepts.
///
/// The entry point every opener shares: the auto-figures popup (a click on a row's name) and the
/// chart's right-click "Configure row…". Storage is per tab kind, so the kind and the index are
/// all it needs.
///
/// The write is guarded against the list moving under the open dialog — another window removing
/// or reordering rows: the row is written where the ORIGINAL still stands (at `row_ix` when it is
/// still there, else wherever it moved), and dropped with a log line when it is gone.
///
/// Args:
///     backend: Application state holding the layout.
///     kind: The kind of chart whose row is edited.
///     row_ix: The row's index in that kind's configuration.
///     window: The window the dialog opens over.
///     cx: Application context.
///     on_dismiss: Run when the dialog closes, whichever way — and NOT run when nothing opened.
///
/// Returns:
///     Whether a dialog opened. `false` for an index past the list and for a row whose detector
///     this build does not know: such a row can be moved or removed, never edited.
pub(crate) fn open_auto_figure_row_editor(
    backend: &Entity<Backend>,
    kind: ChartTabKind,
    row_ix: usize,
    window: &mut Window,
    cx: &mut App,
    on_dismiss: impl Fn(&mut App) + 'static,
) -> bool {
    let original = {
        let mut cfg = backend.read(cx).layout.auto_figures_for(kind).clone();
        cfg.sanitize();
        cfg.rows.get(row_ix).cloned()
    };
    let Some(original) = original else {
        return false;
    };
    if original.detector.known().is_none() {
        return false;
    }
    let title = format!("{}: {}", t!("auto_figures.row_edit"), row_title(&original));
    let target = backend.clone();
    let kept = original.clone();
    open_editor(
        backend,
        title,
        original,
        window,
        cx,
        move |edited, cx| {
            let mut landed = true;
            write_auto_figures(&target, kind, cx, |cfg| {
                let at = match cfg.rows.get(row_ix) == Some(&kept) {
                    true => Some(row_ix),
                    false => cfg.rows.iter().position(|r| *r == kept),
                };
                match at {
                    Some(at) => cfg.rows[at] = edited,
                    None => landed = false,
                }
            });
            if !landed {
                log::warn!(
                    "auto figures: the row was removed while its editor was open; edit dropped"
                );
            }
        },
        on_dismiss,
    );
    true
}

/// Open the editor on a NEW row of `detector`, added to `kind` only if OK accepts it.
///
/// Nothing is written before OK, so a Cancel leaves no row behind — the same reason the captions
/// popup's "new module" reserves no slot.
pub(crate) fn open_new_auto_figure_row(
    backend: &Entity<Backend>,
    kind: ChartTabKind,
    detector: DetectorKind,
    window: &mut Window,
    cx: &mut App,
    on_dismiss: impl Fn(&mut App) + 'static,
) {
    let row = AutoFigureRow::new(detector.default_params());
    let title = t!("auto_figures.new_row", kind = t!(kind_key(detector))).to_string();
    let target = backend.clone();
    open_editor(
        backend,
        title,
        row,
        window,
        cx,
        move |edited, cx| {
            let mut full = false;
            write_auto_figures(&target, kind, cx, |cfg| {
                if cfg.rows.len() >= AF_ROWS_MAX {
                    full = true;
                } else {
                    cfg.rows.push(edited);
                }
            });
            if full {
                log::warn!("auto figures: the new row was not added, the list is full");
            }
        },
        on_dismiss,
    );
}

/// Put the dialog up on `row` and hand what OK accepts to `on_done`.
fn open_editor(
    backend: &Entity<Backend>,
    title: String,
    row: AutoFigureRow,
    window: &mut Window,
    cx: &mut App,
    on_done: impl Fn(AutoFigureRow, &mut App) + 'static,
    on_dismiss: impl Fn(&mut App) + 'static,
) {
    // The theme the chart draws with: the Settings draft while one is open, as the chart reads it.
    let theme = {
        let b = backend.read(cx);
        b.preview
            .as_ref()
            .unwrap_or(&b.config)
            .chart_theme()
            .clone()
    };
    let name_input = cx.new(|cx| MoonInputState::new(window, cx));
    name_input.update(cx, |st, c| st.set_value(row.name.clone(), window, c));
    let mut sanitized = row.clone();
    sanitized.sanitize();
    let nums: Vec<(NumField, Entity<MoonInputState>)> = NumField::ALL
        .into_iter()
        .map(|field| {
            let input = cx.new(|cx| MoonInputState::new(window, cx));
            let text = field
                .get(&sanitized)
                .map(|v| field.display(v))
                .unwrap_or_default();
            input.update(cx, |st, c| st.set_value(text, window, c));
            (field, input)
        })
        .collect();
    let state = cx.new(|cx: &mut Context<AutoFigureEditState>| {
        for (field, input) in &nums {
            let field = *field;
            cx.subscribe_in(
                input,
                window,
                move |this: &mut AutoFigureEditState, input, ev: &MoonInputEvent, window, cx| {
                    match ev {
                        // Every keystroke that parses moves the row, so the preview follows the
                        // typing; one that does not parse (mid-edit "0,") leaves it alone.
                        MoonInputEvent::Change => {
                            let text = input.read(cx).value().to_string();
                            if let Some(v) = parse_number(&text, field.integer())
                                && field.set(&mut this.row, v)
                            {
                                cx.notify();
                            }
                        }
                        // Leaving the field shows the value that took effect — the sanitized one,
                        // so "0" typed into a lookback reads back as the minimum it became.
                        MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. } => {
                            let mut sanitized = this.row.clone();
                            sanitized.sanitize();
                            if let Some(v) = field.get(&sanitized) {
                                field.set(&mut this.row, v);
                                let text = field.display(v);
                                input.update(cx, |st, c| st.set_value(text, window, c));
                            }
                            cx.notify();
                        }
                        MoonInputEvent::Focus => {}
                    }
                },
            )
            .detach();
        }
        AutoFigureEditState {
            row,
            name_input,
            nums,
            theme,
            demo_now_ms: crate::chartdx::now_unix_ms(),
            preview: None,
            on_done: Rc::new(on_done),
            on_dismiss: Rc::new(on_dismiss),
        }
    });

    window.open_unique_moon_dialog("auto-figure-edit", cx, move |dialog, window, cx| {
        let p = MoonPalette::active(cx);
        let content_state = state.clone();
        let footer_state = state.clone();
        let dismiss = state.read(cx).on_dismiss.clone();
        // Never wider than the window with a gutter: MoonUI centres the dialog on a fixed width,
        // so a width past the viewport would put both edges off screen. The panes inside wrap.
        let viewport = window.viewport_size();
        let width = (f32::from(viewport.width) - 32.0).clamp(320.0, body::DIALOG_W);
        let max_body_h = (f32::from(viewport.height) * 0.72).max(240.0);
        dialog
            .w(px(width))
            .close_button(true)
            .overlay(true)
            .overlay_closable(true)
            .when(!design::is_facet(cx), |dialog| {
                dialog.bg(moon(p.shell_high)).border_color(moon(p.border))
            })
            .rounded(design::r_container(cx))
            .text_color(moon(p.text))
            .header(
                div()
                    .w_full()
                    .py_2()
                    .border_b_1()
                    .border_color(moon(p.border))
                    .when(design::is_facet(cx), |header| {
                        design::facet_bottom_rule(
                            header,
                            design::facet_ink(p, design::FACET_PANEL_RULE),
                        )
                    })
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title.clone()),
            )
            .on_cancel(move |_, _, cx: &mut App| {
                dismiss(cx);
                true
            })
            .content(move |content, _window, cx| {
                content.child(body::dialog_body(&content_state, width, max_body_h, cx))
            })
            .footer(dialog_footer(footer_state, p))
    });
}

/// Cancel and OK. Cancel discards the copy; OK hands the edited row back.
///
/// Both run `on_dismiss` themselves: `close_dialog` does not go through the dialog's `on_cancel`
/// (measured: the popup stayed away after Cancel), and bringing the popup back is idempotent, so
/// a path that does reach `on_cancel` as well costs nothing.
fn dialog_footer(state: Entity<AutoFigureEditState>, p: MoonPalette) -> AnyElement {
    let cancel_state = state.clone();
    h_flex()
        .w_full()
        .justify_end()
        .gap_2()
        .child(
            MoonButton::new("afe-cancel")
                .ghost()
                .label(t!("dialogs.cancel").to_string())
                .on_click(move |_, window: &mut Window, cx: &mut App| {
                    let on_dismiss = cancel_state.read(cx).on_dismiss.clone();
                    on_dismiss(cx);
                    window.close_dialog(cx);
                })
                .render(),
        )
        .child(
            MoonButton::new("afe-ok")
                .variant(MoonButtonVariant::Blue)
                .label(t!("dialogs.done").to_string())
                .on_click(move |_, window: &mut Window, cx: &mut App| {
                    let (row, on_done, on_dismiss) = {
                        let s = state.read(cx);
                        (s.accepted_row(cx), s.on_done.clone(), s.on_dismiss.clone())
                    };
                    on_done(row, cx);
                    on_dismiss(cx);
                    window.close_dialog(cx);
                })
                .render(),
        )
        .text_color(moon(p.text))
        .into_any_element()
}

/// Switch the edited row to another detector kind, starting from that kind's defaults.
///
/// The timeframe is pulled under the new kind's coarsest, as sanitize would on OK — doing it now
/// keeps the timeframe dropdown from showing a choice the new kind cannot answer on.
fn switch_detector(row: &mut AutoFigureRow, kind: DetectorKind) {
    if row.detector.known().map(DetectorParams::kind) == Some(kind) {
        return;
    }
    let params = kind.default_params();
    if let moon_core::config::auto_figures::TfSource::Fixed(tf) = &mut row.tf {
        *tf = (*tf).min(params.coarsest_tf());
    }
    row.detector = RowDetector::Known(params);
}
