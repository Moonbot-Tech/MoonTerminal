//! Startup login window: the single place the terminal asks for a password before it opens.
//!
//! One window covers three different questions, because the user should never meet two password
//! dialogs in a row:
//! - [`LoginStep::FilePassword`] — `servers.enc` could not be opened with the key material on this
//!   machine, but the file carries a password key slot. This is the "I copied the file to a clean
//!   machine" case. A correct password re-wraps the file key for this machine, so the question is
//!   asked once per machine, not once per launch.
//! - [`LoginStep::LaunchPassword`] — the local latch, asked on every launch when enabled.
//! - [`LoginStep::Locked`] — nothing on this machine can open the file and no password slot exists.
//!   Before this window existed, that case exited the process before anything appeared, which
//!   reads as "the application does not start".
//!
//! Verification runs on a WORKER THREAD. Argon2id is memory-hard by design — that is the entire
//! point of it — and running it inline would freeze the window for the duration of every attempt,
//! including every wrong one.

use std::sync::mpsc::{Receiver, TryRecvError, channel};

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBackgroundPolicy, MoonButton, MoonInput, MoonInputEvent, MoonInputState, MoonPalette,
    MoonWindowFrame, Root, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use crate::design;
use moon_core::config::crypto;
use moon_core::config::paths;

/// Window header height in unscaled pixels, matching the other tool windows.
const HEADER_H: f32 = 30.0;

/// Failed attempts after which the window says it is slowing down.
const THROTTLE_AFTER: u32 = 3;

/// Delay added between attempts once [`THROTTLE_AFTER`] is passed, in milliseconds.
///
/// Honest about what it buys: a four-character launch password cannot be made brute-force
/// resistant, and this exists to stop an unattended script from spinning, not a determined
/// attacker. The file password's resistance comes from Argon2id, not from here.
const THROTTLE_MS: u64 = 1500;

/// How often the window checks for a finished verification.
const POLL_MS: u64 = 40;

/// What the window is currently asking for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LoginStep {
    FilePassword,
    LaunchPassword,
    Locked,
}

impl LoginStep {
    /// Localization key for the line explaining what is being asked and why.
    fn subtitle(self) -> &'static str {
        match self {
            LoginStep::FilePassword => "login.file_subtitle",
            LoginStep::LaunchPassword => "login.launch_subtitle",
            LoginStep::Locked => "login.locked_subtitle",
        }
    }

    /// Whether this step takes a password at all.
    fn takes_password(self) -> bool {
        !matches!(self, LoginStep::Locked)
    }
}

/// How the window finished.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LoginOutcome {
    /// The user satisfied the step; startup continues.
    Unlocked,
    /// The user closed the window or chose to exit.
    Abandoned,
}

/// What the window reports back to startup when it is finished.
type LoginDone = Box<dyn FnOnce(LoginOutcome, &mut App)>;

/// Result of one verification attempt, sent back from the worker.
enum Attempt {
    Accepted,
    Rejected,
    /// The backend itself failed (keyring, damaged file); the message is already localized.
    Failed(&'static str),
}

/// The login window's view.
struct LoginView {
    step: LoginStep,
    input: Entity<MoonInputState>,
    /// Localization key of the current failure, cleared as soon as the user edits the field.
    error: Option<&'static str>,
    attempts: u32,
    /// A verification is running on a worker thread; the field and buttons are inert until it ends.
    pending: Option<Receiver<Attempt>>,
    /// Called exactly once, with whatever the window concluded.
    done: Option<LoginDone>,
    /// This window, kept so it can be closed from a context that has no `Window`.
    handle: AnyWindowHandle,
}

impl LoginView {
    fn new(step: LoginStep, done: LoginDone, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| MoonInputState::new(window, cx).masked(true));
        cx.subscribe(&input, |this, _emitter, ev: &MoonInputEvent, cx| match ev {
            MoonInputEvent::PressEnter { .. } => this.submit(cx),
            // Guarded so an ordinary keystroke does not repaint the window once the line is
            // already gone; the field repaints itself.
            MoonInputEvent::Change if this.error.is_some() => {
                this.error = None;
                cx.notify();
            }
            _ => {}
        })
        .detach();
        // Focus the field itself, not the window: the point of this window is that you type
        // immediately, without reaching for the mouse first.
        let focus = input.focus_handle(cx);
        window.focus(&focus, cx);
        // The titlebar X destroys this view without going through any button. Startup is waiting
        // on the callback, so a window that closed without one leaves a process with no windows
        // and no way to quit — report the closure as an abandonment.
        cx.on_release(|this, app| {
            if let Some(done) = this.done.take() {
                done(LoginOutcome::Abandoned, app);
            }
        })
        .detach();
        Self {
            handle: window.window_handle(),
            step,
            input,
            error: None,
            attempts: 0,
            pending: None,
            done: Some(done),
        }
    }

    /// Verify the typed password on a worker thread.
    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.pending.is_some() {
            return;
        }
        let password = self.input.read(cx).value().to_string();
        if password.is_empty() {
            self.error = Some("login.err.empty");
            cx.notify();
            return;
        }
        let step = self.step;
        let throttle = self.attempts >= THROTTLE_AFTER;
        let (tx, rx) = channel();
        // A detached thread rather than a pool: this happens at most a handful of times per launch,
        // and the window is the only thing waiting for it.
        std::thread::spawn(move || {
            if throttle {
                std::thread::sleep(std::time::Duration::from_millis(THROTTLE_MS));
            }
            let _ = tx.send(verify(step, &password));
        });
        self.pending = Some(rx);
        self.error = None;
        self.poll(cx);
        cx.notify();
    }

    /// Re-check the worker until it answers.
    ///
    /// A timer rather than a blocking wait: the window keeps painting, so a slow derivation looks
    /// like a busy field instead of a hung application.
    fn poll(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(POLL_MS))
                    .await;
                let finished = this.update(cx, |this, cx| this.drain(cx));
                match finished {
                    Ok(true) | Err(_) => break,
                    Ok(false) => {}
                }
            }
        })
        .detach();
    }

    /// Take the worker's answer if it has arrived. Returns whether polling is over.
    fn drain(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(rx) = &self.pending else {
            return true;
        };
        match rx.try_recv() {
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                self.error = Some("login.err.backend");
                cx.notify();
                true
            }
            Ok(attempt) => {
                self.pending = None;
                match attempt {
                    Attempt::Accepted => self.finish(LoginOutcome::Unlocked, cx),
                    Attempt::Rejected => {
                        self.attempts += 1;
                        self.error = Some("login.err.wrong");
                        cx.notify();
                    }
                    Attempt::Failed(key) => {
                        self.error = Some(key);
                        cx.notify();
                    }
                }
                true
            }
        }
    }

    /// Hand the outcome to startup and close the window.
    ///
    /// The callback runs BEFORE the window is removed, because on the unlocked path it builds the
    /// rest of the application: closing first would briefly leave the process with no window,
    /// which on Windows is indistinguishable from a quit.
    fn finish(&mut self, outcome: LoginOutcome, cx: &mut Context<Self>) {
        let Some(done) = self.done.take() else {
            return;
        };
        let handle = self.handle;
        cx.defer(move |cx| {
            done(outcome, cx);
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        });
    }

    /// Set the encrypted file aside and continue with an empty configuration.
    ///
    /// RENAMES rather than deletes, and that is not a nicety: this state also covers a file that
    /// is perfectly intact but belongs to another machine, whose keyring entry the user may still
    /// recover. Deleting here would destroy cores over a guess about which case it is.
    fn start_over(&mut self, cx: &mut Context<Self>) {
        let path = paths::servers_path();
        // A timestamp keeps a second attempt from overwriting the first file set aside, which
        // would turn "moved, still recoverable" into a silent delete.
        let aside = path.with_extension(format!(
            "enc.unreadable-{}",
            moon_core::util::utc_stamp_compact(moon_core::util::now_unix_ms_i64())
        ));
        match std::fs::rename(&path, &aside) {
            Ok(()) => {
                log::warn!(
                    "servers.enc отложен в {} — старт с пустой конфигурацией",
                    aside.display()
                );
                self.finish(LoginOutcome::Unlocked, cx);
            }
            Err(e) => {
                log::error!("не удалось отложить servers.enc: {e}");
                self.error = Some("login.err.backend");
                cx.notify();
            }
        }
    }

    /// Close the window without unlocking; startup treats this as "quit".
    fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.finish(LoginOutcome::Abandoned, cx);
    }

    /// Build the password field plus its error line.
    fn field(&self, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let busy = self.pending.is_some();
        v_flex()
            .gap(design::ui_px(cx, 6.0))
            .child(
                MoonInput::new("login-password")
                    .state(&self.input)
                    .placeholder(t!("login.placeholder").to_string())
                    .disabled(busy)
                    .loading(busy)
                    .mask_toggle(),
            )
            .when_some(self.error, |this, key| {
                this.child(
                    div()
                        .text_color(rgba_from(p.red, 1.0))
                        .child(t!(key).to_string()),
                )
            })
            .when(self.attempts >= THROTTLE_AFTER, |this| {
                this.child(
                    div()
                        .text_color(rgba_from(p.text_muted, 1.0))
                        .text_size(design::t_caption(cx))
                        .child(t!("login.throttled").to_string()),
                )
            })
    }

    /// Build the action row: the primary action of the step plus Exit.
    fn actions(&self, cx: &Context<Self>) -> impl IntoElement {
        let busy = self.pending.is_some();
        h_flex()
            .gap(design::ui_px(cx, 8.0))
            .items_center()
            .when(self.step.takes_password(), |this| {
                this.child(
                    MoonButton::new("login-submit")
                        .primary()
                        .small()
                        .width(120.0)
                        .disabled(busy)
                        .label(t!("login.enter").to_string())
                        .on_click(cx.listener(|this, _, _window, cx| this.submit(cx)))
                        .render(),
                )
            })
            .when(self.step == LoginStep::Locked, |this| {
                this.child(
                    MoonButton::new("login-reset")
                        .outline()
                        .small()
                        .label(format!("  {}  ", t!("login.start_over")))
                        .on_click(cx.listener(|this, _, _window, cx| this.start_over(cx)))
                        .render(),
                )
            })
            .child(
                MoonButton::new("login-exit")
                    .ghost()
                    .small()
                    .label(t!("login.exit").to_string())
                    .on_click(cx.listener(|this, _, _window, cx| this.dismiss(cx)))
                    .render(),
            )
    }
}

/// Verify one attempt. Runs on a worker thread.
fn verify(step: LoginStep, password: &str) -> Attempt {
    match step {
        LoginStep::LaunchPassword => {
            if crypto::verify_launch_password(password) {
                Attempt::Accepted
            } else {
                Attempt::Rejected
            }
        }
        LoginStep::FilePassword => {
            let bytes = match std::fs::read(paths::servers_path()) {
                Ok(bytes) => bytes,
                Err(e) => {
                    log::error!("не прочитал servers.enc для проверки пароля: {e}");
                    return Attempt::Failed("login.err.backend");
                }
            };
            match crypto::unlock_with_password(&bytes, password) {
                Ok(_) => Attempt::Accepted,
                Err(crypto::AccessError::WrongPassword) => Attempt::Rejected,
                Err(e) => {
                    log::error!("расшифровка servers.enc паролем не удалась: {e}");
                    Attempt::Failed("login.err.backend")
                }
            }
        }
        // Nothing to verify; the window offers other actions.
        LoginStep::Locked => Attempt::Rejected,
    }
}

impl Render for LoginView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let chrome_width = f32::from(window.viewport_size().width);
        let body = v_flex()
            .flex_1()
            .w_full()
            .p(design::ui_px(cx, 20.0))
            .gap(design::ui_px(cx, 10.0))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(design::t_title(cx))
                    .child(t!("login.title").to_string()),
            )
            .child(
                div()
                    .text_color(rgba_from(p.text_soft, 1.0))
                    .child(t!(self.step.subtitle()).to_string()),
            )
            .when(self.step.takes_password(), |this| {
                this.child(self.field(cx))
            })
            .child(div().flex_1())
            .child(self.actions(cx));

        v_flex()
            .size_full()
            .relative()
            .bg(rgba_from(p.shell, 1.0))
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .line_height(design::line_px(cx, 14.0))
            .text_color(rgba_from(p.text, 1.0))
            .child(header(p, cx))
            .child(body)
            .child(
                MoonWindowFrame::tool("login-window-frame-hit", chrome_width)
                    .header_height(HEADER_H)
                    .leading_inset(design::titlebar_leading_inset())
                    .show_controls(design::show_custom_window_controls())
                    .hit_overlay(),
            )
    }
}

/// Build the draggable window header.
fn header(p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .id("login-window-header")
        .relative()
        .flex_none()
        .w_full()
        .h(design::fit_h_px(cx, HEADER_H, 14.0, 8.0))
        .justify_between()
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        .bg(rgba_from(p.shell_high, 1.0))
        .border_b(px(1.0))
        .border_color(rgba_from(p.border, 1.0))
        .child(
            MoonWindowFrame::tool("login-titlebar-title", 0.0)
                .title_cluster(t!("login.window_title").to_string(), cx)
                .h_full()
                .flex_1()
                .min_w_0(),
        )
        .when(design::show_custom_window_controls(), |this| {
            this.child(
                MoonWindowFrame::tool("login-window-frame-visual", 0.0)
                    .header_height(HEADER_H)
                    .show_controls(true)
                    .visual_controls(cx),
            )
        })
}

/// Open the login window for one step and report its outcome exactly once.
///
/// `done` receives [`LoginOutcome::Abandoned`] if the window cannot be created at all, so startup
/// never waits forever on a window that does not exist.
pub(crate) fn open(
    step: LoginStep,
    cx: &mut App,
    done: impl FnOnce(LoginOutcome, &mut App) + 'static,
) {
    let bounds = Bounds {
        origin: point(px(240.0), px(180.0)),
        size: size(px(460.0), px(320.0)),
    };
    let opts = crate::window::windowing::login_window_options(
        t!("login.window_title").to_string(),
        WindowBounds::Windowed(bounds),
        Some(size(px(380.0), px(260.0))),
    );
    let mut done = Some(Box::new(done) as LoginDone);
    let opened = cx.open_window(opts, |window, cx| {
        let callback = done.take().expect("the window body runs once");
        let view = cx.new(|cx| LoginView::new(step, callback, window, cx));
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque))
    });
    match opened {
        Ok(handle) => crate::window::windowing::activate_new_window(handle.into(), cx),
        Err(e) => {
            log::error!("не удалось открыть окно входа: {e}");
            if let Some(done) = done.take() {
                done(LoginOutcome::Abandoned, cx);
            }
        }
    }
}
