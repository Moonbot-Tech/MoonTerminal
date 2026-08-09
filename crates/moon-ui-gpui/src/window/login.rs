//! Startup login window: the single place the terminal asks for a password before it opens.
//!
//! One window covers three different questions, because the user should never meet two password
//! dialogs in a row:
//! - [`LoginStep::FilePassword`] — `servers.enc` could not be opened with the key material on this
//!   machine, but the file carries a password key slot. This is the "I copied the file to a clean
//!   machine" case. A correct password re-wraps the file key for this machine, so the question is
//!   asked once per machine, not once per launch.
//! - [`LoginStep::LaunchPassword`] — the local gate, asked on every launch when enabled.
//! - [`LoginStep::Locked`] — nothing on this machine can open the file and no password slot exists.
//!   Today this case exits the process with an error before any window appears, which reads as "the
//!   app does not start". Naming it is the point of this state.
//!
//! Both questions can be due in one launch (a clean machine with a launch password configured), so
//! the steps run in sequence inside this window: decrypt first, then the gate.
//!
//! **This build is a PREVIEW.** There is no key backend yet (see `settings/security/vault.rs`), so
//! [`submit`] cannot verify anything and every attempt reports a failure. [`open_preview`] is the
//! temporary entry point that shows the window from Settings; phase 2 replaces it with a call from
//! `startup` before the shell is built, and deletes [`PREVIEW`] along with the step switcher.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBackgroundPolicy, MoonButton, MoonButtonSize, MoonInput, MoonInputEvent, MoonInputState,
    MoonPalette, MoonWindowFrame, Root, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use crate::design;

/// Whether this build's login window is a non-functional preview.
///
/// Drives the step switcher and the notice. Phase 2 deletes it; the compiler then points at
/// everything that still assumes the preview.
const PREVIEW: bool = true;

/// Window header height in unscaled pixels, matching the other tool windows.
const HEADER_H: f32 = 30.0;

/// Failed attempts after which the window says it is slowing down.
///
/// The delay itself is phase-2 work and is honest about what it buys: a four-character launch
/// password cannot be made brute-force resistant, and the throttle exists to stop an unattended
/// script, not a determined attacker.
const THROTTLE_AFTER: u32 = 3;

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

/// The login window's view.
pub(crate) struct LoginView {
    step: LoginStep,
    input: Entity<MoonInputState>,
    /// Localization key of the current failure, cleared as soon as the user edits the field.
    error: Option<&'static str>,
    attempts: u32,
}

impl LoginView {
    fn new(step: LoginStep, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| MoonInputState::new(window, cx).masked(true));
        // Enter submits, and any edit clears the previous failure: leaving a red line under a
        // field the user is already retyping makes the window look stuck.
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
        let handle = input.focus_handle(cx);
        window.focus(&handle, cx);
        Self {
            step,
            input,
            error: None,
            attempts: 0,
        }
    }

    /// Verify the typed password and advance, or report the failure.
    ///
    /// PREVIEW: there is no backend to verify against, so every non-empty attempt fails. Phase 2
    /// derives the key on a worker thread and continues startup from the result — never inline,
    /// since Argon2 takes long enough to be visible as a frozen window.
    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.input.read(cx).value().is_empty() {
            self.error = Some("login.err.empty");
            cx.notify();
            return;
        }
        self.attempts += 1;
        self.error = Some(if PREVIEW {
            "login.err.preview"
        } else {
            "login.err.wrong"
        });
        cx.notify();
    }

    /// Close the window.
    ///
    /// This closes the window and NOTHING else. In the real startup flow that is not enough: the
    /// login window can be the only window in the session, and the terminal quits on the last
    /// GROUP window closing (`startup.rs`), so an unlocked-but-abandoned launch would leave a
    /// windowless process running. Phase 2 has to quit explicitly from here.
    fn dismiss(&mut self, window: &mut Window, _cx: &mut Context<Self>) {
        window.remove_window();
    }

    /// Switch the previewed step and reset what belongs to the previous one.
    fn preview_step(&mut self, step: LoginStep, cx: &mut Context<Self>) {
        self.step = step;
        self.error = None;
        self.attempts = 0;
        cx.notify();
    }

    /// Build the password field plus its error line.
    fn field(&self, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        v_flex()
            .gap(design::ui_px(cx, 6.0))
            .child(
                MoonInput::new("login-password")
                    .state(&self.input)
                    .placeholder(t!("login.placeholder").to_string())
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
        h_flex()
            .gap(design::ui_px(cx, 8.0))
            .items_center()
            .when(self.step.takes_password(), |this| {
                this.child(
                    MoonButton::new("login-submit")
                        .primary()
                        .small()
                        .width(120.0)
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
                        // Phase 2 renames `servers.enc` aside and starts with an empty core list.
                        // It stays disabled here rather than pretending: this button destroys the
                        // only copy of the user's keys, and a preview must not offer that.
                        .disabled(PREVIEW)
                        .render(),
                )
            })
            .child(
                MoonButton::new("login-exit")
                    .ghost()
                    .small()
                    .label(t!("login.exit").to_string())
                    .on_click(cx.listener(|this, _, window, cx| this.dismiss(window, cx)))
                    .render(),
            )
    }

    /// Build the preview-only row that switches between the three states.
    fn preview_switcher(&self, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let button = |id: &'static str, step: LoginStep, label: String, cx: &Context<Self>| {
            MoonButton::new(id)
                .outline()
                .size(MoonButtonSize::Micro)
                .selected(self.step == step)
                .label(format!("  {label}  "))
                .on_click(cx.listener(move |this, _, _window, cx| this.preview_step(step, cx)))
                .render()
        };
        v_flex()
            .gap(design::ui_px(cx, 4.0))
            .child(
                div()
                    .text_color(rgba_from(p.amber, 1.0))
                    .text_size(design::t_caption(cx))
                    .child(t!("login.preview_notice").to_string()),
            )
            .child(
                h_flex()
                    .gap(design::ui_px(cx, 6.0))
                    .child(button(
                        "login-preview-file",
                        LoginStep::FilePassword,
                        t!("login.step.file").to_string(),
                        cx,
                    ))
                    .child(button(
                        "login-preview-launch",
                        LoginStep::LaunchPassword,
                        t!("login.step.launch").to_string(),
                        cx,
                    ))
                    .child(button(
                        "login-preview-locked",
                        LoginStep::Locked,
                        t!("login.step.locked").to_string(),
                        cx,
                    )),
            )
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
            .child(self.actions(cx))
            .when(PREVIEW, |this| this.child(self.preview_switcher(cx)));

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

/// Open the login window for one step.
///
/// PREVIEW ENTRY POINT, called from the Settings security block so the window can be reviewed
/// before the backend exists. Phase 2 replaces it with an entry point that takes the resolved
/// startup request and a continuation to run once the terminal is unlocked.
pub(crate) fn open_preview(cx: &mut App) {
    // Reuse the open window instead of stacking a new one on every click, the way every other
    // secondary window in the terminal does. A thread-local is enough: GPUI windows are created
    // only on the main thread, and this handle outlives no session state.
    thread_local! {
        static OPEN: std::cell::RefCell<Option<WindowHandle<Root>>> =
            const { std::cell::RefCell::new(None) };
    }
    let existing = OPEN.with(|open| *open.borrow());
    if let Some(handle) = existing {
        // `update` fails for a window the user already closed, which is the signal to open a new
        // one rather than an error.
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    let bounds = Bounds {
        origin: point(px(240.0), px(180.0)),
        size: size(px(460.0), px(340.0)),
    };
    let opts = crate::window::windowing::login_window_options(
        t!("login.window_title").to_string(),
        WindowBounds::Windowed(bounds),
        Some(size(px(380.0), px(280.0))),
    );
    if let Ok(handle) = cx.open_window(opts, move |window, cx| {
        crate::window::windowing::configure_shell_clear_color(window, cx);
        let view = cx.new(|cx| LoginView::new(LoginStep::FilePassword, window, cx));
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque))
    }) {
        OPEN.with(|open| *open.borrow_mut() = Some(handle));
        crate::window::windowing::activate_new_window(handle.into(), cx);
    }
}
