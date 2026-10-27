//! The "Move to VPS…" window of one core: the server and its login, the server's host key to
//! confirm, then the move's progress and the answers it may ask for.
//!
//! The window owns only the form. The move itself — its threads, its log, its outcome — belongs to
//! the backend (`backend::core_move`), so closing the window does not stop it, and opening the
//! window of the same core again shows where it stands. Several cores move at once, each in its
//! own window.

mod render;

use std::net::Ipv4Addr;

use gpui::*;
use moon_remote::ssh::Target;
use moon_ui::{MoonBackgroundPolicy, MoonInputState, Root};
use rust_i18n::t;
use zeroize::Zeroizing;

use crate::Backend;
use crate::backend::core_move::{Command, MoveForm, MovePhase};
use crate::backend::station::job;
use crate::settings::telegram::server_bot::parse_target;

/// Default and minimum window sizes in design pixels.
const DEFAULT_SIZE: (f32, f32) = (620.0, 640.0);
const MIN_SIZE: (f32, f32) = (420.0, 380.0);
/// The new core's UDP port offered first.
const DEFAULT_PROTO_PORT: &str = "5160";

/// The window's view: the form, and a reader of the backend's move of [`Self::uid`].
pub(crate) struct CoreMoveView {
    backend: Entity<Backend>,
    /// The core being moved.
    uid: u64,
    /// Its name, for the title.
    name: String,
    host: Entity<MoonInputState>,
    login: Entity<MoonInputState>,
    password: Entity<MoonInputState>,
    key_path: Entity<MoonInputState>,
    passphrase: Entity<MoonInputState>,
    dir: Entity<MoonInputState>,
    port: Entity<MoonInputState>,
    /// The provider's login is a key rather than a password.
    by_key: bool,
    /// What is wrong with the form, already in the user's language.
    error: Option<String>,
    log_scroll: ScrollHandle,
    _observe: Subscription,
}

impl CoreMoveView {
    fn new(
        backend: Entity<Backend>,
        uid: u64,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = |masked: bool, value: &str, window: &mut Window, cx: &mut Context<Self>| {
            let state = cx.new(|cx| MoonInputState::new(window, cx).masked(masked));
            if !value.is_empty() {
                let value = value.to_owned();
                state.update(cx, |st, c| st.set_value(value, window, c));
            }
            state
        };
        let observe = cx.observe(&backend, |_, _, cx| cx.notify());
        let dir = default_dir(&name);
        Self {
            host: input(false, "", window, cx),
            login: input(false, "root", window, cx),
            password: input(true, "", window, cx),
            key_path: input(false, "", window, cx),
            passphrase: input(true, "", window, cx),
            dir: input(false, &dir, window, cx),
            port: input(false, DEFAULT_PROTO_PORT, window, cx),
            by_key: false,
            error: None,
            log_scroll: ScrollHandle::new(),
            backend,
            uid,
            name,
            _observe: observe,
        }
    }

    /// The backend's move of this core, if one was started.
    fn phase(&self, cx: &App) -> Option<MovePhase> {
        self.backend
            .read(cx)
            .core_moves
            .get(self.uid)
            .map(|job| job.phase.clone())
    }

    /// Read the server's host key: the first press, which sends no credential.
    fn check(&mut self, cx: &mut Context<Self>) {
        let Some(target) = parse_target(&text(&self.host, cx)) else {
            self.error = Some(t!("core_move.form.need_host").to_string());
            cx.notify();
            return;
        };
        if target.host.parse::<Ipv4Addr>().is_err() {
            self.error = Some(t!("core_move.form.need_ipv4").to_string());
            cx.notify();
            return;
        }
        self.error = None;
        let uid = self.uid;
        self.backend
            .update(cx, |b, bcx| b.core_move_probe(uid, target, bcx));
    }

    /// Start the move against the host key the user saw in the review.
    fn confirm(
        &mut self,
        target: Target,
        fingerprint: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let form = match self.form(target, fingerprint, cx) {
            Ok(form) => form,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        self.error = None;
        // The login is in the move's hands now; the fields forget it.
        for state in [&self.password, &self.passphrase] {
            state.update(cx, |st, c| st.set_value(String::new(), window, c));
        }
        let uid = self.uid;
        self.backend
            .update(cx, |b, bcx| b.core_move_start(uid, form, bcx));
    }

    /// The form as a move takes it, or what is missing.
    fn form(&self, target: Target, pin: String, cx: &App) -> Result<MoveForm, String> {
        let public_ip = target
            .host
            .parse::<Ipv4Addr>()
            .map_err(|_| t!("core_move.form.need_ipv4").to_string())?;
        let dir = text(&self.dir, cx);
        if dir.is_empty() || dir.contains(char::is_whitespace) {
            return Err(t!("core_move.form.need_dir").to_string());
        }
        let proto_port = text(&self.port, cx)
            .parse::<u16>()
            .ok()
            .filter(|port| *port > 0)
            .ok_or_else(|| t!("core_move.form.need_port").to_string())?;
        let key = self
            .by_key
            .then(|| (text(&self.key_path, cx), secret(&self.passphrase, cx)));
        let first = job::first_access(text(&self.login, cx), secret(&self.password, cx), key)
            .map_err(|e| format!("{e:#}"))?;
        Ok(MoveForm {
            target,
            pin,
            first,
            public_ip,
            dir,
            proto_port,
        })
    }

    /// Pass an answer to the move that waits on its old core.
    fn answer(&mut self, command: Command, cx: &mut Context<Self>) {
        let uid = self.uid;
        self.backend
            .update(cx, |b, bcx| b.core_move_command(uid, command, bcx));
    }

    /// Drop a finished move, so the form shows again.
    fn start_over(&mut self, cx: &mut Context<Self>) {
        let uid = self.uid;
        self.backend
            .update(cx, |b, bcx| b.core_move_forget(uid, bcx));
        cx.notify();
    }

    /// Pick the key file through the system dialog, keeping a typed path on cancel.
    fn browse_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selection = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("core_move.form.key_path").to_string().into()),
        });
        cx.spawn_in(window, async move |view, cx| match selection.await {
            Ok(Ok(Some(paths))) => {
                if let Some(path) = paths.into_iter().next() {
                    let _ = view.update_in(cx, |this, window, cx| {
                        this.key_path.update(cx, |state, cx| {
                            state.set_value(path.to_string_lossy().into_owned(), window, cx);
                        });
                        cx.notify();
                    });
                }
            }
            Ok(Err(error)) => log::warn!("key file dialog failed: {error}"),
            _ => {}
        })
        .detach();
    }
}

/// The instance folder offered for a core named `name`: `mb` and its letters and digits.
fn default_dir(name: &str) -> String {
    let tail: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    format!("mb{tail}")
}

/// A field's text, trimmed.
fn text(state: &Entity<MoonInputState>, cx: &App) -> String {
    state.read(cx).value().trim().to_owned()
}

/// A secret field's text, `None` when empty.
fn secret(state: &Entity<MoonInputState>, cx: &App) -> Option<Zeroizing<String>> {
    let value = state.read(cx).value().to_string();
    (!value.is_empty()).then(|| Zeroizing::new(value))
}

/// Open the move window of core `uid` named `name`, or bring up the one it has.
pub(crate) fn open(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    uid: u64,
    name: String,
    cx: &mut App,
) {
    if let Some(handle) = backend.read(cx).core_move_windows.get(&uid).copied()
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let zoom = moon_ui::MoonTheme::content_zoom(cx);
    let bounds = Bounds {
        origin: point(px(180.0), px(120.0)),
        size: size(px(DEFAULT_SIZE.0 * zoom), px(DEFAULT_SIZE.1 * zoom)),
    };
    let opts = crate::window::windowing::tool_window_options(
        t!("core_move.window_title", name = &name).to_string(),
        WindowBounds::Windowed(bounds),
        Some(size(px(MIN_SIZE.0 * zoom), px(MIN_SIZE.1 * zoom))),
        owner,
    );
    let b = backend.clone();
    let created = cx.open_window(opts, move |window, cx| {
        crate::window::windowing::configure_shell_clear_color(window, cx);
        let view = cx.new(|cx| CoreMoveView::new(b.clone(), uid, name, window, cx));
        // The handle goes when the window does; the move it shows goes on in the backend.
        let release = b.clone();
        view.update(cx, |_, cx| {
            cx.on_release(move |_, app| {
                release.update(app, |backend, _| {
                    backend.core_move_windows.remove(&uid);
                });
            })
            .detach();
        });
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque))
    });
    match created {
        Ok(handle) => {
            backend.update(cx, |bk, _| {
                bk.core_move_windows.insert(uid, handle);
            });
            crate::window::windowing::activate_new_window(handle.into(), cx);
        }
        Err(error) => log::warn!("core move window could not be opened: {error:#}"),
    }
}
