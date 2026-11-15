//! Instructions and owned connection attempts for an existing Moonbot core.

use super::render::{Screen, note};
use super::server::text;
use super::{Branch, ConnectWizardView};
use gpui::*;
use logic::{Action, FailScreen, Footer, Host, Step};
use moon_core::config::{CoreTarget, parse_endpoint_override, target_from_key};
use moon_core::feed::{ConnStatus, FailureClass};
use moon_ui::MoonInputState;
use rust_i18n::t;
use std::time::{Duration, Instant};

mod failure;
pub(super) mod logic;
mod parts;
mod steps;
#[cfg(test)]
mod tests;

/// Deadline for a connection that never publishes a terminal verdict.
const CONNECT_WAIT: Duration = Duration::from_secs(60);
/// Brief clipboard acknowledgement, with one repaint on expiry.
const COPY_WAIT: Duration = Duration::from_millis(1200);
/// Row name when the user leaves the name empty.
const DEFAULT_NAME: &str = "Moonbot";

/// First failure retained through automatic retries, until an explicit action or Ready.
#[derive(Clone)]
struct Failure {
    screen: FailScreen,
    address: String,
    port: u16,
    seconds: u64,
    detail: String,
}

/// Inputs, instruction progress and ownership of the by-key connection attempt.
pub(super) struct ByKeyState {
    pub(super) step: Step,
    host: Host,
    port: Entity<MoonInputState>,
    name: Entity<MoonInputState>,
    key: Entity<MoonInputState>,
    ip: Entity<MoonInputState>,
    pub(super) uid: Option<u64>,
    since: Option<Instant>,
    copied_at: Option<Instant>,
    pub(super) accepted_mismatch: Option<(String, u16)>,
    chosen_port: u16,
    created: bool,
    failure: Option<Failure>,
    /// Ignore the queued reconnect's old Failed status until the replacement begins.
    awaiting_reconnect: bool,
}

impl ByKeyState {
    /// Rebuild instructions and mismatch warnings when their input text changes.
    pub(super) fn observe_inputs(&self, cx: &mut Context<ConnectWizardView>) {
        cx.observe(&self.port, |_, _, cx| cx.notify()).detach();
        cx.observe(&self.key, |_, _, cx| cx.notify()).detach();
    }

    /// Create independent inputs, with Moonbot's factory port selected initially.
    pub(super) fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            step: Step::Port,
            host: Host::WindowsServer,
            port: cx.new(|cx| {
                MoonInputState::new(window, cx).default_value(logic::FACTORY_PORT.to_string())
            }),
            name: cx.new(|cx| MoonInputState::new(window, cx)),
            key: cx.new(|cx| MoonInputState::new(window, cx).masked(true)),
            ip: cx.new(|cx| MoonInputState::new(window, cx)),
            uid: None,
            since: None,
            copied_at: None,
            accepted_mismatch: None,
            chosen_port: logic::FACTORY_PORT,
            created: false,
            failure: None,
            awaiting_reconnect: false,
        }
    }

    /// Release a completed or abandoned attempt without deleting a configured core.
    pub(super) fn forget_attempt(&mut self) {
        self.uid = None;
        self.since = None;
        self.failure = None;
        self.created = false;
        self.awaiting_reconnect = false;
        self.accepted_mismatch = None;
        self.step = Step::Key;
    }

    /// Whether the clipboard acknowledgement is still visible.
    fn copied(&self) -> bool {
        self.copied_at.is_some_and(|at| at.elapsed() < COPY_WAIT)
    }

    /// A failed deletion keeps the attempt intact; leaving a reused row never calls deletion.
    fn abandon(&mut self, drop: impl FnOnce(u64) -> anyhow::Result<()>) -> anyhow::Result<()> {
        if self.created
            && let Some(uid) = self.uid
        {
            drop(uid)?;
        }
        self.forget_attempt();
        Ok(())
    }

    /// Observe one verdict without replacing the first failure during automatic retries.
    fn retain_failure(&mut self, ready: bool, observed: Option<Failure>) {
        if ready {
            self.failure = None;
        } else if self.failure.is_none() {
            self.failure = observed;
        }
    }
}

impl ConnectWizardView {
    /// Dispatch instructions before the row exists, then its connection progress.
    pub(super) fn by_key_screen(&mut self, cx: &mut Context<Self>) -> Screen {
        if let Some(uid) = self.by_key.uid {
            return self.by_key_progress(uid, cx);
        }
        let screen = match self.by_key.step {
            Step::Port => steps::port_screen(
                steps::PortOptions {
                    host: self.by_key.host,
                    command_port: logic::parse_port(&text(&self.by_key.port, cx))
                        .unwrap_or(self.by_key.chosen_port),
                    copied: self.by_key.copied(),
                },
                &self.by_key.port,
                {
                    let view = cx.entity().downgrade();
                    move |host, _, _, cx| {
                        let _ = view.update(cx, |this, cx| {
                            this.by_key.host = host;
                            cx.notify();
                        });
                    }
                },
                cx.listener(|this, _, _, cx| {
                    this.choice = Branch::NewCore;
                    this.next_from_start(cx);
                }),
                cx.listener(|this, _, _, cx| this.by_key_copy(cx)),
                cx,
            ),
            Step::Moonbot => steps::moonbot_screen(self.by_key.chosen_port, cx),
            Step::Key => steps::key_screen(
                &self.by_key.name,
                &self.by_key.key,
                self.by_key_mismatch(cx),
                self.by_key.chosen_port,
                cx.listener(|this, _, window, cx| this.by_key_paste(window, cx)),
                cx,
            ),
        };
        let mismatch = (self.by_key.step == Step::Key)
            .then(|| self.by_key_mismatch(cx))
            .flatten();
        self.by_key_footer(screen, Footer::Instructions(self.by_key.step), mismatch, cx)
    }

    /// Resolve pending mismatch acceptance the same way for initial and replacement keys.
    fn by_key_mismatch(&self, cx: &App) -> Option<u16> {
        let key = text(&self.by_key.key, cx);
        let target = target_from_key(&key, None);
        logic::pending_mismatch(
            self.by_key.accepted_mismatch.as_ref(),
            &key,
            self.by_key.chosen_port,
            target.as_ref(),
        )
    }

    /// Bind the shared footer's intents to this wizard's live actions.
    fn by_key_footer(
        &self,
        screen: Screen,
        footer: Footer,
        mismatch: Option<u16>,
        cx: &mut Context<Self>,
    ) -> Screen {
        let view = cx.entity().downgrade();
        parts::with_footer(screen, footer, mismatch, move |action, _, window, cx| {
            let _ = view.update(cx, |this, cx| match action {
                Action::Back => this.by_key_back(cx),
                Action::Next => this.by_key_next(cx),
                Action::Connect => this.by_key_connect(window, cx),
                Action::AcceptPort => {
                    this.by_key.accepted_mismatch =
                        Some((text(&this.by_key.key, cx), this.by_key.chosen_port));
                    if matches!(footer, Footer::Failure(FailScreen::KeyRefused)) {
                        this.by_key_new_key(window, cx);
                    } else {
                        this.by_key_connect(window, cx);
                    }
                }
                Action::Paste => this.by_key_paste(window, cx),
                Action::Drop | Action::Cancel => this.by_key_drop(cx),
                Action::Retry => this.by_key_retry(cx),
                Action::NewKey => this.by_key_new_key(window, cx),
                Action::SetAddress => this.by_key_set_address(cx),
                Action::Again => {
                    this.by_key.forget_attempt();
                    this.back_to_start(cx);
                }
                Action::Finish => this.close(window, cx),
            });
        })
    }

    /// Freeze a valid port before giving instructions which depend on it.
    fn by_key_next(&mut self, cx: &mut Context<Self>) {
        self.by_key.accepted_mismatch = None;
        self.error = None;
        match self.by_key.step {
            Step::Port => match logic::parse_port(&text(&self.by_key.port, cx)) {
                Some(port) => {
                    self.by_key.chosen_port = port;
                    self.by_key.step = Step::Moonbot;
                }
                None => self.error = Some(t!("connect_wizard.server.need_port").to_string()),
            },
            Step::Moonbot => self.by_key.step = Step::Key,
            Step::Key => {}
        }
        cx.notify();
    }

    /// Back follows the rail and invalidates any previous mismatch acceptance.
    fn by_key_back(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        self.by_key.accepted_mismatch = None;
        match self.by_key.step {
            Step::Key => self.by_key.step = Step::Moonbot,
            Step::Moonbot => self.by_key.step = Step::Port,
            Step::Port => self.back_to_start(cx),
        }
        cx.notify();
    }

    /// Paste a new export and require a new decision about its port.
    fn by_key_paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(key) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.by_key
                .key
                .update(cx, |state, cx| state.set_value(key, window, cx));
        }
        self.by_key.accepted_mismatch = None;
        self.error = None;
        cx.notify();
    }

    /// Copy the displayed command, repainting once after its acknowledgement expires.
    fn by_key_copy(&mut self, cx: &mut Context<Self>) {
        let port = self
            .by_key
            .failure
            .as_ref()
            .map(|failure| failure.port)
            .unwrap_or_else(|| {
                logic::parse_port(&text(&self.by_key.port, cx)).unwrap_or(self.by_key.chosen_port)
            });
        cx.write_to_clipboard(ClipboardItem::new_string(logic::firewall_command(port)));
        self.by_key.copied_at = Some(Instant::now());
        cx.spawn(async move |view, cx| {
            cx.background_executor().timer(COPY_WAIT).await;
            let _ = view.update(cx, |_, cx| cx.notify());
        })
        .detach();
        cx.notify();
    }

    /// Reuse an existing key's row without taking ownership, or persist a new one.
    fn by_key_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = text(&self.by_key.key, cx);
        if !moon_core::config::key_is_readable(&key) {
            self.error = Some(t!("connect_wizard.by_key.bad_key").to_string());
            cx.notify();
            return;
        }
        let known = self
            .backend
            .read(cx)
            .config
            .servers
            .iter()
            .find(|row| row.key.expose().trim() == key)
            .map(|row| row.uid);
        let result = if let Some(uid) = known {
            Ok(uid)
        } else {
            let name = text(&self.by_key.name, cx);
            self.backend.update(cx, |b, cx| {
                b.wizard_add_row(if name.is_empty() { DEFAULT_NAME } else { &name }, &key, cx)
            })
        };
        match result {
            Ok(uid) => {
                self.by_key.uid = Some(uid);
                self.by_key.created = known.is_none();
                self.error = None;
                self.start_by_key_wait(cx);
                self.by_key
                    .key
                    .update(cx, |state, cx| state.set_value(String::new(), window, cx));
            }
            Err(error) => self.by_key_error(error),
        }
        cx.notify();
    }

    /// Start a bounded wait with one repaint at its deadline.
    fn start_by_key_wait(&mut self, cx: &mut Context<Self>) {
        self.by_key.since = Some(Instant::now());
        self.by_key.failure = None;
        cx.spawn(async move |view, cx| {
            cx.background_executor().timer(CONNECT_WAIT).await;
            let _ = view.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    /// Resolve the saved key together with its separately persisted override.
    fn by_key_target(&self, uid: u64, cx: &App) -> Option<CoreTarget> {
        let row = self
            .backend
            .read(cx)
            .config
            .servers
            .iter()
            .find(|row| row.uid == uid)?;
        let endpoint = parse_endpoint_override(&row.endpoint_override).ok()?;
        target_from_key(row.key.expose(), endpoint.as_ref())
    }

    /// Ready wins; the first failure survives the session's automatic retry statuses.
    fn by_key_progress(&mut self, uid: u64, cx: &mut Context<Self>) -> Screen {
        let (status, diagnosis) = {
            let backend = self.backend.read(cx);
            let core = backend.session.store().core(uid);
            (
                backend.session.status_map().get(&uid).cloned(),
                core.and_then(|core| {
                    moon_core::feed::diagnose(&core.status, core.fault.as_ref(), &core.startup)
                }),
            )
        };
        let target = self.by_key_target(uid, cx);
        let address = target.as_ref().map(CoreTarget::text).unwrap_or_default();
        if status == Some(ConnStatus::Ready) {
            self.by_key.retain_failure(true, None);
            let screen = Screen::new(t!("connect_wizard.done.title")).child(self.done_body(
                uid,
                Some(address),
                None,
                false,
                cx,
            ));
            return self.by_key_footer(screen, Footer::Done, None, cx);
        }
        let failed = matches!(status, Some(ConnStatus::Failed(_)));
        if !failed {
            self.by_key.awaiting_reconnect = false;
        }
        let timed_out = self
            .by_key
            .since
            .is_some_and(|since| since.elapsed() >= CONNECT_WAIT);
        let pick = logic::pick_failure(
            diagnosis.as_ref().map(|d| &d.class),
            target.as_ref(),
            self.by_key.host,
        );
        if self.by_key.failure.is_none()
            && logic::should_latch(
                failed,
                diagnosis.is_some(),
                self.by_key.awaiting_reconnect,
                timed_out,
                pick,
            )
        {
            let seconds = match diagnosis.as_ref().map(|d| &d.class) {
                Some(FailureClass::NoResponse { elapsed_ms, .. }) => {
                    logic::shown_seconds(*elapsed_ms)
                }
                _ => 0,
            };
            let detail = diagnosis
                .as_ref()
                .map(|d| crate::conn_diag::fault_line(d, None))
                .unwrap_or_else(|| match status {
                    Some(ConnStatus::Failed(reason) | ConnStatus::Stage(reason)) => reason,
                    _ => String::new(),
                });
            self.by_key.retain_failure(
                false,
                Some(Failure {
                    screen: pick,
                    address: address.clone(),
                    port: target
                        .as_ref()
                        .map(|t| t.port)
                        .unwrap_or(self.by_key.chosen_port),
                    seconds,
                    detail,
                }),
            );
        }
        if let Some(failure) = self.by_key.failure.clone() {
            return self.by_key_failure(failure, cx);
        }
        self.by_key_footer(
            steps::connecting_screen(&address, cx),
            Footer::Connecting,
            None,
            cx,
        )
    }

    /// Recovery screens retain one primary action and an ownership-aware delete action.
    fn by_key_failure(&self, failure: Failure, cx: &mut Context<Self>) -> Screen {
        let mismatch = (failure.screen == FailScreen::KeyRefused)
            .then(|| self.by_key_mismatch(cx))
            .flatten();
        let screen = match failure.screen {
            FailScreen::NoResponse { silent } => failure::no_response_screen(
                failure::NoResponse {
                    port: failure.port,
                    address: &failure.address,
                    seconds: failure.seconds,
                    silent,
                    fault: failure.detail,
                    copied: self.by_key.copied(),
                },
                cx.listener(|this, _, _, cx| this.by_key_copy(cx)),
                cx,
            ),
            FailScreen::KeyRefused => failure::key_refused_screen(
                &self.by_key.key,
                mismatch,
                self.by_key.chosen_port,
                cx.listener(|this, _, window, cx| this.by_key_paste(window, cx)),
                cx,
            ),
            FailScreen::NoAddress => failure::no_address_screen(&self.by_key.ip, failure.port, cx),
            FailScreen::Fallback => {
                Screen::new(t!("connect_wizard.by_key.fail_title")).child(note(failure.detail, cx))
            }
        };
        self.by_key_footer(screen, Footer::Failure(failure.screen), mismatch, cx)
    }

    /// Explicit retry clears the latch and starts waiting for the queued replacement.
    fn by_key_retry(&mut self, cx: &mut Context<Self>) {
        if let Some(uid) = self.by_key.uid {
            self.backend.update(cx, |b, cx| {
                b.reconnect_request.push(uid);
                cx.notify();
            });
            self.error = None;
            self.start_by_key_wait(cx);
            self.by_key.awaiting_reconnect = true;
            cx.notify();
        }
    }

    /// Validate a replacement export before updating the same configured row.
    fn by_key_new_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = text(&self.by_key.key, cx);
        if !moon_core::config::key_is_readable(&key) {
            self.error = Some(t!("connect_wizard.by_key.bad_key").to_string());
            cx.notify();
            return;
        }
        if self.by_key_mismatch(cx).is_some() {
            cx.notify();
            return;
        }
        let Some(uid) = self.by_key.uid else {
            return;
        };
        match self
            .backend
            .update(cx, |b, cx| b.wizard_set_key(uid, &key, cx))
        {
            Ok(()) => {
                self.by_key
                    .key
                    .update(cx, |state, cx| state.set_value(String::new(), window, cx));
                self.by_key.accepted_mismatch = None;
                self.error = None;
                self.start_by_key_wait(cx);
                self.by_key.awaiting_reconnect = true;
                cx.notify();
            }
            Err(error) => {
                self.by_key_error(error);
                cx.notify();
            }
        }
    }

    /// Let the backend validate the host and preserve the effective port before restarting the wait.
    fn by_key_set_address(&mut self, cx: &mut Context<Self>) {
        let ip = text(&self.by_key.ip, cx);
        let Some(uid) = self.by_key.uid else {
            return;
        };
        match self
            .backend
            .update(cx, |b, cx| b.wizard_set_address(uid, &ip, cx))
        {
            Ok(()) => {
                self.error = None;
                self.start_by_key_wait(cx);
                self.by_key.awaiting_reconnect = true;
                cx.notify();
            }
            Err(error) => {
                if error.is::<crate::backend::core_new::InvalidWizardAddress>() {
                    self.error = Some(t!("connect_wizard.by_key.need_host").to_string());
                } else {
                    self.by_key_error(error);
                }
                cx.notify();
            }
        }
    }

    /// Delete only a row created here; reused cores are simply left configured.
    fn by_key_drop(&mut self, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        if let Err(error) = self
            .by_key
            .abandon(|uid| backend.update(cx, |b, cx| b.wizard_drop_row(uid, cx)))
        {
            self.by_key_error(error);
            cx.notify();
            return;
        }
        self.error = None;
        cx.notify();
    }

    /// Report persistence errors without discarding the recovery state.
    fn by_key_error(&mut self, error: anyhow::Error) {
        self.error =
            Some(t!("connect_wizard.row_failed", reason = format!("{error:#}")).to_string());
    }
}
