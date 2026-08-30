//! The rendered run cell: a fixed-width row of slots that never changes size with its content.
//!
//! Size stability is the whole reason the slots are drawn rather than skipped. A button that
//! appears when a core stops would move every name in the table at the moment the user most wants
//! to read it, so an idle slot draws a dot (or nothing) at exactly the width the busy slot takes.
//! A line with nothing to command at all takes [`reserved_cell`], which is the same width empty.
//!
//! Every button STOPS PROPAGATION. These cells sit inside table lines that are themselves click
//! targets — in the Profit Monitor a row click re-filters every panel in the main window — and a
//! `MoonButton` only occludes its parent while it is disabled.

use std::time::Instant;

use gpui::*;
use moon_core::session::{AutoAction, CoreId, RunSummary, TradingAction};
use moon_ui::{MoonButtonIconSlot, MoonButtonVariant, MoonPalette, h_flex};
use rust_i18n::t;

use super::pending::RunHalf;
use super::{RunScope, RunSlot, RunSlots, SLOT_W, actions};
use crate::Backend;
use crate::design;

/// Render one run cell for a core row, a group caption, or an exchange row.
///
/// Args:
///     scope: What this cell stands for and how much width its table reserves.
///     backend: Shared terminal state, read for the run state and written by the buttons.
///     palette: Active MoonUI palette.
///     cx: Application context used to read state and scale geometry.
///
/// Returns:
///     The fixed-width cell, or nothing when the column is switched off.
pub(crate) fn run_cell(
    scope: &RunScope,
    backend: &Entity<Backend>,
    palette: MoonPalette,
    cx: &App,
) -> Option<AnyElement> {
    if !scope.reserve.any() {
        return None;
    }
    // Read ONCE for every slot, and once per cell: this runs inside a virtual-list item builder the
    // hosting table can drive at 10 Hz, and each core costs a store lookup.
    let state = CellState::read(scope, backend, cx);
    let mut cell = h_flex()
        .flex_none()
        .w(scope.reserve.width_px(cx))
        .gap(design::ui_px(cx, super::SLOT_GAP))
        .items_center();
    // Reserved decides the GEOMETRY, offered decides the content: a line that fills no slot at all
    // still holds them open, or its name would start left of the lines around it.
    if scope.reserve.status {
        cell = cell.child(if scope.offers.status {
            status_slot(scope, &state, backend, palette, cx)
        } else {
            slot_frame(cx).into_any_element()
        });
    }
    if scope.reserve.trading {
        cell = cell.child(if scope.offers.trading {
            trading_slot(scope, &state, backend, palette, cx)
        } else {
            slot_frame(cx).into_any_element()
        });
    }
    if scope.reserve.auto {
        cell = cell.child(if scope.offers.auto {
            auto_slot(scope, &state, backend, palette, cx)
        } else {
            slot_frame(cx).into_any_element()
        });
    }
    Some(cell.into_any_element())
}

/// Render the column's width with nothing in it.
///
/// For the lines that command nothing at all — a summary footer, a fold — which still have to start
/// their label where the names above them start.
///
/// Args:
///     slots: Slots the table reserves.
///     cx: Application context supplying the scaled width.
///
/// Returns:
///     The empty reservation, or nothing when the column is off.
pub(crate) fn reserved_cell(slots: RunSlots, cx: &App) -> Option<AnyElement> {
    slots
        .any()
        .then(|| div().flex_none().w(slots.width_px(cx)).into_any_element())
}

/// Everything the slots need, read once per cell.
struct CellState {
    /// The scope's folded run state.
    summary: RunSummary,
    /// The core this cell may restart, and whether that claim is confirmed by this connection.
    restart: Option<(CoreId, bool)>,
    /// Whether the status slot is waiting on a restart it sent.
    waiting_restart: bool,
    /// Whether the trading slot is waiting on a start/stop it sent, anywhere in scope.
    waiting_trading: bool,
    /// Whether the AutoDetect slot is waiting on a flip it sent, anywhere in scope.
    waiting_auto: bool,
}

impl CellState {
    /// Read the scope's state and pending intents in one pass.
    ///
    /// One store lookup per core, and the pending register is consulted only when something is
    /// actually outstanding — the usual case is an empty register, and asking it costs a clock
    /// read per cell. Each slot's wait is tracked separately, because a core can be waiting on all
    /// three at once and a slot may only show the wait it owns.
    ///
    /// Args:
    ///     scope: The cell's scope.
    ///     backend: Shared terminal state.
    ///     cx: Application context used to read the session.
    ///
    /// Returns:
    ///     Everything the slots decide from.
    fn read(scope: &RunScope, backend: &Entity<Backend>, cx: &App) -> Self {
        let backend = backend.read(cx);
        let now = (!backend.run_pending.is_empty()).then(Instant::now);
        let mut summary = RunSummary::default();
        let mut waiting_restart = false;
        let mut waiting_trading = false;
        let mut waiting_auto = false;
        let mut first = None;
        // Whether this cell stands for many cores, which decides whose waiting face it may show.
        let group = scope.cores.len() > 1;
        // Loop-invariant, and each one also gates the probe below: a slot this cell never draws
        // costs no register lookup, and neither does one that is already known to be waiting.
        let may_restart = scope.allows_restart();
        let probe_trading = scope.reserve.trading && scope.offers.trading;
        let probe_auto = scope.reserve.auto && scope.offers.auto;
        for core in scope.cores.iter() {
            let state = backend.session.core_run_state(*core);
            summary.add(state);
            first.get_or_insert((*core, state));
            if let Some(now) = now {
                let pending = &backend.run_pending;
                // A single-core control shows ANY outstanding ask for its core — including one a
                // group press armed, or it would stay pressable and re-send the same command. A
                // control standing for MANY shows only what it armed itself, matched by identity:
                // one row's press must not blank the group control nobody touched, and a caption
                // and the table-wide heading overlap on the same cores while being two controls.
                let waits = |half| {
                    pending
                        .active(*core, half, state, now)
                        .is_some_and(|ask| !group || ask.from == scope.key)
                };
                // The restart slot is the exception: it exists only on a cell standing for one
                // core, so there is no second control whose press it could be showing.
                if may_restart && !waiting_restart {
                    waiting_restart = pending
                        .active(*core, RunHalf::Runtime, state, now)
                        .is_some();
                }
                if probe_trading && !waiting_trading {
                    waiting_trading = waits(RunHalf::Trading);
                }
                if probe_auto && !waiting_auto {
                    waiting_auto = waits(RunHalf::Auto);
                }
            }
        }
        Self {
            summary,
            // A caption standing for six cores has no single runtime to restart, and the protocol
            // has no "restart these six" to offer. A table-wide cell declines it outright — see
            // `RunScope::allows_restart`.
            restart: first
                .filter(|_| may_restart)
                .filter(|(_, state)| state.needs_restart())
                .map(|(core, state)| (core, state.started_confirmed)),
            waiting_restart,
            waiting_trading,
            waiting_auto,
        }
    }
}

/// Build the leading slot: the runtime dot, or the restart button for a single stopped core.
///
/// Args:
///     scope: The cell's scope.
///     state: State read once for this cell.
///     backend: Shared terminal state the button commands.
///     palette: Active MoonUI palette.
///     cx: Application context used to scale geometry.
///
/// Returns:
///     The slot's element.
fn status_slot(
    scope: &RunScope,
    state: &CellState,
    backend: &Entity<Backend>,
    palette: MoonPalette,
    cx: &App,
) -> AnyElement {
    if state.waiting_restart {
        return waiting_slot(scope, RunSlot::Status, palette, cx);
    }
    if let Some((core, confirmed)) = state.restart {
        // WEAK, and taken once per drawn button: `Entity::clone` takes the process entity map's
        // lock, and this runs inside a virtual-list item builder. A click can also outlive the
        // window it was made in.
        let target = backend.downgrade();
        let tip = t!("core_run.restart");
        // The stopped claim may itself predate a reconnect nobody re-reported through. The button
        // stays — it is the only way back for a genuinely stopped core — but it is drawn faded and
        // says what it is based on: `restart_now` also leaves passive mode and starts checked
        // strategies, so pressing it on a guess is a trade action taken on a guess.
        let icon = MoonButtonIconSlot::new("icons/redo-2.svg").color(palette.amber);
        return guarded(
            crate::panels::micro_icon_button(
                scope.slot_element_id(RunSlot::Status),
                if confirmed {
                    icon
                } else {
                    icon.alpha(design::STALE_ALPHA)
                },
                if confirmed {
                    tip.to_string()
                } else {
                    t!("core_run.unconfirmed", state = tip).to_string()
                },
                MoonButtonVariant::Ghost,
                design::ui_value(cx, SLOT_W),
                move |_window, app| {
                    app.stop_propagation();
                    if let Some(backend) = target.upgrade() {
                        actions::restart(&backend, core, app);
                    }
                },
            ),
            cx,
        );
    }
    // Dot colours, in the order they are decided: nothing reachable is muted (we know nothing), a
    // reported stop inside the scope is amber (a group with one core down), a reported start is
    // positive, and a core that has said nothing yet stays muted.
    let summary = state.summary;
    // Colour, tooltip, and whether the very state being described is unconfirmed — decided
    // together, so the fade can never belong to a different core than the dot does.
    let (color, tip, stale) = if summary.online == 0 {
        (palette.text_muted, t!("core_run.status_offline"), false)
    } else if summary.stopped > 0 {
        (
            palette.amber,
            if scope.cores.len() > 1 {
                t!("core_run.status_stopped_group", n = summary.stopped)
            } else {
                t!("core_run.status_stopped")
            },
            summary.stopped_stale > 0,
        )
    } else if summary.started_on > 0 {
        (
            design::positive_color(palette),
            t!("core_run.status_started"),
            summary.started_on_stale > 0,
        )
    } else {
        (palette.text_muted, t!("core_run.status_unknown"), false)
    };
    slot_frame(cx)
        .id(scope.slot_element_id(RunSlot::Status))
        .tooltip(crate::panels::common::text_tooltip(if stale {
            t!("core_run.unconfirmed", state = tip).to_string()
        } else {
            tip.to_string()
        }))
        .child(if stale {
            design::status_dot_stale(color, cx).into_any_element()
        } else {
            design::status_dot(color, cx).into_any_element()
        })
        .into_any_element()
}

/// Build the middle slot: start or stop the strategy engine across the scope.
///
/// Args:
///     scope: The cell's scope.
///     state: State read once for this cell.
///     backend: Shared terminal state the button commands.
///     palette: Active MoonUI palette.
///     cx: Application context used to scale geometry.
///
/// Returns:
///     The slot's element.
fn trading_slot(
    scope: &RunScope,
    state: &CellState,
    backend: &Entity<Backend>,
    palette: MoonPalette,
    cx: &App,
) -> AnyElement {
    if state.waiting_trading {
        return waiting_slot(scope, RunSlot::Trading, palette, cx);
    }
    let summary = state.summary;
    let many = scope.cores.len() > 1;
    // Nothing reported leaves an empty slot rather than a button that would fire blind at cores
    // which may already be trading. That covers an unreachable scope too: an offline core reports
    // nothing, so a scope with none of its cores connected can only be Unknown.
    //
    // The count in a group tooltip is what the press REACHES — every connected core not already in
    // the asked-for state — and the fade follows the cores that decided the offered action, never
    // an unrelated one in the same group.
    let (icon, color, tip, start, stale) = match summary.trading_action() {
        TradingAction::Unknown => return slot_frame(cx).into_any_element(),
        TradingAction::Stop => (
            "icons/pause.svg",
            design::danger_color(palette),
            if many {
                t!("core_run.trading_stop_group", n = summary.needing_stop)
            } else {
                t!("core_run.trading_stop")
            },
            false,
            summary.trading_on_stale > 0,
        ),
        // A scope where some cores trade and some do not: the press starts the rest, and the
        // tooltip says how many rather than leaving it to be guessed from a glyph.
        TradingAction::Start if summary.trading_mixed() => (
            "icons/play.svg",
            palette.amber,
            t!("core_run.trading_start_partial", n = summary.needing_start),
            true,
            summary.trading_off_stale > 0,
        ),
        TradingAction::Start => (
            "icons/play.svg",
            design::positive_color(palette),
            if many {
                t!("core_run.trading_start_group", n = summary.needing_start)
            } else {
                t!("core_run.trading_start")
            },
            true,
            summary.trading_off_stale > 0,
        ),
    };
    // Weak for the same two reasons as the restart button above.
    let target = backend.downgrade();
    let cores = scope.cores.clone();
    let key = scope.key;
    // An unconfirmed value still decides what the button offers — a control that went blank on
    // every reconnect is the regression this replaced — but the tooltip says the core has not
    // re-reported since.
    let icon = MoonButtonIconSlot::new(icon).color(color);
    guarded(
        crate::panels::micro_icon_button(
            scope.slot_element_id(RunSlot::Trading),
            if stale {
                icon.alpha(design::STALE_ALPHA)
            } else {
                icon
            },
            if stale {
                t!("core_run.unconfirmed", state = tip).to_string()
            } else {
                tip.to_string()
            },
            MoonButtonVariant::Ghost,
            design::ui_value(cx, SLOT_W),
            move |_window, app| {
                app.stop_propagation();
                if let Some(backend) = target.upgrade() {
                    actions::set_trading(&backend, &cores, key, start, app);
                }
            },
        ),
        cx,
    )
}

/// Build the trailing slot: turn AutoDetect on or off across the scope.
///
/// Drawn as the STATE rather than as the action — a green ring when the scope is detecting, a red
/// one when it is passive — because AutoDetect is a two-way switch with no natural "next step"
/// glyph, and because this is the one run control a user reads at a glance across a whole table.
/// What the press will do is in the tooltip, the way every other toggle in this window says it.
///
/// The amber face means a SPLIT scope, which a single-core cell can never show; the core-settings
/// popup spends its own amber on "passive on a running core", a state this slot paints red because
/// it has a second colour to spend and that popup does not.
///
/// Args:
///     scope: The cell's scope.
///     state: State read once for this cell.
///     backend: Shared terminal state the button commands.
///     palette: Active MoonUI palette.
///     cx: Application context used to scale geometry.
///
/// Returns:
///     The slot's element.
fn auto_slot(
    scope: &RunScope,
    state: &CellState,
    backend: &Entity<Backend>,
    palette: MoonPalette,
    cx: &App,
) -> AnyElement {
    if state.waiting_auto {
        return waiting_slot(scope, RunSlot::Auto, palette, cx);
    }
    let summary = state.summary;
    let many = scope.cores.len() > 1;
    // Nothing reported leaves an empty slot rather than a switch whose face would be a guess — the
    // same rule the trading slot follows, and the same reason: an offline scope reports nothing.
    let (icon, color, tip, on, stale) = match summary.auto_action() {
        AutoAction::Unknown => return slot_frame(cx).into_any_element(),
        AutoAction::Disable => (
            "icons/circle-check.svg",
            design::positive_color(palette),
            if many {
                t!("core_run.auto_off_group", n = summary.needing_auto_off)
            } else {
                t!("core_run.auto_off")
            },
            false,
            summary.auto_on_stale > 0,
        ),
        // Part of the scope detects and part does not: the press turns the rest on, and the amber
        // says the face describes a split state rather than a uniform one.
        AutoAction::Enable if summary.auto_mixed() => (
            "icons/circle-x.svg",
            palette.amber,
            t!("core_run.auto_on_partial", n = summary.needing_auto_on),
            true,
            summary.auto_off_stale > 0,
        ),
        AutoAction::Enable => (
            "icons/circle-x.svg",
            design::danger_color(palette),
            if many {
                t!("core_run.auto_on_group", n = summary.needing_auto_on)
            } else {
                t!("core_run.auto_on")
            },
            true,
            summary.auto_off_stale > 0,
        ),
    };
    // Weak for the same two reasons as the buttons above.
    let target = backend.downgrade();
    let cores = scope.cores.clone();
    let key = scope.key;
    let icon = MoonButtonIconSlot::new(icon).color(color);
    guarded(
        crate::panels::micro_icon_button(
            scope.slot_element_id(RunSlot::Auto),
            if stale {
                icon.alpha(design::STALE_ALPHA)
            } else {
                icon
            },
            if stale {
                t!("core_run.unconfirmed", state = tip).to_string()
            } else {
                tip.to_string()
            },
            MoonButtonVariant::Ghost,
            design::ui_value(cx, SLOT_W),
            move |_window, app| {
                app.stop_propagation();
                if let Some(backend) = target.upgrade() {
                    actions::set_auto_detect(&backend, &cores, key, on, app);
                }
            },
        ),
        cx,
    )
}

/// Wrap a button so neither its click nor the press under it reaches the line it sits on.
///
/// The click handler stops propagation itself; this covers the mouse-down, which the hosting row
/// also listens for. Same pairing the tuner's per-row strategy button uses.
///
/// Args:
///     button: The rendered button.
///     cx: Application context used to scale the wrapper.
///
/// Returns:
///     The guarded button.
fn guarded(button: impl IntoElement, cx: &App) -> AnyElement {
    div()
        .flex_none()
        .w(design::ui_px(cx, SLOT_W))
        .on_mouse_down(MouseButton::Left, |_, _, app| app.stop_propagation())
        .child(button)
        .into_any_element()
}

/// Render the slot a sent-but-unanswered intent occupies.
///
/// A dot in the palette's soft text colour, not a spinner: an animated element here would repaint
/// the hosting table at frame rate for the whole round trip, in a window whose repaints are
/// deliberately gated.
///
/// Args:
///     scope: The cell's scope, supplying the stable element identity.
///     slot: Which slot is waiting.
///     palette: Active MoonUI palette.
///     cx: Application context used to scale geometry.
///
/// Returns:
///     The waiting slot.
fn waiting_slot(scope: &RunScope, slot: RunSlot, palette: MoonPalette, cx: &App) -> AnyElement {
    // Guarded like the button it stands in for: the pixel under the pointer was eating clicks a
    // moment ago, and a press during the wait must not silently become a row click instead.
    guarded(
        slot_frame(cx)
            .id(scope.slot_element_id(slot))
            .tooltip(crate::panels::common::text_tooltip(
                t!("core_run.waiting").to_string(),
            ))
            .child(design::status_dot(palette.text_soft, cx)),
        cx,
    )
}

/// The empty frame every slot occupies, so content changes never move the column.
///
/// Args:
///     cx: Application context supplying the scaled width.
///
/// Returns:
///     The empty slot.
fn slot_frame(cx: &App) -> Div {
    div()
        .flex_none()
        .w(design::ui_px(cx, SLOT_W))
        .flex()
        .items_center()
        .justify_center()
}
