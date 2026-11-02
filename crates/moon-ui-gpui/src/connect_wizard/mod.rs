//! The connect wizard: one window for every way a core reaches this terminal — a new core on a
//! Linux server, a core of the user's Moonbot by its key, and (on Windows) Moonbot itself.
//!
//! It opens by itself while the terminal has no core, and from the "Connect a core" button of
//! Settings → Cores. The window owns only its forms. A new core's install belongs to the backend
//! (`backend::core_new`), so closing the window does not stop it, and opening the wizard again
//! shows where it stands.

mod by_key;
mod done;
#[cfg(windows)]
mod download;
mod new_core;
mod render;
mod server;
mod start;

use std::time::Instant;

use gpui::*;
use moon_core::config::eula::EulaAcceptance;
use moon_ui::{MoonBackgroundPolicy, MoonInputState, MoonSelectState, MoonTextAreaState, Root};
use rust_i18n::t;

use crate::Backend;
use crate::backend::core_new::{NewCommand, NewForm, NewPhase};
use server::ServerFields;

/// Default and minimum window sizes in design pixels.
const DEFAULT_SIZE: (f32, f32) = (900.0, 660.0);
const MIN_SIZE: (f32, f32) = (460.0, 420.0);
/// The instance folder offered for a new core.
const DEFAULT_DIR: &str = "mbcore";

/// The ways the wizard connects a core, picked on its start screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Branch {
    /// A new core on a Linux server.
    NewCore,
    /// A core of the user's Moonbot, by the key it copies.
    ByKey,
}

/// The wizard's view.
pub(crate) struct ConnectWizardView {
    backend: Entity<Backend>,
    /// The branch the user went into; `None` on the start screen.
    branch: Option<Branch>,
    /// The branch picked on the start screen.
    choice: Branch,
    server: ServerFields,
    /// The stored acceptance of Moonbot's agreement, read when the window opened.
    eula: Option<EulaAcceptance>,
    /// The agreement's checkbox on the server step.
    eula_ticked: bool,
    /// The exchange picked on the keys step, by its `mb --exchange` id.
    exchange: Entity<MoonSelectState<String>>,
    api_key: Entity<MoonInputState>,
    api_secret: Entity<MoonInputState>,
    rsa_bundle: Entity<MoonTextAreaState>,
    /// Binance's RSA keys instead of a key pair.
    rsa: bool,
    /// The keys form is open again over a core that refused its keys.
    fixing_keys: bool,
    /// "Real trading" was clicked once and waits for its confirmation.
    real_confirm: bool,
    /// The by-key branch: the core's name and its key.
    by_key_name: Entity<MoonInputState>,
    by_key_key: Entity<MoonInputState>,
    /// The by-key branch's row, once added, and since when it is being connected to.
    by_key_uid: Option<u64>,
    by_key_since: Option<Instant>,
    #[cfg(windows)]
    download: download::Download,
    /// What is wrong with a form, already in the user's language.
    error: Option<String>,
    log_scroll: ScrollHandle,
    _observe: Subscription,
}

impl ConnectWizardView {
    fn new(backend: Entity<Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&backend, |_, _, cx| cx.notify());
        let input = |masked: bool, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| MoonInputState::new(window, cx).masked(masked))
        };
        let exchange = cx.new(|cx| MoonSelectState::new(Vec::new(), None, window, cx));
        // A running install continues where it stands.
        let (branch, choice) = if backend.read(cx).core_new.is_some() {
            (Some(Branch::NewCore), Branch::NewCore)
        } else {
            (None, Branch::NewCore)
        };
        Self {
            server: ServerFields::new(DEFAULT_DIR, window, cx),
            eula: EulaAcceptance::load().filter(EulaAcceptance::covers_current),
            eula_ticked: false,
            exchange,
            api_key: input(false, window, cx),
            api_secret: input(true, window, cx),
            rsa_bundle: cx.new(|cx| MoonTextAreaState::new(window, cx).multi_line(true).rows(4)),
            rsa: false,
            fixing_keys: false,
            real_confirm: false,
            by_key_name: input(false, window, cx),
            by_key_key: input(false, window, cx),
            by_key_uid: None,
            by_key_since: None,
            #[cfg(windows)]
            download: download::Download::default(),
            error: None,
            log_scroll: ScrollHandle::new(),
            backend,
            branch,
            choice,
            _observe: observe,
        }
    }

    /// The new core's install, if one is running or over.
    fn job_phase(&self, cx: &App) -> Option<NewPhase> {
        self.backend
            .read(cx)
            .core_new
            .as_ref()
            .map(|job| job.phase.clone())
    }

    /// Go into the branch picked on the start screen.
    fn next_from_start(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        self.branch = Some(self.choice);
        cx.notify();
    }

    /// Back to the start screen, dropping an install that is over.
    fn back_to_start(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        if self
            .backend
            .read(cx)
            .core_new
            .as_ref()
            .is_some_and(|job| job.phase.ended())
        {
            self.backend.update(cx, |b, bcx| b.core_new_forget(bcx));
        }
        if self.backend.read(cx).core_new.is_none() {
            self.branch = None;
        }
        cx.notify();
    }

    /// Read the server's host key: sends no credential.
    fn probe(&mut self, cx: &mut Context<Self>) {
        match self.server.target(cx) {
            Ok(target) => {
                self.error = None;
                self.server.fingerprint_ok = false;
                self.backend
                    .update(cx, |b, bcx| b.core_new_probe(target, bcx));
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    /// Log in against the confirmed host key and check the server.
    fn check_server(
        &mut self,
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
        // The login is in the install's hands now; the fields forget it.
        self.server.forget_secrets(window, cx);
        let form = NewForm {
            target: access.target,
            pin,
            first: access.first,
            public_ip: access.public_ip,
            dir: access.dir,
            proto_port: access.proto_port,
        };
        self.backend
            .update(cx, |b, bcx| b.core_new_start(form, bcx));
    }

    /// Install the core, recording the agreement first when it was ticked now.
    fn install(&mut self, cx: &mut Context<Self>) {
        if self.eula.is_none() {
            if !self.eula_ticked {
                self.error = Some(t!("connect_wizard.eula.need").to_string());
                cx.notify();
                return;
            }
            match EulaAcceptance::accept_current(moon_core::util::time::unix_ms_i64_of(
                std::time::SystemTime::now(),
            )) {
                Ok(accepted) => self.eula = Some(accepted),
                Err(e) => {
                    self.error = Some(
                        t!("connect_wizard.eula.save_failed", reason = format!("{e:#}"))
                            .to_string(),
                    );
                    cx.notify();
                    return;
                }
            }
        }
        self.error = None;
        self.command(NewCommand::Install, cx);
    }

    /// Pass a command to the install.
    fn command(&mut self, command: NewCommand, cx: &mut Context<Self>) {
        self.backend
            .update(cx, |b, bcx| b.core_new_command(command, bcx));
        cx.notify();
    }

    /// Pick the key file through the system dialog, keeping a typed path on cancel.
    fn browse_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selection = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("connect_wizard.server.key_path").to_string().into()),
        });
        cx.spawn_in(window, async move |view, cx| match selection.await {
            Ok(Ok(Some(paths))) => {
                if let Some(path) = paths.into_iter().next() {
                    let _ = view.update_in(cx, |this, window, cx| {
                        this.server
                            .set_key_path(path.to_string_lossy().into_owned(), window, cx);
                        cx.notify();
                    });
                }
            }
            Ok(Err(error)) => log::warn!("key file dialog failed: {error}"),
            _ => {}
        })
        .detach();
    }

    /// Close the wizard; an install that is over is forgotten with it.
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .backend
            .read(cx)
            .core_new
            .as_ref()
            .is_some_and(|job| job.phase.ended())
        {
            self.backend.update(cx, |b, bcx| b.core_new_forget(bcx));
        }
        window.remove_window();
    }
}

/// Open the wizard, or bring up the one that is open.
///
/// `owner_display` is the owner's display, read by the caller with `window.display(cx)`: called
/// from the owner's own click handler, the owner window cannot be reached from here to ask.
pub(crate) fn open(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    cx: &mut App,
) {
    if let Some(handle) = backend.read(cx).connect_wizard_window
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let zoom = moon_ui::MoonTheme::content_zoom(cx);
    // Centred on the display of the window it was opened from: a one-off dialog keeps no
    // position.
    let display_id =
        owner_display.or_else(|| crate::window::windowing::owner_display_id(owner, cx));
    let bounds = Bounds::centered(
        display_id,
        size(px(DEFAULT_SIZE.0 * zoom), px(DEFAULT_SIZE.1 * zoom)),
        cx,
    );
    let mut opts = crate::window::windowing::tool_window_options(
        t!("connect_wizard.window_title").to_string(),
        WindowBounds::Windowed(bounds),
        Some(size(px(MIN_SIZE.0 * zoom), px(MIN_SIZE.1 * zoom))),
        owner,
    );
    opts.display_id = display_id;
    let b = backend.clone();
    let created = cx.open_window(opts, move |window, cx| {
        crate::window::windowing::configure_shell_clear_color(window, cx);
        let view = cx.new(|cx| ConnectWizardView::new(b.clone(), window, cx));
        // The handle goes when the window does; an install it shows goes on in the backend.
        let release = b.clone();
        view.update(cx, |_, cx| {
            cx.on_release(move |_, app| {
                release.update(app, |backend, _| backend.connect_wizard_window = None);
            })
            .detach();
        });
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque))
    });
    match created {
        Ok(handle) => {
            backend.update(cx, |bk, _| bk.connect_wizard_window = Some(handle));
            crate::window::windowing::activate_new_window(handle.into(), cx);
        }
        Err(error) => log::warn!("connect wizard could not be opened: {error:#}"),
    }
}
