//! The last step of every branch: the connected core, its key's license and its trading mode.
//!
//! The license is only shown (`KernelLicenseState` as the core reports it). The mode switches the
//! core's emulator through its ClientSettings, as the core settings popup does; real trading takes
//! a second, explicit confirmation.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::feed::{ClientSettingsEdit, ConnStatus};
use moon_ui::{
    MoonBadge, MoonButton, MoonButtonVariant, MoonPalette, MoonSegmentItem, MoonSegmentedControl,
    MoonTone, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::ConnectWizardView;
use super::render::note;
use crate::design;

/// Label column of the summary, unscaled.
const LABEL_W: f32 = 130.0;

impl ConnectWizardView {
    /// The connected core `uid`: name, address, status, license and the trading mode switch.
    /// `address` is shown when the wizard knows it; `emulator` is the mode the core reported
    /// over SSH, the fallback until its ClientSettings arrive.
    pub(super) fn done_body(
        &self,
        uid: u64,
        address: Option<String>,
        emulator: Option<bool>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let b = self.backend.read(cx);
        let name = b
            .config
            .servers
            .iter()
            .find(|s| s.uid == uid)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let ready = matches!(b.session.status_map().get(&uid), Some(ConnStatus::Ready));
        let core = b.session.store().core(uid);
        let license = core.and_then(|c| c.license);
        let emu = core
            .and_then(|c| c.client_settings.as_ref())
            .map(|cs| cs.emu_mode)
            .or(emulator);
        let settings_known = core.is_some_and(|c| c.client_settings.is_some());
        let p = MoonPalette::active(cx);
        let row = |label: String, value: AnyElement, cx: &App| {
            h_flex()
                .flex_wrap()
                .gap(design::ui_px(cx, 10.0))
                .items_center()
                .child(
                    div()
                        .w(design::font_w_px(cx, LABEL_W))
                        .text_color(rgba_from(p.text_soft, 1.0))
                        .child(label),
                )
                .child(value)
        };
        let license_value = match license {
            Some(l) => h_flex()
                .flex_wrap()
                .items_center()
                .gap(design::ui_px(cx, 8.0))
                .child(
                    MoonBadge::new(if l.paid_version { "PRO" } else { "Free" })
                        .tone(if l.paid_version {
                            MoonTone::Positive
                        } else {
                            MoonTone::Muted
                        })
                        .render(),
                )
                .child(
                    t!(
                        "connect_wizard.done.license",
                        id = l.reg_id,
                        credits = l.moon_credits
                    )
                    .to_string(),
                )
                .into_any_element(),
            None => note(t!("connect_wizard.done.license_pending"), cx).into_any_element(),
        };
        let mut summary = v_flex()
            .gap(design::ui_px(cx, 6.0))
            .child(row(
                t!("connect_wizard.done.name").to_string(),
                div().child(name).into_any_element(),
                cx,
            ))
            .when_some(address, |s, address| {
                s.child(row(
                    t!("connect_wizard.done.address").to_string(),
                    div()
                        .font_family(design::mono())
                        .child(address)
                        .into_any_element(),
                    cx,
                ))
            })
            .child(row(
                t!("connect_wizard.done.status").to_string(),
                div()
                    .child(if ready {
                        t!("connect_wizard.done.ready").to_string()
                    } else {
                        t!("connect_wizard.done.connecting").to_string()
                    })
                    .into_any_element(),
                cx,
            ))
            .child(row(
                t!("connect_wizard.done.key_license").to_string(),
                license_value,
                cx,
            ));
        summary = summary.child(self.mode_switch(uid, emu, settings_known, cx));
        summary
    }

    /// "Emulator / Real trading": the core's mode, switchable once its ClientSettings are known.
    fn mode_switch(
        &self,
        uid: u64,
        emulator: Option<bool>,
        settings_known: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let view = cx.entity();
        let control = MoonSegmentedControl::new("cw-mode")
            .items([
                MoonSegmentItem::new("", t!("connect_wizard.done.emulator").to_string())
                    .fit_width(cx, 0.0, f32::MAX)
                    .selected(emulator == Some(true))
                    .disabled(!settings_known),
                MoonSegmentItem::new("", t!("connect_wizard.done.real").to_string())
                    .fit_width(cx, 0.0, f32::MAX)
                    .selected(emulator == Some(false))
                    .disabled(!settings_known),
            ])
            .on_click(move |ix, _, _, app| {
                view.update(app, |this, cx| {
                    if ix == 0 {
                        this.real_confirm = false;
                        this.set_emulator(uid, true, cx);
                    } else if emulator != Some(false) {
                        this.real_confirm = true;
                    }
                    cx.notify();
                });
            })
            .render();
        v_flex()
            .gap(design::ui_px(cx, 6.0))
            .pt(design::ui_px(cx, 8.0))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t!("connect_wizard.done.mode").to_string()),
            )
            .child(note(
                if settings_known {
                    t!("connect_wizard.done.mode_hint")
                } else {
                    t!("connect_wizard.done.mode_pending")
                },
                cx,
            ))
            .child(h_flex().child(control))
            .when(self.real_confirm && emulator != Some(false), |s| {
                s.child(note(t!("connect_wizard.done.real_warning"), cx))
                    .child(
                        h_flex().child(
                            MoonButton::new("cw-real-confirm")
                                .variant(MoonButtonVariant::Danger)
                                .size(design::CONTROL_TIER)
                                .label(t!("connect_wizard.done.real_confirm").to_string())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.real_confirm = false;
                                    this.set_emulator(uid, false, cx);
                                }))
                                .render(),
                        ),
                    )
            })
    }

    /// Switch core `uid`'s emulator through its ClientSettings.
    fn set_emulator(&mut self, uid: u64, on: bool, cx: &mut Context<Self>) {
        if let Err(e) = self
            .backend
            .read(cx)
            .session
            .edit_client_settings(uid, ClientSettingsEdit::EmuMode(on))
        {
            log::warn!("connect wizard: emulator switch failed: {e:#}");
            self.error =
                Some(t!("connect_wizard.done.mode_failed", reason = format!("{e:#}")).to_string());
        }
        cx.notify();
    }
}
