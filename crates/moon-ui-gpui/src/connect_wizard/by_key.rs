//! The "connect to a Moonbot core" branch: the user's Moonbot copies its key, the wizard adds the
//! core with it and waits until this terminal is connected — the probe that the key and the
//! address in it work. What used to be adding a row in Settings → Cores and pasting the key.

use std::time::{Duration, Instant};

use gpui::*;
use moon_core::feed::ConnStatus;
use moon_ui::{MoonButton, MoonPalette, MoonSpinner, h_flex, rgba_from};
use rust_i18n::t;

use super::ConnectWizardView;
use super::render::{Screen, note};
use super::server::{field, text};
use crate::design;

/// How long the terminal may take to get through to a core before the wizard says it did not.
const CONNECT_WAIT: Duration = Duration::from_secs(60);
/// The name a core gets when the user typed none.
const DEFAULT_NAME: &str = "Moonbot";

impl ConnectWizardView {
    pub(super) fn by_key_screen(&mut self, cx: &mut Context<Self>) -> Screen {
        match self.by_key_uid {
            None => self.by_key_form(cx),
            Some(uid) => self.by_key_progress(uid, cx),
        }
    }

    /// The instruction, the name and the key.
    fn by_key_form(&mut self, cx: &mut Context<Self>) -> Screen {
        Screen::new(t!("connect_wizard.by_key.title"))
            .sub(t!("connect_wizard.by_key.sub"))
            .child(note(t!("connect_wizard.by_key.how"), cx))
            .child(field(
                "cw-key-name",
                t!("connect_wizard.by_key.name"),
                &self.by_key_name,
                false,
                cx,
            ))
            .child(field(
                "cw-key-key",
                t!("connect_wizard.by_key.key"),
                &self.by_key_key,
                true,
                cx,
            ))
            .left(self.back_button(cx))
            .right(
                MoonButton::new("cw-key-connect")
                    .primary()
                    .size(design::CONTROL_TIER)
                    .label(t!("connect_wizard.by_key.connect").to_string())
                    .on_click(cx.listener(|this, _, window, cx| this.by_key_connect(window, cx)))
                    .render(),
            )
    }

    /// Add the core with the pasted key and start waiting for the connection.
    fn by_key_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = text(&self.by_key_key, cx);
        if !moon_core::config::key_is_readable(&key) {
            self.error = Some(t!("connect_wizard.by_key.bad_key").to_string());
            cx.notify();
            return;
        }
        // A core already added with this key — a window closed while it was connecting — is
        // waited for again, not added twice.
        let known = self
            .backend
            .read(cx)
            .config
            .servers
            .iter()
            .find(|s| s.key.expose().trim() == key)
            .map(|s| s.uid);
        if let Some(uid) = known {
            self.error = None;
            self.by_key_uid = Some(uid);
            self.start_by_key_wait(cx);
            cx.notify();
            return;
        }
        let name = match text(&self.by_key_name, cx) {
            name if name.is_empty() => DEFAULT_NAME.to_owned(),
            name => name,
        };
        match self
            .backend
            .update(cx, |b, bcx| b.wizard_add_row(&name, &key, bcx))
        {
            Ok(uid) => {
                self.error = None;
                self.by_key_uid = Some(uid);
                self.start_by_key_wait(cx);
                // The key is in the settings now; the field forgets it.
                self.by_key_key
                    .update(cx, |st, c| st.set_value(String::new(), window, c));
            }
            Err(e) => {
                self.error =
                    Some(t!("connect_wizard.row_failed", reason = format!("{e:#}")).to_string())
            }
        }
        cx.notify();
    }

    /// Start (or restart) the wait for the connection, with one repaint when it runs out: a core
    /// that never answers changes nothing that would repaint the window by itself.
    fn start_by_key_wait(&mut self, cx: &mut Context<Self>) {
        self.by_key_since = Some(Instant::now());
        cx.spawn(async move |view, cx| {
            cx.background_executor().timer(CONNECT_WAIT).await;
            let _ = view.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    /// The added core: connecting, connected, or not reached in time.
    fn by_key_progress(&mut self, uid: u64, cx: &mut Context<Self>) -> Screen {
        let status = self
            .backend
            .read(cx)
            .session
            .status_map()
            .get(&uid)
            .cloned();
        let timed_out = self
            .by_key_since
            .is_some_and(|since| since.elapsed() >= CONNECT_WAIT);
        match status {
            Some(ConnStatus::Ready) => Screen::new(t!("connect_wizard.done.title"))
                .child(self.done_body(uid, None, None, cx))
                .left(self.again_button(cx))
                .right(self.finish_button(cx)),
            status if timed_out => {
                let detail = match status {
                    Some(ConnStatus::Failed(reason)) => reason,
                    Some(ConnStatus::Stage(stage)) => stage,
                    _ => String::new(),
                };
                let mut screen = Screen::new(t!("connect_wizard.by_key.unreached_title"))
                    .sub(t!("connect_wizard.by_key.unreached_sub"));
                if !detail.is_empty() {
                    screen = screen.child(
                        div()
                            .text_color(rgba_from(MoonPalette::active(cx).red_text, 1.0))
                            .child(detail),
                    );
                }
                screen
                    .left(
                        MoonButton::new("cw-key-drop")
                            .size(design::CONTROL_TIER)
                            .label(t!("connect_wizard.by_key.drop").to_string())
                            .on_click(cx.listener(move |this, _, _, cx| this.by_key_drop(uid, cx)))
                            .render(),
                    )
                    .right(
                        MoonButton::new("cw-key-wait")
                            .primary()
                            .size(design::CONTROL_TIER)
                            .label(t!("connect_wizard.by_key.wait").to_string())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.backend.update(cx, |b, bcx| {
                                    b.reconnect_request.push(uid);
                                    bcx.notify();
                                });
                                this.start_by_key_wait(cx);
                                cx.notify();
                            }))
                            .render(),
                    )
            }
            status => {
                let doing = match status {
                    Some(ConnStatus::Stage(stage)) => stage,
                    _ => t!("connect_wizard.by_key.connecting").to_string(),
                };
                Screen::new(t!("connect_wizard.by_key.title")).child(
                    h_flex()
                        .items_center()
                        .gap(design::ui_px(cx, 8.0))
                        .child(MoonSpinner::new())
                        .child(doing),
                )
            }
        }
    }

    /// Give up on the added core: its row goes again.
    fn by_key_drop(&mut self, uid: u64, cx: &mut Context<Self>) {
        if let Err(e) = self
            .backend
            .update(cx, |b, bcx| b.wizard_drop_row(uid, bcx))
        {
            self.error =
                Some(t!("connect_wizard.row_failed", reason = format!("{e:#}")).to_string());
        } else {
            self.by_key_uid = None;
            self.by_key_since = None;
        }
        cx.notify();
    }
}
