//! The "move a core to a Linux server" branch: which core, the server (the same step a new core
//! has), the move, and the result. The move itself is the backend's (`backend::core_move`): the
//! settings snapshot, the report database from the replica, the install without a start, the old
//! core's stop, the new core's start, the strategies, and the switch of this terminal and the
//! station. These screens show its phases and pass the user's answers to it.

use gpui::*;
use moon_ui::{
    MoonButton, MoonCheckbox, MoonPalette, MoonSelectableCard, MoonSpinner, h_flex, rgba_from,
    v_flex,
};
use rust_i18n::t;

use super::ConnectWizardView;
use super::render::{Screen, note};
use crate::backend::core_move::{Command, MoveForm, MovePhase};
use crate::design;

impl ConnectWizardView {
    pub(super) fn move_screen(&mut self, cx: &mut Context<Self>) -> Screen {
        let Some(uid) = self.move_uid else {
            return self.move_pick_screen(cx);
        };
        let phase = self
            .backend
            .read(cx)
            .core_moves
            .get(uid)
            .map(|job| job.phase.clone());
        match phase {
            None if !self.move_at_server => self.move_pick_screen(cx),
            None => self.move_server_screen(uid, cx),
            Some(phase) => self.move_phase_screen(uid, phase, cx),
        }
    }

    /// Which core moves, and what moves with it.
    fn move_pick_screen(&mut self, cx: &mut Context<Self>) -> Screen {
        let cores = {
            let b = self.backend.read(cx);
            let venues = b.session.core_venues();
            let cores: Vec<(u64, String, Option<String>)> = b
                .config
                .servers
                .iter()
                .filter(|s| s.uid != 0)
                .map(|s| {
                    (
                        s.uid,
                        s.name.clone(),
                        venues.get(&s.id).map(crate::controls::venue_label),
                    )
                })
                .collect();
            cores
        };
        let mut list = v_flex().gap(design::ui_px(cx, 6.0));
        for (uid, name, venue) in cores {
            list = list.child(
                MoonSelectableCard::new(SharedString::from(format!("cw-move-core-{uid}")))
                    .selected(self.move_uid == Some(uid))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.pick_move_core(uid, window, cx);
                    }))
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap(design::ui_px(cx, 8.0))
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(name))
                            .children(venue.map(|v| note(v, cx))),
                    ),
            );
        }
        let picked = self.move_uid.is_some();
        Screen::new(t!("connect_wizard.move.pick_title"))
            .sub(t!("connect_wizard.move.pick_sub"))
            .child(list)
            .child(
                div()
                    .pt(design::ui_px(cx, 6.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t!("connect_wizard.move.all_title").to_string()),
            )
            .child(what_moves(cx))
            .left(self.back_button(cx))
            .right(
                MoonButton::new("cw-move-next")
                    .primary()
                    .size(design::CONTROL_TIER)
                    .disabled(!picked)
                    .label(t!("connect_wizard.move.next").to_string())
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.move_uid.is_some() {
                            this.move_at_server = true;
                            this.error = None;
                            cx.notify();
                        }
                    }))
                    .render(),
            )
    }

    /// Pick the core to move; the server step offers a folder named after it.
    fn pick_move_core(&mut self, uid: u64, window: &mut Window, cx: &mut Context<Self>) {
        let name = self
            .backend
            .read(cx)
            .config
            .servers
            .iter()
            .find(|s| s.uid == uid)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        if self.move_uid != Some(uid) {
            self.server.fingerprint_ok = false;
        }
        self.move_uid = Some(uid);
        self.server.set_dir(&default_dir(&name), window, cx);
        cx.notify();
    }

    /// The server form, before anything is asked of the server.
    fn move_server_screen(&mut self, uid: u64, cx: &mut Context<Self>) -> Screen {
        let form = self.server.render(
            |this: &mut Self, on, _| this.server.by_key = on,
            |this: &mut Self, window, cx| this.browse_key(window, cx),
            cx,
        );
        Screen::new(t!("connect_wizard.server.title"))
            .sub(t!("connect_wizard.move.server_sub"))
            .child(form)
            .child(note(t!("connect_wizard.server.probe_note"), cx))
            .left(
                MoonButton::new("cw-move-back")
                    .size(design::CONTROL_TIER)
                    .label(t!("connect_wizard.back").to_string())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.move_at_server = false;
                        cx.notify();
                    }))
                    .render(),
            )
            .right(
                MoonButton::new("cw-move-probe")
                    .primary()
                    .size(design::CONTROL_TIER)
                    .label(t!("connect_wizard.server.check").to_string())
                    .on_click(cx.listener(move |this, _, _, cx| this.move_probe(uid, cx)))
                    .render(),
            )
    }

    /// Read the server's host key for the move of `uid`: sends no credential.
    fn move_probe(&mut self, uid: u64, cx: &mut Context<Self>) {
        match self.server.target(cx) {
            Ok(target) => {
                self.error = None;
                self.server.fingerprint_ok = false;
                self.backend
                    .update(cx, |b, bcx| b.core_move_probe(uid, target, bcx));
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    /// Start the move of `uid` against the host key the user confirmed.
    fn move_start(
        &mut self,
        uid: u64,
        target: moon_remote::ssh::Target,
        pin: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let access = match self.server.access(target, cx) {
            Ok(access) => access,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        self.error = None;
        // The login is in the move's hands now; the fields forget it.
        self.server.forget_secrets(window, cx);
        let form = MoveForm {
            target: access.target,
            pin,
            first: access.first,
            public_ip: access.public_ip,
            dir: access.dir,
            proto_port: access.proto_port,
        };
        self.backend
            .update(cx, |b, bcx| b.core_move_start(uid, form, bcx));
    }

    /// Pass an answer to the move that waits on its old core.
    fn move_answer(&mut self, uid: u64, command: Command, cx: &mut Context<Self>) {
        self.backend
            .update(cx, |b, bcx| b.core_move_command(uid, command, bcx));
    }

    /// Drop a finished move and go back to picking a core.
    fn move_start_over(&mut self, uid: u64, cx: &mut Context<Self>) {
        self.backend
            .update(cx, |b, bcx| b.core_move_forget(uid, bcx));
        self.move_at_server = false;
        cx.notify();
    }

    /// Where the move of `uid` stands, with the answers it takes there.
    fn move_phase_screen(&mut self, uid: u64, phase: MovePhase, cx: &mut Context<Self>) -> Screen {
        let answering = self
            .backend
            .read(cx)
            .core_moves
            .get(uid)
            .is_some_and(|job| job.answering);
        let spinner = |text: String, cx: &App| {
            h_flex()
                .items_center()
                .gap(design::ui_px(cx, 8.0))
                .child(MoonSpinner::new())
                .child(text)
        };
        match phase {
            MovePhase::Probing => Screen::new(t!("connect_wizard.server.title"))
                .child(spinner(t!("connect_wizard.server.probing").to_string(), cx)),
            MovePhase::Review {
                target,
                fingerprint,
            } => {
                let ok = self.server.fingerprint_ok;
                let (t2, f2) = (target.clone(), fingerprint.clone());
                Screen::new(t!("connect_wizard.server.title"))
                    .sub(t!("connect_wizard.review.sub", addr = target.addr()))
                    .child(div().font_family(design::mono()).child(fingerprint))
                    .child(
                        MoonCheckbox::new("cw-move-fingerprint")
                            .checked(ok)
                            .label(t!("connect_wizard.review.matches").to_string())
                            .on_change(cx.listener(|this, v: &bool, _, cx| {
                                this.server.fingerprint_ok = *v;
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .text_color(rgba_from(MoonPalette::active(cx).red_text, 1.0))
                            .child(t!("connect_wizard.move.secrets").to_string()),
                    )
                    .child(note(t!("connect_wizard.move.order"), cx))
                    .left(
                        MoonButton::new("cw-move-review-back")
                            .size(design::CONTROL_TIER)
                            .label(t!("connect_wizard.back").to_string())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.move_start_over(uid, cx);
                                this.move_at_server = true;
                            }))
                            .render(),
                    )
                    .right(
                        MoonButton::new("cw-move-start")
                            .primary()
                            .size(design::CONTROL_TIER)
                            .disabled(!ok)
                            .label(t!("connect_wizard.move.start").to_string())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if this.server.fingerprint_ok {
                                    this.move_start(uid, t2.clone(), f2.clone(), window, cx);
                                }
                            }))
                            .render(),
                    )
            }
            MovePhase::Running => Screen::new(t!("connect_wizard.move.running_title"))
                .sub(t!("connect_wizard.move.running_sub"))
                .child(spinner(t!("connect_wizard.move.running").to_string(), cx))
                .with_log(),
            MovePhase::Refused { open_deals } => {
                let mut screen = Screen::new(t!("connect_wizard.move.refused_title"))
                    .sub(t!("connect_wizard.move.refused_sub"))
                    .child(note(
                        match open_deals {
                            Some(n) => t!("connect_wizard.move.refused_deals", n = n),
                            None => t!("connect_wizard.move.refused_unknown"),
                        },
                        cx,
                    ))
                    .child(answer_help(cx))
                    .with_log();
                if !answering {
                    screen = screen
                        .left(self.answer_button(
                            uid,
                            "cw-move-cancel",
                            "connect_wizard.move.cancel",
                            Command::Cancel,
                            false,
                            cx,
                        ))
                        .right(self.answer_button(
                            uid,
                            "cw-move-retry",
                            "connect_wizard.check_again",
                            Command::Retry,
                            false,
                            cx,
                        ))
                        .right(self.answer_button(
                            uid,
                            "cw-move-wait",
                            "connect_wizard.move.wait",
                            Command::WaitForStop,
                            true,
                            cx,
                        ));
                }
                screen
            }
            MovePhase::Waiting => {
                let mut screen = Screen::new(t!("connect_wizard.move.waiting_title"))
                    .sub(t!("connect_wizard.move.waiting_sub"))
                    .child(spinner(t!("connect_wizard.move.waiting").to_string(), cx))
                    .with_log();
                if !answering {
                    screen = screen
                        .left(self.answer_button(
                            uid,
                            "cw-move-cancel",
                            "connect_wizard.move.cancel",
                            Command::Cancel,
                            false,
                            cx,
                        ))
                        .right(self.answer_button(
                            uid,
                            "cw-move-retry",
                            "connect_wizard.check_again",
                            Command::Retry,
                            true,
                            cx,
                        ));
                }
                screen
            }
            MovePhase::Done { address } => Screen::new(t!("connect_wizard.move.done_title"))
                .sub(t!("connect_wizard.move.done_sub"))
                .child(self.done_body(uid, Some(address), None, cx))
                .left(
                    MoonButton::new("cw-move-again")
                        .size(design::CONTROL_TIER)
                        .label(t!("connect_wizard.again").to_string())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.move_start_over(uid, cx);
                            this.back_to_start(cx);
                        }))
                        .render(),
                )
                .right(
                    MoonButton::new("cw-move-finish")
                        .primary()
                        .size(design::CONTROL_TIER)
                        .label(t!("connect_wizard.finish").to_string())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.move_start_over(uid, cx);
                            this.close(window, cx);
                        }))
                        .render(),
                )
                .with_log(),
            MovePhase::Failed { reason } => Screen::new(t!("connect_wizard.move.failed_title"))
                .child(
                    div()
                        .text_color(rgba_from(MoonPalette::active(cx).red_text, 1.0))
                        .child(reason),
                )
                .left(self.move_start_over_button(uid, cx))
                .with_log(),
            MovePhase::Cancelled => Screen::new(t!("connect_wizard.move.cancelled_title"))
                .sub(t!("connect_wizard.move.cancelled_sub"))
                .left(self.move_start_over_button(uid, cx))
                .with_log(),
        }
    }

    /// One answer to a move that waits on its old core.
    fn answer_button(
        &self,
        uid: u64,
        id: &'static str,
        label: &'static str,
        command: Command,
        primary: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let button = MoonButton::new(id)
            .size(design::CONTROL_TIER)
            .label(t!(label).to_string())
            .on_click(cx.listener(move |this, _, _, cx| this.move_answer(uid, command, cx)));
        if primary { button.primary() } else { button }
            .render()
            .into_any_element()
    }

    /// "Back": drops the finished move and shows the cores again.
    fn move_start_over_button(&self, uid: u64, cx: &mut Context<Self>) -> AnyElement {
        MoonButton::new("cw-move-over")
            .size(design::CONTROL_TIER)
            .label(t!("connect_wizard.back").to_string())
            .on_click(cx.listener(move |this, _, _, cx| this.move_start_over(uid, cx)))
            .render()
            .into_any_element()
    }
}

/// What a move carries over, and what it means for trading meanwhile.
fn what_moves(cx: &App) -> impl IntoElement {
    let item = |title: String, text: String| {
        v_flex()
            .gap(design::ui_px(cx, 2.0))
            .child(div().child(title))
            .child(note(text, cx))
    };
    v_flex()
        .gap(design::ui_px(cx, 6.0))
        .child(item(
            t!("connect_wizard.move.all_settings").to_string(),
            t!("connect_wizard.move.all_settings_desc").to_string(),
        ))
        .child(item(
            t!("connect_wizard.move.all_strategies").to_string(),
            t!("connect_wizard.move.all_strategies_desc").to_string(),
        ))
        .child(item(
            t!("connect_wizard.move.all_history").to_string(),
            t!("connect_wizard.move.all_history_desc").to_string(),
        ))
        .child(item(
            t!("connect_wizard.move.all_stop").to_string(),
            t!("connect_wizard.move.all_stop_desc").to_string(),
        ))
        .child(note(t!("connect_wizard.move.pause"), cx))
        .child(note(t!("connect_wizard.move.old_kept"), cx))
}

/// What each answer to a refusing old core does.
fn answer_help(cx: &App) -> impl IntoElement {
    v_flex()
        .gap(design::ui_px(cx, 4.0))
        .child(note(t!("connect_wizard.move.retry_help"), cx))
        .child(note(t!("connect_wizard.move.wait_help"), cx))
        .child(note(t!("connect_wizard.move.cancel_help"), cx))
}

/// The instance folder offered for a core named `name`: `mb` and its letters and digits.
pub(super) fn default_dir(name: &str) -> String {
    let tail: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    format!("mb{tail}")
}

#[cfg(test)]
mod tests;
