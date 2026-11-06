//! Owner consent for AI clients, administered through the Station's existing private connection.

use std::collections::BTreeSet;

use gpui::*;
use moon_core::station_api::mcp::{AdminRequest, Scope};
use moon_ui::{
    MoonAlert, MoonButton, MoonCheckbox, MoonGroupBox, MoonInput, MoonInputEvent, MoonInputState,
    v_flex,
};
use rust_i18n::t;

use super::{SettingsView, Tab, common::group_title, frame::PageParts};
use crate::{
    backend::station::{job::Job, known_target},
    design,
};

/// Consent is deliberately explicit: no core or write permission is preselected.
pub(super) struct McpEd {
    code: Entity<MoonInputState>,
    cores: BTreeSet<u64>,
    scopes: BTreeSet<Scope>,
}

impl McpEd {
    /// Keep input changes reactive without performing network work during rendering.
    pub(super) fn new(window: &mut Window, cx: &mut Context<SettingsView>) -> Self {
        let code = cx.new(|cx| MoonInputState::new(window, cx));
        cx.subscribe(&code, |_, _, event: &MoonInputEvent, cx| {
            if matches!(event, MoonInputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        Self {
            code,
            cores: BTreeSet::new(),
            scopes: BTreeSet::from([Scope::Read]),
        }
    }
}

/// Describe the actual server-enforced permission rather than an AI client's promises.
fn permission(scope: Scope) -> String {
    match scope {
        Scope::Read => t!("station_mcp.scope_read"),
        Scope::Strategies => t!("station_mcp.scope_strategies"),
        Scope::Trading => t!("station_mcp.scope_trading"),
        Scope::Settings => t!("station_mcp.scope_settings"),
    }
    .to_string()
}

impl SettingsView {
    /// Use the authenticated administrative channel; public clients cannot invoke these actions.
    pub(super) fn mcp_admin(&mut self, request: AdminRequest, cx: &mut Context<Self>) {
        if let Some(target) = known_target() {
            self.backend.update(cx, |backend, cx| {
                backend.station_start(Job::Mcp { target, request }, false, cx)
            });
        }
    }

    /// Display cached authorization state; buttons, never layout, initiate remote operations.
    pub(super) fn st_mcp_page(&self, width: f32, cx: &Context<Self>) -> PageParts {
        let station = &self.backend.read(cx).station;
        let busy = station.busy();
        let refresh = MoonButton::new("mcp-refresh")
            .label(t!("station_mcp.refresh").to_string())
            .disabled(busy || !self.station_known())
            .on_click(cx.listener(|this, _, _, cx| this.mcp_admin(AdminRequest::Status, cx)))
            .into_any_element();
        let mut body = v_flex()
            .w(px((width - design::ui_value(cx, 32.0)).max(0.0)))
            .gap(design::ui_px(cx, 12.0))
            .text_size(design::t_body(cx));
        if !self.station_known() {
            body = body.child(MoonAlert::info(
                "mcp-no-station",
                t!("station_mcp.no_station").to_string(),
            ));
        } else {
            let mut connection = MoonGroupBox::new("mcp-connection")
                .padding(14.0)
                .gap(10.0)
                .child(group_title(t!("station_mcp.connection").to_string(), cx));
            if let Some(state) = &station.mcp {
                if let Some(endpoint) = &state.endpoint {
                    let endpoint = endpoint.clone();
                    connection = connection
                        .child(div().child(endpoint.clone()))
                        .child(
                            MoonButton::new("mcp-copy")
                                .label(t!("station_mcp.copy").to_string())
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        endpoint.clone(),
                                    ))
                                }),
                        )
                        .child(
                            MoonCheckbox::new("mcp-enabled")
                                .checked(state.enabled)
                                .disabled(busy)
                                .label(t!("station_mcp.enabled").to_string())
                                .on_change(cx.listener(|this, enabled: &bool, _, cx| {
                                    this.mcp_admin(
                                        AdminRequest::SetEnabled { enabled: *enabled },
                                        cx,
                                    );
                                })),
                        )
                        .child(div().child(t!("station_mcp.disable_hint").to_string()));
                } else {
                    connection =
                        connection.child(div().child(t!("station_mcp.unconfigured").to_string()));
                }
            } else if busy {
                connection = connection.child(div().child(t!("station_mcp.loading").to_string()));
            }
            connection = connection
                .child(
                    MoonButton::new("mcp-setup")
                        .label(t!("station_mcp.setup").to_string())
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(target) = known_target() {
                                this.backend.update(cx, |backend, cx| {
                                    backend.station_start(Job::McpSetup { target }, false, cx)
                                });
                            }
                        })),
                )
                .child(div().child(t!("station_mcp.setup_hint").to_string()));
            body = body.child(connection);
        }
        if let Some(state) = station.mcp.as_ref().filter(|_| self.station_known()) {
            if state.enabled {
                let mut consent = MoonGroupBox::new("mcp-consent")
                    .padding(14.0)
                    .gap(10.0)
                    .child(group_title(t!("station_mcp.consent").to_string(), cx))
                    .child(div().child(t!("station_mcp.consent_hint").to_string()));
                if state.pending.is_empty() {
                    consent = consent.child(div().child(t!("station_mcp.no_pending").to_string()));
                }
                for pending in &state.pending {
                    let request_id = pending.request_id.clone();
                    consent = consent
                        .child(div().child(pending.client_name.clone()))
                        .child(div().child(pending.redirect_uri.clone()))
                        .child(
                            div().child(
                                pending
                                    .scopes
                                    .iter()
                                    .copied()
                                    .map(permission)
                                    .collect::<Vec<_>>()
                                    .join("; "),
                            ),
                        )
                        .child(
                            MoonButton::new(SharedString::from(format!("mcp-deny-{request_id}")))
                                .label(t!("station_mcp.deny").to_string())
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.mcp_admin(
                                        AdminRequest::Deny {
                                            request_id: request_id.clone(),
                                        },
                                        cx,
                                    );
                                })),
                        );
                }
                consent = consent
                    .child(
                        MoonInput::new("mcp-code")
                            .state(&self.mcp.code)
                            .placeholder(t!("station_mcp.code").to_string())
                            .size(design::INPUT_SIZE),
                    )
                    .child(div().child(t!("station_mcp.cores").to_string()));
                if state.cores.is_empty() {
                    consent = consent.child(div().child(t!("station_mcp.no_cores").to_string()));
                }
                for core in &state.cores {
                    let uid = core.uid;
                    consent = consent.child(
                        MoonCheckbox::new(SharedString::from(format!("mcp-core-{uid}")))
                            .label(format!("#{uid} {}", core.name))
                            .checked(self.mcp.cores.contains(&uid))
                            .on_change(cx.listener(move |this, checked: &bool, _, cx| {
                                if *checked {
                                    this.mcp.cores.insert(uid);
                                } else {
                                    this.mcp.cores.remove(&uid);
                                }
                                cx.notify();
                            })),
                    );
                }
                for scope in [
                    Scope::Read,
                    Scope::Strategies,
                    Scope::Trading,
                    Scope::Settings,
                ] {
                    consent = consent.child(
                        MoonCheckbox::new(SharedString::from(format!(
                            "mcp-scope-{}",
                            scope.as_str()
                        )))
                        .label(permission(scope))
                        .checked(self.mcp.scopes.contains(&scope))
                        .disabled(scope == Scope::Read)
                        .on_change(cx.listener(
                            move |this, checked: &bool, _, cx| {
                                if *checked {
                                    this.mcp.scopes.insert(scope);
                                } else {
                                    this.mcp.scopes.remove(&scope);
                                }
                                cx.notify();
                            },
                        )),
                    );
                }
                consent = consent.child(
                    MoonButton::new("mcp-approve")
                        .label(t!("station_mcp.approve").to_string())
                        .disabled(
                            busy || self.mcp.cores.is_empty()
                                || self.mcp.code.read(cx).value().trim().is_empty(),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.mcp_admin(
                                AdminRequest::Approve {
                                    code: this.mcp.code.read(cx).value().to_string(),
                                    cores: this.mcp.cores.iter().copied().collect(),
                                    scopes: this.mcp.scopes.iter().copied().collect(),
                                },
                                cx,
                            );
                        })),
                );
                body = body.child(consent);
            }
            let mut clients = MoonGroupBox::new("mcp-clients")
                .padding(14.0)
                .gap(10.0)
                .child(group_title(t!("station_mcp.clients").to_string(), cx));
            if state.grants.is_empty() {
                clients = clients.child(div().child(t!("station_mcp.no_clients").to_string()));
            }
            for grant in &state.grants {
                let grant_id = grant.id.clone();
                let cores = grant
                    .cores
                    .iter()
                    .map(|id| {
                        state
                            .cores
                            .iter()
                            .find(|core| core.uid == *id)
                            .map_or_else(|| format!("#{id}"), |core| format!("#{id} {}", core.name))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                clients = clients
                    .child(div().child(grant.client_name.clone()))
                    .child(
                        div().child(
                            t!(
                                "station_mcp.last_access",
                                time = grant
                                    .last_access_ms
                                    .and_then(chrono::DateTime::from_timestamp_millis)
                                    .map_or_else(
                                        || t!("station_mcp.never").to_string(),
                                        |time| time.format("%Y-%m-%d %H:%M:%S UTC").to_string()
                                    )
                            )
                            .to_string(),
                        ),
                    )
                    .child(div().child(cores))
                    .child(
                        div().child(
                            grant
                                .scopes
                                .iter()
                                .copied()
                                .map(permission)
                                .collect::<Vec<_>>()
                                .join("; "),
                        ),
                    )
                    .child(
                        MoonButton::new(SharedString::from(format!("mcp-revoke-{grant_id}")))
                            .label(t!("station_mcp.revoke").to_string())
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.mcp_admin(
                                    AdminRequest::Revoke {
                                        grant_id: grant_id.clone(),
                                    },
                                    cx,
                                );
                            })),
                    );
            }
            body = body.child(clients);
            let mut history = MoonGroupBox::new("mcp-audit")
                .padding(14.0)
                .gap(10.0)
                .child(group_title(t!("station_mcp.audit").to_string(), cx));
            if state.audit.is_empty() {
                history = history.child(div().child(t!("station_mcp.no_audit").to_string()));
            }
            for entry in state.audit.iter().take(30) {
                let time = chrono::DateTime::from_timestamp_millis(entry.at_ms)
                    .map_or_else(String::new, |time| {
                        time.format("%Y-%m-%d %H:%M:%S UTC").to_string()
                    });
                history = history
                    .child(div().child(format!("{time} · {} · {}", entry.action, entry.outcome)));
            }
            body = body.child(history);
        }
        if let Some(Err(error)) = &station.outcome {
            body = body.child(MoonAlert::error("mcp-error", error.clone()));
        }
        self.page_parts(Tab::StMcp, body.into_any_element(), vec![refresh], cx)
    }
}
