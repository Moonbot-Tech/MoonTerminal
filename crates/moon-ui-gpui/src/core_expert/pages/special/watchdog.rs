//! The "Защита от зависаний" section of the Special page, drawn from the core's observer snapshot.
//!
//! Row for row Moonbot's group: the worker-bot line (mark this bot's orders, the observer address),
//! the follower-bot line (report, auto-sell, the exclusion list), the observer table with its add,
//! remove and Cancel buys, the UDP listener's state and the group's own Apply. What is new beside
//! Moonbot's is the editor under the table — the row picked in the table, field by field — because a
//! data table here is a list, not a grid one types into.
//!
//! Every control is disabled, with the reason above the block, while there is no CURRENT snapshot:
//! an older core that never sends one, a link that dropped, a replaced process. The table and its
//! row editor are read-only without the core's licence for observer rows, while the settings above
//! it stay editable — the core keeps the sender independent of that licence. The window's side of
//! the block (its edit, its requests) lives in `core_expert::watchdog`.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonAlert, MoonButton, MoonButtonVariant, MoonCheckbox, MoonDataCell, MoonDataRow,
    MoonDataTable, MoonDataTableColumn, MoonInput, MoonPalette, h_flex, v_flex,
};
use rust_i18n::t;

use moon_core::feed::{
    CoreWatchdogEntry, CoreWatchdogState, ServiceTrack, WATCHDOG_MIN_CREDENTIAL_CHARS,
    WatchdogEditError, WatchdogEntryEdit, WatchdogSettingsEdit, WatchdogStatus,
};

use crate::design;

use super::super::super::CoreExpertView;
use super::super::super::service::verdict;
use super::super::super::watchdog::WatchdogBlock;
use super::super::super::widgets::{caption, hint, text_block};
use super::service::verdict_alert;

/// Ids of the block's editors. The page owns its ids, as every page here does, so they carry its
/// prefix; the window builds the editors under them (`CoreExpertView::build_watchdog_inputs`).
pub(in crate::core_expert) mod ids {
    pub(in crate::core_expert) const IP: &str = "exp-sp-wd-ip";
    pub(in crate::core_expert) const BLACKLIST: &str = "exp-sp-wd-blacklist";
    pub(in crate::core_expert) const NAME: &str = "exp-sp-wd-name";
    pub(in crate::core_expert) const KEY: &str = "exp-sp-wd-key";
    pub(in crate::core_expert) const SECRET: &str = "exp-sp-wd-secret";
    pub(in crate::core_expert) const PRICE: &str = "exp-sp-wd-price";
    pub(in crate::core_expert) const TIME: &str = "exp-sp-wd-time";
    /// Every editor the block builds.
    pub(in crate::core_expert) const ALL: [&str; 7] =
        [IP, BLACKLIST, NAME, KEY, SECRET, PRICE, TIME];
}

/// Rows the table shows before it scrolls. A core rarely carries more observers than this, and a
/// fixed height is what a virtual list needs inside a scrolling page.
const TABLE_ROWS: f32 = 6.0;

/// What the block needs from the window and the store, read before the page is built.
pub(in crate::core_expert) struct WatchdogCtx<'a> {
    pub(in crate::core_expert) block: &'a WatchdogBlock,
    /// The anchor's snapshot, current or stale.
    pub(in crate::core_expert) snapshot: Option<&'a CoreWatchdogState>,
    /// The snapshot is current and the link is up — the only time anything may be edited.
    pub(in crate::core_expert) live: bool,
    pub(in crate::core_expert) track: Option<&'a ServiceTrack>,
    /// The anchor's name, for the note shown while several cores are selected.
    pub(in crate::core_expert) core_name: Option<SharedString>,
    /// How many cores the list has selected.
    pub(in crate::core_expert) selected_cores: usize,
}

/// Build the section.
pub(in crate::core_expert) fn body(
    view: &Entity<CoreExpertView>,
    wd: &WatchdogCtx<'_>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let gap = design::ui_px(cx, 6.0);
    let block = wd.block;
    let snap = wd.snapshot;
    let edit = block.edit_view(snap);
    // While a request is in flight nothing is editable: an applied verdict rebuilds the edit from
    // the core's state, and anything typed meanwhile would vanish with it.
    let busy = block.awaiting.is_some();
    // The settings above the table: editable on any current snapshot.
    let settings_on = wd.live && snap.is_some() && !busy;
    // The rows: also the core's licence and its platform's support.
    let rows_on = settings_on && snap.is_some_and(|s| s.available && s.can_edit && s.supported);
    let selected = block
        .selected
        .filter(|ix| edit.as_ref().is_some_and(|e| *ix < e.entries.len()));
    let row = selected.and_then(|ix| edit.as_ref().map(|e| &e.entries[ix]));
    let saved = row.and_then(|r| snap.and_then(|s| s.entry(&r.id)));

    v_flex()
        .w_full()
        .gap(gap)
        .children(notices(wd, p, cx))
        .child(caption(
            t!("core_expert.sp_worker_bot").to_string(),
            settings_on,
            p,
            cx,
        ))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 10.0))
                .child(check(
                    "exp-sp-wd-sign-orders",
                    t!("core_expert.sp_watch_orders").to_string(),
                    edit.as_ref().is_some_and(|e| e.sign_orders),
                    settings_on,
                    view,
                    |e, on| e.sign_orders = on,
                ))
                .child(caption("Control VDS IP".to_string(), settings_on, p, cx))
                .child(input(block, ids::IP, 140.0, settings_on, None, cx)),
        )
        .child(caption(
            t!("core_expert.sp_follower_bot").to_string(),
            settings_on,
            p,
            cx,
        ))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 10.0))
                .child(check(
                    "exp-sp-wd-report",
                    t!("core_expert.sp_report_to_telegram").to_string(),
                    edit.as_ref().is_some_and(|e| e.report),
                    settings_on,
                    view,
                    |e, on| e.report = on,
                ))
                .child(check(
                    "exp-sp-wd-autosell",
                    t!("core_expert.sp_autosell").to_string(),
                    edit.as_ref().is_some_and(|e| e.auto_sell),
                    settings_on,
                    view,
                    |e, on| e.auto_sell = on,
                ))
                .child(caption(
                    t!("core_expert.sp_skip_balances").to_string(),
                    settings_on,
                    p,
                    cx,
                ))
                .child(div().flex_1().min_w_0().child(input(
                    block,
                    ids::BLACKLIST,
                    0.0,
                    settings_on,
                    None,
                    cx,
                ))),
        )
        .child(text_block(
            t!("core_expert.sp_open_udp_port").to_string(),
            p.text_soft,
            false,
            cx,
        ))
        .child(table(view, block, snap, edit.as_ref(), selected, p, cx))
        .children(row.map(|row| row_editor(view, block, row, saved, rows_on, p, cx)))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 8.0))
                .child(button(
                    "exp-sp-wd-add",
                    "+".to_string(),
                    rows_on && !busy,
                    view,
                    |this, cx| this.watchdog_add(cx),
                ))
                .child(button(
                    "exp-sp-wd-remove",
                    "−".to_string(),
                    rows_on && !busy && selected.is_some(),
                    view,
                    |this, cx| this.watchdog_remove(cx),
                ))
                .child(button(
                    "exp-sp-wd-cancel-buys",
                    "Cancel buys".to_string(),
                    settings_on && !busy && snap.is_some_and(|s| s.available),
                    view,
                    |this, cx| this.watchdog_cancel_buys(cx),
                ))
                .child(div().flex_1())
                .child(caption(udp_text(snap), snap.is_some(), p, cx))
                .child(button(
                    "exp-sp-wd-discard",
                    t!("core_expert.wd_discard").to_string(),
                    block.draft.is_some() && !busy,
                    view,
                    |this, cx| this.watchdog_discard(cx),
                ))
                .child(
                    MoonButton::new("exp-sp-wd-apply")
                        .label(t!("common.apply").to_string())
                        .variant(MoonButtonVariant::Blue)
                        .disabled(!(settings_on && block.draft.is_some() && !busy))
                        .padding_x(14.0)
                        .on_click({
                            let view = view.clone();
                            move |_, _window, app| {
                                view.update(app, |this, cx| this.watchdog_apply(cx));
                            }
                        })
                        .render(),
                ),
        )
        .into_any_element()
}

/// The lines above the block: why it is unavailable, what it addresses, how the last request went.
fn notices(wd: &WatchdogCtx<'_>, p: MoonPalette, cx: &App) -> Vec<AnyElement> {
    let mut out = Vec::new();
    match wd.snapshot {
        None => out.push(
            MoonAlert::warning("exp-sp-wd-absent", t!("core_expert.wd_absent").to_string())
                .into_any_element(),
        ),
        Some(_) if !wd.live => out.push(
            MoonAlert::warning("exp-sp-wd-stale", t!("core_expert.svc_stale").to_string())
                .into_any_element(),
        ),
        Some(s) => {
            let reason = if !s.available {
                Some(t!("core_expert.wd_no_controller"))
            } else if !s.supported {
                Some(t!("core_expert.wd_unsupported"))
            } else if !s.can_edit {
                Some(t!("core_expert.wd_no_licence"))
            } else {
                None
            };
            if let Some(reason) = reason {
                out.push(
                    MoonAlert::info("exp-sp-wd-readonly", reason.to_string()).into_any_element(),
                );
            }
        }
    }
    if wd.selected_cores > 1 {
        let name = wd.core_name.clone().unwrap_or_default();
        out.push(
            text_block(
                t!("core_expert.wd_anchor_only", core = name.as_ref()).to_string(),
                p.amber,
                false,
                cx,
            )
            .into_any_element(),
        );
    }
    if wd.block.queue_failed {
        out.push(
            MoonAlert::error(
                "exp-sp-wd-queue",
                t!("core_expert.svc_not_sent").to_string(),
            )
            .into_any_element(),
        );
    }
    if let Some(error) = &wd.block.invalid {
        out.push(MoonAlert::error("exp-sp-wd-invalid", invalid_text(error)).into_any_element());
    }
    if let Some(v) = wd
        .track
        .and_then(|track| verdict(wd.block.sent_here, wd.block.awaiting.is_some(), track))
    {
        out.push(verdict_alert("exp-sp-wd-verdict", &v));
    }
    out
}

/// A local refusal, naming the row as the table numbers it.
fn invalid_text(error: &WatchdogEditError) -> String {
    let n = |index: &usize| (index + 1).to_string();
    let min = WATCHDOG_MIN_CREDENTIAL_CHARS.to_string();
    match error {
        WatchdogEditError::ServerIp => t!("core_expert.wd_bad_ip").to_string(),
        WatchdogEditError::ApiKey { index } => {
            t!("core_expert.wd_bad_key", n = n(index), min = min).to_string()
        }
        WatchdogEditError::ApiSecret { index } => {
            t!("core_expert.wd_bad_secret", n = n(index), min = min).to_string()
        }
        WatchdogEditError::NewRowCredentials { index } => {
            t!("core_expert.wd_new_row_keys", n = n(index)).to_string()
        }
        WatchdogEditError::Price { index } => {
            t!("core_expert.wd_bad_price", n = n(index)).to_string()
        }
        WatchdogEditError::Time { index } => {
            t!("core_expert.wd_bad_time", n = n(index)).to_string()
        }
    }
}

/// The listener's state, as Moonbot's "UDP сервер:" line prints it.
fn udp_text(snap: Option<&CoreWatchdogState>) -> String {
    let v = match snap {
        None => "—".to_string(),
        Some(s) if s.udp_active => t!("core_expert.wd_udp_on").to_string(),
        Some(s) if !s.udp_error.is_empty() => s.udp_error.clone(),
        Some(_) => t!("core_expert.wd_udp_off").to_string(),
    };
    t!("core_expert.sp_udp_server", v = v).to_string()
}

/// One checkbox of the block, staging into its edit.
fn check(
    id: &'static str,
    label: String,
    checked: bool,
    enabled: bool,
    view: &Entity<CoreExpertView>,
    set: fn(&mut WatchdogSettingsEdit, bool),
) -> impl IntoElement {
    let view = view.clone();
    MoonCheckbox::new(SharedString::from(id))
        .label(label)
        .checked(checked)
        .disabled(!enabled)
        .on_change(move |on: &bool, _w, app| {
            let on = *on;
            view.update(app, |this, cx| this.watchdog_edit(|e, _| set(e, on), cx));
        })
}

/// One checkbox of the picked row, staging into that row of the edit.
fn row_check(
    id: &'static str,
    label: String,
    checked: bool,
    enabled: bool,
    view: &Entity<CoreExpertView>,
    set: fn(&mut WatchdogEntryEdit, bool),
) -> impl IntoElement {
    let view = view.clone();
    MoonCheckbox::new(SharedString::from(id))
        .label(label)
        .checked(checked)
        .disabled(!enabled)
        .on_change(move |on: &bool, _w, app| {
            let on = *on;
            view.update(app, |this, cx| {
                this.watchdog_edit(
                    |e, row| {
                        if let Some(entry) = row.and_then(|ix| e.entries.get_mut(ix)) {
                            set(entry, on);
                        }
                    },
                    cx,
                );
            });
        })
}

/// One action button of the block.
fn button(
    id: &'static str,
    label: String,
    enabled: bool,
    view: &Entity<CoreExpertView>,
    act: fn(&mut CoreExpertView, &mut Context<CoreExpertView>),
) -> impl IntoElement {
    let view = view.clone();
    MoonButton::new(id)
        .label(label)
        .variant(MoonButtonVariant::Soft)
        .disabled(!enabled)
        .padding_x(14.0)
        .on_click(move |_, _window, app| {
            view.update(app, act);
        })
        .render()
}

/// One editor of the block, or an empty slot on the frame before the window built it.
///
/// `width` zero takes the width the parent gives; `placeholder` says what an empty box means.
fn input(
    block: &WatchdogBlock,
    id: &'static str,
    width: f32,
    enabled: bool,
    placeholder: Option<String>,
    cx: &App,
) -> AnyElement {
    let Some(state) = block.input(id) else {
        return div().into_any_element();
    };
    let masked = id == ids::KEY || id == ids::SECRET;
    let field = MoonInput::new(id)
        .state(state)
        .size(design::INPUT_SIZE)
        .disabled(!enabled)
        .placeholder(placeholder.unwrap_or_default())
        .when(masked, |input| input.mask_toggle());
    let host = div().child(field);
    if width > 0.0 {
        host.w(design::ui_px(cx, width))
            .flex_none()
            .into_any_element()
    } else {
        host.w_full().into_any_element()
    }
}

/// The observer table: one row per row of the edit drawn, with the core's status beside it.
fn table(
    view: &Entity<CoreExpertView>,
    block: &WatchdogBlock,
    snap: Option<&CoreWatchdogState>,
    edit: Option<&WatchdogSettingsEdit>,
    selected: Option<usize>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let rows: Vec<[SharedString; 10]> = edit
        .map(|e| {
            e.entries
                .iter()
                .map(|row| row_cells(row, snap.and_then(|s| s.entry(&row.id))))
                .collect()
        })
        .unwrap_or_default();
    let empty = rows.is_empty();
    let count = rows.len();
    let height = design::table_head_h(cx) + design::table_row_h(cx) * TABLE_ROWS;
    let Some(state) = block.table.as_ref() else {
        return div().h(px(height)).into_any_element();
    };
    let pick_view = view.clone();
    let table = MoonDataTable::new("exp-sp-wd-table", count, move |ix, _window, _app| {
        let cells = rows.get(ix).cloned().unwrap_or_default();
        MoonDataRow::new(cells.into_iter().map(MoonDataCell::text)).selected(selected == Some(ix))
    })
    .columns(columns())
    .state(state)
    .controlled_row_selection(true)
    .style(design::table_style(p))
    .on_select_row(move |ix, _window, app| {
        pick_view.update(app, |this, cx| this.watchdog_select(Some(ix), cx));
    });
    // A column with a fixed height is what gives a virtual list its height: `flex_1` alone is
    // inert in a page that scrolls.
    v_flex()
        .w_full()
        .h(px(height))
        .flex_none()
        .border_1()
        .border_color(rgb(p.border))
        .child(crate::panels::common::data_table_host(
            "exp-sp-wd-table-host",
            empty,
            t!("core_expert.wd_empty").to_string(),
            p,
            cx,
            table,
        ))
        .into_any_element()
}

/// The table's columns, in the order [`row_cells`] fills them.
fn columns() -> Vec<MoonDataTableColumn> {
    vec![
        MoonDataTableColumn::new("name", t!("core_expert.wd_col_name").to_string(), 120.0),
        MoonDataTableColumn::new("key", t!("core_expert.wd_col_key").to_string(), 110.0),
        MoonDataTableColumn::new("orders", t!("core_expert.wd_col_orders").to_string(), 60.0)
            .center(),
        MoonDataTableColumn::new(
            "positions",
            t!("core_expert.wd_col_positions").to_string(),
            60.0,
        )
        .center(),
        MoonDataTableColumn::new("ip", "IP".to_string(), 40.0).center(),
        MoonDataTableColumn::new("price", t!("core_expert.wd_col_price").to_string(), 60.0).right(),
        MoonDataTableColumn::new("time", t!("core_expert.wd_col_time").to_string(), 60.0).right(),
        MoonDataTableColumn::new("status", t!("core_expert.wd_col_status").to_string(), 120.0),
        MoonDataTableColumn::new("online", t!("core_expert.wd_col_online").to_string(), 60.0)
            .right(),
        MoonDataTableColumn::new("added", t!("core_expert.wd_col_added").to_string(), 80.0),
    ]
}

/// One table row as text: the edit's values, and the core's state of the saved row.
fn row_cells(row: &WatchdogEntryEdit, saved: Option<&CoreWatchdogEntry>) -> [SharedString; 10] {
    let mark = |on: bool| SharedString::from(if on { "✓" } else { "" });
    let key = if row.api_key.is_some() {
        t!("core_expert.wd_key_new").to_string()
    } else {
        match saved {
            Some(s) if s.key_set && !s.api_key_hint.is_empty() => s.api_key_hint.clone(),
            Some(s) if s.key_set => t!("core_expert.wd_key_saved").to_string(),
            _ => "—".to_string(),
        }
    };
    let status = match saved {
        None => t!("core_expert.wd_status_new").to_string(),
        Some(s) => {
            let mut text = status_text(s.status);
            if s.duplicate {
                text = format!("{text} · {}", t!("core_expert.wd_duplicate"));
            }
            if !s.error.is_empty() {
                text = format!("{text}: {}", s.error);
            }
            text
        }
    };
    [
        row.name.clone().into(),
        key.into(),
        mark(row.orders),
        mark(row.positions),
        mark(row.ip),
        format!("{}", row.price).into(),
        row.time.to_string().into(),
        status.into(),
        saved
            .map_or_else(|| "—".to_string(), |s| s.online_count.to_string())
            .into(),
        saved.map_or_else(String::new, |s| s.added.clone()).into(),
    ]
}

/// A row status in the window's language.
fn status_text(status: WatchdogStatus) -> String {
    match status {
        WatchdogStatus::Added => t!("core_expert.wd_status_added").to_string(),
        WatchdogStatus::Error => t!("core_expert.wd_status_error").to_string(),
        WatchdogStatus::Ready => t!("core_expert.wd_status_ready").to_string(),
        WatchdogStatus::Checking => t!("core_expert.wd_status_checking").to_string(),
        WatchdogStatus::Removing => t!("core_expert.wd_status_removing").to_string(),
        WatchdogStatus::Unknown(code) => format!("#{code}"),
    }
}

/// The editor of the picked row: its label, its credentials (write-only), its four switches and
/// its two thresholds.
fn row_editor(
    view: &Entity<CoreExpertView>,
    block: &WatchdogBlock,
    row: &WatchdogEntryEdit,
    saved: Option<&CoreWatchdogEntry>,
    enabled: bool,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let gap = design::ui_px(cx, 6.0);
    let key_hint = match saved {
        Some(s) if s.key_set && !s.api_key_hint.is_empty() => {
            t!("core_expert.wd_keep_key", hint = s.api_key_hint.as_str()).to_string()
        }
        Some(s) if s.key_set => t!("core_expert.wd_keep_saved").to_string(),
        _ => t!("core_expert.wd_required").to_string(),
    };
    let secret_hint = if saved.is_some_and(|s| s.secret_set) {
        t!("core_expert.wd_keep_saved").to_string()
    } else {
        t!("core_expert.wd_required").to_string()
    };
    let labeled = |label: String, control: AnyElement| {
        v_flex()
            .gap(design::ui_px(cx, 2.0))
            .child(hint(label, p, cx))
            .child(control)
    };
    v_flex()
        .w_full()
        .gap(gap)
        .p(design::ui_px(cx, 6.0))
        .rounded(design::r_button(cx))
        .border_1()
        .border_color(rgb(p.border))
        // Facet skin: the form is a card resting on the page.
        .when(design::is_facet(cx), |this| {
            design::facet_surface(this, moon_ui::MoonFacetKind::Card, cx)
        })
        .child(
            h_flex()
                .w_full()
                .items_end()
                .gap(design::ui_px(cx, 8.0))
                .child(labeled(
                    t!("core_expert.wd_col_name").to_string(),
                    input(block, ids::NAME, 140.0, enabled, None, cx),
                ))
                .child(div().flex_1().min_w_0().child(labeled(
                    "API key".to_string(),
                    input(block, ids::KEY, 0.0, enabled, Some(key_hint), cx),
                )))
                .child(div().flex_1().min_w_0().child(labeled(
                    "API secret".to_string(),
                    input(block, ids::SECRET, 0.0, enabled, Some(secret_hint), cx),
                ))),
        )
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 12.0))
                .child(row_check(
                    "exp-sp-wd-rsa",
                    "RSA".to_string(),
                    row.rsa,
                    enabled,
                    view,
                    |e, on| e.rsa = on,
                ))
                .child(row_check(
                    "exp-sp-wd-orders",
                    t!("core_expert.wd_col_orders").to_string(),
                    row.orders,
                    enabled,
                    view,
                    |e, on| e.orders = on,
                ))
                .child(row_check(
                    "exp-sp-wd-positions",
                    t!("core_expert.wd_col_positions").to_string(),
                    row.positions,
                    enabled,
                    view,
                    |e, on| e.positions = on,
                ))
                .child(row_check(
                    "exp-sp-wd-heartbeat",
                    t!("core_expert.wd_heartbeat").to_string(),
                    row.ip,
                    enabled,
                    view,
                    |e, on| e.ip = on,
                ))
                .child(div().flex_1())
                .child(labeled(
                    t!("core_expert.wd_price").to_string(),
                    input(block, ids::PRICE, 72.0, enabled, None, cx),
                ))
                .child(labeled(
                    t!("core_expert.wd_time").to_string(),
                    input(block, ids::TIME, 72.0, enabled, None, cx),
                )),
        )
        .when_some(saved.filter(|s| !s.error.is_empty()), |this, s| {
            this.child(text_block(s.error.clone(), p.red, false, cx))
        })
        .into_any_element()
}
