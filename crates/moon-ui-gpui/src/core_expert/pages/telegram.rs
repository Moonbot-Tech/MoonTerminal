//! Moonbot's "Телеграм" page, control for control.
//!
//! Nothing here is live yet. The values behind it — the signal channels, the multi-channel rules,
//! the Moon Premium and network-blacklist switches — DO cross the wire in the safe-share `signals`
//! section, but `moon_core::feed::CoreConfig` does not project them and
//! `ExpertTab::add_sections` would not carry them back. Drawing them live would promise an OK
//! that cannot deliver, so the page is drawn whole and disabled until both are widened.
//!
//! The channel box is therefore EMPTY rather than filled with a guess, and says why in place of the
//! list Moonbot shows there.

use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use moon_core::feed::CoreConfig;

use crate::design;
use crate::shell::editors::EditorStore;

use super::super::CoreExpertView;
use super::super::widgets::{
    action, caption, columns, field, flag, group, hint, list_box, rows, text_block, text_line,
};

/// Nothing on this page reaches the draft.
const DEAD: fn(&mut CoreConfig, &str) = |_, _| {};

/// See [`super::field_specs`].
#[allow(clippy::type_complexity)]
pub(super) fn field_specs(
    _draft: &CoreConfig,
) -> Vec<(&'static str, String, fn(&mut CoreConfig, &str))> {
    vec![("exp-tlg-add-channel", String::new(), DEAD)]
}

/// Build the page.
pub(super) fn body(
    view: &Entity<CoreExpertView>,
    store: &EditorStore,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let gap = design::ui_px(cx, 8.0);

    // --- Left: the channel box and its remove button ---------------------------------------------
    let left = v_flex()
        .w_full()
        .flex_1()
        .gap(gap)
        .child(list_box(
            "exp-tlg-channels",
            Vec::new(),
            t!("core_expert.tlg_channels_empty").to_string(),
            p,
            cx,
        ))
        .child(action(
            "exp-tlg-remove-channel",
            t!("core_expert.tlg_remove_channel").to_string(),
            false,
        ));

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
        .child(action(
            "exp-tlg-add-channel-btn",
            t!("core_expert.tlg_add_channel").to_string(),
            false,
        ))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .child(caption("@".to_string(), false, p, cx))
                .child(
                    div()
                        .flex_1()
                        .children(field(store, "exp-tlg-add-channel", false)),
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
                    false,
                    false,
                    view,
                    |_, _| {},
                )))
                .child(div().flex_1().min_w_0().child(flag(
                    "exp-tlg-two-channels",
                    t!("core_expert.tlg_two_channels").to_string(),
                    false,
                    false,
                    view,
                    |_, _| {},
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
                            false,
                            false,
                            view,
                            |_, _| {},
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
            false,
            false,
            view,
            |_, _| {},
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
                    t!("core_expert.tlg_signal_channels", v = "—").to_string(),
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
