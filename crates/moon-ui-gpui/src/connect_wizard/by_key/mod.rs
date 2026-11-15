//! Instructions and owned connection attempts for an existing Moonbot core.

use std::time::{Duration, Instant};
use gpui::*;
use moon_core::config::{CoreTarget, parse_endpoint_override, target_from_key};
use moon_core::feed::{ConnStatus, FailureClass};
use moon_ui::{MoonButton, MoonInputState};
use rust_i18n::t;
use super::{Branch, ConnectWizardView};
use super::render::{Screen, note};
use super::server::text;
use crate::design;
use logic::{FailScreen, Host, Step};

pub(super) mod logic;
mod parts;
mod steps;
mod failure;
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
            step: Step::Port, host: Host::WindowsServer,
            port: cx.new(|cx| MoonInputState::new(window, cx).default_value(logic::FACTORY_PORT.to_string())),
            name: cx.new(|cx| MoonInputState::new(window, cx)),
            key: cx.new(|cx| MoonInputState::new(window, cx).masked(true)),
            ip: cx.new(|cx| MoonInputState::new(window, cx)),
            uid: None, since: None, copied_at: None, accepted_mismatch: None,
            chosen_port: logic::FACTORY_PORT, created: false, failure: None, awaiting_reconnect: false,
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
        if self.created && let Some(uid) = self.uid { drop(uid)?; }
        self.forget_attempt();
        Ok(())
    }

    /// Observe one verdict without replacing the first failure during automatic retries.
    fn retain_failure(&mut self, ready: bool, observed: Option<Failure>) {
        if ready { self.failure = None; }
        else if self.failure.is_none() { self.failure = observed; }
    }
}

impl ConnectWizardView {
    /// Dispatch instructions before the row exists, then its connection progress.
    pub(super) fn by_key_screen(&mut self, cx: &mut Context<Self>) -> Screen {
        if let Some(uid) = self.by_key.uid { return self.by_key_progress(uid, cx); }
        let screen = match self.by_key.step {
            Step::Port => steps::port_screen(steps::PortOptions {
                host: self.by_key.host,
                command_port: logic::parse_port(&text(&self.by_key.port, cx)).unwrap_or(self.by_key.chosen_port),
                copied: self.by_key.copied(),
            }, &self.by_key.port,
                {
                    let view = cx.entity().downgrade();
                    move |host, _, _, cx| { let _ = view.update(cx, |this, cx| { this.by_key.host = host; cx.notify(); }); }
                },
                cx.listener(|this, _, _, cx| { this.choice = Branch::NewCore; this.next_from_start(cx); }),
                cx.listener(|this, _, _, cx| this.by_key_copy(cx)), cx)
                .right(self.by_key_next_button(t!("connect_wizard.next").to_string(), cx)),
            Step::Moonbot => steps::moonbot_screen(self.by_key.chosen_port, cx)
                .right(self.by_key_next_button(t!("connect_wizard.by_key.key_copied").to_string(), cx)),
            Step::Key => {
                let key = text(&self.by_key.key, cx);
                let target = target_from_key(&key, None);
                let mismatch = logic::pending_mismatch(self.by_key.accepted_mismatch.as_ref(), &key, self.by_key.chosen_port, target.as_ref());
                let mut screen = steps::key_screen(&self.by_key.name, &self.by_key.key, mismatch, self.by_key.chosen_port,
                    cx.listener(|this, _, window, cx| this.by_key_paste(window, cx)), cx);
                if let Some(port) = mismatch {
                    screen = screen.right(MoonButton::new("cw-key-accept").size(design::CONTROL_TIER)
                        .label(t!("connect_wizard.by_key.connect_with_port", port = port).to_string())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.by_key.accepted_mismatch = Some((text(&this.by_key.key, cx), this.by_key.chosen_port));
                            this.by_key_connect(window, cx);
                        })).render())
                        .right(MoonButton::new("cw-key-repaste").primary().size(design::CONTROL_TIER)
                            .label(t!("connect_wizard.by_key.paste_again").to_string())
                            .on_click(cx.listener(|this, _, window, cx| this.by_key_paste(window, cx))).render());
                } else {
                    screen = screen.right(MoonButton::new("cw-key-connect").primary().size(design::CONTROL_TIER)
                        .label(t!("connect_wizard.by_key.connect").to_string())
                        .on_click(cx.listener(|this, _, window, cx| this.by_key_connect(window, cx))).render());
                }
                screen
            }
        };
        screen.left(MoonButton::new("cw-key-back").size(design::CONTROL_TIER)
            .label(t!("connect_wizard.back").to_string())
            .on_click(cx.listener(|this, _, _, cx| this.by_key_back(cx))).render())
    }

    /// Shared primary action for the two instruction steps.
    fn by_key_next_button(&self, label: String, cx: &mut Context<Self>) -> impl IntoElement {
        MoonButton::new("cw-key-next").primary().size(design::CONTROL_TIER).label(label)
            .on_click(cx.listener(|this, _, _, cx| this.by_key_next(cx))).render()
    }

    /// Freeze a valid port before giving instructions which depend on it.
    fn by_key_next(&mut self, cx: &mut Context<Self>) {
        self.by_key.accepted_mismatch = None;
        self.error = None;
        match self.by_key.step {
            Step::Port => match logic::parse_port(&text(&self.by_key.port, cx)) {
                Some(port) => { self.by_key.chosen_port = port; self.by_key.step = Step::Moonbot; }
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
            self.by_key.key.update(cx, |state, cx| state.set_value(key, window, cx));
        }
        self.by_key.accepted_mismatch = None;
        self.error = None;
        cx.notify();
    }

    /// Copy the displayed command, repainting once after its acknowledgement expires.
    fn by_key_copy(&mut self, cx: &mut Context<Self>) {
        let port = self.by_key.failure.as_ref().map(|failure| failure.port)
            .unwrap_or_else(|| logic::parse_port(&text(&self.by_key.port, cx)).unwrap_or(self.by_key.chosen_port));
        cx.write_to_clipboard(ClipboardItem::new_string(logic::firewall_command(port)));
        self.by_key.copied_at = Some(Instant::now());
        cx.spawn(async move |view, cx| {
            cx.background_executor().timer(COPY_WAIT).await;
            let _ = view.update(cx, |_, cx| cx.notify());
        }).detach();
        cx.notify();
    }

    /// Reuse an existing key's row without taking ownership, or persist a new one.
    fn by_key_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = text(&self.by_key.key, cx);
        if !moon_core::config::key_is_readable(&key) {
            self.error = Some(t!("connect_wizard.by_key.bad_key").to_string());
            cx.notify(); return;
        }
        let known = self.backend.read(cx).config.servers.iter()
            .find(|row| row.key.expose().trim() == key).map(|row| row.uid);
        let result = if let Some(uid) = known { Ok(uid) } else {
            let name = text(&self.by_key.name, cx);
            self.backend.update(cx, |b, cx| b.wizard_add_row(if name.is_empty() { DEFAULT_NAME } else { &name }, &key, cx))
        };
        match result {
            Ok(uid) => {
                self.by_key.uid = Some(uid);
                self.by_key.created = known.is_none();
                self.error = None;
                self.start_by_key_wait(cx);
                self.by_key.key.update(cx, |state, cx| state.set_value(String::new(), window, cx));
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
        }).detach();
    }

    /// Resolve the saved key together with its separately persisted override.
    fn by_key_target(&self, uid: u64, cx: &App) -> Option<CoreTarget> {
        let row = self.backend.read(cx).config.servers.iter().find(|row| row.uid == uid)?;
        let endpoint = parse_endpoint_override(&row.endpoint_override).ok()?;
        target_from_key(row.key.expose(), endpoint.as_ref())
    }

    /// Ready wins; the first failure survives the session's automatic retry statuses.
    fn by_key_progress(&mut self, uid: u64, cx: &mut Context<Self>) -> Screen {
        let (status, diagnosis) = {
            let backend = self.backend.read(cx);
            let core = backend.session.store().core(uid);
            (backend.session.status_map().get(&uid).cloned(), core.and_then(|core|
                moon_core::feed::diagnose(&core.status, core.fault.as_ref(), &core.startup)))
        };
        let target = self.by_key_target(uid, cx);
        let address = target.as_ref().map(CoreTarget::text).unwrap_or_default();
        if status == Some(ConnStatus::Ready) {
            self.by_key.retain_failure(true, None);
            return Screen::new(t!("connect_wizard.done.title"))
                .child(self.done_body(uid, Some(address), None, false, cx))
                .left(self.again_button(cx)).right(self.finish_button(cx));
        }
        let failed = matches!(status, Some(ConnStatus::Failed(_)));
        if !failed { self.by_key.awaiting_reconnect = false; }
        let timed_out = self.by_key.since.is_some_and(|since| since.elapsed() >= CONNECT_WAIT);
        let pick = logic::pick_failure(diagnosis.as_ref().map(|d| &d.class), target.as_ref(), self.by_key.host);
        if self.by_key.failure.is_none() && (pick == FailScreen::NoAddress || timed_out
            || (failed && diagnosis.is_some() && !self.by_key.awaiting_reconnect)) {
            let seconds = match diagnosis.as_ref().map(|d| &d.class) {
                Some(FailureClass::NoResponse { elapsed_ms, .. }) => elapsed_ms / 1000,
                _ => 0,
            };
            let detail = diagnosis.as_ref().map(|d| crate::conn_diag::fault_line(d, None))
                .unwrap_or_else(|| match status { Some(ConnStatus::Failed(reason) | ConnStatus::Stage(reason)) => reason, _ => String::new() });
            self.by_key.retain_failure(false, Some(Failure { screen: pick, address: address.clone(), port: target.as_ref().map(|t| t.port).unwrap_or(self.by_key.chosen_port), seconds, detail }));
        }
        if let Some(failure) = self.by_key.failure.clone() { return self.by_key_failure(failure, cx); }
        steps::connecting_screen(&address, cx).right(MoonButton::new("cw-key-cancel").size(design::CONTROL_TIER)
            .label(t!("connect_wizard.cancel").to_string())
            .on_click(cx.listener(|this, _, _, cx| this.by_key_drop(cx))).render())
    }

    /// Recovery screens retain one primary action and an ownership-aware delete action.
    fn by_key_failure(&self, failure: Failure, cx: &mut Context<Self>) -> Screen {
        let screen = match failure.screen {
            FailScreen::NoResponse { silent } => failure::no_response_screen(failure::NoResponse {
                port: failure.port, address: &failure.address, seconds: failure.seconds, silent,
                fault: failure.detail, copied: self.by_key.copied(),
            }, cx.listener(|this, _, _, cx| this.by_key_copy(cx)), cx),
            FailScreen::KeyRefused => failure::key_refused_screen(&self.by_key.name, &self.by_key.key,
                cx.listener(|this, _, window, cx| this.by_key_paste(window, cx)), cx),
            FailScreen::NoAddress => failure::no_address_screen(&self.by_key.ip, failure.port, cx),
            FailScreen::Fallback => Screen::new(t!("connect_wizard.by_key.noresp_title")).child(note(failure.detail, cx)),
        };
        let label = match failure.screen {
            FailScreen::KeyRefused => t!("connect_wizard.by_key.refused_connect"),
            FailScreen::NoAddress => t!("connect_wizard.by_key.connect"),
            _ => t!("connect_wizard.check_again"),
        };
        screen.left(MoonButton::new("cw-key-drop").size(design::CONTROL_TIER)
            .label(t!("connect_wizard.by_key.drop").to_string())
            .on_click(cx.listener(|this, _, _, cx| this.by_key_drop(cx))).render())
            .right(MoonButton::new("cw-key-retry").primary().size(design::CONTROL_TIER).label(label.to_string())
                .on_click(cx.listener(move |this, _, window, cx| match failure.screen {
                    FailScreen::KeyRefused => this.by_key_new_key(window, cx),
                    FailScreen::NoAddress => this.by_key_set_address(cx),
                    _ => this.by_key_retry(cx),
                })).render())
    }

    /// Explicit retry clears the latch and starts waiting for the queued replacement.
    fn by_key_retry(&mut self, cx: &mut Context<Self>) {
        if let Some(uid) = self.by_key.uid {
            self.backend.update(cx, |b, cx| { b.reconnect_request.push(uid); cx.notify(); });
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
            cx.notify(); return;
        }
        let Some(uid) = self.by_key.uid else { return; };
        match self.backend.update(cx, |b, cx| b.wizard_set_key(uid, &key, cx)) {
            Ok(()) => {
                self.by_key.key.update(cx, |state, cx| state.set_value(String::new(), window, cx));
                self.by_key.accepted_mismatch = None;
                self.by_key_retry(cx);
            }
            Err(error) => { self.by_key_error(error); cx.notify(); }
        }
    }

    /// Save a host-only override while keeping the exported port.
    fn by_key_set_address(&mut self, cx: &mut Context<Self>) {
        let ip = text(&self.by_key.ip, cx);
        let valid = parse_endpoint_override(&ip).ok().flatten()
            .is_some_and(|value| value.host.is_some() && value.port.is_none());
        if !valid {
            self.error = Some(t!("connect_wizard.server.need_ipv4").to_string());
            cx.notify(); return;
        }
        let Some(uid) = self.by_key.uid else { return; };
        match self.backend.update(cx, |b, cx| b.wizard_set_address(uid, &ip, cx)) {
            Ok(()) => self.by_key_retry(cx),
            Err(error) => { self.by_key_error(error); cx.notify(); }
        }
    }

    /// Delete only a row created here; reused cores are simply left configured.
    fn by_key_drop(&mut self, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        if let Err(error) = self.by_key.abandon(|uid| backend.update(cx, |b, cx| b.wizard_drop_row(uid, cx))) {
            self.by_key_error(error); cx.notify(); return;
        }
        self.error = None;
        cx.notify();
    }

    /// Report persistence errors without discarding the recovery state.
    fn by_key_error(&mut self, error: anyhow::Error) {
        self.error = Some(t!("connect_wizard.row_failed", reason = format!("{error:#}")).to_string());
    }
}
