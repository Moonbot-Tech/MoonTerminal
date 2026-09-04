//! Moonbot's "Телеграм" page, control for control — and live where the wire reaches.
//!
//! `moon_core::feed::TelegramSettings` carries the channel list and the four rules over it. The
//! box shows the core's real channels, the primary one first, and the two buttons beside it edit
//! the ADDITIONAL list: the wire keeps the primary channel in a field of its own, and which of the
//! others would take its place is a rule the protocol does not state — so the primary row is shown
//! and cannot be removed here.
//!
//! Moonbot's built-in Telegram client — its switch, its proxy button, its status and "выйти" — is
//! dead, and always will be: the safe-share subset carries no part of it, the same reason the whole
//! Login page is dead. "Отправлять статистику на сервер" is dead for the same reason.

use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use moon_core::feed::{CoreConfig, TelegramSettings};

use crate::design;
use crate::shell::editors::{CoreDraftHost, EditorStore};

use super::super::CoreExpertView;
use super::super::widgets::{
    action, action_live, caption, columns, field, flag, group, hint, list_box_select, rows,
    text_block, text_line,
};

/// See [`super::scratch_specs`].
///
/// The "add a channel" box holds a name on its way INTO the list, not a setting. It is declared
/// here rather than among the staging fields so that typing in it cannot mark the page edited: the
/// button beside it reads the text back out of the control.
pub(super) const SCRATCH_FIELDS: &[&str] = &["exp-tlg-add-channel"];

/// The channel box's rows: the primary channel first, then the additional ones.
///
/// One list because Moonbot shows one. The split matters only to the buttons, which is why the
/// index they work with is an index into THIS list — see the module doc.
fn channel_rows(t: &TelegramSettings) -> Vec<String> {
    let mut rows = Vec::with_capacity(t.pump_channels.len() + 1);
    // Trimmed, not merely non-empty: a primary of spaces would occupy row 0 as a blank the trader
    // cannot remove, and shift every row below it.
    if !t.pump_channel.trim().is_empty() {
        rows.push(t.pump_channel.clone());
    }
    rows.extend(t.pump_channels.iter().cloned());
    rows
}

/// The channel a picked row would remove, as `(index, name)`.
///
/// Both, because neither alone is enough. The pick is made against one render and acted on in a
/// later one, so the core can publish a new list in between: an index would then point at a
/// different channel — the one case a bounds check cannot catch, since it would still be in range.
/// A name alone loses which of two identical entries was picked, and the wire does not forbid the
/// same channel appearing twice.
///
/// The primary is not removable here: which of the others would take its place is a rule the
/// protocol does not state.
fn removable(t: &TelegramSettings, row: Option<usize>) -> Option<(usize, String)> {
    let row = row?;
    let first_extra = usize::from(!t.pump_channel.trim().is_empty());
    let index = row.checked_sub(first_extra)?;
    t.pump_channels.get(index).map(|name| (index, name.clone()))
}

/// Build the page.
pub(super) fn body(
    view: &Entity<CoreExpertView>,
    store: &EditorStore,
    draft: &CoreConfig,
    selected: Option<usize>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let gap = design::ui_px(cx, 8.0);
    let t = &draft.telegram;
    let rows_of_channels = channel_rows(t);
    let to_remove = removable(t, selected);
    let channel_count = rows_of_channels.len();

    // --- Left: the channel box and its remove button ---------------------------------------------
    let left = v_flex()
        .w_full()
        .flex_1()
        .gap(gap)
        .child(list_box_select(
            "exp-tlg-channels",
            rows_of_channels,
            selected,
            t!("core_expert.tlg_channels_empty").to_string(),
            view,
            p,
            cx,
        ))
        .child({
            let view = view.clone();
            action_live(
                "exp-tlg-remove-channel",
                t!("core_expert.tlg_remove_channel").to_string(),
                to_remove.is_some(),
                move |app| {
                    let Some((row, name)) = to_remove.clone() else {
                        return;
                    };
                    view.update(app, |this, cx| {
                        // Matched against the draft this write actually sees, so a list that moved
                        // since the pick removes nothing rather than the wrong channel — and
                        // decided BEFORE staging, so a removal that removes nothing does not mark
                        // the page edited and put its whole section on the wire. The picked row is
                        // preferred over a search, so two identical names remove the one chosen.
                        let at = this.draft.as_ref().and_then(|d| {
                            let list = &d.telegram.pump_channels;
                            if list.get(row).is_some_and(|c| *c == name) {
                                return Some(row);
                            }
                            list.iter().position(|c| *c == name)
                        });
                        if let Some(at) = at {
                            this.edit_draft(
                                |d| {
                                    d.telegram.pump_channels.remove(at);
                                },
                                cx,
                            );
                        }
                        this.set_selected_channel(None, cx);
                    });
                },
            )
        });

    // --- Right: adding a channel, the channel rules, and Moonbot's own Telegram client -----------
    let client = group(
        "exp-tlg-client",
        t!("core_expert.tlg_client_frame").to_string(),
    )
    .child(
        rows(cx)
            .gap(gap)
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 10.0))
                    .child(flag(
                        "exp-tlg-client-on",
                        t!("core_expert.tlg_client_enable").to_string(),
                        false,
                        false,
                        view,
                        |_, _| {},
                    ))
                    .child(div().flex_1())
                    .child(action(
                        "exp-tlg-proxy",
                        t!("core_expert.tlg_proxy").to_string(),
                        false,
                    )),
            )
            .child(caption(
                t!("core_expert.tlg_client_status", v = "—").to_string(),
                false,
                p,
                cx,
            ))
            .child(caption(
                t!("core_expert.tlg_log_out").to_string(),
                false,
                p,
                cx,
            )),
    );

    let right = rows(cx)
        .gap(gap)
        .child({
            let view = view.clone();
            let box_state = store.input("exp-tlg-add-channel");
            let typed = box_state
                .as_ref()
                .is_some_and(|state| !state.read(cx).value().trim().is_empty());
            action_live(
                "exp-tlg-add-channel-btn",
                t!("core_expert.tlg_add_channel").to_string(),
                typed,
                move |app| {
                    let Some(state) = box_state.clone() else {
                        return;
                    };
                    // Read back out of the control: the box holds a name on its way into the list,
                    // so there is no draft field for it to have staged into.
                    // The "@" Moonbot prints in front of the box is a caption, not part of the
                    // name: typing it as well would put "@name" on a wire that holds "name".
                    let name = state
                        .read(app)
                        .value()
                        .trim()
                        .trim_start_matches('@')
                        .trim()
                        .to_string();
                    if name.is_empty() {
                        return;
                    }
                    view.update(app, |this, cx| {
                        // Decided before staging: the same channel twice is not a second source, and
                        // an add that adds nothing must not mark the page edited.
                        // `None` rather than `is_some_and`: with no draft there is nothing to add
                        // to, and clearing the box below would then discard what was typed.
                        let Some(known) = this.draft.as_ref().map(|d| {
                            d.telegram.pump_channel == name
                                || d.telegram.pump_channels.contains(&name)
                        }) else {
                            return;
                        };
                        if known {
                            return;
                        }
                        let name_to_add = name.clone();
                        this.edit_draft(
                            |d| {
                                // A core with no primary channel takes the first one added as its
                                // primary: the wire uses `pump_channels` for MULTI-channel mode, so
                                // a name added there while the primary is empty would sit in a list
                                // the core only reads with multi-channel on.
                                if d.telegram.pump_channel.trim().is_empty() {
                                    d.telegram.pump_channel = name_to_add;
                                } else {
                                    d.telegram.pump_channels.push(name_to_add);
                                }
                            },
                            cx,
                        );
                        // Filling an empty primary inserts a row ABOVE the extras, so a pick made
                        // before it would now name the channel below.
                        this.set_selected_channel(None, cx);
                        // The name has landed in the list, so the box that carried it is emptied —
                        // otherwise the next press would be refused as a duplicate with no word
                        // said. Deferred through the window handle because `set_value` needs a
                        // `&mut Window`, which a click handler does not have.
                        let handle = this.editor_window();
                        let box_state = state.clone();
                        cx.defer(move |app| {
                            let _ = handle.update(app, move |_, window, app| {
                                box_state
                                    .update(app, |st, c| st.set_value(String::new(), window, c));
                            });
                        });
                    });
                },
            )
        })
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .child(caption("@".to_string(), true, p, cx))
                .child(
                    div()
                        .flex_1()
                        .children(field(store, "exp-tlg-add-channel", true)),
                ),
        )
        .child(
            h_flex()
                .w_full()
                .items_start()
                .gap(design::ui_px(cx, 12.0))
                .child(div().flex_1().min_w_0().child(flag(
                    "exp-tlg-multi",
                    t!("core_expert.tlg_multi_channels").to_string(),
                    t.multi_channels,
                    true,
                    view,
                    |d, on| d.telegram.multi_channels = on,
                )))
                .child(div().flex_1().min_w_0().child(flag(
                    "exp-tlg-two-channels",
                    t!("core_expert.tlg_two_channels").to_string(),
                    t.more_then_1_channel,
                    true,
                    view,
                    |d, on| d.telegram.more_then_1_channel = on,
                ))),
        )
        .child(
            h_flex()
                .w_full()
                .items_start()
                .gap(design::ui_px(cx, 12.0))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(design::ui_px(cx, 2.0))
                        .child(flag(
                            "exp-tlg-premium",
                            t!("core_expert.tlg_premium").to_string(),
                            t.listen_moon_channel,
                            true,
                            view,
                            |d, on| d.telegram.listen_moon_channel = on,
                        ))
                        .child(hint(t!("core_expert.tlg_premium_paid").to_string(), p, cx)),
                )
                .child(div().flex_1().min_w_0().child(flag(
                    "exp-tlg-send-stats",
                    t!("core_expert.tlg_send_stats").to_string(),
                    false,
                    false,
                    view,
                    |_, _| {},
                ))),
        )
        .child(flag(
            "exp-tlg-network-bl",
            t!("core_expert.tlg_network_blacklist").to_string(),
            t.use_moon_bl,
            true,
            view,
            |d, on| d.telegram.use_moon_bl = on,
        ))
        // Moonbot prints this warning in its own alarm colour; it is an instruction about ITS Login
        // page, so it is reproduced as text rather than turned into anything this window can act on.
        .child(text_block(
            t!("core_expert.tlg_stats_consent").to_string(),
            design::danger_color(p),
            false,
            cx,
        ))
        .child(client);

    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 10.0))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 12.0))
                .child(caption(
                    t!(
                        "core_expert.tlg_signal_channels",
                        v = channel_count.to_string()
                    )
                    .to_string(),
                    false,
                    p,
                    cx,
                ))
                .child(div().flex_1())
                .child(text_line(
                    t!("core_expert.gen_need_help").to_string(),
                    design::positive_color(p),
                    false,
                    cx,
                )),
        )
        .child(columns(left, right, cx))
        .into_any_element()
}
