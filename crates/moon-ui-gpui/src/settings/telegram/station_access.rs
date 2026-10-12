//! The station's access on the Maintenance page: the address change with its inline fingerprint
//! check, and the danger zone whose Forget and Remove run only through a confirmation dialog.
//! The dialog captures the exact host the user saw, so it never silently switches targets.

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

/// Session-only editors; none of these choices persist through Settings Save.
pub(super) struct AccessEd {
    address: Entity<MoonInputState>,
    editing: bool,
}

impl AccessEd {
    /// Create an address input without reading or mutating any server.
    pub(super) fn new<T: 'static>(window: &mut Window, cx: &mut Context<T>) -> Self {
        Self {
            address: cx.new(|cx| MoonInputState::new(window, cx)),
            editing: false,
        }
    }

    /// Close the address editor after cancellation or losing the known host.
    pub(super) fn clear(&mut self) {
        self.editing = false;
    }
}

/// Whether a confirmed Forget or Remove may run NOW, checked at the moment of "Yes": no station
/// job running and no bot hand-over or return pending.
///
/// Returns:
///     `Err` with the locale key of the reason the job does not start.
pub(super) fn access_check(
    running: bool,
    refusal: Option<&'static str>,
) -> Result<(), &'static str> {
    if running {
        return Err("telegram.mini_error_busy_station");
    }
    refusal.map_or(Ok(()), Err)
}

/// A danger action's handler; the zone only forwards the click.
pub(super) type DangerClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// The red-framed danger zone: Forget and Remove, both disabled while `busy`.
pub(super) fn danger_el(
    busy: bool,
    on_forget: DangerClick,
    on_remove: DangerClick,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    let red = rgba_from(p.red_text, 1.0);
    let button = |id, key| {
        MoonButton::new(id)
            .label(t!(key).to_string())
            .size(design::CONTROL_TIER)
            .disabled(busy)
            .danger()
    };
    v_flex()
        .id("station-danger")
        .debug_selector(|| "station-danger".into())
        .w_full()
        .min_w_0()
        .gap(design::ui_px(cx, 8.0))
        .p(design::ui_px(cx, 12.0))
        .border_1()
        .border_color(red)
        .rounded(design::ui_px(cx, 6.0))
        .child(
            div()
                .text_color(red)
                .font_weight(FontWeight::SEMIBOLD)
                .child(t!("station.maint.danger").to_string()),
        )
        .child(
            h_flex()
                .flex_wrap()
                .gap(design::ui_px(cx, 8.0))
                .child(
                    div().debug_selector(|| "station-forget".into()).child(
                        button("station-forget", "station.maint.forget")
                            .on_click(on_forget)
                            .render(),
                    ),
                )
                .child(
                    div().debug_selector(|| "station-remove".into()).child(
                        button("station-remove", "station.maint.remove")
                            .on_click(on_remove)
                            .render(),
                    ),
                ),
        )
        // What Forget does, in place of a long tooltip: the zone is where it is read before a click.
        .child(
            div()
                .debug_selector(|| "station-forget-note".into())
                .min_w_0()
                .text_color(rgba_from(p.text_muted, 1.0))
                .child(t!("telegram.server.forget_hint").to_string()),
        )
        .into_any_element()
}

impl SettingsView {
    /// Start address editing from the known address, discarding a previous probe's consent.
    pub(super) fn station_address_begin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.backend.read(cx).station.running {
            return;
        }
        if let Some(reason) = self.backend.read(cx).station.access_refusal() {
            self.server_bot_run(Err(t!(reason).to_string()), cx);
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

    /// Ask before forgetting or removing the station; nothing changes until the dialog's red
    /// button. The dialog holds the host read now, so a later address change cannot redirect it.
    fn station_access_confirm(
        &mut self,
        remove: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.backend.read(cx).station.running {
            return;
        }
        if let Some(reason) = self.backend.read(cx).station.access_refusal() {
            self.server_bot_run(Err(t!(reason).to_string()), cx);
            return;
        }
        let source = match self.station_access_source() {
            Ok(source) => source,
            Err(reason) => {
                self.server_bot_run(Err(reason), cx);
                return;
            }
        };
        self.telegram.server.station_access.clear();
        self.station_address_cancel_probe(cx);
        let has_bot = self
            .backend
            .read(cx)
            .station
            .bot
            .as_ref()
            .is_none_or(|bot| bot.has_token);
        let (title, body, which, label) = if remove {
            (
                "station.maint.remove_title",
                "station.maint.remove_body",
                "telegram.server.remove_confirm",
                "telegram.server.remove",
            )
        } else {
            (
                "station.maint.forget_title",
                "station.maint.forget_body",
                "telegram.server.forget_confirm",
                "telegram.server.forget",
            )
        };
        let mut text = format!("{}\n\n{}", t!(which, addr = source.addr.clone()), t!(body));
        if remove && has_bot {
            text = format!("{text}\n\n{}", t!("telegram.server.remove_bot_confirm"));
        }
        let view = cx.entity().downgrade();
        let source = std::cell::Cell::new(Some(source));
        super::confirm_danger(
            window,
            cx,
            t!(title).to_string(),
            text,
            t!(label).to_string(),
            std::rc::Rc::new(move |_, app| {
                let Some(source) = source.take() else {
                    return;
                };
                let _ = view.update(app, |this, cx| {
                    this.telegram.server.station_access.clear();
                    let station = &this.backend.read(cx).station;
                    if let Err(key) = access_check(station.running, station.access_refusal()) {
                        this.server_bot_run(Err(t!(key).to_string()), cx);
                        return;
                    }
                    if remove {
                        this.server_bot_run(Ok(Job::Remove { source }), cx);
                    } else {
                        this.server_bot_forget(source, cx);
                    }
                });
            }),
        );
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
        let p = MoonPalette::active(cx);
        let button = |id, key| {
            MoonButton::new(id)
                .label(t!(key).to_string())
                .size(design::CONTROL_TIER)
                .disabled(busy)
        };
        let mut block = v_flex().gap(design::ui_px(cx, 8.0));
        if let Some(reason) = refusal {
            block = block.child(
                div()
                    .text_color(rgba_from(p.red_text, 1.0))
                    .child(t!(reason).to_string()),
            );
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
        let danger = danger_el(
            busy,
            Box::new(
                cx.listener(|this, _, window, cx| this.station_access_confirm(false, window, cx)),
            ),
            Box::new(
                cx.listener(|this, _, window, cx| this.station_access_confirm(true, window, cx)),
            ),
            cx,
        );
        block
            .when(
                ed.editing || backend.station.address_change.is_some(),
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
            .child(danger)
    }
}

#[cfg(test)]
mod tests;
