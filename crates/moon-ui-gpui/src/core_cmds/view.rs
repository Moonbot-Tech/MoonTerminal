//! Command-window interaction, form lifetime, and rendering of pure page descriptions.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_remote::core_cmds::{Cmd, LogLines};
use moon_ui::{
    MoonButton, MoonButtonVariant, MoonInput, MoonInputState, MoonPalette, MoonSelect,
    MoonSelectEvent, MoonSelectItem, MoonSelectState, MoonWindowFrame, MoonWindowFrameControls,
    h_flex, rgba_from, v_flex,
};
use rust_i18n::t;
use zeroize::Zeroizing;

use super::pages::{Action, Block, Confirm, ViewState};
use super::{chrome, pages, rail};
use crate::backend::core_cmds::{CmdArgs, CmdPhase, CmdRequest, CoreFacts, MenuForm};
use crate::{Backend, design};

/// Command forms hold inputs; command outcomes and the revealed connection key stay in Backend.
pub(super) struct CoreCmdsView {
    pub(super) backend: Option<Entity<Backend>>,
    pub(super) uid: u64,
    pub(super) cmd: Cmd,
    pub(super) log_lines: LogLines,
    pub(super) key_input: Entity<MoonInputState>,
    pub(super) secret_input: Entity<MoonInputState>,
    pub(super) host_input: Entity<MoonInputState>,
    pub(super) port_input: Entity<MoonInputState>,
    pub(super) revealed: bool,
    pub(super) confirm: Option<Confirm>,
    pub(super) picker: Entity<MoonSelectState<Cmd>>,
    pub(super) log_picker: Entity<MoonSelectState<LogLines>>,
    pub(super) elapsed: Option<std::time::Duration>,
    pub(super) subscriptions: Vec<Subscription>,
    pub(super) hosts: moon_remote::hosts::Hosts,
    pub(super) nav_signature: Option<(MenuForm, Option<bool>, String)>,
    #[cfg(test)]
    pub(super) fixture: Option<CoreFacts>,
    #[cfg(test)]
    pub(super) fixture_form: Option<MenuForm>,
    #[cfg(test)]
    pub(super) fixture_version: Option<(String, Option<String>)>,
}

/// The saved Settings server name is exactly the name copied into the Connections list row.
pub(super) fn core_name(backend: &Backend, uid: u64) -> String {
    backend
        .config
        .servers
        .iter()
        .find(|s| s.uid == uid)
        .map(|s| s.name.clone())
        .unwrap_or_default()
}

impl CoreCmdsView {
    /// Create controls and observe only the shared backend and this window's input entities.
    pub(super) fn new(
        backend: Entity<Backend>,
        uid: u64,
        cmd: Cmd,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::controls(uid, cmd, window, cx);
        view.subscriptions
            .push(cx.observe(&backend, |_, _, cx| cx.notify()));
        view.hosts = match moon_remote::hosts::Hosts::load(&moon_remote::hosts::Hosts::path()) {
            Ok(hosts) => hosts,
            Err(error) => {
                log::warn!("core commands could not read host metadata: {error:#}");
                Default::default()
            }
        };
        view.backend = Some(backend);
        view
    }

    /// Initialize the production form controls; tests reuse this same real view without a session.
    pub(super) fn controls(
        uid: u64,
        cmd: Cmd,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = |masked, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| MoonInputState::new(window, cx).masked(masked))
        };
        let key_input = input(false, window, cx);
        let secret_input = input(true, window, cx);
        let host_input = input(false, window, cx);
        let port_input = input(false, window, cx);
        key_input.update(cx, |s, cx| {
            s.set_placeholder(t!("core_cmds.keys.key_ph").to_string(), window, cx)
        });
        secret_input.update(cx, |s, cx| {
            s.set_placeholder(t!("core_cmds.keys.secret_ph").to_string(), window, cx)
        });
        host_input.update(cx, |s, cx| s.set_value("0.0.0.0", window, cx));
        port_input.update(cx, |s, cx| s.set_value(String::new(), window, cx));
        let choices: Vec<_> = rail::RAIL
            .iter()
            .filter_map(|row| match row {
                rail::Row::Command(cmd, key) => {
                    Some(MoonSelectItem::new(*cmd, t!(*key).to_string()))
                }
                _ => None,
            })
            .collect();
        let picker = cx.new(|cx| MoonSelectState::new(choices, None, window, cx));
        picker.update(cx, |picker, _| {
            picker.set_selected_value(&cmd);
        });
        let log_choices = [
            (LogLines::L200, "core_cmds.log.lines_200"),
            (LogLines::L500, "core_cmds.log.lines_500"),
            (LogLines::L1000, "core_cmds.log.lines_1000"),
        ]
        .map(|(lines, key)| MoonSelectItem::new(lines, t!(key).to_string()))
        .to_vec();
        let log_picker = cx.new(|cx| MoonSelectState::new(log_choices, None, window, cx));
        log_picker.update(cx, |picker, _| {
            picker.set_selected_value(&LogLines::L200);
        });
        let mut subscriptions = vec![];
        for input in [&key_input, &secret_input, &host_input, &port_input] {
            subscriptions.push(cx.observe(input, |_, _, cx| cx.notify()));
        }
        subscriptions.push(cx.subscribe_in(
            &picker,
            window,
            |this, _, event: &MoonSelectEvent<Cmd>, window, cx| {
                if let MoonSelectEvent::Confirm(Some(cmd)) = event {
                    this.select(*cmd, false, window, cx);
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &log_picker,
            window,
            |this, _, event: &MoonSelectEvent<LogLines>, window, cx| {
                if let MoonSelectEvent::Confirm(Some(lines)) = event {
                    this.act(Action::LogLines(*lines), window, cx);
                }
            },
        ));
        Self {
            backend: None,
            uid,
            cmd,
            log_lines: LogLines::L200,
            key_input,
            secret_input,
            host_input,
            port_input,
            revealed: false,
            confirm: None,
            picker,
            log_picker,
            elapsed: None,
            subscriptions,
            hosts: Default::default(),
            nav_signature: None,
            #[cfg(test)]
            fixture: None,
            #[cfg(test)]
            fixture_form: None,
            #[cfg(test)]
            fixture_version: None,
        }
    }

    /// Select a page; only read commands and an explicitly requested retry auto-start.
    pub(super) fn select(
        &mut self,
        cmd: Cmd,
        start: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (form, active) = self
            .backend
            .as_ref()
            .and_then(|b| b.read(cx).core_cmds.facts.get(&self.uid))
            .map(|f| {
                (
                    form(f),
                    f.info
                        .as_ref()
                        .and_then(|i| i.located.active)
                        .or_else(|| f.located.as_ref().and_then(|l| l.active)),
                )
            })
            .unwrap_or((MenuForm::Checking, None));
        if !rail::available(form, active, cmd) && self.cmd != cmd {
            let current = self.cmd;
            self.picker.update(cx, |picker, _| {
                picker.set_selected_value(&current);
            });
            return;
        }
        if self.cmd != cmd {
            self.clear_keys(window, cx);
        }
        self.cmd = cmd;
        self.picker.update(cx, |picker, _| {
            picker.set_selected_value(&cmd);
        });
        self.confirm = matches!(cmd, Cmd::Restart | Cmd::Stop).then_some(Confirm::Service(cmd));
        if cmd == Cmd::MoonProtoAddress {
            let port = self
                .backend
                .as_ref()
                .and_then(|b| b.read(cx).core_cmds.facts.get(&self.uid))
                .map(|f| f.port.to_string())
                .unwrap_or_default();
            self.port_input
                .update(cx, |s, cx| s.set_value(port, window, cx));
        }
        if matches!(cmd, Cmd::Status | Cmd::Log | Cmd::SwitchExchange)
            || (start && cmd == Cmd::RetryLogin)
        {
            self.run(window, cx);
        }
        cx.notify();
    }

    /// Queue one typed request and notify backend observers without opening any network connection.
    fn send(&self, request: CmdRequest, cx: &mut App) {
        if let Some(backend) = &self.backend {
            backend.update(cx, |backend, cx| {
                if let CmdRequest::Run { cmd, .. } = &request
                    && *cmd == Cmd::RetryLogin
                    && backend.core_cmds.pending.iter().any(|(uid, request)| *uid == self.uid && matches!(request, CmdRequest::Run { cmd: queued, .. } if queued == cmd))
                {
                    return;
                }
                if matches!(request, CmdRequest::Forget)
                    && backend.core_cmds.pending.iter().any(|(uid,request)| *uid == self.uid && matches!(request,CmdRequest::Forget))
                {
                    return;
                }
                backend.core_cmds_send(self.uid, request);
                cx.notify();
            });
        }
    }

    /// Discard backend-owned connection-key material and stop displaying it immediately.
    pub(super) fn forget(&mut self, cx: &mut App) {
        let needed = self.revealed;
        self.revealed = false;
        if needed {
            self.send(CmdRequest::Forget, cx);
        }
    }

    /// Clear API credential fields after submitting them or leaving their page.
    fn clear_keys(&self, window: &mut Window, cx: &mut Context<Self>) {
        for input in [&self.key_input, &self.secret_input] {
            input.update(cx, |s, cx| s.set_value(String::new(), window, cx));
        }
    }

    /// Validate form arguments and gate destructive operations before they reach the queue.
    fn run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(backend) = &self.backend
            && let Some(facts) = backend.read(cx).core_cmds.facts.get(&self.uid)
        {
            let active = facts
                .info
                .as_ref()
                .and_then(|i| i.located.active)
                .or_else(|| facts.located.as_ref().and_then(|l| l.active));
            if !rail::available(self.state(facts, cx).form, active, self.cmd) {
                return;
            }
            if matches!(facts.phases.get(&self.cmd), Some(CmdPhase::Running { .. })) {
                return;
            }
        }
        if matches!(self.cmd, Cmd::Restart | Cmd::Stop) {
            return;
        }
        if self.cmd == Cmd::SwitchExchange {
            self.send(
                CmdRequest::Run {
                    cmd: self.cmd,
                    args: CmdArgs::None,
                },
                cx,
            );
            return;
        }
        let args = match self.cmd {
            Cmd::Log => CmdArgs::Log(self.log_lines),
            Cmd::ApiKeys => {
                let key = Zeroizing::new(self.key_input.read(cx).value().to_string());
                let secret = Zeroizing::new(self.secret_input.read(cx).value().to_string());
                if key.is_empty() || secret.is_empty() {
                    return;
                }
                self.clear_keys(window, cx);
                CmdArgs::ApiKeys { key, secret }
            }
            Cmd::MoonProtoAddress => {
                let host = self.host_input.read(cx).value().trim().to_string();
                let Some(port) = self
                    .port_input
                    .read(cx)
                    .value()
                    .trim()
                    .parse::<u16>()
                    .ok()
                    .filter(|p| *p > 0)
                else {
                    return;
                };
                if host.parse::<std::net::IpAddr>().is_err() {
                    return;
                }
                CmdArgs::Address { host, port }
            }
            _ => CmdArgs::None,
        };
        self.send(
            CmdRequest::Run {
                cmd: self.cmd,
                args,
            },
            cx,
        );
    }

    /// Dispatch one page action; confirmation alone is allowed to enqueue service/exchange changes.
    pub(super) fn act(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            Action::Close => {
                self.forget(cx);
                self.clear_keys(window, cx);
                window.remove_window();
            }
            Action::Select(cmd) => {
                self.select(cmd, cmd == Cmd::RetryLogin, window, cx);
            }
            Action::Run if matches!(self.cmd, Cmd::Restart | Cmd::Stop) => {
                self.confirm = Some(Confirm::Service(self.cmd));
                self.send(CmdRequest::Refresh, cx);
            }
            Action::Run => self.run(window, cx),
            Action::Confirm => {
                let Some(confirm) = self.confirm.take() else {
                    return;
                };
                let (cmd, args) = match confirm {
                    Confirm::Service(cmd) => (cmd, CmdArgs::None),
                    Confirm::Exchange { id, .. } => (Cmd::SwitchExchange, CmdArgs::Exchange(id)),
                };
                self.send(CmdRequest::Run { cmd, args }, cx);
            }
            Action::CancelConfirm => self.confirm = None,
            Action::Exchange { id, name } => self.confirm = Some(Confirm::Exchange { id, name }),
            Action::LogLines(lines) => {
                self.log_lines = lines;
                self.run(window, cx);
            }
            Action::CopyLog => {
                if let Some(log) = self
                    .backend
                    .as_ref()
                    .and_then(|b| b.read(cx).core_cmds.facts.get(&self.uid))
                    .and_then(|f| f.log.as_ref())
                {
                    cx.write_to_clipboard(ClipboardItem::new_string(log.lines.join("\n")));
                }
            }
            Action::ShowKey => {
                self.revealed = true;
                self.run(window, cx);
            }
            Action::HideKey => self.forget(cx),
            Action::CopyKey => {
                if let Some(key) = self
                    .backend
                    .as_ref()
                    .and_then(|b| b.read(cx).core_cmds.facts.get(&self.uid))
                    .and_then(|f| f.key.as_ref())
                {
                    cx.write_to_clipboard(ClipboardItem::new_string(key.to_string()));
                }
            }
            Action::OpenUpdates => self.on_open_updates(window, cx),
            Action::AddLogin => self.on_add_login(window, cx),
        }
        cx.notify();
    }

    /// Open the existing fleet Updates surface from the command window's button handler.
    pub(crate) fn on_open_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(backend) = &self.backend {
            crate::panels::open_updates(backend, self.uid, window, cx);
        }
    }

    /// Open the provider-login wizard for this core's IP from its no-login page.
    pub(crate) fn on_add_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(backend) = self.backend.clone() else {
            return;
        };
        let ip = {
            let b = backend.read(cx);
            b.core_cmds_facts(self.uid)
                .map(|facts| target_host(&facts.target).to_string())
                .filter(|host| !host.is_empty())
                .or_else(|| {
                    let server = b
                        .config
                        .servers
                        .iter()
                        .find(|server| server.uid == self.uid)?;
                    let endpoint =
                        moon_core::config::parse_endpoint_override(&server.endpoint_override)
                            .ok()?;
                    moon_core::config::target_from_key(server.key.expose(), endpoint.as_ref())
                        .map(|target| target.host.to_string())
                })
        };
        if let Some(ip) = ip {
            crate::connect_wizard::open_login(
                backend,
                Some(window.window_handle()),
                window.display(cx).map(|display| display.id()),
                ip,
                cx,
            );
        }
    }

    /// Build the non-secret state used by both the production renderer and tests.
    pub(super) fn state(&self, facts: &CoreFacts, cx: &App) -> ViewState {
        let (core, version, latest) = self
            .backend
            .as_ref()
            .map(|b| {
                let b = b.read(cx);
                let data = b.session.store().core(self.uid);
                let current = data.and_then(|c| c.server_version);
                let version = current.map(|version| {
                    moon_core::util::fmt::core_build_named(
                        version,
                        data.and_then(|c| c.server_version_suffix.as_deref()),
                    )
                });
                let latest = current
                    .and_then(|current| {
                        b.session
                            .fleet_newest_version()
                            .filter(|newest| *newest > current)
                    })
                    .map(moon_core::util::fmt::core_build);
                (core_name(b, self.uid), version, latest)
            })
            .unwrap_or_else(|| ("CORE_A".to_owned(), None, None));
        #[cfg(test)]
        let (version, latest) = self
            .fixture_version
            .as_ref()
            .map(|(version, latest)| (Some(version.clone()), latest.clone()))
            .unwrap_or((version, latest));
        let elapsed = match facts.phases.get(&self.cmd) {
            Some(CmdPhase::Running { since, .. }) => Some(since.elapsed()),
            _ => self.elapsed,
        };
        let elapsed = elapsed
            .map(|d| format!("{}:{:02}", d.as_secs() / 60, d.as_secs() % 60))
            .unwrap_or_else(|| t!("core_cmds.status.unknown").to_string());
        let form = if let Some(backend) = &self.backend {
            let b = backend.read(cx);
            let host = target_host(&facts.target);
            let has_login = facts.located.is_some()
                || self.hosts.admin_by_ip(host).is_some()
                || b.core_cmds.logins.contains_key(host);
            crate::backend::core_cmds::menu_form(
                has_login || facts.target.is_empty(),
                facts.located.as_ref(),
            )
        } else {
            form(facts)
        };
        #[cfg(test)]
        let form = self.fixture_form.unwrap_or(form);
        ViewState {
            cmd: self.cmd,
            core,
            form,
            version: version.unwrap_or_else(|| t!("core_cmds.status.unknown").to_string()),
            latest,
            key_filled: !self.key_input.read(cx).value().is_empty(),
            secret_filled: !self.secret_input.read(cx).value().is_empty(),
            address_valid: self
                .host_input
                .read(cx)
                .value()
                .trim()
                .parse::<std::net::IpAddr>()
                .is_ok()
                && self
                    .port_input
                    .read(cx)
                    .value()
                    .trim()
                    .parse::<u16>()
                    .is_ok_and(|p| p > 0),
            revealed: self.revealed,
            confirm: self.confirm.clone(),
            elapsed,
        }
    }

    /// Draw one description block; API inputs and connection-key text have separate lifetimes.
    fn block(
        &self,
        block: Block,
        id: usize,
        facts: &CoreFacts,
        state: &ViewState,
        narrow: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match block {
            Block::Text(text) => {
                #[cfg(test)]
                super::tests::record(
                    format!("cc-text-{id}"),
                    text.clone(),
                    13.0,
                    FontWeight::NORMAL,
                );
                div()
                    .debug_selector(move || format!("cc-text-{id}"))
                    .w_full()
                    .text_color(rgba_from(MoonPalette::active(cx).text_muted, 1.0))
                    .child(text)
                    .into_any_element()
            }
            Block::State(tone, title, why) => {
                chrome::state_box(tone, title, why, cx).into_any_element()
            }
            Block::Kv(rows) => chrome::kv_block(rows, cx).into_any_element(),
            Block::Log(lines) => chrome::mono_block(lines, cx).into_any_element(),
            Block::LogControls => h_flex()
                .w_full()
                .flex_wrap()
                .gap(px(8.0))
                .items_center()
                .child(t!("core_cmds.log.last").to_string())
                .child(MoonSelect::new(&self.log_picker).id("cc-log-lines"))
                .into_any_element(),
            Block::Confirm {
                danger,
                title,
                why,
                buttons,
            } => chrome::confirm_box(danger, title, why, buttons, id, cx).into_any_element(),
            Block::Buttons(buttons) => {
                chrome::buttons_row(buttons, &format!("cc-body-{id}"), cx).into_any_element()
            }
            Block::Keys => self.fields(
                [
                    ("core_cmds.keys.key", &self.key_input),
                    ("core_cmds.keys.secret", &self.secret_input),
                ],
                narrow,
                cx,
            ),
            Block::Address => self.fields(
                [
                    ("core_cmds.addr.ip", &self.host_input),
                    ("core_cmds.addr.port", &self.port_input),
                ],
                narrow,
                cx,
            ),
            Block::Secret => {
                let shown = state.revealed && facts.key.is_some();
                let text = if shown {
                    facts
                        .key
                        .as_ref()
                        .map(|k| k.to_string())
                        .unwrap_or_default()
                } else {
                    "••••••••••••••••".to_owned()
                };
                let buttons = if shown {
                    vec![
                        pages::Button::new("core_cmds.common.copy", Action::CopyKey),
                        pages::Button::new("core_cmds.mpkey.hide", Action::HideKey),
                    ]
                } else {
                    vec![pages::Button::new("core_cmds.mpkey.show", Action::ShowKey).primary()]
                };
                div()
                    .flex()
                    .when(narrow, |row| row.flex_col())
                    .w_full()
                    .p(px(10.0))
                    .gap(px(8.0))
                    .bg(rgba_from(MoonPalette::active(cx).panel, 1.0))
                    .rounded(px(6.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_family(design::mono())
                            .child(text),
                    )
                    .child(
                        chrome::buttons_row(buttons, "cc-secret", cx)
                            .w_auto()
                            .flex_none(),
                    )
                    .into_any_element()
            }
            Block::Exchanges(exchanges) => {
                let p = MoonPalette::active(cx);
                v_flex()
                    .id("cc-exchanges")
                    .w_full()
                    .max_h(px(220.0))
                    .overflow_y_scroll()
                    .gap(px(1.0))
                    .children(
                        exchanges
                            .into_iter()
                            .enumerate()
                            .map(|(i, (id, name, current, pass))| {
                                let hint = if current {
                                    t!("core_cmds.exchange.current").to_string()
                                } else if pass {
                                    t!("core_cmds.exchange.passphrase").to_string()
                                } else {
                                    String::new()
                                };
                                let action = Action::Exchange {
                                    id,
                                    name: name.clone(),
                                };
                                h_flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(8.0))
                                    .w_full()
                                    .bg(rgba_from(
                                        if current { p.accent } else { p.panel },
                                        if current { 0.11 } else { 1.0 },
                                    ))
                                    .child(
                                        MoonButton::new(SharedString::from(format!(
                                            "cc-exchange-{i}"
                                        )))
                                        .label(name)
                                        .variant(MoonButtonVariant::Ghost)
                                        .mono(false)
                                        .disabled(current)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.act(action.clone(), window, cx)
                                        }))
                                        .render(),
                                    )
                                    .when(!hint.is_empty(), |row| {
                                        row.child(
                                            div()
                                                .px(px(12.0))
                                                .text_size(px(12.0))
                                                .text_color(rgba_from(p.text_muted, 1.0))
                                                .child(hint),
                                        )
                                    })
                            })
                            .collect::<Vec<_>>(),
                    )
                    .into_any_element()
            }
        }
    }

    /// Two-column forms collapse to one column at the actual minimum window width.
    fn fields(
        &self,
        fields: [(&'static str, &Entity<MoonInputState>); 2],
        narrow: bool,
        cx: &App,
    ) -> AnyElement {
        div()
            .flex()
            .w_full()
            .gap(px(10.0))
            .when(narrow, |row| row.flex_col())
            .children(fields.into_iter().enumerate().map(|(i, (key, input))| {
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgba_from(MoonPalette::active(cx).text_muted, 1.0))
                            .child(t!(key).to_string()),
                    )
                    .child(
                        MoonInput::new(SharedString::from(format!("cc-field-{key}-{i}")))
                            .state(input)
                            .size(design::INPUT_SIZE),
                    )
            }))
            .into_any_element()
    }

    /// Render the complete production frame; the fixture source is available only in test builds.
    pub(super) fn frame(
        &self,
        facts: &CoreFacts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        let width = f32::from(window.viewport_size().width);
        let narrow = width < 640.0;
        let state = self.state(facts, cx);
        let page = pages::build(facts, &state);
        let title = t!("core_cmds.window.title", core = state.core).to_string();
        let target = format!(
            "{} · {}",
            facts.target,
            facts
                .located
                .as_ref()
                .map(|l| l.dir.as_str())
                .unwrap_or_default()
        );
        let header = h_flex()
            .debug_selector(|| "cc-header".to_owned())
            .w_full()
            .flex_none()
            .h(px(34.0))
            .gap(px(8.0))
            .px(px(12.0))
            .border_b_1()
            .border_color(rgba_from(p.border, 1.0))
            .child(
                MoonWindowFrame::tool("cc-title", width)
                    .title_cluster(title, cx)
                    .min_w_0()
                    .flex_1(),
            )
            .when(!narrow, |row| {
                row.child(
                    div()
                        .max_w(px(260.0))
                        .truncate()
                        .font_family(design::mono())
                        .text_size(px(12.0))
                        .text_color(rgba_from(p.text_muted, 1.0))
                        .child(target),
                )
            })
            .when(design::show_custom_window_controls(), |row| {
                row.child(
                    MoonWindowFrame::tool("cc-controls", width)
                        .controls(MoonWindowFrameControls::Close)
                        .header_height(34.0)
                        .show_controls(true)
                        .visual_controls(cx),
                )
            });
        let active = facts
            .info
            .as_ref()
            .and_then(|i| i.located.active)
            .or_else(|| facts.located.as_ref().and_then(|l| l.active));
        let rail = v_flex()
            .id("cc-rail-scroll")
            .debug_selector(|| "cc-rail".to_owned())
            .w(px(200.0))
            .h_full()
            .flex_none()
            .overflow_y_scroll()
            .py(px(10.0))
            .bg(rgba_from(p.shell_high, 1.0))
            .border_r_1()
            .border_color(rgba_from(p.border, 1.0))
            .children(
                rail::RAIL
                    .iter()
                    .enumerate()
                    .map(|(i, row)| match row {
                        rail::Row::Caption(key) => div()
                            .flex_none()
                            .px(px(14.0))
                            .pt(px(8.0))
                            .pb(px(4.0))
                            .text_size(px(11.0))
                            .text_color(rgba_from(p.text_faint, 1.0))
                            .child(t!(*key).to_string())
                            .into_any_element(),
                        rail::Row::Command(cmd, key) => {
                            let cmd = *cmd;
                            div()
                                .w_full()
                                .flex_none()
                                .debug_selector(move || format!("cc-rail-{i}"))
                                .border_l(px(3.0))
                                .border_color(rgba_from(
                                    p.accent,
                                    if cmd == self.cmd { 1.0 } else { 0.0 },
                                ))
                                .when(cmd == self.cmd, |row| row.bg(rgba_from(p.accent, 0.11)))
                                .child(
                                    MoonButton::new(SharedString::from(format!(
                                        "cc-rail-button-{i}"
                                    )))
                                    .full_width()
                                    .size(design::CONTROL_TIER)
                                    .text_segment(
                                        t!(*key).to_string(),
                                        if cmd == self.cmd { p.accent } else { p.text },
                                        if cmd == self.cmd { 600.0 } else { 400.0 },
                                    )
                                    .mono(false)
                                    .variant(MoonButtonVariant::Ghost)
                                    .disabled(!rail::available(state.form, active, cmd))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.select(cmd, false, window, cx)
                                    }))
                                    .render(),
                                )
                                .into_any_element()
                        }
                    })
                    .collect::<Vec<_>>(),
            );
        let body = v_flex()
            .id(SharedString::from(format!("cc-page-{:?}", page.kind)))
            .debug_selector(|| "cc-page".to_owned())
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .px(px(if narrow { 14.0 } else { 20.0 }))
            .py(px(18.0))
            .gap(px(12.0))
            .when(narrow, |body| {
                body.child(MoonSelect::new(&self.picker).id("cc-command-picker"))
            })
            .child(
                div()
                    .debug_selector(|| "cc-h1".to_owned())
                    .flex_none()
                    .text_size(px(16.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t!(page.title).to_string()),
            )
            .children(
                page.body
                    .into_iter()
                    .enumerate()
                    .map(|(i, block)| {
                        div()
                            .w_full()
                            .flex_none()
                            .child(self.block(block, i, facts, &state, narrow, cx))
                    })
                    .collect::<Vec<_>>(),
            );
        v_flex()
            .size_full()
            .relative()
            .bg(rgba_from(p.shell, 1.0))
            .font_family(design::ui_font())
            .text_size(px(13.0))
            .line_height(px(19.0))
            .text_color(rgba_from(p.text, 1.0))
            .child(header)
            .child(
                h_flex()
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .when(!narrow, |row| row.child(rail))
                    .child(body),
            )
            .child(chrome::footer(
                page.footer_left,
                page.footer_right,
                narrow,
                cx,
            ))
            .child(
                MoonWindowFrame::tool("cc-frame-hit", width)
                    .controls(MoonWindowFrameControls::Close)
                    .header_height(34.0)
                    .show_controls(design::show_custom_window_controls())
                    .hit_overlay(),
            )
            .into_any_element()
    }
}

/// Cached installation identity drives availability; an explicit missing-login error has its own page.
fn form(facts: &CoreFacts) -> MenuForm {
    match facts.located.as_ref() {
        Some(located) if located.ours => MenuForm::Ours,
        Some(_) => MenuForm::Hand,
        None => MenuForm::Checking,
    }
}

/// Read the IP part of the backend's SSH display address for session/admin login lookup.
pub(super) fn target_host(target: &str) -> &str {
    let address = target
        .rsplit_once('@')
        .map_or(target, |(_, address)| address);
    if address.parse::<std::net::IpAddr>().is_ok() {
        return address;
    }
    address
        .rsplit_once(':')
        .filter(|(_, port)| port.parse::<u16>().is_ok())
        .map_or(address, |(host, _)| {
            host.trim_start_matches('[').trim_end_matches(']')
        })
}

/// Empty facts keep the loading page available before the first worker response arrives.
fn empty_facts() -> CoreFacts {
    CoreFacts {
        target: String::new(),
        port: 0,
        located: None,
        status: None,
        info: None,
        log: None,
        key: None,
        exchanges: None,
        phases: Default::default(),
        checked_at: None,
    }
}

/// Release the backend read before constructing GPUI controls; no snapshot survives a frame.
fn snapshot(facts: &CoreFacts) -> CoreFacts {
    CoreFacts {
        target: facts.target.clone(),
        port: facts.port,
        located: facts.located.clone(),
        status: facts.status.clone(),
        info: facts.info.clone(),
        log: facts.log.clone(),
        key: facts.key.as_ref().map(|k| Zeroizing::new(k.to_string())),
        exchanges: facts.exchanges.clone(),
        checked_at: facts.checked_at,
        phases: facts
            .phases
            .iter()
            .map(|(cmd, phase)| {
                (
                    *cmd,
                    match phase {
                        CmdPhase::Idle => CmdPhase::Idle,
                        CmdPhase::Done => CmdPhase::Done,
                        CmdPhase::Running { since, seen } => CmdPhase::Running {
                            since: *since,
                            seen: seen.clone(),
                        },
                        CmdPhase::Failed(error) => CmdPhase::Failed(error.clone()),
                    },
                )
            })
            .collect(),
    }
}

impl Render for CoreCmdsView {
    /// Running commands request a frame so their elapsed clock advances while facts are unchanged.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let backend = self.backend.clone();
        let fallback = empty_facts();
        let facts = backend
            .as_ref()
            .and_then(|b| b.read(cx).core_cmds.facts.get(&self.uid))
            .map(snapshot);
        #[cfg(test)]
        let facts = facts.or_else(|| self.fixture.as_ref().map(snapshot));
        let facts = facts.as_ref().unwrap_or(&fallback);
        let state = self.state(facts, cx);
        let active = facts
            .info
            .as_ref()
            .and_then(|info| info.located.active)
            .or_else(|| facts.located.as_ref().and_then(|located| located.active));
        let signature = (state.form, active, rust_i18n::locale().to_string());
        if self.nav_signature.as_ref() != Some(&signature) {
            let choices = rail::RAIL
                .iter()
                .filter_map(|row| match row {
                    rail::Row::Command(cmd, key) => Some(
                        MoonSelectItem::new(*cmd, t!(*key).to_string())
                            .disabled(!rail::available(state.form, active, *cmd)),
                    ),
                    _ => None,
                })
                .collect::<Vec<_>>();
            self.picker
                .update(cx, |picker, cx| picker.set_items(choices, cx));
            self.nav_signature = Some(signature);
        }
        if let Some(CmdPhase::Running { since, .. }) = facts.phases.get(&self.cmd) {
            self.elapsed = Some(since.elapsed());
            window.request_animation_frame();
        }
        self.frame(facts, window, cx)
    }
}
