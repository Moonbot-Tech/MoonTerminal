//! Core table for the Connections tab: server rows with active/window toggles, name, key, group,
//! chart bundle, feed count, color, delete, reconnect, and status controls; shared column layout
//! and headers; feed-flag dropdown; and server add/delete actions.
//!
//! The row-level pieces here (`server_row`, `feed_popover`, `srv_check`, `paste_key_affix`,
//! `status_dot`) are FREE functions taking a weak `SettingsView` handle rather than `&self`/
//! `cx.listener`: they are called from `MoonVirtualList`'s row factory, which only ever hands out
//! `&mut App` -- there is no `Context<SettingsView>` to make a strong listener from, and a strong
//! `cx.entity()` captured into a closure MoonUI retains for the list's whole life would close
//! `SettingsView -> element -> closure -> SettingsView` and leak the window (see
//! `strategies/tree/moon.rs::moon_tree_el` for the same shape). `add_server` and `delete_server`
//! stay ordinary `&mut self` methods; the row buttons reach them through `weak.update`.

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonColorPicker,
    MoonDropdown, MoonInput, MoonInputState, MoonMenuItem, MoonMenuSize, MoonPalette, MoonText,
    MoonTone, MoonTooltipView, StyledExt, h_flex,
};
use rust_i18n::t;

use super::columns::{CONN_TABLE_INSET, ConnColAlign, ConnColId, MicroTriggerMetrics};
use super::{ConnRow, ConnRowIds, SettingsView, build_conn, sync_groups_from_servers};
use crate::design;
use crate::panels::common::{RadioMark, radio_items};
use moon_core::config::{
    AppConfig, FeedFlags, Secret, ServerConfig, TransportVersion, WorkspaceMembership,
};
use moon_core::feed::ConnStatus;
use moon_core::session::CoreId;

/// Eight flags controlling incoming core data, each with a label key, getter, and setter.
///
/// `feed_popover` localizes each `conn.tip.*` key and appends the localized client-side-filter
/// note from `conn.filter_note`. The constant stores static keys rather than rendered labels.
const FEED_FLAGS: [(&str, fn(&FeedFlags) -> bool, fn(&mut FeedFlags, bool)); 8] = [
    ("conn.tip.orders", |f| f.orders, |f, v| f.orders = v),
    ("conn.tip.detects", |f| f.detects, |f, v| f.detects = v),
    ("conn.tip.reports", |f| f.reports, |f, v| f.reports = v),
    ("conn.tip.balance", |f| f.balance, |f, v| f.balance = v),
    ("conn.tip.strat", |f| f.strategies, |f, v| f.strategies = v),
    ("conn.tip.log", |f| f.log, |f, v| f.log = v),
    ("conn.tip.alerts", |f| f.alerts, |f, v| f.alerts = v),
    ("conn.tip.arb", |f| f.arb, |f, v| f.arb = v),
];

/// Fixed pitch of one core, group, or exchange row in the virtualized Connections list.
///
/// Every entry the list draws -- pending/group/exchange headers included -- takes this same
/// height, because `MoonVirtualList` is `uniform_list`-backed and needs one row height for every
/// item. Provably clipping-free: `fit_height(h, lh, pad) = ui(h).max(line(lh) + ui(pad)*2)`
/// (MoonUI `theme.rs`); the tallest row content is a Small `MoonInput`, whose own fit is
/// `ui(22).max(line(13) + ui(4.5)*2)`. Both terms in this row height strictly dominate the
/// corresponding input-height terms at every font scale because `ui()` and `line()` are monotone,
/// so a Small input never clips.
///
/// No `conn_row_h_px` companion (the house `_value`/`_px` pair, e.g.
/// `analytics/profit_monitor/line.rs::row_h_value`/`row_h_px`): `MoonVirtualList` already wraps
/// every item in a `div().h(px(item_height))` of its own (`virtual_list.rs::render_range`), so
/// nothing here needs a `Pixels` copy of the same number -- one would be dead code.
///
/// Args:
///     cx: Application context used to resolve scaled dimensions.
///
/// Returns:
///     The uniform virtual-list row height in unscaled design units.
pub(super) fn conn_row_h_value(cx: &App) -> f32 {
    design::fit_h_value(cx, 30.0, 13.0, 8.5)
}

/// What a status dot's tooltip says when no connection verdict is available for it.
#[derive(Clone, Copy)]
enum StatusFallback {
    /// A plain `conn.status.*` key that needs no interpolation.
    Plain(&'static str),
    /// `conn.status.failed`, whose message slot has nothing to put in it.
    FailedWithoutDetail,
}

impl StatusFallback {
    /// Resolve this fallback state to its localized tooltip text.
    ///
    /// Returns:
    ///     The translated status message, with a placeholder for an absent failure detail.
    fn render(self) -> String {
        match self {
            Self::Plain(key) => t!(key).to_string(),
            Self::FailedWithoutDetail => t!("conn.status.failed", err = "-").to_string(),
        }
    }
}

/// Render a connection-status dot with a localized tooltip, including failure details.
///
/// The dot's COLOUR still comes from the coarse status, because a glance down the column has to
/// stay readable. Its tooltip comes from the verdict: this is where a user pastes the key, so it is
/// also where "why did that not work, and what do I do" belongs.
///
/// The VERDICT is resolved inside the tooltip closure, not here: only two of the six arms can carry
/// one, only a hover ever reads it, and building it eagerly cost a `diagnose()` per core per frame.
///
/// Args:
///     row_key: The owning `ConnRow`'s per-session identity, used for the dot's element id.
///     core_id: Core whose live record the tooltip re-reads at hover time.
///     active: Whether the configured core is enabled.
///     status: Latest lifecycle state, when the runtime has reported one.
///     view: Weak handle used by the tooltip to reach the backend.
///     p: Active palette supplying status colours.
///
/// Returns:
///     A status dot with a tooltip appropriate to the available connection evidence.
fn status_dot(
    row_key: u64,
    core_id: CoreId,
    active: bool,
    status: Option<&ConnStatus>,
    view: WeakEntity<SettingsView>,
    p: MoonPalette,
) -> impl IntoElement {
    let (color, fallback, wants_verdict) = match status {
        _ if !active => (
            p.text_soft,
            StatusFallback::Plain("conn.status.inactive"),
            false,
        ),
        Some(ConnStatus::Ready) => (p.green, StatusFallback::Plain("conn.status.ready"), false),
        Some(ConnStatus::Failed(_)) => (p.red, StatusFallback::FailedWithoutDetail, true),
        Some(ConnStatus::Connecting | ConnStatus::Stage(_)) => (
            p.amber,
            StatusFallback::Plain("conn.status.connecting"),
            true,
        ),
        Some(ConnStatus::Disconnected) => (
            p.text_soft,
            StatusFallback::Plain("conn.status.disconnected"),
            false,
        ),
        None => (
            p.text_soft,
            StatusFallback::Plain("conn.status.none"),
            false,
        ),
    };
    div()
        .id(("conn-st", row_key))
        .w(px(10.0))
        .h(px(10.0))
        .rounded_full()
        .bg(rgb(color))
        .tooltip(move |_window, cx| {
            let verdict = wants_verdict
                .then(|| {
                    let view = view.upgrade()?;
                    let backend = view.read(cx).backend.clone();
                    let b = backend.read(cx);
                    let store = b.session.store();
                    let core = store.core(core_id)?;
                    let diag = moon_core::feed::diagnose(
                        &core.status,
                        core.fault.as_ref(),
                        &core.startup,
                    )?;
                    let mode_suggestion =
                        crate::conn_diag::fleet_mode_suggestion(core_id, &b.config.servers, |id| {
                            store
                                .core(id)
                                .is_some_and(|c| c.status == ConnStatus::Ready)
                        });
                    Some(crate::panels::problem_diagnostic_text(
                        &diag,
                        core.fault.as_ref(),
                        &core.startup,
                        mode_suggestion,
                    ))
                })
                .flatten();
            let tip = verdict.unwrap_or_else(|| fallback.render());
            cx.new(|_| MoonTooltipView::new(tip).max_width(320.0))
                .into()
        })
}

/// Build a checkbox bound to a boolean field of draft server `servers[i]`, driven by a weak view
/// handle rather than `cx.listener` -- see the module doc comment.
///
/// Deliberately NOT a change to `SettingsView::draft_checkbox`: that shared helper is used by every
/// other tab, which are not virtualized and keep their strong `cx.listener`.
///
/// Args:
///     weak: Weak owner used by the retained checkbox callback.
///     id: Stable element identity.
///     init: Initial checked value.
///     apply: Draft mutation that reports whether it changed the configuration.
///
/// Returns:
///     A checkbox whose change callback updates the Settings draft through the weak handle.
fn draft_checkbox_weak(
    weak: &WeakEntity<SettingsView>,
    id: impl Into<SharedString>,
    init: bool,
    apply: impl Fn(&mut AppConfig, bool) -> bool + 'static,
) -> MoonCheckbox {
    let weak = weak.clone();
    MoonCheckbox::new(id.into())
        .checked(init)
        .on_change(move |ch: &bool, _window, cx| {
            let v = *ch;
            let _ = weak.update(cx, |this, ctx| {
                let changed = this.backend.update(ctx, |b, bcx| {
                    let mut changed = false;
                    if let Some(p) = b.preview.as_mut() {
                        if apply(p, v) {
                            bcx.notify();
                            changed = true;
                        }
                    }
                    changed
                });
                if changed {
                    ctx.notify();
                }
            });
        })
}

/// Build a checkbox bound to a boolean field of draft server `servers[i]`.
///
/// Args:
///     view: Settings state used to read the current draft value.
///     weak: Weak owner used by the checkbox callback.
///     cx: Application context.
///     i: Draft index of the server being edited.
///     id: Stable element identity.
///     label: Optional checkbox label.
///     get: Accessor for the draft field.
///     set: Mutator for the draft field.
///
/// Returns:
///     A compact checkbox synchronized with the selected draft-server field.
#[allow(clippy::too_many_arguments)]
fn srv_check(
    view: &SettingsView,
    weak: &WeakEntity<SettingsView>,
    cx: &App,
    i: usize,
    id: SharedString,
    label: &'static str,
    get: fn(&ServerConfig) -> bool,
    set: fn(&mut ServerConfig, bool),
) -> impl IntoElement {
    let cur = {
        let b = view.backend.read(cx);
        b.preview
            .as_ref()
            .unwrap_or(&b.config)
            .servers
            .get(i)
            .map(get)
            .unwrap_or(false)
    };
    let mut checkbox = draft_checkbox_weak(weak, id, cur, move |p, v| {
        if let Some(s) = p.servers.get_mut(i) {
            if get(s) != v {
                set(s, v);
                return true;
            }
        }
        false
    })
    .size(MoonCheckboxSize::Compact);
    if !label.is_empty() {
        checkbox = checkbox.label(label);
    }
    checkbox
}

/// Build a Paste glyph control beside the key field, styled like its built-in affixes.
///
/// Clicking reads a nonempty key from the clipboard and updates both input state and
/// `servers[i].key`; `set_value` does not emit Change, so the draft is updated directly. Pasting
/// is also when the key gets to speak for `servers[i].transport`, as MoonBot fills its own radio
/// on paste -- but only while that row has no mode of its own, which is what
/// `config::seeded_transport` decides and documents.
///
/// Args:
///     weak: Weak owner used to update the Settings draft.
///     i: Draft index of the server being edited.
///     row_key: Stable identity for the control.
///     key_state: Input state that mirrors the pasted key.
///     p: Active palette.
///
/// Returns:
///     A clipboard-paste control for the server-key input.
fn paste_key_affix(
    weak: &WeakEntity<SettingsView>,
    i: usize,
    row_key: u64,
    key_state: Entity<MoonInputState>,
    p: MoonPalette,
) -> impl IntoElement {
    let weak = weak.clone();
    div()
        .id(("paste-key", row_key))
        .flex()
        .items_center()
        .justify_center()
        .px(px(2.0))
        .cursor_pointer()
        .tooltip(|_window, cx| {
            cx.new(|_| MoonTooltipView::new(t!("conn.paste_key_tip").to_string()).max_width(320.0))
                .into()
        })
        .child(
            MoonText::new("⧉")
                .color(p.text_muted)
                .font_size(11.0)
                .mono(true)
                .uppercase(false)
                .render(),
        )
        .on_click(move |_, window, cx| {
            let Some(text) = cx
                .read_from_clipboard()
                .and_then(|it| it.text())
                .filter(|t| !t.trim().is_empty())
            else {
                return;
            };
            let text = text.trim().to_string();
            key_state.update(cx, |st, c| st.set_value(text.clone(), window, c));
            let _ = weak.update(cx, |this, ctx| {
                this.backend.update(ctx, |b, bcx| {
                    if let Some(pv) = b.preview.as_mut() {
                        if let Some(s) = pv.servers.get_mut(i) {
                            s.transport = moon_core::config::seeded_transport(s.transport, &text);
                            s.key = Secret::new(text.clone());
                            // A new key can point this row at a DIFFERENT Moonbot, and strategy ids
                            // are unique per host, not globally — so the pinned id would silently
                            // name whatever strategy inherited that number there. The NAME survives
                            // and re-pins itself against the new host's list.
                            if let Some(manual) = s.manual_strategy.as_mut() {
                                manual.id = 0;
                            }
                            bcx.notify();
                        }
                    }
                });
            });
        })
}

/// Rendered-width inputs a Micro `MoonDropdown` trigger resolves itself against.
///
/// MIRRORS MoonUI, which exposes no accessor for either number: `MoonDropdown` resolves a
/// `Scaled(w)` trigger to `max(w * tokens.font(fs) / fs, tokens.ui(14.0) + measure(ellipsis +
/// caret))`, and `button_text_metrics` gives a Micro trigger `fs = 10.0`. The two terms follow
/// DIFFERENT sliders -- the first the Font delta, the second UI geometry -- which is why the
/// floor cannot be folded into the scale.
///
/// The glyph run inside that floor is measured within MoonUI; here it is OVER-estimated at two
/// ems, deliberately and in the safe direction: a column wider than its trigger merely centres
/// it, while a column narrower than its trigger is the misalignment this module exists to
/// prevent. If MoonUI's Micro metrics or trigger padding move, this must follow by hand -- the
/// same standing caveat `design::glyph_btn_w` carries.
///
/// Args:
///     cx: Application context used to read active theme tokens.
///
/// Returns:
///     The scale and floor every core-table column is laid out against.
fn micro_trigger_metrics(cx: &App) -> MicroTriggerMetrics {
    /// Design-reference font size of a `MoonButtonSize::Micro` trigger.
    const MICRO_TRIGGER_FONT: f32 = 10.0;
    /// MoonUI's `DROPDOWN_TRIGGER_PAD_X`, its trigger's horizontal visual padding.
    const TRIGGER_PAD_X: f32 = 14.0;

    MicroTriggerMetrics {
        scale: design::font_value(cx, MICRO_TRIGGER_FONT) / MICRO_TRIGGER_FONT,
        min_width: design::ui_value(cx, TRIGGER_PAD_X)
            + 2.0 * design::font_value(cx, MICRO_TRIGGER_FONT),
    }
}

impl SettingsView {
    /// Start (or restart) the first-run hint's repaint chain.
    ///
    /// Called once when the window opens and again after a row is added, because adding a row MOVES
    /// the hint: with no rows there is no key field to point at, so the add button carries it, and
    /// the moment a row exists the empty key field is the thing the user actually needs.
    ///
    /// Args:
    ///     cx: The Settings view context.
    ///
    /// Returns:
    ///     Nothing; the chain repaints this view until the hint expires.
    pub(in crate::settings) fn arm_conn_hint(&mut self, cx: &mut Context<Self>) {
        if self.backend.read(cx).config.core_ever_configured() {
            return;
        }
        self.conn_hint_at = Some(std::time::Instant::now());
        crate::pulse::arm(
            self,
            cx,
            |this| &mut this.conn_hint_armed,
            |this| {
                this.conn_hint_at
                    .is_some_and(|at| at.elapsed() < crate::pulse::ATTENTION)
            },
        );
    }

    /// The live first-run hint, or `None` when there is nothing to point at.
    ///
    /// Two conditions, deliberately: the timer bounds how long it breathes, and the SAVED config
    /// decides whether it is still relevant. That second half is what makes a successful Save tear
    /// the ring down on the next frame instead of at the end of the timer, on every control that
    /// asks -- so the teardown lives in ONE place rather than at each call site.
    ///
    /// Args:
    ///     cx: Any app context that can read the backend.
    ///
    /// Returns:
    ///     The arming instant while the hint is live.
    pub(super) fn conn_hint(&self, cx: &App) -> Option<std::time::Instant> {
        self.conn_hint_at
            .filter(|_| !self.backend.read(cx).config.core_ever_configured())
    }

    /// Add a draft server with `id = max + 1` to the given group, then rebuild editor state.
    ///
    /// Args:
    ///     group: Group assigned to the new draft server.
    ///     window: Settings window used to build the new editor states.
    ///     cx: Settings context used to update the draft and request a repaint.
    ///
    /// Returns:
    ///     Nothing; the method adds the row, resets row-owned transient state, and repaints.
    pub(super) fn add_server(
        &mut self,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let default_color = design::u32_to_rgb(MoonPalette::active(cx).accent);
        self.backend.update(cx, |b, bcx| {
            if let Some(p) = b.preview.as_mut() {
                let next = p.servers.iter().map(|s| s.id).max().unwrap_or(0) + 1;
                p.servers.push(ServerConfig {
                    id: next,
                    uid: 0,
                    name: format!("server {next}"),
                    active: true,
                    show_window: true,
                    feed: FeedFlags::default(),
                    key: Secret::new(""),
                    group,
                    market: "BTCUSDT".into(),
                    color: default_color,
                    synthetic: false,
                    chart_bundle: String::new(),
                    default_alert_strategy: 0,
                    own_trade_config: false,
                    strat_slots: None,
                    manual_strategy: None,
                    trade: None,
                    // No key yet, so no mode to seed: pasting one fills this in.
                    transport: None,
                    // A new core ships shown everywhere.
                    workspace_membership: WorkspaceMembership::default(),
                });
                sync_groups_from_servers(&p.servers, &mut p.groups);
                bcx.notify();
            }
        });
        let rows = build_conn(&self.backend, window, cx);
        self.conn = rows;
        // Every row just got a fresh key, so any open popup now names a row that no longer exists.
        // Shut it rather than leave it pointing at nothing.
        self.feed_open = None;
        self.proto_open = None;
        self.preset_open = None;
        self.picking = None;
        self.focused_conn_row = None;
        // The hint MOVES with this row: it pointed at this button, and the thing the newcomer needs
        // next is the empty key field that just appeared. Re-arming restarts the clock so the ring
        // is at full strength on the control that now matters.
        self.arm_conn_hint(cx);
        cx.notify();
    }

    /// Delete draft server `i`, synchronize groups, then rebuild editor state.
    ///
    /// Args:
    ///     i: Draft index of the server to remove.
    ///     window: Settings window used to rebuild the remaining editor states.
    ///     cx: Settings context used to update the draft and request a repaint.
    ///
    /// Returns:
    ///     Nothing; the method removes the row when it exists and clears row-owned transient state.
    pub(super) fn delete_server(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.backend.update(cx, |b, bcx| {
            if let Some(p) = b.preview.as_mut() {
                if i < p.servers.len() {
                    p.servers.remove(i);
                    sync_groups_from_servers(&p.servers, &mut p.groups);
                    bcx.notify();
                }
            }
        });
        let rows = build_conn(&self.backend, window, cx);
        self.conn = rows;
        // See `add_server`: the keys the open menu was named by are gone.
        self.feed_open = None;
        self.proto_open = None;
        self.preset_open = None;
        self.picking = None;
        self.focused_conn_row = None;
        cx.notify();
    }
}

/// Build the `Data n/8` dropdown ported from egui's `feed_button`.
///
/// The trigger reports enabled feed flags; its eight checkbox items update the draft.
///
/// The menu is CONTROLLED through `SettingsView::feed_open` so the eight items exist only for
/// the row whose menu is actually open. MoonUI cannot do this for us: `MoonDropdown::items`
/// consumes the whole `Vec<MoonMenuItem>` into `MoonMenuLevel::from_parts` before it ever looks
/// at the open flag, so there is no lazy-item API to reach for.
///
/// Args:
///     view: Current Settings state, read for the draft feed flags.
///     weak: Weak callback owner that avoids a retained-element cycle.
///     i: Draft index of the server this dropdown edits.
///     row_key: Owning row's identity, the value `feed_open` is compared against.
///     ids: The row's precomputed element-id strings.
///     cx: Application context.
///
/// Returns:
///     The feed-flag dropdown for one core row.
fn feed_popover(
    view: &SettingsView,
    weak: &WeakEntity<SettingsView>,
    i: usize,
    row_key: u64,
    ids: &ConnRowIds,
    cx: &App,
) -> impl IntoElement {
    let feed = {
        let b = view.backend.read(cx);
        let s = b.preview.as_ref().unwrap_or(&b.config).servers.get(i);
        s.map(|s| s.feed.clone()).unwrap_or_default()
    };
    let on = FEED_FLAGS.iter().filter(|(_, g, _)| g(&feed)).count();
    let tinted = on < FEED_FLAGS.len();
    let open = view.feed_open == Some(row_key);

    // Only the OPEN row pays for menu items. Everything below this point is skipped 55 times
    // out of 56 on a page with one menu open, and 56 times out of 56 with none.
    let mut items = Vec::new();
    if open {
        for (ix, (key, get, set)) in FEED_FLAGS.iter().copied().enumerate() {
            let cur = get(&feed);
            let backend = view.backend.clone();
            items.push(
                MoonMenuItem::with_key(
                    format!("feed-{row_key}-{ix}"),
                    format!("{} ({})", t!(key), t!("conn.filter_note")),
                )
                .checked(cur)
                // Make states explicit: enabled items are green and checked, while disabled
                // items are muted and unchecked, so the user need not infer the missing flag
                // from a count such as `7/8`.
                .tone(if cur {
                    MoonTone::Positive
                } else {
                    MoonTone::Muted
                })
                .on_click(move |_, _, cx| {
                    backend.update(cx, |b, bcx| {
                        if let Some(p) = b.preview.as_mut() {
                            if let Some(s) = p.servers.get_mut(i) {
                                set(&mut s.feed, !cur);
                                bcx.notify();
                            }
                        }
                    });
                }),
            );
        }
    }

    // WEAK, not a strong entity: `on_open_change` takes a plain `Fn(bool, &mut Window, &mut App)`
    // that MoonUI stores for the life of the element, and a strong handle there would close
    // SettingsView -> element -> closure -> SettingsView and keep the window alive forever.
    let view_weak = weak.clone();
    MoonDropdown::new(ids.feed.clone())
        .label(format!("{on}/8"))
        .trigger_caret(true)
        .trigger_variant(if tinted {
            MoonButtonVariant::Amber
        } else {
            MoonButtonVariant::Neutral
        })
        .trigger_size(MoonButtonSize::Micro)
        .trigger_width_scaled(ConnColId::Data.spec().basis)
        .menu_width_scaled(272.0)
        .menu_size(MoonMenuSize::Compact)
        .close_on_select(false)
        .items(items)
        .open(open)
        // In CONTROLLED mode MoonUI deliberately does not repaint the parent view for us -- it
        // only does that when it owns the open flag itself -- so this handler both stores the
        // new state and asks for the repaint that makes it visible.
        .on_open_change(move |now_open, _window, app| {
            let _ = view_weak.update(app, |this, cx| {
                this.feed_open = now_open.then_some(row_key);
                cx.notify();
            });
        })
}

/// Build the MoonProto transport selector for one server row.
///
/// The mode is seeded from the core's own key (`config::seeded_transport`) and is the user's
/// from then on, mirroring MoonBot's `V0 / V1 / V2` radio: MoonBot lets a core's switch move
/// without issuing a new key, so a terminal that could only read the key would force a re-export
/// of every key to follow one switch. A dash means nothing is pinned yet -- no key, or a legacy
/// export that carries no mode -- and the connection then follows the key, as before.
///
/// The choice lands in the DRAFT, like every other field here: it takes effect on Save, which
/// respawns the core by itself because `session::conn_sig` hashes the mode.
///
/// Args:
///     view: Settings state read for the row's current draft value.
///     weak: Weak owner the select handler closes over.
///     i: Draft index of the server being edited.
///     ids: Precomputed element ids for the row.
///     cx: Application context.
///
/// Returns:
///     A compact dropdown bound to draft `servers[i].transport`.
fn proto_dropdown(
    view: &SettingsView,
    weak: &WeakEntity<SettingsView>,
    i: usize,
    row_key: u64,
    ids: &ConnRowIds,
    cx: &App,
) -> impl IntoElement {
    let cur = {
        let b = view.backend.read(cx);
        b.preview
            .as_ref()
            .unwrap_or(&b.config)
            .servers
            .get(i)
            .and_then(|s| s.transport)
    };
    let open = view.proto_open == Some(row_key);

    // Only the OPEN row pays for menu items, exactly as `feed_popover` above: three items, three
    // boxed handlers and a `Vec` per row per frame is what `SettingsView::feed_open` exists to
    // avoid, and a wheel notch over the page rebuilds every row.
    let items = if open {
        // WEAK, not a strong entity, for the reason stated on `server_row`: MoonUI keeps this
        // handler for the element's whole life, and a strong handle would leak the Settings
        // window.
        let weak_select = weak.clone();
        // `Option<TransportVersion>` is the item value, so an unset row marks nothing as current
        // instead of pretending it is pinned to V0. Key and label are both the mode's own
        // `&'static str`, so neither allocates.
        radio_items(
            TransportVersion::ALL.into_iter().map(|v| {
                (
                    Some(v),
                    SharedString::from(v.label()),
                    SharedString::from(v.label()),
                )
            }),
            cur,
            RadioMark::Check,
            move |app, v| {
                let _ = weak_select.update(app, |this, ctx| {
                    let changed = this.backend.update(ctx, |b, bcx| {
                        let Some(s) = b.preview.as_mut().and_then(|p| p.servers.get_mut(i)) else {
                            return false;
                        };
                        if s.transport == v {
                            return false;
                        }
                        s.transport = v;
                        bcx.notify();
                        true
                    });
                    if changed {
                        ctx.notify();
                    }
                });
            },
        )
    } else {
        Vec::new()
    };

    let view_weak = weak.clone();
    MoonDropdown::new(ids.proto.clone())
        .label(cur.map_or(SharedString::from("-"), |v| SharedString::from(v.label())))
        .trigger_caret(true)
        .trigger_variant(MoonButtonVariant::Neutral)
        .trigger_size(MoonButtonSize::Micro)
        .trigger_width_scaled(ConnColId::Proto.spec().basis)
        .menu_width_scaled(96.0)
        .menu_size(MoonMenuSize::Compact)
        .items(items)
        .open(open)
        // Controlled mode leaves the repaint to us, as on the feed menu beside it.
        .on_open_change(move |now_open, _window, app| {
            let _ = view_weak.update(app, |this, cx| {
                this.proto_open = now_open.then_some(row_key);
                cx.notify();
            });
        })
}

/// Build the workspace-preset selector for one server row.
///
/// Modelled line-for-line on [`proto_dropdown`] above, one difference only: the value is a
/// bare `WorkspaceMembership` rather than an `Option<TransportVersion>`, because every row
/// always has one -- there is no unset state to represent with a dash. Its three items are the
/// whole enforcement of "a core may not be excluded from both presets": `WorkspaceMembership`
/// has no fourth variant, so there is nothing else to offer.
///
/// Args:
///     view: Settings state read for the row's current draft value.
///     weak: Weak owner the select handler closes over.
///     i: Draft index of the server being edited.
///     ids: Precomputed element ids for the row.
///     cx: Application context.
///
/// Returns:
///     A compact dropdown bound to draft `servers[i].workspace_membership`.
fn preset_dropdown(
    view: &SettingsView,
    weak: &WeakEntity<SettingsView>,
    i: usize,
    row_key: u64,
    ids: &ConnRowIds,
    cx: &App,
) -> impl IntoElement {
    let cur = {
        let b = view.backend.read(cx);
        b.preview
            .as_ref()
            .unwrap_or(&b.config)
            .servers
            .get(i)
            .map(|s| s.workspace_membership)
            .unwrap_or_default()
    };
    let open = view.preset_open == Some(row_key);

    // Only the OPEN row pays for menu items, exactly as `proto_dropdown` above.
    let items = if open {
        let weak_select = weak.clone();
        radio_items(
            WorkspaceMembership::ALL.into_iter().map(|m| {
                (
                    m,
                    SharedString::from(m.code()),
                    SharedString::from(preset_label(m)),
                )
            }),
            cur,
            RadioMark::Check,
            move |app, m| {
                let _ = weak_select.update(app, |this, ctx| {
                    let changed = this.backend.update(ctx, |b, bcx| {
                        let Some(s) = b.preview.as_mut().and_then(|p| p.servers.get_mut(i)) else {
                            return false;
                        };
                        if s.workspace_membership == m {
                            return false;
                        }
                        s.workspace_membership = m;
                        bcx.notify();
                        true
                    });
                    if changed {
                        ctx.notify();
                    }
                });
            },
        )
    } else {
        Vec::new()
    };

    let view_weak = weak.clone();
    MoonDropdown::new(ids.preset.clone())
        .label(SharedString::from(preset_label(cur)))
        .trigger_caret(true)
        .trigger_variant(MoonButtonVariant::Neutral)
        .trigger_size(MoonButtonSize::Micro)
        .trigger_width_scaled(ConnColId::Preset.spec().basis)
        .menu_width_scaled(140.0)
        .menu_size(MoonMenuSize::Compact)
        .items(items)
        .open(open)
        // Controlled mode leaves the repaint to us, as on the transport menu beside it.
        .on_open_change(move |now_open, _window, app| {
            let _ = view_weak.update(app, |this, cx| {
                this.preset_open = now_open.then_some(row_key);
                cx.notify();
            });
        })
}

/// Localized label for one workspace-membership value, shared by the trigger and its menu items.
fn preset_label(m: WorkspaceMembership) -> String {
    match m {
        WorkspaceMembership::Both => t!("conn.preset.both"),
        WorkspaceMembership::ClassicOnly => t!("conn.preset.classic"),
        WorkspaceMembership::AutoOnly => t!("conn.preset.auto"),
    }
    .to_string()
}

/// Render a server row ported from egui's `servers_panel`.
///
/// A free function, not a method: `MoonVirtualList`'s row factory is `'static` and outlives the
/// render, so a strong `cx.entity()` capture would close `SettingsView -> element -> closure ->
/// SettingsView` and leak the window -- the same cycle `strategies/tree/moon.rs::moon_tree_el`
/// guards for `MoonTree`. Every interactive child below reaches `SettingsView` only through `weak`.
///
/// Columns contain active and window toggles, name, key, transport mode, group, chart bundle,
/// feed flags, color, delete, reconnect, and status controls.
///
/// Args:
///     view: Current Settings state, read (never mutated) while building the row.
///     weak: Weak callback owner every listener below closes over.
///     row: The row's persistent editor state, including its precomputed `ConnRowIds`.
///     i: Draft index of the server this row edits.
///     core_id: The server's live core identity.
///     active: Whether the draft server is enabled.
///     status: Latest live connection status, if known.
///     cx: Application context used for rendering.
///
/// Returns:
///     One core row. Performs zero `format!` calls of its own -- see [`ConnRowIds`] -- except
///     inside the feed dropdown's items, which are built only for the one row whose menu is open.
#[allow(clippy::too_many_arguments)]
pub(super) fn server_row(
    view: &SettingsView,
    weak: &WeakEntity<SettingsView>,
    row: &ConnRow,
    i: usize,
    core_id: CoreId,
    active: bool,
    status: Option<ConnStatus>,
    cx: &App,
) -> AnyElement {
    crate::diag::bump(&crate::diag::SETTINGS_CONN_ROW_BUILD);
    let p = MoonPalette::active(cx);
    let ids = &row.ids;
    let row_key = row.row_key;
    // Whether this row's key field is still blank, for the first-run ring below.
    let key_empty = row.key.read(cx).value().is_empty();
    // Show reconnect only when the draft server is active. Session status still comes from
    // the live saved runtime and may not yet match unsaved draft activity.
    let recon: AnyElement = if active {
        let weak_recon = weak.clone();
        div()
            .id(("rec-tip", row_key))
            .tooltip(|_window, cx| {
                cx.new(|_| MoonTooltipView::new(t!("conn.reconnect").to_string()))
                    .into()
            })
            .child(
                MoonButton::new(ids.rec.clone())
                    .ghost()
                    .size(MoonButtonSize::Micro)
                    .width(24.0)
                    .label("↻")
                    .on_click(move |_, _, cx| {
                        let _ = weak_recon.update(cx, |this, ctx| {
                            this.backend.update(ctx, |b, bcx| {
                                b.reconnect_request.push(core_id);
                                bcx.notify();
                            });
                        });
                    })
                    .render(),
            )
            .into_any_element()
    } else {
        div().w(px(24.0)).into_any_element()
    };
    // The row is BUILT from `ConnColId::ALL` rather than merely numbered to agree with it: the
    // array below takes its LENGTH from that list, so a column added, dropped or reordered in
    // `columns.rs` without the same move here is a compile error -- not a heading that quietly
    // slides one column to the right, which is exactly how this table drifted before.
    let micro = micro_trigger_metrics(cx);
    let cells: [AnyElement; ConnColId::ALL.len()] = [
        srv_check(
            view,
            weak,
            cx,
            i,
            ids.act.clone(),
            "",
            |s| s.active,
            |s, v| s.active = v,
        )
        .into_any_element(),
        srv_check(
            view,
            weak,
            cx,
            i,
            ids.win.clone(),
            "",
            |s| s.show_window,
            |s, v| s.show_window = v,
        )
        .into_any_element(),
        MoonInput::new(ids.name.clone())
            .state(&row.name)
            .small()
            .into_any_element(),
        // Place the key field and Paste glyph side by side. The fork fixes built-in affix order,
        // so the button cannot be inserted between visibility and clear. `set_value` emits no
        // Change event, so Paste also writes directly to draft.
        h_flex()
            .w_full()
            .gap_1()
            .items_center()
            .child(
                div()
                    .flex_grow_1()
                    .min_w_0()
                    // `relative()` hosts the first-run ring below; it changes no layout, because
                    // the ring is an inset overlay.
                    .relative()
                    .child(
                        MoonInput::new(ids.key.clone())
                            .state(&row.key)
                            .small()
                            // Indicate that this field expects a core key.
                            .placeholder(t!("conn.key_ph").to_string())
                            .mask_toggle()
                            // Allow the key to be cleared quickly before replacement.
                            .cleanable(true),
                    )
                    // Only an EMPTY key is worth pointing at: once the newcomer has pasted
                    // something the field has done its job, and a ring around a filled input
                    // reads as an error rather than as a hint.
                    .children(view.conn_hint(cx).filter(|_| key_empty).and_then(|at| {
                        crate::pulse::attention_ring(MoonPalette::active(cx).accent, at)
                    })),
            )
            .child(paste_key_affix(weak, i, row_key, row.key.clone(), p))
            .into_any_element(),
        proto_dropdown(view, weak, i, row_key, ids, cx).into_any_element(),
        preset_dropdown(view, weak, i, row_key, ids, cx).into_any_element(),
        MoonInput::new(ids.group.clone())
            .state(&row.group)
            .small()
            .into_any_element(),
        MoonInput::new(ids.bundle.clone())
            .state(&row.bundle)
            .small()
            .into_any_element(),
        feed_popover(view, weak, i, row_key, ids, cx).into_any_element(),
        MoonColorPicker::new(&row.color).into_any_element(),
        {
            let weak_del = weak.clone();
            MoonButton::new(ids.del.clone())
                .danger()
                .size(MoonButtonSize::Micro)
                .width(24.0)
                .label("x")
                .on_click(move |_, window, cx| {
                    let _ = weak_del.update(cx, |this, ctx| this.delete_server(i, window, ctx));
                })
                .render()
                .into_any_element()
        },
        recon,
        status_dot(row_key, core_id, active, status.as_ref(), weak.clone(), p).into_any_element(),
    ];
    h_flex()
        .w_full()
        .gap_1()
        .items_center()
        .py_0p5()
        // The list's scrollbar is an overlay that reserves no width of its own, so the status dot,
        // the reconnect glyph and the delete button rendered beneath it. The header subtracts the
        // same gutter, or the two stop lining up.
        .pr(design::ui_px(cx, design::MOON_SCROLLBAR_OVERLAY_W))
        .children(
            ConnColId::ALL
                .into_iter()
                .zip(cells)
                .map(|(col, content)| SettingsView::cell(col, micro).child(content)),
        )
        .into_any_element()
}

impl SettingsView {
    /// Build one column's cell from the SHARED specification in [`super::columns`].
    ///
    /// Both [`server_row`] and [`Self::conn_col_head_row`] call this with the same [`ConnColId`],
    /// so a column's width, growth and alignment cannot differ between the header and the rows.
    ///
    /// `min_w_0()` is the load-bearing part: gpui's default `min_size: auto` is the CONTENT-based
    /// automatic minimum, which clamps a flex item UP to its child's min-content width. A control
    /// that renders wider than its column would then eat free space in the rows that the header
    /// still had -- and since only the header can hand that space to its two growing columns,
    /// every column after them drifted, further with each one. Pinned to the basis, an oversized
    /// child overlaps instead of shifting the grid.
    ///
    /// Args:
    ///     col: Which column to build.
    ///     micro: Micro dropdown trigger metrics, from [`micro_trigger_metrics`].
    ///
    /// Returns:
    ///     The empty cell, ready for its control or label.
    fn cell(col: ConnColId, micro: MicroTriggerMetrics) -> Div {
        let spec = col.spec();
        let d = div().flex_basis(px(col.width(micro))).min_w_0();
        // Centred on the header AND on the row, so the two centre on one axis.
        let d = if spec.align == ConnColAlign::Center {
            d.flex().items_center().justify_center()
        } else {
            d
        };
        if spec.grow {
            d.flex_grow_1()
        } else {
            d.flex_grow_0().flex_shrink_0()
        }
    }

    /// Build one column heading, with its tooltip, ported from egui's `head_tip`.
    ///
    /// Underlining and brighter text signal hover help. A column with no `label` -- colour,
    /// delete, reconnect, status -- yields the bare cell, which still has to be emitted: the
    /// header's growing columns only receive the same free space the rows give them when the
    /// trailing widths are reserved too.
    ///
    /// Args:
    ///     col: Which column to head.
    ///     micro: Micro dropdown trigger metrics, from [`micro_trigger_metrics`].
    ///     p: Active palette.
    ///     cx: Application context used for the label's text size.
    ///
    /// Returns:
    ///     The column's header cell.
    fn col_head(
        col: ConnColId,
        micro: MicroTriggerMetrics,
        p: MoonPalette,
        cx: &App,
    ) -> AnyElement {
        let spec = col.spec();
        let base = Self::cell(col, micro);
        let Some(label_key) = spec.label else {
            return base.into_any_element();
        };
        let tip: SharedString = t!(spec.tip.unwrap_or(label_key)).to_string().into();
        base.id(spec.id)
            .child(
                div()
                    .ml(px(spec.head_pad))
                    .min_w_0()
                    // A label longer than its column ellipsises INSIDE the column; it may not
                    // widen it, or the heading would push the grid it is describing.
                    .truncate()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text))
                    .underline()
                    .text_decoration_color(rgb(p.text_soft))
                    .child(t!(label_key).to_string()),
            )
            .tooltip(move |_window, cx| {
                cx.new(|_| MoonTooltipView::new(tip.clone()).max_width(320.0))
                    .into()
            })
            .into_any_element()
    }

    /// Build an arbitrary-width help label with underlining and a wrapping tooltip.
    ///
    /// Used for section or group headings that need an explanation on hover rather than a column.
    pub(super) fn hint_label(
        id: &'static str,
        label: impl Into<SharedString>,
        tip: SharedString,
        p: MoonPalette,
    ) -> impl IntoElement {
        div()
            .id(id)
            .font_bold()
            .text_color(rgb(p.text))
            .underline()
            .text_decoration_color(rgb(p.text_soft))
            .child(label.into())
            .tooltip(move |_window, cx| {
                cx.new(|_| MoonTooltipView::new(tip.clone()).max_width(360.0))
                    .into()
            })
    }

    /// Render the core table header over the columns it names.
    ///
    /// Every cell comes from [`super::columns`], in the same order and with the same widths
    /// `server_row` uses, and the row is inset by the same [`CONN_TABLE_INSET`] `tab.rs` gives a
    /// core row -- the two used to carry that number separately. A muted bottom rule in the
    /// palette's own border colour separates the heading from the tree below it.
    ///
    /// Args:
    ///     p: Active palette.
    ///     cx: Application context used for label sizing.
    ///
    /// Returns:
    ///     The one header row, drawn above the virtualized list.
    pub(super) fn conn_col_head_row(p: MoonPalette, cx: &App) -> impl IntoElement {
        // Measured ONCE, as `server_row` does and as `ConnColId::width` asks for -- not
        // once per column.
        let micro = micro_trigger_metrics(cx);
        h_flex()
            .w_full()
            .gap_1()
            .items_center()
            .pl(px(CONN_TABLE_INSET))
            // The same gutter every row of the list below reserves for its overlay scrollbar, so
            // the headings stay over their own columns.
            .pr(design::ui_px(cx, design::MOON_SCROLLBAR_OVERLAY_W))
            .pb(px(3.0))
            .border_b_1()
            .border_color(rgb(p.border))
            .children(ConnColId::ALL.map(|col| Self::col_head(col, micro, p, cx)))
    }
}
