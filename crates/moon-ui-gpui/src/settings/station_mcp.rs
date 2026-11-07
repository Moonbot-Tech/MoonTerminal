//! Owner consent for AI clients, administered through the Station's existing private connection.

use std::collections::{BTreeMap, BTreeSet};

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use moon_core::station_api::mcp::{
    AdminRequest, AdminState, CoreAccess, CoreInfo, GrantInfo, Scope,
};
use moon_ui::{
    MoonAlert, MoonButton, MoonCheckbox, MoonGroupBox, MoonInput, MoonInputEvent, MoonInputState,
    MoonPalette, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::{
    SettingsView, Tab,
    common::{floored_checkbox, group_title, prose},
    frame::PageParts,
};
use crate::{
    backend::station::{job::Job, known_target},
    design,
};

mod journal;

/// Width the consent column starts from, unscaled and font-scaled: one core-table row on one line
/// (the name, three permission columns and a short access cell).
const MCP_CONSENT_W: f32 = 650.0;
/// Width the clients-and-journal column starts from, unscaled and font-scaled. The two bases add up
/// to the narrowest body that keeps the columns side by side; kept below the 640 + 400 that fitted
/// beside each other in a real enlarged-font window, because past it the journal drops under the
/// long table. A narrower body stacks them instead of squeezing them.
const MCP_HISTORY_W: f32 = 360.0;
/// Width of the core-table's name column, unscaled and font-scaled.
const MCP_CORE_COL_W: f32 = 190.0;
/// Width of each write-permission column of the core table, unscaled and font-scaled: room for the
/// heading's checkbox and label.
const MCP_SCOPE_COL_W: f32 = 112.0;
/// The permissions with a column of their own. Read has none: selecting a core grants it, and the
/// Station requires it on every selected core.
const WRITE_SCOPES: [Scope; 3] = [Scope::Strategies, Scope::Trading, Scope::Settings];

/// The form for a connected client's permissions.
struct AccessEdit {
    grant_id: String,
    access: BTreeMap<u64, BTreeSet<Scope>>,
    /// The Station's permissions this form's ticks were built on.
    base: Vec<CoreAccess>,
    /// What this form last sent. Once the Station reports it, the form has done its job.
    sent: Option<Vec<CoreAccess>>,
}

/// Whether the form's ticks still build on what the Station holds. Anything else was changed
/// elsewhere, and applying these ticks would undo that change, down to returning a permission just
/// taken away. Comparing only with the latest status would not catch it: a refresh makes that status
/// current.
fn edit_in_sync(current: &[CoreAccess], edit: &AccessEdit) -> bool {
    current == edit.base.as_slice()
}

/// Whether the Station now reports what the form sent, so the form has done its job. It closes then,
/// not when Apply is clicked: a refused change keeps its ticks and the owner sees why beside them.
fn edit_applied(edit: &AccessEdit, grants: &[GrantInfo]) -> bool {
    grants
        .iter()
        .find(|grant| grant.id == edit.grant_id)
        .is_some_and(|grant| edit.sent.as_deref() == Some(grant.access.as_slice()))
}

/// The core table's two forms. Consent is deliberately explicit: no core or write permission is
/// preselected for a new connection. So the two never share a selection: a client's loaded rights
/// cannot reach a new connection, even when that client disappears while its form is open.
pub(super) struct McpEd {
    code: Entity<MoonInputState>,
    /// A new connection's selected cores and what each is being granted; a selected core always
    /// holds read.
    access: BTreeMap<u64, BTreeSet<Scope>>,
    /// The client being edited, until its change is applied or cancelled. A client gone since simply
    /// stops being found, and the table shows the new connection.
    edit: Option<AccessEdit>,
}

impl McpEd {
    /// Close the form once its change is applied, on the status that says so. Closing for good
    /// rather than hiding it: a later change elsewhere must not bring the old ticks back.
    ///
    /// Returns:
    ///     Whether the form closed, so the page redraws.
    pub(super) fn settle(&mut self, state: Option<&AdminState>) -> bool {
        let applied = self
            .edit
            .as_ref()
            .zip(state)
            .is_some_and(|(edit, state)| edit_applied(edit, &state.grants));
        if applied {
            self.edit = None;
        }
        applied
    }

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
            access: BTreeMap::new(),
            edit: None,
        }
    }

    /// The selection the table draws: the client's being edited, or the new connection's.
    fn form(&self, editing: bool) -> &BTreeMap<u64, BTreeSet<Scope>> {
        match (&self.edit, editing) {
            (Some(edit), true) => &edit.access,
            _ => &self.access,
        }
    }

    /// The selection a tick on the table changes, the same one [`Self::form`] drew.
    fn form_mut(&mut self, editing: bool) -> &mut BTreeMap<u64, BTreeSet<Scope>> {
        match (&mut self.edit, editing) {
            (Some(edit), true) => &mut edit.access,
            _ => &mut self.access,
        }
    }
}

/// A connected client's permissions as the table's selection.
fn access_of(grant: &GrantInfo) -> BTreeMap<u64, BTreeSet<Scope>> {
    grant
        .access
        .iter()
        .map(|entry| (entry.core, entry.scopes.iter().copied().collect()))
        .collect()
}

/// Why a new connection cannot be approved yet, or `None` when it can.
fn approval_block(busy: bool, code_empty: bool, no_cores: bool) -> Option<String> {
    if busy {
        Some(t!("station_mcp.hint_busy").to_string())
    } else if code_empty {
        Some(t!("station_mcp.hint_need_code").to_string())
    } else if no_cores {
        Some(t!("station_mcp.hint_need_cores").to_string())
    } else {
        None
    }
}

/// Why a client's edited permissions cannot be applied yet, or `None` when they can. All cores
/// unticked is not "no access": revoking is its own button, and the Station refuses an empty grant.
fn edit_block(
    busy: bool,
    in_sync: bool,
    chosen: &[CoreAccess],
    grant: &GrantInfo,
) -> Option<String> {
    if busy {
        Some(t!("station_mcp.hint_busy").to_string())
    } else if !in_sync {
        Some(t!("station_mcp.hint_stale").to_string())
    } else if chosen.is_empty() {
        Some(t!("station_mcp.hint_need_cores").to_string())
    } else if chosen == grant.access.as_slice() {
        Some(t!("station_mcp.hint_no_changes").to_string())
    } else {
        None
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

/// One word for a permission, where a table cell has no room for the full [`permission`] sentence.
fn scope_short(scope: Scope) -> String {
    match scope {
        Scope::Read => t!("station_mcp.scope_short_read"),
        Scope::Strategies => t!("station_mcp.scope_short_strategies"),
        Scope::Trading => t!("station_mcp.scope_short_trading"),
        Scope::Settings => t!("station_mcp.scope_short_settings"),
    }
    .to_string()
}

/// A permission's column heading in the core table.
fn scope_column(scope: Scope) -> String {
    match scope {
        Scope::Strategies => t!("station_mcp.col_strategies"),
        Scope::Trading => t!("station_mcp.col_trading"),
        Scope::Settings => t!("station_mcp.col_settings"),
        Scope::Read => t!("station_mcp.scope_short_read"),
    }
    .to_string()
}

/// Tick or untick one permission of one core. Read is required: a write permission selects its
/// core, and unselecting a core drops everything it was being granted.
fn set_scope(access: &mut BTreeMap<u64, BTreeSet<Scope>>, uid: u64, scope: Scope, on: bool) {
    match (scope, on) {
        (Scope::Read, false) => {
            access.remove(&uid);
        }
        (_, true) => {
            let scopes = access.entry(uid).or_default();
            scopes.insert(Scope::Read);
            scopes.insert(scope);
        }
        (_, false) => {
            if let Some(scopes) = access.get_mut(&uid) {
                scopes.remove(&scope);
            }
        }
    }
}

/// "Select all" selects every core for reading and keeps what is already ticked. It never adds a
/// write permission: those have a heading checkbox of their own, named for what it grants.
fn select_all(access: &mut BTreeMap<u64, BTreeSet<Scope>>, cores: &[CoreInfo]) {
    for core in cores {
        set_scope(access, core.uid, Scope::Read, true);
    }
}

/// Back to reading only, once a consent has been sent: a write permission left ticked would
/// silently go to the next client connected from this page.
fn drop_write_scopes(access: &mut BTreeMap<u64, BTreeSet<Scope>>) {
    for scopes in access.values_mut() {
        scopes.retain(|scope| *scope == Scope::Read);
    }
}

/// The consent for the cores the Station still lists. A core removed since has no row to unselect
/// it, and the Station rejects a consent that names it, so it must never reach the request. Sorted
/// by core whatever order the Station lists them in, the order a stored grant keeps.
fn listed_access(cores: &[CoreInfo], access: &BTreeMap<u64, BTreeSet<Scope>>) -> Vec<CoreAccess> {
    let mut listed: Vec<CoreAccess> = cores
        .iter()
        .filter_map(|core| {
            let scopes = access.get(&core.uid)?;
            Some(CoreAccess {
                core: core.uid,
                scopes: scopes.iter().copied().collect(),
            })
        })
        .collect();
    listed.sort_by_key(|entry| entry.core);
    listed
}

/// The select-all checkbox as `(checked, indeterminate)`, over [`listed_access`]: a selected core
/// removed since would otherwise make a partial choice look complete.
fn select_all_state(cores: &[CoreInfo], access: &BTreeMap<u64, BTreeSet<Scope>>) -> (bool, bool) {
    let chosen = listed_access(cores, access).len();
    let all = !cores.is_empty() && chosen == cores.len();
    (all, chosen > 0 && !all)
}

/// A permission column's heading checkbox as `(checked, indeterminate)`, over the listed cores.
fn column_state(
    cores: &[CoreInfo],
    access: &BTreeMap<u64, BTreeSet<Scope>>,
    scope: Scope,
) -> (bool, bool) {
    let holding = cores
        .iter()
        .filter(|core| {
            access
                .get(&core.uid)
                .is_some_and(|scopes| scopes.contains(&scope))
        })
        .count();
    let all = !cores.is_empty() && holding == cores.len();
    (all, holding > 0 && !all)
}

/// Tick or untick a permission on every listed core. Ticking selects each core, like a tick in its
/// row; unticking leaves the cores selected for reading.
fn set_column(
    access: &mut BTreeMap<u64, BTreeSet<Scope>>,
    cores: &[CoreInfo],
    scope: Scope,
    on: bool,
) {
    for core in cores {
        set_scope(access, core.uid, scope, on);
    }
}

/// A grant's reach as lines, one per distinct set of permissions: `read, trading: #3 BinF1`. A grant
/// over many cores usually holds one or two such sets, so this stays readable where one line per
/// core would not.
fn grant_lines(
    grant: &GrantInfo,
    cores: &[CoreInfo],
    scope_name: impl Fn(Scope) -> String,
) -> Vec<String> {
    let mut groups: BTreeMap<&[Scope], Vec<String>> = BTreeMap::new();
    for entry in &grant.access {
        let name = cores
            .iter()
            .find(|core| core.uid == entry.core)
            .map_or_else(
                || format!("#{}", entry.core),
                |core| format!("#{} {}", core.uid, core.name),
            );
        groups.entry(&entry.scopes).or_default().push(name);
    }
    groups
        .into_iter()
        .map(|(scopes, names)| {
            let scopes: Vec<String> = scopes.iter().copied().map(&scope_name).collect();
            format!("{}: {}", scopes.join(", "), names.join(", "))
        })
        .collect()
}

/// Who may reach this core and with what, or `None` when no client may.
///
/// Args:
///     grants: The Station's current grants.
///     uid: The core the table row shows.
///     scope_name: How a permission reads in the cell.
///
/// Returns:
///     One `client: permissions` entry per grant covering the core.
fn access_text(
    grants: &[GrantInfo],
    uid: u64,
    scope_name: impl Fn(Scope) -> String,
) -> Option<String> {
    let entries: Vec<String> = grants
        .iter()
        .filter_map(|grant| {
            let scopes: Vec<String> = grant
                .scopes_on(uid)?
                .iter()
                .copied()
                .map(&scope_name)
                .collect();
            Some(format!("{}: {}", grant.client_name, scopes.join(", ")))
        })
        .collect();
    (!entries.is_empty()).then(|| entries.join(" · "))
}

/// One column of the consent/history split, starting from `basis`. Each shares the free width and
/// wraps under the other once two no longer fit.
fn mcp_column(basis: f32, cx: &App) -> Div {
    v_flex()
        .flex_grow(1.0)
        .flex_shrink_1()
        .flex_basis(design::font_w_px(cx, basis))
        .min_w_0()
        .max_w_full()
        .gap(design::ui_px(cx, 12.0))
}

/// The consent column beside the clients and the journal. The core table is long: who has access
/// and what they did stay beside it, not under it, until the body is too narrow for both.
fn mcp_columns(consent: Option<Div>, history: Div, cx: &App) -> Div {
    h_flex()
        .w_full()
        .min_w_0()
        .flex_wrap()
        .items_start()
        .gap(design::ui_px(cx, 12.0))
        .children(consent)
        .child(history)
}

/// A line of the core table: the name, the write permissions in fixed columns, then the current
/// access in the rest. A row too narrow for a word of that last cell wraps it under the others.
fn mcp_core_row(
    first: AnyElement,
    scopes: [AnyElement; WRITE_SCOPES.len()],
    second: AnyElement,
    cx: &App,
) -> Div {
    let p = MoonPalette::active(cx);
    let scope_cells = h_flex()
        .flex_shrink_0()
        .items_center()
        .gap_x(design::ui_px(cx, 4.0))
        .children(
            scopes
                .into_iter()
                .map(|cell| div().w(design::font_w_px(cx, MCP_SCOPE_COL_W)).child(cell)),
        );
    h_flex()
        .w_full()
        .min_w_0()
        .flex_wrap()
        .items_center()
        .gap_x(design::ui_px(cx, 12.0))
        .gap_y(design::ui_px(cx, 2.0))
        .py(design::ui_px(cx, 4.0))
        .border_b_1()
        .border_color(rgba_from(p.border, 1.0))
        .child(
            div()
                .flex_shrink_0()
                .w(design::font_w_px(cx, MCP_CORE_COL_W))
                .max_w_full()
                .child(first),
        )
        .child(scope_cells)
        .child(second)
}

impl SettingsView {
    /// The cores to choose from, each with its own write permissions and beside the access it
    /// already has. The checkboxes describe the client in `editing`, or else the connection being
    /// approved; the right column the current grants. An edited client's permissions cannot go
    /// past what it requested, so what the Station would refuse is not offered.
    fn mcp_core_table(
        &self,
        state: &AdminState,
        editing: Option<&GrantInfo>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        let muted = rgba_from(p.text_muted, 1.0);
        let is_editing = editing.is_some();
        let form = self.mcp.form(is_editing);
        let ceiling = editing.and_then(|grant| grant.requested.as_ref());
        let (all, partial) = select_all_state(&state.cores, form);
        let listed = state.cores.clone();
        let select_all_label = t!("station_mcp.select_all").to_string();
        // Toggles from the drawn state, not the callback's value: an indeterminate MoonCheckbox
        // draws as checked, so a click on a partial choice reports `false`, and clearing the
        // choice there would contradict the "Select all" label.
        let select_all = MoonCheckbox::new("mcp-core-all")
            .checked(all)
            .indeterminate(partial)
            .label(select_all_label.clone())
            .on_change(cx.listener(move |this, _: &bool, _, cx| {
                let form = this.mcp.form_mut(is_editing);
                if all {
                    form.clear();
                } else {
                    select_all(form, &listed);
                }
                cx.notify();
            }));
        let head = mcp_core_row(
            floored_checkbox("mcp-core-all", &select_all_label, select_all, cx),
            WRITE_SCOPES.map(|scope| {
                let id = format!("mcp-core-head-{}", scope.as_str());
                let label = scope_column(scope);
                let (all, partial) = column_state(&state.cores, form, scope);
                let listed = state.cores.clone();
                // Toggles from the drawn state, for the reason given at "Select all" above.
                let checkbox = MoonCheckbox::new(SharedString::from(id.clone()))
                    .checked(all)
                    .indeterminate(partial)
                    .disabled(ceiling.is_some_and(|requested| !requested.contains(&scope)))
                    .label(label.clone())
                    .on_change(cx.listener(move |this, _: &bool, _, cx| {
                        set_column(this.mcp.form_mut(is_editing), &listed, scope, !all);
                        cx.notify();
                    }));
                floored_checkbox(&id, &label, checkbox, cx)
            }),
            prose(
                "mcp-core-access-head".to_string(),
                t!("station_mcp.col_access").to_string(),
                true,
                cx,
            )
            .flex_1()
            .text_size(design::t_caption(cx))
            .text_color(muted)
            .into_any_element(),
            cx,
        );
        let rows = state.cores.iter().map(|core| {
            let uid = core.uid;
            let id = format!("mcp-core-{uid}");
            let label = format!("#{uid} {}", core.name);
            let access = access_text(&state.grants, uid, scope_short);
            let granted = access.is_some();
            let chosen = form.get(&uid);
            let checkbox = MoonCheckbox::new(SharedString::from(id.clone()))
                .label(label.clone())
                .checked(chosen.is_some())
                .on_change(cx.listener(move |this, checked: &bool, _, cx| {
                    set_scope(this.mcp.form_mut(is_editing), uid, Scope::Read, *checked);
                    cx.notify();
                }));
            let text = access.unwrap_or_else(|| t!("station_mcp.no_access").to_string());
            mcp_core_row(
                floored_checkbox(&id, &label, checkbox, cx),
                WRITE_SCOPES.map(|scope| {
                    MoonCheckbox::new(SharedString::from(format!("{id}-{}", scope.as_str())))
                        .checked(chosen.is_some_and(|scopes| scopes.contains(&scope)))
                        .disabled(ceiling.is_some_and(|requested| !requested.contains(&scope)))
                        .on_change(cx.listener(move |this, checked: &bool, _, cx| {
                            set_scope(this.mcp.form_mut(is_editing), uid, scope, *checked);
                            cx.notify();
                        }))
                        .into_any_element()
                }),
                prose(format!("{id}-access"), text, false, cx)
                    .flex_1()
                    .when(!granted, |cell| cell.text_color(muted))
                    .into_any_element(),
                cx,
            )
        });
        v_flex()
            .w_full()
            .child(head)
            .children(rows)
            .into_any_element()
    }

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
                        .child(div().child(t!("station_mcp.paste_where").to_string()))
                        .child(div().child(t!("station_mcp.path_claude").to_string()))
                        .child(div().child(t!("station_mcp.path_chatgpt").to_string()))
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
        // Above the long table, where a refused change is seen beside the form that caused it.
        if let Some(Err(error)) = &station.outcome {
            body = body.child(MoonAlert::error("mcp-error", error.clone()));
        }
        if let Some(state) = station.mcp.as_ref().filter(|_| self.station_known()) {
            let mut consent_column = None;
            let muted = rgba_from(MoonPalette::active(cx).text_muted, 1.0);
            let editing = self
                .mcp
                .edit
                .as_ref()
                .filter(|_| state.enabled)
                .and_then(|edit| state.grants.iter().find(|grant| grant.id == edit.grant_id));
            if state.enabled {
                let title = match editing {
                    Some(grant) => t!(
                        "station_mcp.edit_title",
                        client = grant.client_name.as_str()
                    )
                    .to_string(),
                    None => t!("station_mcp.consent").to_string(),
                };
                let mut consent = MoonGroupBox::new("mcp-consent")
                    .padding(14.0)
                    .gap(10.0)
                    .child(group_title(title, cx));
                if editing.is_some() {
                    consent = consent.child(div().child(t!("station_mcp.edit_hint").to_string()));
                } else {
                    consent =
                        consent.child(div().child(t!("station_mcp.consent_hint").to_string()));
                    if state.pending.is_empty() {
                        consent =
                            consent.child(div().child(t!("station_mcp.no_pending").to_string()));
                    }
                    for pending in &state.pending {
                        let request_id = pending.request_id.clone();
                        consent =
                            consent
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
                                    MoonButton::new(SharedString::from(format!(
                                        "mcp-deny-{request_id}"
                                    )))
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
                    consent = consent.child(
                        MoonInput::new("mcp-code")
                            .state(&self.mcp.code)
                            .placeholder(t!("station_mcp.code").to_string())
                            .size(design::INPUT_SIZE),
                    );
                }
                consent = consent.child(div().child(t!("station_mcp.cores").to_string()));
                if state.cores.is_empty() {
                    consent = consent.child(div().child(t!("station_mcp.no_cores").to_string()));
                } else {
                    consent = consent
                        .child(self.mcp_core_table(state, editing, cx))
                        .child(div().child(t!("station_mcp.access_hint").to_string()));
                    for scope in WRITE_SCOPES {
                        consent = consent.child(div().text_color(muted).child(format!(
                            "{}: {}",
                            scope_column(scope),
                            permission(scope)
                        )));
                    }
                }
                if editing.is_none() {
                    let chosen = listed_access(&state.cores, self.mcp.form(false));
                    let blocked = approval_block(
                        busy,
                        self.mcp.code.read(cx).value().trim().is_empty(),
                        chosen.is_empty(),
                    );
                    let disabled = blocked.is_some();
                    consent = consent
                        .children(blocked.map(|hint| div().text_color(muted).child(hint)))
                        .child(
                            MoonButton::new("mcp-approve")
                                .label(t!("station_mcp.approve").to_string())
                                .disabled(disabled)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.mcp_admin(
                                        AdminRequest::Approve {
                                            code: this.mcp.code.read(cx).value().to_string(),
                                            access: chosen.clone(),
                                        },
                                        cx,
                                    );
                                    this.mcp
                                        .code
                                        .update(cx, |state, cx| state.set_value("", window, cx));
                                    drop_write_scopes(&mut this.mcp.access);
                                    cx.notify();
                                })),
                        );
                }
                consent_column = Some(mcp_column(MCP_CONSENT_W, cx).child(consent));
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
                    .children(
                        grant_lines(grant, &state.cores, scope_short)
                            .into_iter()
                            .map(|line| div().child(line)),
                    );
                if let Some(edit) = self
                    .mcp
                    .edit
                    .as_ref()
                    .filter(|_| editing.is_some_and(|open| open.id == grant.id))
                {
                    // In place of "Change permissions": the two ways out of the form.
                    let chosen = listed_access(&state.cores, &edit.access);
                    let blocked =
                        edit_block(busy, edit_in_sync(&grant.access, edit), &chosen, grant);
                    let disabled = blocked.is_some();
                    let id = grant_id.clone();
                    let expected = edit.base.clone();
                    clients = clients
                        .children(blocked.map(|hint| div().text_color(muted).child(hint)))
                        .child(
                            h_flex()
                                .w_full()
                                .min_w_0()
                                .gap(design::ui_px(cx, 8.0))
                                .child(
                                    v_flex().flex_1().min_w_0().child(
                                        MoonButton::new("mcp-apply-access")
                                            .label(t!("station_mcp.apply_access").to_string())
                                            .disabled(disabled)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.mcp_admin(
                                                    AdminRequest::SetAccess {
                                                        grant_id: id.clone(),
                                                        expected: expected.clone(),
                                                        access: chosen.clone(),
                                                    },
                                                    cx,
                                                );
                                                if let Some(edit) = &mut this.mcp.edit {
                                                    edit.sent = Some(chosen.clone());
                                                }
                                            })),
                                    ),
                                )
                                .child(
                                    v_flex().flex_1().min_w_0().child(
                                        MoonButton::new("mcp-cancel-edit")
                                            .label(t!("station_mcp.cancel_edit").to_string())
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.mcp.edit = None;
                                                cx.notify();
                                            })),
                                    ),
                                ),
                        );
                } else if state.enabled {
                    let loaded = access_of(grant);
                    let base = grant.access.clone();
                    let id = grant_id.clone();
                    clients = clients.child(
                        MoonButton::new(SharedString::from(format!("mcp-edit-{grant_id}")))
                            .label(t!("station_mcp.edit_access").to_string())
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.mcp.edit = Some(AccessEdit {
                                    grant_id: id.clone(),
                                    access: loaded.clone(),
                                    base: base.clone(),
                                    sent: None,
                                });
                                cx.notify();
                            })),
                    );
                }
                clients = clients.child(
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
            let mut history = MoonGroupBox::new("mcp-audit")
                .padding(14.0)
                .gap(10.0)
                .child(group_title(t!("station_mcp.audit").to_string(), cx));
            if state.audit.is_empty() {
                history = history.child(div().child(t!("station_mcp.no_audit").to_string()));
            }
            for entry in &state.audit {
                history = history.child(journal::entry_el(entry, &state.cores, cx));
            }
            body = body.child(mcp_columns(
                consent_column,
                mcp_column(MCP_HISTORY_W, cx).child(clients).child(history),
                cx,
            ));
        }
        self.page_parts(Tab::StMcp, body.into_any_element(), vec![refresh], cx)
    }
}

#[cfg(test)]
mod tests;
