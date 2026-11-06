//! The move's six steps as a bordered checklist: a state mark, the step's title and one plain
//! detail line, and the time the step finished. The rows come from the backend's step fold
//! (`backend::core_move::Steps`); this module only words and draws them.

use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonPalette, MoonSpinner, MoonSpinnerSize, h_flex, rgba_from, v_flex};
use rust_i18n::t;

use crate::backend::core_move::{StepId, StepNote, StepRow, StepState};
use crate::design;

/// The state mark's box, unscaled.
const MARK: f32 = 18.0;

/// The step's title in the user's language.
pub(super) fn title(id: StepId) -> String {
    match id {
        StepId::Settings => t!("connect_wizard.move.step.settings"),
        StepId::Server => t!("connect_wizard.move.step.server"),
        StepId::Install => t!("connect_wizard.move.step.install"),
        StepId::StopOld => t!("connect_wizard.move.step.stop_old"),
        StepId::StartNew => t!("connect_wizard.move.step.start_new"),
        StepId::Switch => t!("connect_wizard.move.step.switch"),
    }
    .to_string()
}

/// A step's detail line in the user's language.
pub(super) fn detail(note: &StepNote) -> String {
    let join = |parts: Vec<String>| parts.join(", ");
    match note {
        StepNote::TakingSettings => t!("connect_wizard.move.note.taking").to_string(),
        StepNote::SettingsTaken {
            strategies,
            history,
        } => {
            let mut parts = vec![t!("connect_wizard.move.note.keys").to_string()];
            if let Some(n) = strategies {
                parts.push(t!("connect_wizard.move.note.strategies", n = n).to_string());
            }
            if *history {
                parts.push(t!("connect_wizard.move.note.history").to_string());
            }
            join(parts)
        }
        StepNote::CheckingServer => t!("connect_wizard.move.note.checking").to_string(),
        StepNote::ServerFits { port } => {
            t!("connect_wizard.move.note.server_fits", port = port).to_string()
        }
        StepNote::Installing => t!("connect_wizard.move.note.installing").to_string(),
        StepNote::Installed { dir } => {
            t!("connect_wizard.move.note.installed", dir = dir).to_string()
        }
        StepNote::Removed => t!("connect_wizard.move.note.removed").to_string(),
        StepNote::Stopping => t!("connect_wizard.move.note.stopping").to_string(),
        StepNote::Refused { open_deals } => match open_deals {
            Some(n) => t!("connect_wizard.move.note.refused", n = n).to_string(),
            None => t!("connect_wizard.move.note.refused_unknown").to_string(),
        },
        StepNote::OldStopped => t!("connect_wizard.move.note.old_stopped").to_string(),
        StepNote::Starting => t!("connect_wizard.move.note.starting").to_string(),
        StepNote::Started {
            logged_in,
            strategies,
        } => join(vec![
            if *logged_in {
                t!("connect_wizard.move.note.logged_in")
            } else {
                t!("connect_wizard.move.note.not_logged_in")
            }
            .to_string(),
            if *strategies {
                t!("connect_wizard.move.note.strategies_ok")
            } else {
                t!("connect_wizard.move.note.strategies_unconfirmed")
            }
            .to_string(),
        ]),
        StepNote::Switching => t!("connect_wizard.move.note.switching").to_string(),
        StepNote::Switched => t!("connect_wizard.move.note.switched").to_string(),
        StepNote::Reason(reason) => reason.clone(),
        StepNote::Cancelled => t!("connect_wizard.move.note.cancelled").to_string(),
        StepNote::NotNeeded => t!("connect_wizard.move.note.not_needed").to_string(),
    }
}

/// `at` as `m:ss`.
pub(super) fn clock(at: Duration) -> String {
    let secs = at.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// The six rows in one bordered list.
///
/// Under the Facet skin the list is a card resting on the page.
pub(super) fn checklist(rows: &[StepRow], cx: &App) -> impl IntoElement + use<> {
    let p = MoonPalette::active(cx);
    let mut list = v_flex()
        .debug_selector(|| "cw-move-steps".into())
        .w_full()
        .border_1()
        .border_color(rgba_from(p.border, 1.0))
        .rounded(design::ui_px(cx, 6.0))
        .bg(rgba_from(p.panel, 1.0))
        .when(design::is_facet(cx), |this| {
            design::facet_surface(this, moon_ui::MoonFacetKind::Card, cx)
        });
    for (i, row) in rows.iter().enumerate() {
        list = list.child(step_row(i, row, p, cx));
    }
    list
}

/// One step: its mark, title and detail, and its finish time.
fn step_row(i: usize, row: &StepRow, p: MoonPalette, cx: &App) -> Div {
    let quiet = matches!(row.state, StepState::Waiting | StepState::Skipped);
    let detail_color = match row.state {
        StepState::Hold => p.amber,
        StepState::Failed => p.red_text,
        _ => p.text_muted,
    };
    let text = v_flex()
        .debug_selector(move || format!("cw-move-step-text-{i}"))
        .flex_1()
        .min_w(px(0.0))
        .gap(design::ui_px(cx, 1.0))
        .child(
            div()
                .debug_selector(move || format!("cw-move-step-title-{i}"))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgba_from(if quiet { p.text_muted } else { p.text }, 1.0))
                .child(title(row.id)),
        )
        .children(row.note.as_ref().map(|note| {
            div()
                .debug_selector(move || format!("cw-move-step-detail-{i}"))
                .text_size(design::t_caption(cx))
                .text_color(rgba_from(detail_color, 1.0))
                .child(detail(note))
        }));
    h_flex()
        .debug_selector(move || format!("cw-move-step-{i}"))
        .w_full()
        .items_start()
        .gap(design::ui_px(cx, 10.0))
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 9.0))
        .when(i > 0, |row| {
            row.border_t_1().border_color(rgba_from(p.border, 1.0))
        })
        .child(mark(row.state, p, cx))
        .child(text)
        .children(row.at.map(|at| {
            div()
                .debug_selector(move || format!("cw-move-step-time-{i}"))
                .flex_none()
                .font_family(design::mono())
                .text_size(design::t_caption(cx))
                .text_color(rgba_from(p.text_faint, 1.0))
                .child(clock(at))
        }))
}

/// The state mark: an empty circle, a spinner, a tick, "!", a cross, or a dashed circle.
fn mark(state: StepState, p: MoonPalette, cx: &App) -> AnyElement {
    let size = design::ui_px(cx, MARK);
    let icon = |path: &'static str, color: u32| {
        svg()
            .size(size)
            .text_color(rgba_from(color, 1.0))
            .path(path)
            .into_any_element()
    };
    let ring = || {
        div()
            .size(size)
            .rounded_full()
            .border_1()
            .border_color(rgba_from(p.border, 1.0))
    };
    let mark = match state {
        StepState::Waiting => ring().into_any_element(),
        StepState::Skipped => ring().border_dashed().into_any_element(),
        StepState::Running => MoonSpinner::new()
            .size(MoonSpinnerSize::Custom(f32::from(size)))
            .into_any_element(),
        StepState::Done => icon("icons/circle-check.svg", p.green_text),
        StepState::Hold => icon("icons/triangle-alert.svg", p.amber),
        StepState::Failed => icon("icons/circle-x.svg", p.red_text),
    };
    div()
        .flex_none()
        .size(size)
        .mt(design::ui_px(cx, 1.0))
        .child(mark)
        .into_any_element()
}

#[cfg(test)]
mod tests;
