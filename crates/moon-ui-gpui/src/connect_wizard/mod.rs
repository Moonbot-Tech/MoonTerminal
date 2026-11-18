//! The connect wizard: one window for every way a core reaches this terminal — a new core on a
//! Linux server, a core moved onto one, a core of the user's Moonbot by its key, and (on
//! Windows) Moonbot itself.
//!
//! It opens by itself while the terminal has no core, from the "Connect a core" button of
//! Settings → Cores, and on the move branch from a core card's "Move to VPS…". The window owns
//! only its forms. An install (`backend::core_new`) and a move (`backend::core_move`) belong to
//! the backend, so closing the window stops neither, and opening the wizard again shows where
//! they stand.

mod by_key;
mod done;
#[cfg(windows)]
mod download;
mod login;
mod move_core;
mod new_core;
mod render;
mod server;
mod start;
#[cfg(test)]
mod tests;

use gpui::*;
use moon_core::config::eula::EulaAcceptance;
use moon_core::config::{Language, UiThemeMode};
use moon_ui::{
    IndexPath, MoonBackgroundPolicy, MoonInputState, MoonSelectEvent, MoonSelectItem,
    MoonSelectState, MoonTextAreaState, Root,
};
use rust_i18n::t;

use crate::Backend;
use crate::backend::core_new::{NewCommand, NewForm, NewPhase};
use server::ServerFields;

/// Default and minimum window sizes in design pixels. The default height holds the start screen —
/// its four cards, the demo tick with its description under the new-core card, and the note
/// shown while Settings is open — with room for a translation that wraps a line or two more.
/// Text scales with the window (the content zoom), so the fit is the same for every user.
const DEFAULT_SIZE: (f32, f32) = (900.0, 780.0);
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
    /// A connected core moved to a Linux server.
    Move,
    /// A provider login remembered for core server commands in this terminal session.
    Login,
}

/// The wizard's view.
pub(crate) struct ConnectWizardView {
    backend: Entity<Backend>,
    /// The branch the user went into; `None` on the start screen.
    branch: Option<Branch>,
    /// The branch picked on the start screen.
    choice: Branch,
    /// The new-core card's demo tick: the core starts on MoonBot's demo settings and account, so
    /// the wizard asks for no exchange keys. An install takes it with its first step; from then
    /// on the install's own flag speaks ([`Self::demo_install`]).
    demo: bool,
    /// The start screen's language and theme pickers, applied and saved at once.
    language: Entity<MoonSelectState<Language>>,
    theme: Entity<MoonSelectState<UiThemeMode>>,
    /// The language the theme picker's names were built in.
    look_language: Language,
    server: ServerFields,
    /// The stored acceptance of Moonbot's agreement, read when the window opened.
    eula: Option<EulaAcceptance>,
    /// The agreement's checkbox on the server step.
    eula_ticked: bool,
    /// The exchange picked on the keys step, by its `mb --exchange` id.
    exchange: Entity<MoonSelectState<String>>,
    /// The new core's outgoing address, picked on the check step from the server's addresses;
    /// `None` is the primary one.
    egress: Entity<MoonSelectState<Option<std::net::Ipv4Addr>>>,
    /// The values the egress select was last filled with, to refill it only when a check differs.
    egress_offered: Vec<Option<std::net::Ipv4Addr>>,
    api_key: Entity<MoonInputState>,
    api_secret: Entity<MoonInputState>,
    rsa_bundle: Entity<MoonTextAreaState>,
    /// Binance's RSA keys instead of a key pair.
    rsa: bool,
    /// The keys form is open again over a core that refused its keys.
    fixing_keys: bool,
    /// "Real trading" was clicked once and waits for its confirmation.
    real_confirm: bool,
    /// Instructions, inputs and the owned connection attempt of the by-key branch.
    by_key: by_key::ByKeyState,
    /// The move branch: the core picked to move, and whether the user went on to its server.
    move_uid: Option<u64>,
    move_at_server: bool,
    /// Whether the move's technical log is unfolded.
    move_log_open: bool,
    /// The moved core's outgoing address, typed on the move's server step; empty is the primary.
    /// The move checks and installs in one run, so there is no check screen to pick it on.
    move_egress: Entity<MoonInputState>,
    /// The session-only provider login branch and its injectable network call.
    login: login::LoginState,
    /// A folder name for the server step, set where no window is at hand to set it.
    pending_dir: Option<String>,
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
        let egress = cx.new(|cx| MoonSelectState::new(Vec::new(), None, window, cx));
        let (language, theme) = look_pickers(&backend, window, cx);
        let look_language = backend.read(cx).config.language;
        // A running install continues where it stands.
        let (branch, choice) = if backend.read(cx).core_new.is_some() {
            (Some(Branch::NewCore), Branch::NewCore)
        } else {
            (None, Branch::NewCore)
        };
        let by_key = by_key::ByKeyState::new(window, cx);
        by_key.observe_inputs(cx);
        Self {
            server: ServerFields::new(DEFAULT_DIR, window, cx),
            eula: EulaAcceptance::load().filter(EulaAcceptance::covers_current),
            eula_ticked: false,
            exchange,
            egress,
            egress_offered: Vec::new(),
            api_key: input(false, window, cx),
            api_secret: input(true, window, cx),
            rsa_bundle: cx.new(|cx| MoonTextAreaState::new(window, cx).multi_line(true).rows(4)),
            rsa: false,
            fixing_keys: false,
            real_confirm: false,
            by_key,
            move_uid: None,
            move_at_server: false,
            move_log_open: false,
            move_egress: input(false, window, cx),
            login: login::LoginState::default(),
            pending_dir: None,
            #[cfg(windows)]
            download: download::Download::default(),
            error: None,
            log_scroll: ScrollHandle::new(),
            backend,
            branch,
            choice,
            demo: false,
            look_language,
            language,
            theme,
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

    /// Whether the install the backend holds runs on MoonBot's demo settings — the install's own
    /// word, which outlives this window and ends when the user gives the core keys of their own.
    fn job_demo(&self, cx: &App) -> bool {
        self.backend
            .read(cx)
            .core_new
            .as_ref()
            .is_some_and(|job| job.demo)
    }

    /// Whether the new-core branch is, or is about to be, a demo install: the install's own word
    /// while the backend holds one — it took the tick with its first step and outlives this
    /// window — and the start screen's tick before that.
    fn demo_install(&self, cx: &App) -> bool {
        match self.backend.read(cx).core_new.as_ref() {
            Some(job) => job.demo,
            None => self.demo,
        }
    }

    /// Show in the pickers what the terminal runs with: a pick that was refused or not saved, or
    /// a Settings save made meanwhile, must not leave a value on screen that is not in effect.
    /// The theme names follow the language they are shown in.
    fn sync_look(&mut self, cx: &mut Context<Self>) {
        let (language, theme) = {
            let config = &self.backend.read(cx).config;
            (config.language, config.ui_theme_mode)
        };
        if self.look_language != language {
            self.look_language = language;
            self.theme
                .update(cx, |state, cx| state.set_items(theme_items(), cx));
        }
        if self.language.read(cx).selected_value() != Some(&language) {
            self.language
                .update(cx, |state, _| state.set_selected_value(&language));
        }
        if self.theme.read(cx).selected_value() != Some(&theme) {
            self.theme
                .update(cx, |state, _| state.set_selected_value(&theme));
        }
    }

    /// Apply and save the language or theme picked on the start screen.
    fn set_look(
        &mut self,
        language: Option<Language>,
        theme: Option<UiThemeMode>,
        cx: &mut Context<Self>,
    ) {
        if self.backend.read(cx).preview.is_some() {
            self.error = Some(t!("connect_wizard.start.look_in_settings").to_string());
            cx.notify();
            return;
        }
        match self
            .backend
            .update(cx, |b, bcx| b.wizard_set_look(language, theme, bcx))
        {
            Ok(()) => self.error = None,
            Err(e) => {
                log::warn!("connect wizard: language/theme not saved: {e:#}");
                self.error = Some(
                    t!(
                        "connect_wizard.start.look_failed",
                        reason = format!("{e:#}")
                    )
                    .to_string(),
                );
            }
        }
        cx.notify();
    }

    /// Go into the branch picked on the start screen.
    fn next_from_start(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        if self.choice == Branch::ByKey && self.by_key.uid.is_none() {
            self.by_key.step = by_key::logic::Step::Port;
            self.by_key.accepted_mismatch = None;
        }
        self.branch = Some(self.choice);
        self.leave_login();
        cx.notify();
    }

    /// Take a card's "Move to VPS…" request: the move branch, on that core's server step — or on
    /// its move, when one runs.
    fn take_move_request(&mut self, cx: &mut Context<Self>) {
        let Some(uid) = self.backend.update(cx, |b, _| b.wizard_move_request.take()) else {
            return;
        };
        self.error = None;
        self.branch = Some(Branch::Move);
        self.choice = Branch::Move;
        self.leave_login();
        self.move_at_server = true;
        // A host key ticked for another flow is never this core's confirmation.
        self.server.fingerprint_ok = false;
        if self.move_uid != Some(uid) {
            self.move_uid = Some(uid);
            let name = self
                .backend
                .read(cx)
                .config
                .servers
                .iter()
                .find(|s| s.uid == uid)
                .map(|s| s.name.clone())
                .unwrap_or_default();
            self.pending_dir = Some(move_core::default_dir(&name));
        }
        cx.notify();
    }

    /// Take a core command's login request and prefill its reduced server form.
    fn take_login_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ip) = self
            .backend
            .update(cx, |b, _| b.connect_login_request.take())
        else {
            return;
        };
        self.backend
            .update(cx, |b, bcx| b.core_new_prepare_login(bcx));
        self.error = None;
        self.branch = Some(Branch::Login);
        // A host key ticked for another flow is never this login's confirmation.
        self.server.fingerprint_ok = false;
        self.server.login_only = true;
        self.server.set_host(&ip, window, cx);
        self.login.restart_flow();
        cx.notify();
    }

    /// Back to the start screen, dropping an install that is over. From the move branch it is
    /// always the start screen: a running install is one choice away there.
    fn back_to_start(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        self.move_uid = None;
        self.move_at_server = false;
        if self.branch == Some(Branch::Move) {
            self.branch = None;
            self.leave_login();
            cx.notify();
            return;
        }
        let ended = self
            .backend
            .read(cx)
            .core_new
            .as_ref()
            .filter(|job| job.phase.ended())
            .map(|job| (matches!(job.phase, NewPhase::Done), job.demo));
        if let Some((done, demo)) = ended {
            // The tick follows what a finished install ended as: one that went on to the user's
            // own keys is no demo to offer again by default.
            if done {
                self.demo = demo;
            }
            self.backend.update(cx, |b, bcx| b.core_new_forget(bcx));
        }
        if self.backend.read(cx).core_new.is_none() {
            self.branch = None;
        }
        self.leave_login();
        cx.notify();
    }

    /// Read the server's host key: sends no credential.
    fn probe(&mut self, cx: &mut Context<Self>) {
        match self.server.target(cx) {
            Ok(target) => {
                self.error = None;
                self.server.fingerprint_ok = false;
                // The login-only branch shares this probe and installs nothing.
                let demo = self.demo && self.branch == Some(Branch::NewCore);
                self.backend
                    .update(cx, |b, bcx| b.core_new_probe(target, demo, bcx));
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
        if self.branch == Some(Branch::Login) {
            self.login_server(target, pin, window, cx);
            return;
        }
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
            demo: self.demo_install(cx),
            close_password: access.close_password,
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
        let egress = self.egress.read(cx).selected_value().copied().flatten();
        self.command(NewCommand::Install { egress }, cx);
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

/// The language and theme pickers, set to what the terminal shows now; a pick is applied to every
/// window and saved at once.
fn look_pickers(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<ConnectWizardView>,
) -> (
    Entity<MoonSelectState<Language>>,
    Entity<MoonSelectState<UiThemeMode>>,
) {
    let (lang_now, theme_now) = {
        let config = &backend.read(cx).config;
        (config.language, config.ui_theme_mode)
    };
    let language = cx.new(|cx| {
        MoonSelectState::new(
            Language::ALL
                .iter()
                .map(|l| MoonSelectItem::new(*l, l.label())),
            Language::ALL
                .iter()
                .position(|l| *l == lang_now)
                .map(IndexPath::new),
            window,
            cx,
        )
    });
    let labels = crate::settings::THEME_MODE_LABELS;
    let theme = cx.new(|cx| {
        MoonSelectState::new(
            theme_items(),
            labels
                .iter()
                .position(|(_, mode)| *mode == theme_now)
                .map(IndexPath::new),
            window,
            cx,
        )
    });
    cx.subscribe(&language, |this, _, ev: &MoonSelectEvent<Language>, cx| {
        if let MoonSelectEvent::Confirm(Some(language)) = ev {
            this.set_look(Some(*language), None, cx);
        }
    })
    .detach();
    cx.subscribe(&theme, |this, _, ev: &MoonSelectEvent<UiThemeMode>, cx| {
        if let MoonSelectEvent::Confirm(Some(mode)) = ev {
            this.set_look(None, Some(*mode), cx);
        }
    })
    .detach();
    (language, theme)
}

/// The theme picker's items, named in the current language.
fn theme_items() -> Vec<MoonSelectItem<UiThemeMode>> {
    crate::settings::THEME_MODE_LABELS
        .iter()
        .map(|(key, mode)| MoonSelectItem::new(*mode, t!(*key).to_string()))
        .collect()
}

/// Open the wizard on the move of core `uid` — a core card's "Move to VPS…" — or bring up the
/// one that is open and switch it there.
pub(crate) fn open_move(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    uid: u64,
    cx: &mut App,
) {
    backend.update(cx, |b, bcx| {
        b.wizard_move_request = Some(uid);
        bcx.notify();
    });
    open(backend.clone(), owner, owner_display, cx);
    // No window took the request: it must not surface in a later, unrelated opening.
    if backend.read(cx).connect_wizard_window.is_none() {
        backend.update(cx, |b, _| b.wizard_move_request = None);
    }
}

/// Open the login-only wizard for core server commands with `ip` already filled in.
pub(crate) fn open_login(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    ip: String,
    cx: &mut App,
) {
    backend.update(cx, |b, bcx| {
        b.connect_login_request = Some(ip);
        bcx.notify();
    });
    open(backend.clone(), owner, owner_display, cx);
    // No window took the request: it must not surface in a later, unrelated opening.
    if backend.read(cx).connect_wizard_window.is_none() {
        backend.update(cx, |b, _| b.connect_login_request = None);
    }
}

/// Where the wizard opens: [`DEFAULT_SIZE`] at the content `zoom`, centred in `work` — the
/// display's work area, the part the taskbar or dock leaves free — and held to its size, so the
/// footer's buttons never open under a small screen's taskbar. Never under [`MIN_SIZE`]: a work
/// area smaller than that is overhung rather than given a window its layout is not made for.
/// Without a known display the default size stands at the origin.
fn opening_bounds(work: Option<Bounds<Pixels>>, zoom: f32) -> Bounds<Pixels> {
    let fit = |want: f32, min: f32, room: Option<Pixels>| {
        let held = room.map_or(want, |room| want.min(f32::from(room)));
        px(held.max(min))
    };
    let size = size(
        fit(
            DEFAULT_SIZE.0 * zoom,
            MIN_SIZE.0 * zoom,
            work.map(|work| work.size.width),
        ),
        fit(
            DEFAULT_SIZE.1 * zoom,
            MIN_SIZE.1 * zoom,
            work.map(|work| work.size.height),
        ),
    );
    match work {
        Some(work) => Bounds::centered_at(work.center(), size),
        None => Bounds {
            origin: point(px(0.0), px(0.0)),
            size,
        },
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
    let work = display_id
        .and_then(|id| cx.find_display(id))
        .or_else(|| cx.primary_display())
        .map(|display| display.visible_bounds());
    let bounds = opening_bounds(work, zoom);
    let title = |_: &App| t!("connect_wizard.window_title").to_string();
    let mut opts = crate::window::windowing::tool_window_options(
        title(cx),
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
        cx.new(|cx| {
            crate::window::windowing::frame(
                Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque),
            )
        })
    });
    match created {
        Ok(handle) => {
            backend.update(cx, |bk, _| bk.connect_wizard_window = Some(handle));
            crate::window::titles::track(handle.into(), owner.is_some(), title, cx);
            crate::window::windowing::activate_new_window(handle.into(), cx);
        }
        Err(error) => log::warn!("connect wizard could not be opened: {error:#}"),
    }
}
