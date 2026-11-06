//! The wizard's frame: the window header, the rail of steps, the step's body and its buttons.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonDisclosure, MoonPalette, MoonSteps, MoonWindowFrame, h_flex, rgba_from, v_flex};
use rust_i18n::t;

use super::{Branch, ConnectWizardView};
use crate::backend::core_move::MovePhase;
use crate::backend::core_new::NewPhase;
use crate::design;
use crate::settings::telegram::server_bot::progress::StationProgress;

/// Window header height in unscaled pixels, matching the other tool windows.
const HEADER_H: f32 = 30.0;
/// The rail of steps, unscaled.
const RAIL_W: f32 = 210.0;
/// Below this content width the rail is hidden and the step's own title carries the place.
const RAIL_MIN_WINDOW_W: f32 = 640.0;

/// One step as the frame shows it.
pub(super) struct Screen {
    pub(super) title: String,
    pub(super) sub: Option<String>,
    pub(super) body: Vec<AnyElement>,
    /// The buttons at the footer's left: going back, giving up.
    pub(super) left: Vec<AnyElement>,
    /// The buttons at the footer's right: going on.
    pub(super) right: Vec<AnyElement>,
    /// Whether the install's progress log belongs under the body.
    pub(super) log: bool,
}

impl Screen {
    pub(super) fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            sub: None,
            body: Vec::new(),
            left: Vec::new(),
            right: Vec::new(),
            log: false,
        }
    }

    pub(super) fn sub(mut self, sub: impl Into<String>) -> Self {
        self.sub = Some(sub.into());
        self
    }

    pub(super) fn child(mut self, child: impl IntoElement) -> Self {
        self.body.push(child.into_any_element());
        self
    }

    pub(super) fn left(mut self, button: impl IntoElement) -> Self {
        self.left.push(button.into_any_element());
        self
    }

    pub(super) fn right(mut self, button: impl IntoElement) -> Self {
        self.right.push(button.into_any_element());
        self
    }

    pub(super) fn with_log(mut self) -> Self {
        self.log = true;
        self
    }
}

impl Render for ConnectWizardView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.take_move_request(cx);
        if let Some(dir) = self.pending_dir.take() {
            self.server.set_dir(&dir, window, cx);
        }
        let p = MoonPalette::active(cx);
        let chrome_width = f32::from(window.viewport_size().width);
        let screen = match self.branch {
            None => self.start_screen(cx),
            Some(Branch::NewCore) => self.new_core_screen(window, cx),
            Some(Branch::ByKey) => self.by_key_screen(cx),
            Some(Branch::Move) => self.move_screen(cx),
        };
        let (steps, current) = self.rail(cx);
        let show_rail = chrome_width >= design::ui_value(cx, RAIL_MIN_WINDOW_W);
        let lines = match (screen.log, self.branch) {
            (false, _) => Vec::new(),
            (true, Some(Branch::Move)) => self
                .move_uid
                .and_then(|uid| self.backend.read(cx).core_moves.get(uid))
                .map(|job| job.lines.clone())
                .unwrap_or_default(),
            (true, _) => self
                .backend
                .read(cx)
                .core_new
                .as_ref()
                .map(|job| job.lines.clone())
                .unwrap_or_default(),
        };
        let mut main = v_flex()
            .id("cw-main")
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .overflow_y_scroll()
            .p(design::ui_px(cx, 22.0))
            .gap(design::ui_px(cx, 12.0))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(design::t_title(cx))
                    .child(screen.title),
            )
            .when_some(screen.sub, |main, sub| main.child(note(sub, cx)))
            .children(screen.body);
        if let Some(error) = &self.error {
            main = main.child(
                div()
                    .text_color(rgba_from(p.red_text, 1.0))
                    .child(error.clone()),
            );
        }
        // The move's raw log is for whoever needs the technical detail: folded by default.
        let folded_log = self.branch == Some(Branch::Move);
        if !lines.is_empty() && folded_log {
            let open = self.move_log_open;
            main = main.child(
                h_flex()
                    .id("cw-move-log-toggle")
                    .items_center()
                    .gap(design::ui_px(cx, 6.0))
                    .cursor_pointer()
                    .text_size(design::t_caption(cx))
                    .text_color(rgba_from(p.accent, 1.0))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.move_log_open = !this.move_log_open;
                        cx.notify();
                    }))
                    .child(
                        MoonDisclosure::glyph(open)
                            .size(design::DISCLOSURE_GLYPH)
                            .color(p.accent),
                    )
                    .child(t!("connect_wizard.move.details").to_string()),
            );
        }
        if !lines.is_empty() && (!folded_log || self.move_log_open) {
            main = main.child(StationProgress {
                status: None,
                lines,
                scroll: self.log_scroll.clone(),
            });
        }
        // Under the Facet skin `MoonRoot` paints the window backdrop behind this view: the root
        // and the step rail paint no fill over it, and the rail's edge is a rule that fades
        // toward the top and the bottom of the window.
        let facet = design::is_facet(cx);
        v_flex()
            .size_full()
            .relative()
            .when(!facet, |this| this.bg(rgba_from(p.shell, 1.0)))
            .font_family(design::ui_font())
            .text_size(design::t_body(cx))
            .line_height(design::line_px(cx, 14.0))
            .text_color(rgba_from(p.text, 1.0))
            .child(header(p, cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .when(show_rail, |row| {
                        row.child(
                            div()
                                .flex_none()
                                .w(design::ui_px(cx, RAIL_W))
                                .h_full()
                                .p(design::ui_px(cx, 16.0))
                                .when(!facet, |this| this.bg(rgba_from(p.shell_high, 1.0)))
                                .border_r(px(1.0))
                                .border_color(rgba_from(p.border, 1.0))
                                .when(facet, |this| {
                                    design::facet_right_rule(
                                        this,
                                        design::facet_ink(p, design::FACET_CHROME_RULE),
                                    )
                                })
                                .child(
                                    MoonSteps::new("cw-steps")
                                        .steps(steps)
                                        .current(current)
                                        .vertical(true),
                                ),
                        )
                    })
                    .child(main),
            )
            .child(footer(screen.left, screen.right, p, cx))
            .child(
                MoonWindowFrame::tool("cw-window-frame-hit", chrome_width)
                    .controls(moon_ui::MoonWindowFrameControls::MinimizeMaximizeClose)
                    .header_height(HEADER_H)
                    .leading_inset(design::titlebar_leading_inset())
                    .show_controls(design::show_custom_window_controls())
                    .hit_overlay(),
            )
    }
}

impl ConnectWizardView {
    /// The rail's steps for the branch being taken (or picked) and the index of the current one.
    fn rail(&self, cx: &App) -> (Vec<String>, usize) {
        let first = t!("connect_wizard.rail.way").to_string();
        let branch = self.branch.unwrap_or(self.choice);
        let steps = match branch {
            Branch::NewCore => vec![
                first,
                t!("connect_wizard.rail.server").to_string(),
                t!("connect_wizard.rail.install").to_string(),
                t!("connect_wizard.rail.keys").to_string(),
                t!("connect_wizard.rail.done").to_string(),
            ],
            Branch::ByKey => vec![
                first,
                t!("connect_wizard.rail.key").to_string(),
                t!("connect_wizard.rail.done").to_string(),
            ],
            Branch::Move => vec![
                first,
                t!("connect_wizard.rail.core").to_string(),
                t!("connect_wizard.rail.server").to_string(),
                t!("connect_wizard.rail.move").to_string(),
                t!("connect_wizard.rail.done").to_string(),
            ],
        };
        let current = match self.branch {
            None => 0,
            Some(Branch::NewCore) => match self.job_phase(cx) {
                None
                | Some(
                    NewPhase::Probing
                    | NewPhase::Review { .. }
                    | NewPhase::Checking
                    | NewPhase::Checked { .. }
                    | NewPhase::Failed { .. },
                ) => 1,
                Some(NewPhase::Installing) => 2,
                Some(NewPhase::Keys | NewPhase::LoggingIn | NewPhase::Stuck { .. }) => 3,
                Some(
                    NewPhase::Connecting { .. }
                    | NewPhase::Unreachable
                    | NewPhase::Done
                    | NewPhase::Removing
                    | NewPhase::Removed,
                ) => 4,
            },
            Some(Branch::ByKey) => {
                if self.by_key_uid.is_some() {
                    2
                } else {
                    1
                }
            }
            Some(Branch::Move) => {
                let phase = self
                    .move_uid
                    .and_then(|uid| self.backend.read(cx).core_moves.get(uid))
                    .map(|job| job.phase.clone());
                match phase {
                    None if self.move_at_server => 2,
                    None => 1,
                    Some(MovePhase::Probing | MovePhase::Review { .. }) => 2,
                    Some(MovePhase::Done { .. }) => 4,
                    Some(_) => 3,
                }
            }
        };
        (steps, current)
    }
}

/// Secondary prose in the hint's style.
pub(super) fn note(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_color(rgba_from(MoonPalette::active(cx).text_muted, 1.0))
        .child(text.into())
}

/// A wrapping row of buttons.
pub(super) fn buttons(cx: &App) -> Div {
    h_flex().flex_wrap().gap(design::ui_px(cx, 8.0))
}

/// The footer: going back on the left, going on on the right.
///
/// Under the Facet skin the footer paints no fill, so the window backdrop runs under it, and its
/// top edge is a rule that fades toward the window edges.
pub(super) fn footer(
    left: Vec<AnyElement>,
    right: Vec<AnyElement>,
    p: MoonPalette,
    cx: &App,
) -> Div {
    let facet = design::is_facet(cx);
    h_flex()
        .flex_none()
        .w_full()
        .flex_wrap()
        .justify_between()
        .gap(design::ui_px(cx, 8.0))
        .px(design::ui_px(cx, 18.0))
        .py(design::ui_px(cx, 10.0))
        .when(!facet, |this| this.bg(rgba_from(p.shell_high, 1.0)))
        .border_t(px(1.0))
        .border_color(rgba_from(p.border, 1.0))
        .when(facet, |this| {
            design::facet_top_rule(this, design::facet_ink(p, design::FACET_CHROME_RULE))
        })
        .child(buttons(cx).children(left))
        .child(buttons(cx).children(right))
}

/// The draggable window header.
///
/// Under the Facet skin the header paints no fill of its own, so the window backdrop's top light
/// shows through it, and its bottom border becomes a rule that fades toward the window edges.
fn header(p: MoonPalette, cx: &App) -> impl IntoElement {
    let facet = design::is_facet(cx);
    h_flex()
        .id("cw-window-header")
        .relative()
        .flex_none()
        .w_full()
        .h(design::fit_h_px(cx, HEADER_H, 14.0, 8.0))
        .justify_between()
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        .when(!facet, |this| this.bg(rgba_from(p.shell_high, 1.0)))
        .border_b(px(1.0))
        .border_color(rgba_from(p.border, 1.0))
        .when(facet, |this| {
            design::facet_bottom_rule(this, design::facet_ink(p, design::FACET_CHROME_RULE))
        })
        .child(
            MoonWindowFrame::tool("cw-titlebar-title", 0.0)
                .title_cluster(t!("connect_wizard.window_title").to_string(), cx)
                .h_full()
                .flex_1()
                .min_w_0(),
        )
        .when(design::show_custom_window_controls(), |this| {
            this.child(
                MoonWindowFrame::tool("cw-window-frame-visual", 0.0)
                    .controls(moon_ui::MoonWindowFrameControls::MinimizeMaximizeClose)
                    .header_height(HEADER_H)
                    .show_controls(true)
                    .visual_controls(cx),
            )
        })
}
