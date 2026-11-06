//! Pure page models: command facts select content and actions before GPUI lays them out.

mod addr;
mod errors;
mod exchange;
mod keys;
mod log;
mod mpkey;
mod retry;
mod service;
mod status;
#[cfg(test)]
pub(super) mod tests;
mod update;

use crate::backend::core_cmds::{CmdPhase, CoreFacts, MenuForm};
use moon_remote::core_cmds::{Cmd, LogLines};
use moon_remote::core_new::CoreState;
use rust_i18n::t;

/// The view's non-secret form and presentation state.
pub(super) struct ViewState {
    pub cmd: Cmd,
    pub core: String,
    pub form: MenuForm,
    pub version: String,
    pub latest: Option<String>,
    pub key_filled: bool,
    pub secret_filled: bool,
    pub address_valid: bool,
    pub revealed: bool,
    pub confirm: Option<Confirm>,
    pub elapsed: String,
}

/// A pending explicit confirmation, never a queued server command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Confirm {
    Service(Cmd),
    Exchange { id: String, name: String },
}

/// User intent; only the window turns an intent into a backend request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Close,
    Select(Cmd),
    Run,
    Confirm,
    CancelConfirm,
    Exchange { id: String, name: String },
    LogLines(LogLines),
    CopyLog,
    ShowKey,
    HideKey,
    CopyKey,
    OpenUpdates,
    AddLogin,
}

/// Semantic severity mapped to palette tokens by the chrome module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tone {
    Ok,
    Bad,
    Run,
    Warn,
}

/// A button's behavior and styling remain inspectable without a GPUI runtime.
pub(super) struct Button {
    pub key: &'static str,
    pub action: Action,
    pub primary: bool,
    pub danger: bool,
    pub enabled: bool,
}

impl Button {
    /// Ordinary enabled action, promoted to primary or danger by its page builder.
    pub fn new(key: &'static str, action: Action) -> Self {
        Self {
            key,
            action,
            primary: false,
            danger: false,
            enabled: true,
        }
    }
    /// Mark the page's principal action.
    pub fn primary(mut self) -> Self {
        self.primary = true;
        self
    }
    /// Mark a destructive action using MoonUI's danger variant.
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
    /// Make invalid or in-progress actions inert.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// A page block; secret values are deliberately absent from this presentation model.
pub(super) enum Block {
    Text(String),
    State(Tone, String, String),
    Kv(Vec<(&'static str, String)>),
    Log(Vec<String>),
    LogControls,
    Confirm {
        danger: bool,
        title: String,
        why: String,
        buttons: Vec<Button>,
    },
    Exchanges(Vec<(String, String, bool, bool)>),
    Buttons(Vec<Button>),
    Keys,
    Secret,
    Address,
}

/// Named page states used by the behavior and real-window layout sweeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Status,
    Loading,
    LoginError,
    Retry,
    Done,
    Failed,
    Log,
    Confirm,
    Exchange,
    Keys,
    Secret,
    Address,
    Update,
    NoLogin,
    NotOurs,
}

/// Complete page content and footer from one immutable facts snapshot.
pub(super) struct Page {
    pub kind: Kind,
    pub title: &'static str,
    pub body: Vec<Block>,
    pub footer_left: Option<String>,
    pub footer_right: Vec<Button>,
}

impl Page {
    /// Start with the common close action; forms replace it with cancel and submit.
    fn new(kind: Kind, title: &'static str) -> Self {
        Self {
            kind,
            title,
            body: Vec::new(),
            footer_left: None,
            footer_right: vec![Button::new("core_cmds.common.close", Action::Close)],
        }
    }
    /// Append a semantic content block.
    fn block(mut self, block: Block) -> Self {
        self.body.push(block);
        self
    }
}

/// Route errors first so stale successful data never conceals the latest command failure.
pub(super) fn build(facts: &CoreFacts, view: &ViewState) -> Page {
    if let Some(mut page) = errors::build(facts, view) {
        if page.kind == Kind::Failed && matches!(view.cmd, Cmd::ApiKeys | Cmd::MoonProtoAddress) {
            let valid = if view.cmd == Cmd::ApiKeys {
                page.body.push(Block::Keys);
                view.key_filled && view.secret_filled
            } else {
                page.body.push(Block::Address);
                view.address_valid
            };
            for button in &mut page.footer_right {
                if button.action == Action::Run {
                    button.enabled = valid;
                }
            }
        }
        return page;
    }
    match view.cmd {
        Cmd::Status => status::build(facts, view),
        Cmd::RetryLogin => retry::build(facts, view),
        Cmd::Log => log::build(facts, view),
        Cmd::Start | Cmd::Stop | Cmd::Restart => service::build(facts, view),
        Cmd::SwitchExchange => exchange::build(facts, view),
        Cmd::ApiKeys => keys::build(facts, view),
        Cmd::ConnectionKey => mpkey::build(facts, view),
        Cmd::MoonProtoAddress => addr::build(facts, view),
        Cmd::UpdateCore => update::build(facts, view),
    }
}

/// The selected command's lifecycle, defaulting to an unstarted page.
fn phase<'a>(facts: &'a CoreFacts, view: &ViewState) -> &'a CmdPhase {
    facts.phases.get(&view.cmd).unwrap_or(&CmdPhase::Idle)
}

/// Stable wire spelling for a core-reported state, including newer unknown states.
pub(super) fn state_name(state: &CoreState) -> &str {
    match state {
        CoreState::Initializing => "initializing",
        CoreState::Ready => "ready",
        CoreState::NeedsKeys => "needs_keys",
        CoreState::LoginError => "login_error",
        CoreState::RegistrationError => "registration_error",
        CoreState::ActionRequired => "action_required",
        CoreState::Other(name) => name,
    }
}

/// Prefer the human exchange name, falling back to its technical identifier or unknown.
fn exchange_name(facts: &CoreFacts) -> String {
    facts
        .status
        .as_ref()
        .map(|s| {
            if s.exchange_name.is_empty() {
                s.exchange.clone()
            } else {
                s.exchange_name.clone()
            }
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| t!("core_cmds.status.unknown").to_string())
}

/// The dictionary heading for a command, also used when an error replaces its page.
pub(super) fn title(cmd: Cmd) -> &'static str {
    match cmd {
        Cmd::Status => "core_cmds.status.title",
        Cmd::RetryLogin => "core_cmds.retry.title",
        Cmd::Log => "core_cmds.log.title",
        Cmd::Restart => "core_cmds.service.restart_title",
        Cmd::Stop => "core_cmds.service.stop_title",
        Cmd::Start => "core_cmds.service.start_title",
        Cmd::SwitchExchange => "core_cmds.exchange.title",
        Cmd::ApiKeys => "core_cmds.keys.title",
        Cmd::ConnectionKey => "core_cmds.mpkey.title",
        Cmd::MoonProtoAddress => "core_cmds.addr.title",
        Cmd::UpdateCore => "core_cmds.update.title",
    }
}
