//! Inline confirmations for address changes, forgetting and removing a station. Pending
//! destructive actions retain the exact host the user saw and never silently switch targets.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_remote::hosts::{Host, Hosts};
use moon_remote::ssh::Target;
use moon_ui::{MoonButton, MoonInput, MoonInputState, MoonPalette, h_flex, rgba_from, v_flex};
use rust_i18n::t;

use super::super::SettingsView;
use super::server_bot::parse_target;
use crate::backend::station::job::Job;
use crate::design;

/// One exact destructive action awaiting an inline second click.
enum Confirmation {
    Forget(Host),
    Remove(Host),
}

/// Session-only editors; none of these choices persist through Settings Save.
pub(super) struct AccessEd {
    address: Entity<MoonInputState>,
    editing: bool,
    confirmation: Option<Confirmation>,
}

impl AccessEd {
    /// Create an address input without reading or mutating any server.
    pub(super) fn new<T: 'static>(window: &mut Window, cx: &mut Context<T>) -> Self {
        Self {
            address: cx.new(|cx| MoonInputState::new(window, cx)),
            editing: false,
            confirmation: None,
        }
    }

    /// Close all local confirmations after cancellation or losing the known host.
    pub(super) fn clear(&mut self) {
        self.editing = false;
        self.confirmation = None;
    }
}

impl SettingsView {
    /// Start address editing from the known address, discarding a previous probe's consent.
    pub(super) fn station_address_begin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.backend.read(cx).station.running {
            return;
        }
        if let Some(reason) = self.backend.read(cx).station.access_refusal() {
            self.server_bot_run(Err(reason), cx);
            return;
        }
        let Some(target) = self.telegram.server.known() else {
            return;
        };
        let addr = target.addr();
        let ed = &mut self.telegram.server.station_access;
        ed.clear();
        ed.editing = true;
        ed.address
            .update(cx, |st, cx| st.set_value(addr, window, cx));
        self.station_address_cancel_probe(cx);
        cx.notify();
    }

    /// Clear only a pending fingerprint; a cancelled dialog grants no connection permission.
    fn station_address_cancel_probe(&mut self, cx: &mut Context<Self>) {
        self.backend.update(cx, |b, cx| {
            b.station.address_change = None;
            b.station.revision = b.station.revision.wrapping_add(1);
            cx.notify();
        });
    }

    /// Load the exact host behind this UI; missing or unreadable state is a visible refusal.
    fn station_access_source(&self) -> Result<Host, String> {
        let target = self
            .telegram
            .server
            .known()
            .ok_or_else(|| t!("telegram.server.no_known_server").to_string())?;
        Hosts::load(&Hosts::path())
            .map_err(|e| crate::backend::station::text::error(&e))?
            .get(&target.addr())
            .cloned()
            .ok_or_else(|| t!("telegram.server.no_known_server").to_string())
    }

    /// Arm a destructive confirmation without changing the server or saved host record.
    fn station_access_confirm(&mut self, remove: bool, cx: &mut Context<Self>) {
        if self.backend.read(cx).station.running {
            return;
        }
        if let Some(reason) = self.backend.read(cx).station.access_refusal() {
            self.server_bot_run(Err(reason), cx);
            return;
        }
        match self.station_access_source() {
            Ok(source) => {
                let ed = &mut self.telegram.server.station_access;
                ed.clear();
                ed.confirmation = Some(if remove {
                    Confirmation::Remove(source)
                } else {
                    Confirmation::Forget(source)
                });
                self.station_address_cancel_probe(cx);
            }
            Err(reason) => self.server_bot_run(Err(reason), cx),
        }
        cx.notify();
    }

    /// Separate dangerous, always-visible actions from maintenance and keep inline consent intact.
    pub(super) fn station_access_block(
        &self,
        target: &Target,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let backend = self.backend.read(cx);
        let refusal = backend.station.access_refusal();
        let busy = backend.station.running || refusal.is_some();
        let ed = &self.telegram.server.station_access;
        let change = backend
            .station
            .address_change
            .clone()
            .filter(|change| change.source.addr == target.addr());
        let has_bot = backend.station.bot.as_ref().is_none_or(|bot| bot.has_token);
        let p = MoonPalette::active(cx);
        let button = |id, key| {
            MoonButton::new(id)
                .label(t!(key).to_string())
                .size(design::CONTROL_TIER)
                .disabled(busy)
        };
        let mut block =
            v_flex()
                .gap(design::ui_px(cx, 8.0))
                .pt(design::ui_px(cx, 10.0))
                .border_t_1()
                .border_color(rgba_from(p.border, 1.0))
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap(design::ui_px(cx, 8.0))
                        .child(
                            button("station-forget", "telegram.server.forget")
                                .danger()
                                .tooltip(t!("telegram.server.forget_hint").to_string())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.station_access_confirm(false, cx)
                                }))
                                .render(),
                        )
                        .child(
                            button("station-remove", "telegram.server.remove")
                                .danger()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.station_access_confirm(true, cx)
                                }))
                                .render(),
                        ),
                );
        if let Some(reason) = refusal {
            block = block.child(div().text_color(rgba_from(p.red_text, 1.0)).child(reason));
        }
        if ed.editing {
            block = block
                .child(div().child(t!("telegram.server.address_hint").to_string()))
                .child(
                    div().w_full().max_w(design::font_w_px(cx, 300.0)).child(
                        MoonInput::new("station-new-address")
                            .state(&ed.address)
                            .size(design::INPUT_SIZE),
                    ),
                )
                .child(
                    button("station-address-probe", "telegram.server.address_probe")
                        .on_click(cx.listener(|this, _, _, cx| {
                            let text = this
                                .telegram
                                .server
                                .station_access
                                .address
                                .read(cx)
                                .value()
                                .to_string();
                            let job = this.station_access_source().and_then(|source| {
                                let target = parse_target(&text)
                                    .ok_or_else(|| t!("telegram.server.need_host").to_string())?;
                                Ok(Job::AddressProbe { source, target })
                            });
                            this.server_bot_run(job, cx);
                        }))
                        .render(),
                );
        }
        if let Some(change) = change {
            block = block
                .child(
                    div().child(
                        t!(
                            "telegram.server.address_fingerprint",
                            addr = change.target.addr(),
                            fingerprint = change.fingerprint.clone()
                        )
                        .to_string(),
                    ),
                )
                .child(
                    button("station-address-confirm", "telegram.server.address_confirm")
                        .primary()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            // The captured candidate is exactly the fingerprint displayed by this button.
                            this.telegram.server.station_access.clear();
                            this.server_bot_run(
                                Ok(Job::AddressChange {
                                    change: change.clone(),
                                }),
                                cx,
                            );
                        }))
                        .render(),
                );
        }
        if let Some(confirmation) = &ed.confirmation {
            let (remove, source) = match confirmation {
                Confirmation::Forget(h) => (false, h),
                Confirmation::Remove(h) => (true, h),
            };
            let source = source.clone();
            block = block
                .child(
                    div().text_color(rgba_from(p.red_text, 1.0)).child(
                        t!(
                            if remove {
                                "telegram.server.remove_confirm"
                            } else {
                                "telegram.server.forget_confirm"
                            },
                            addr = source.addr.clone()
                        )
                        .to_string(),
                    ),
                )
                .when(remove && has_bot, |block| {
                    block.child(div().child(t!("telegram.server.remove_bot_confirm").to_string()))
                })
                .child(
                    button("station-destructive-confirm", "telegram.server.confirm")
                        .primary()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.telegram.server.station_access.clear();
                            if remove {
                                this.server_bot_run(
                                    Ok(Job::Remove {
                                        source: source.clone(),
                                    }),
                                    cx,
                                );
                            } else {
                                this.server_bot_forget(source.clone(), cx);
                            }
                        }))
                        .render(),
                );
        }
        block.when(
            ed.editing || ed.confirmation.is_some() || backend.station.address_change.is_some(),
            |block| {
                block.child(
                    button("station-access-cancel", "telegram.server.cancel")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.telegram.server.station_access.clear();
                            this.station_address_cancel_probe(cx);
                            cx.notify();
                        }))
                        .render(),
                )
            },
        )
    }
}
