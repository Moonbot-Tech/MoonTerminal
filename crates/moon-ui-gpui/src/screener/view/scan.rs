//! The Screener window's half of the auto-figures column: the Calculate button and the run it
//! starts, the filter list beside it, and what the header and an empty table say about them.
//!
//! The computation, the answers and the column's order live in `screener::auto_figures`; this file
//! only owns the window state around them. Nothing here runs unless the button is pressed: a press
//! starts one run over the publication's markets in background slices, each slice hands its
//! answers back over the task, and the window — this window only — repaints with them.

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_chart::auto_figures::scan::{ScanFilter, ScanRow};
use moon_core::config::ChartTabKind;
use moon_core::config::auto_figures::AutoEvent;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonInput, MoonMenuItem,
    MoonPalette, h_flex,
};
use rust_i18n::t;

use super::super::auto_figures::{
    AUTO_FIGURES_KEY, Answer, CacheSource, SLICE, ScanLabels, ScanQuery, ScanWork, scan_targets,
};
use super::{CHOICE_MENU_W, CHOICE_TRIGGER_MIN_W, ScreenerView, choice_caption};
use crate::design;

/// The approach distance the filter uses while its box is empty, in %.
pub(super) const DEFAULT_APPROACH_PCT: f32 = 0.5;

/// What the auto-figures list offers. One choice at a time: each is a different question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScanChoice {
    Any,
    Squeeze,
    /// Within the box's percentage of the nearest line.
    Approach,
    Breakout,
    FalseBreak,
}

/// The list's choices, in menu order.
const SCAN_CHOICES: [ScanChoice; 5] = [
    ScanChoice::Any,
    ScanChoice::Squeeze,
    ScanChoice::Approach,
    ScanChoice::Breakout,
    ScanChoice::FalseBreak,
];

impl ScanChoice {
    /// The scan's filter for this choice; `approach_pct` is what the box holds
    /// ([`approach_pct`]).
    pub(super) fn filter(self, approach_pct: f32) -> ScanFilter {
        match self {
            Self::Any => ScanFilter::Any,
            Self::Squeeze => ScanFilter::Squeeze,
            Self::Approach => ScanFilter::Approach {
                max_pct: approach_pct,
            },
            Self::Breakout => ScanFilter::Breakout,
            Self::FalseBreak => ScanFilter::FalseBreak,
        }
    }

    /// The choice's caption in the list: the event's own name, as the chart and the column print
    /// it, and "all" for no filter.
    fn label(self) -> String {
        let event = |e: AutoEvent| t!(crate::chartdx::auto_figures_sync::caption::event_key(e));
        match self {
            Self::Any => t!("report.filter.all"),
            Self::Squeeze => event(AutoEvent::Squeeze),
            Self::Approach => t!("screener.af_approach"),
            Self::Breakout => event(AutoEvent::Breakout),
            Self::FalseBreak => event(AutoEvent::FalseBreak),
        }
        .to_string()
    }
}

/// The approach box's percentage: [`DEFAULT_APPROACH_PCT`] while it is empty, the number typed
/// (a comma read as the decimal point) otherwise, and not-a-number for text that is not one — which
/// the scan's filter passes nothing for, so a mistyped box empties the table instead of silently
/// filtering by some other distance.
pub(super) fn approach_pct(text: &str) -> f32 {
    let text = text.trim();
    if text.is_empty() {
        return DEFAULT_APPROACH_PCT;
    }
    text.replace(',', ".").parse().unwrap_or(f32::NAN)
}

impl ScreenerView {
    /// The filter the row selection applies, with the latest press's answers.
    pub(super) fn scan_query(&self, cx: &App) -> ScanQuery {
        let pct = approach_pct(&self.scan_pct_input.read(cx).value());
        ScanQuery::new(self.scan_book.clone(), self.scan_choice.filter(pct))
    }

    /// The scan rows of Main's auto-figure set: its drawn rows, as `ScanRow::from_rows` keeps them.
    ///
    /// Main's master switch is not consulted: it stops the detectors on the chart, and the button is
    /// an explicit request to run them here.
    fn scan_rows(&self, cx: &App) -> Vec<ScanRow> {
        let b = self.backend.read(cx);
        ScanRow::from_rows(&b.layout.auto_figures_for(ChartTabKind::Main).rows)
    }

    /// Start a run over every (provider, market) of the current publication — replacing the one
    /// in progress, if any.
    ///
    /// A second press restarts rather than being refused: the markets may have changed since the
    /// first (a core connected), and "Calculate" means "as of now". The old run's task is dropped
    /// here, which stops it at its next slice boundary, and `ScanBook::begin` numbers the new run so
    /// a slice of the old one that still arrives is not taken.
    pub(super) fn start_scan(&mut self, cx: &mut Context<Self>) {
        let rows = self.scan_rows(cx);
        // Nothing published yet: a run over no markets would stamp the header and answer nothing,
        // leaving the rows that land a moment later blank until a second press.
        if rows.is_empty() || self.prepared.entries.is_empty() {
            return;
        }
        let targets = scan_targets(&self.prepared.entries);
        let now_ms = moon_core::util::time::now_unix_ms_i64();
        let run = Rc::make_mut(&mut self.scan_book).begin(now_ms, targets.len());
        let source = CacheSource::new(self.backend.read(cx).session.market_source());
        let mut carried = (ScanWork::new(targets, rows, now_ms), source);
        self.scan_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let (work, source, answers) = cx
                    .background_spawn(async move {
                        let (mut work, source) = carried;
                        let answers = work.step(&source, SLICE);
                        (work, source, answers)
                    })
                    .await;
                let finished = work.finished();
                let taken = this.update(cx, |this, cx| this.take_scan_slice(run, answers, cx));
                if taken.is_err() || finished {
                    return;
                }
                carried = (work, source);
            }
        }));
        self.after_scan_change(cx);
    }

    /// Take one slice of run `run`; a slice of a run a newer press replaced is dropped.
    fn take_scan_slice(&mut self, run: u64, answers: Vec<Answer>, cx: &mut Context<Self>) {
        if Rc::make_mut(&mut self.scan_book).apply(run, answers) {
            self.after_scan_change(cx);
        }
    }

    /// Repaint after the answers moved: the rows are re-selected only when the filter or the
    /// column's sort reads the answers — every other control's result is the same as before.
    fn after_scan_change(&mut self, cx: &mut Context<Self>) {
        if self.scan_choice != ScanChoice::Any || self.sort_key == AUTO_FIGURES_KEY {
            self.filter_cached(cx);
        }
        cx.notify();
    }

    /// Pick an entry of the auto-figures list.
    fn set_scan_choice(&mut self, choice: ScanChoice, cx: &mut Context<Self>) {
        if self.scan_choice == choice {
            return;
        }
        self.scan_choice = choice;
        self.filter_cached(cx);
        cx.notify();
    }

    /// The approach box changed: only the approach choice reads it.
    pub(super) fn scan_pct_changed(&mut self, cx: &mut Context<Self>) {
        if self.scan_choice == ScanChoice::Approach {
            self.filter_cached(cx);
            cx.notify();
        }
    }

    /// The column's header: its name, and the time of the latest press — the moment its closed
    /// bars were judged at — in the selected display zone.
    pub(super) fn scan_column_title(&self) -> SharedString {
        let name = t!("auto_figures.title").to_string();
        match self.scan_book.as_of_ms() {
            Some(at) => {
                let clock = moon_core::util::display_time::format_chart_clock(
                    at,
                    crate::chartdx::axes::display_zone(),
                    false,
                    moon_core::util::time::now_unix_ms_i64(),
                );
                format!("{name} {clock}").into()
            }
            None => name.into(),
        }
    }

    /// The column's words, for the row style of one table build.
    pub(super) fn scan_labels() -> ScanLabels {
        ScanLabels::new(
            |event| t!(crate::chartdx::auto_figures_sync::caption::event_key(event)).to_string(),
            t!("screener.af_nothing").to_string(),
            t!("screener.af_short").to_string(),
            t!("screener.af_no_cache").to_string(),
            t!("screener.af_unread").to_string(),
            |lag_ms| {
                let age = crate::chartdx::auto_figures_sync::caption::age_text(lag_ms as f64);
                t!("screener.af_stale", age = age).to_string()
            },
        )
    }

    /// What an empty table says when the auto-figures filter is what can leave it empty and has
    /// nothing to go by yet: no press so far, or the run still being worked off. `None` otherwise.
    pub(super) fn scan_empty_text(&self) -> Option<String> {
        if self.scan_choice == ScanChoice::Any || !self.has_displayed_rows {
            return None;
        }
        if self.scan_book.as_of_ms().is_none() {
            Some(t!("screener.af_press_calculate").to_string())
        } else if self.scan_book.progress().is_some() {
            Some(t!("screener.af_running").to_string())
        } else {
            None
        }
    }

    /// The bottom bar's auto-figures controls: the list, the approach box while that choice is
    /// picked, and the Calculate button with the run's progress.
    pub(super) fn scan_controls(&self, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        let approach = self.scan_choice == ScanChoice::Approach;
        h_flex()
            .flex_none()
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .child(self.scan_menu(cx))
            .when(approach, |row| {
                row.child(
                    div().w(design::ui_px(cx, 44.0)).child(
                        MoonInput::new("scr-af-pct")
                            .state(&self.scan_pct_input)
                            .size(design::INPUT_SIZE),
                    ),
                )
                .child(
                    div()
                        .text_size(design::t_body(cx))
                        .text_color(rgb(p.text_soft))
                        .child("%"),
                )
            })
            .child(self.scan_button(cx))
    }

    /// The auto-figures list: one choice at a time, its button sized once for the longest caption,
    /// in the same family as the Type and Listed lists beside it.
    fn scan_menu(&self, cx: &Context<Self>) -> MoonDropdown {
        let title = t!("auto_figures.title").to_string();
        let choices = SCAN_CHOICES.map(|choice| (choice, choice.label()));
        let captions: Vec<String> = choices
            .iter()
            .map(|(_, label)| choice_caption(&title, label))
            .collect();
        let size = MoonButtonSize::density(cx);
        let trigger_w = crate::controls::trigger_fit::widest_design(
            cx,
            captions.iter().map(String::as_str),
            size,
            CHOICE_TRIGGER_MIN_W,
        );
        let current = choices
            .iter()
            .find(|(choice, _)| *choice == self.scan_choice)
            .map_or_else(String::new, |(_, label)| choice_caption(&title, label));
        let mut menu = MoonDropdown::new("screener-af-filter")
            .label(current)
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(size)
            .fit_trigger_width(trigger_w, trigger_w)
            .fit_menu_width(CHOICE_MENU_W.0, CHOICE_MENU_W.1);
        for (index, (choice, label)) in choices.into_iter().enumerate() {
            let on = choice == self.scan_choice;
            let view = cx.entity();
            menu = menu.item(
                MoonMenuItem::with_key(format!("screener-af-filter-{index}"), label)
                    .checked(on)
                    .selected(on)
                    .on_click(move |_, _, app| {
                        view.update(app, |this, cx| this.set_scan_choice(choice, cx));
                    }),
            );
        }
        menu
    }

    /// The Calculate button: "Calculate" when idle, the run's `done / total` while one is being
    /// worked off (a press then restarts it), disabled while Main's set has no row to scan or no
    /// market has been published yet.
    fn scan_button(&self, cx: &Context<Self>) -> impl IntoElement {
        let has_rows = !self.scan_rows(cx).is_empty();
        let has_markets = !self.prepared.entries.is_empty();
        let (label, tip) = match self.scan_book.progress() {
            Some((done, total)) => (
                t!("screener.af_progress", done = done, total = total).to_string(),
                t!("screener.af_restart_tip").to_string(),
            ),
            None => (
                t!("screener.af_calculate").to_string(),
                t!("screener.af_calculate_tip").to_string(),
            ),
        };
        let tip = if !has_rows {
            t!("screener.af_no_rows_tip").to_string()
        } else if !has_markets {
            t!("screener.empty_no_data").to_string()
        } else {
            tip
        };
        let view = cx.entity();
        MoonButton::new("screener-af-calculate")
            .label(label)
            .size(MoonButtonSize::density(cx))
            .variant(MoonButtonVariant::Soft)
            .disabled(!has_rows || !has_markets)
            .tooltip(tip)
            .on_click(move |_, _, app| {
                view.update(app, |this, cx| this.start_scan(cx));
            })
            .render()
    }
}

#[cfg(test)]
mod tests;
