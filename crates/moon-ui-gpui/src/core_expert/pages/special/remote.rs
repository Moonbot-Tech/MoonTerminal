//! The live rows of the Special page's Remote block (#1174): the channel binding and the outgoing
//! bot, on the core's `client.telegram()` service API.
//!
//! Moonbot's two ways, both here: `Generate PIN code` and send the PIN into the channel, after which
//! `My Channel` shows its name; or `I have my own bot ID` with that bot's token, saved by its own
//! button. Every control is disabled, with the reason above the block, unless the core sent a
//! CURRENT snapshot that supports these actions — an older core lacks them altogether. The rows of
//! the block the protocol does not carry (`Add @TMoonBot to your channel`, `This bot name`,
//! `Слать отчёты о сделках`, the UDP command port) stay dead in `special.rs`. The window's side —
//! the pending switch, the token box, the requests — lives in `core_expert::remote`.

use gpui::*;
use moon_ui::{
    MoonAlert, MoonButton, MoonButtonVariant, MoonCheckbox, MoonInput, MoonPalette, h_flex, v_flex,
};
use rust_i18n::t;

use moon_core::feed::{CoreTelegramState, ServiceTrack};

use crate::design;

use super::super::super::CoreExpertView;
use super::super::super::remote::{RemoteBlock, RemoteInvalid};
use super::super::super::service::verdict;
use super::super::super::widgets::{caption, hint, text_block};
use super::service::verdict_alert;

/// What the block needs from the window and the store, read before the page is built.
pub(in crate::core_expert) struct RemoteCtx<'a> {
    pub(in crate::core_expert) block: &'a RemoteBlock,
    /// The anchor's Telegram snapshot, current or stale.
    pub(in crate::core_expert) snapshot: Option<&'a CoreTelegramState>,
    /// The snapshot is current and the link is up.
    pub(in crate::core_expert) fresh: bool,
    pub(in crate::core_expert) track: Option<&'a ServiceTrack>,
    /// The anchor's name, for the note shown while several cores are selected.
    pub(in crate::core_expert) core_name: Option<SharedString>,
    /// How many cores the list has selected.
    pub(in crate::core_expert) selected_cores: usize,
}

impl RemoteCtx<'_> {
    /// Whether the outgoing-bot actions may be used right now: a current snapshot from a core that
    /// supports them, and no request of this window in flight — see `core_expert::remote`.
    fn live(&self) -> bool {
        self.fresh
            && self.snapshot.is_some_and(|s| s.settings_supported)
            && self.block.awaiting.is_none()
    }
}

/// The lines above the block: why it is unavailable, what it addresses, how the last request went.
pub(super) fn notices(rm: &RemoteCtx<'_>, p: MoonPalette, cx: &App) -> Vec<AnyElement> {
    let mut out = Vec::new();
    match rm.snapshot {
        // An older core still sends a Telegram snapshot, only without these actions — that, not a
        // missing snapshot, is what names the build it needs.
        Some(s) if !s.settings_supported => out.push(
            MoonAlert::warning(
                "exp-sp-remote-absent",
                t!("core_expert.remote_absent").to_string(),
            )
            .into_any_element(),
        ),
        // Nothing arrived yet: the refresh is on its way, or the link is not up.
        None => out.push(
            MoonAlert::info(
                "exp-sp-remote-waiting",
                t!("core_expert.remote_waiting").to_string(),
            )
            .into_any_element(),
        ),
        Some(_) if !rm.fresh => out.push(
            MoonAlert::warning(
                "exp-sp-remote-stale",
                t!("core_expert.svc_stale").to_string(),
            )
            .into_any_element(),
        ),
        Some(_) => {}
    }
    if rm.selected_cores > 1 {
        let name = rm.core_name.clone().unwrap_or_default();
        out.push(
            text_block(
                t!("core_expert.remote_anchor_only", core = name.as_ref()).to_string(),
                p.amber,
                false,
                cx,
            )
            .into_any_element(),
        );
    }
    if rm.block.queue_failed {
        out.push(
            MoonAlert::error(
                "exp-sp-remote-queue",
                t!("core_expert.svc_not_sent").to_string(),
            )
            .into_any_element(),
        );
    }
    if let Some(RemoteInvalid::OwnBotNeedsToken) = rm.block.invalid {
        out.push(
            MoonAlert::error(
                "exp-sp-remote-invalid",
                t!("core_expert.remote_own_bot_needs_token").to_string(),
            )
            .into_any_element(),
        );
    }
    if let Some(v) = rm
        .track
        .and_then(|track| verdict(rm.block.sent_here, rm.block.awaiting.is_some(), track))
    {
        out.push(verdict_alert("exp-sp-remote-verdict", &v));
    }
    out
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

/// `Generate PIN code`, `Reset channel` and `My Channel`, after Moonbot's add-bot button.
pub(super) fn channel_actions(
    view: &Entity<CoreExpertView>,
    rm: &RemoteCtx<'_>,
    p: MoonPalette,
    cx: &App,
) -> Vec<AnyElement> {
    let live = rm.live();
    let bound = rm
        .snapshot
        .is_some_and(|s| s.channel_id.is_some() || s.channel_name.is_some());
    let channel = rm
        .snapshot
        .and_then(|s| {
            s.channel_name
                .clone()
                .filter(|n| !n.is_empty())
                .or_else(|| s.channel_id.map(|id| id.to_string()))
        })
        .unwrap_or_else(|| "—".to_string());
    vec![
        button(
            "exp-sp-gen-pin",
            "Generate PIN code".to_string(),
            live,
            view,
            |this, cx| this.remote_generate_pin(cx),
        )
        .into_any_element(),
        button(
            "exp-sp-reset-channel",
            "Reset channel".to_string(),
            live && bound,
            view,
            |this, cx| this.remote_reset_channel(cx),
        )
        .into_any_element(),
        v_flex()
            .gap(design::ui_px(cx, 2.0))
            .child(hint(t!("core_expert.sp_my_channel").to_string(), p, cx))
            .child(caption(channel, bound, p, cx))
            .into_any_element(),
    ]
}

/// `I have my own bot ID`, the write-only token box under it and the button that saves the pair.
pub(super) fn own_bot(view: &Entity<CoreExpertView>, rm: &RemoteCtx<'_>, cx: &App) -> AnyElement {
    let live = rm.live();
    let checked = rm.block.own_bot_shown(rm.snapshot);
    let placeholder = if rm.snapshot.is_some_and(|s| s.token_set) {
        t!("core_expert.remote_token_keep").to_string()
    } else {
        t!("core_expert.remote_token_none").to_string()
    };
    let check_view = view.clone();
    let token = rm.block.token.as_ref().map(|state| {
        div().flex_1().min_w_0().child(
            MoonInput::new("exp-sp-bot-token")
                .state(state)
                .size(design::INPUT_SIZE)
                .disabled(!live)
                .placeholder(placeholder)
                .mask_toggle(),
        )
    });
    v_flex()
        .gap(design::ui_px(cx, 2.0))
        .child(
            MoonCheckbox::new("exp-sp-own-bot")
                .label(t!("core_expert.sp_own_bot_id").to_string())
                .checked(checked)
                .disabled(!live)
                .on_change(move |on: &bool, _w, app| {
                    let on = *on;
                    check_view.update(app, |this, cx| this.remote_set_own_bot(on, cx));
                }),
        )
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .children(token)
                .child(button(
                    "exp-sp-save-bot",
                    t!("core_expert.remote_save_bot").to_string(),
                    live,
                    view,
                    |this, cx| this.remote_save_bot(cx),
                )),
        )
        .into_any_element()
}

/// The PIN to type into the channel, with a copy action while the core has supplied one.
pub(super) fn pin(rm: &RemoteCtx<'_>, p: MoonPalette, cx: &App) -> AnyElement {
    let pin = rm
        .snapshot
        .and_then(|s| s.channel_pin.clone())
        .filter(|pin| !pin.is_empty());
    let shown = pin.is_some();
    h_flex()
        .debug_selector(|| "exp-sp-pin-row".to_string())
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .child(
            div()
                .debug_selector(|| "exp-sp-pin-value".to_string())
                .flex_none()
                .font_family(design::mono())
                .child(caption(
                    pin.clone().unwrap_or_else(|| "—".to_string()),
                    shown && rm.fresh,
                    p,
                    cx,
                )),
        )
        .children(pin.map(|pin| {
            div()
                .debug_selector(|| "exp-sp-copy-pin".to_string())
                .flex_none()
                .child(
                    MoonButton::new("exp-sp-copy-pin-button")
                        .xsmall()
                        .mono(false)
                        .label(t!("core_cmds.common.copy").to_string())
                        .variant(MoonButtonVariant::Neutral)
                        // Keep the full localized label beside the PIN at the window minimum.
                        .padding_x(4.0)
                        .on_click(move |_, _window, app| {
                            app.write_to_clipboard(ClipboardItem::new_string(pin.clone()));
                        })
                        .render(),
                )
                .into_any_element()
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests;
