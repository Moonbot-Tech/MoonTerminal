//! Per-core dust conversion headers and their single-use confirmation, sharing the Engine toast.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, Window, div, px, rgb,
};
use moon_core::feed::ConnStatus;
use moon_core::session::CoreId;
use moon_core::venue::{Brand, CoreVenue, MarketKind};
use moon_ui::{
    MoonButton, MoonButtonVariant, MoonNotification, MoonPalette, MoonSize, MoonWindowExt as _,
    h_flex, v_flex,
};
use rust_i18n::t;

use super::AssetsView;
use crate::design;

/// Target supported by the pinned SDK's `emk_ConvertDustBNB` contract.
/// Gate's GT behavior is not documented by that dependency, so it is deliberately not offered.
fn dust_target(venue: Option<&CoreVenue>) -> Option<&'static str> {
    let venue = venue?.resolved()?;
    matches!(
        (venue.brand, venue.kind),
        (Brand::Binance, MarketKind::Spot)
    )
    .then_some("BNB")
}

/// Captured identity and one-shot guard survive dialog rerenders and duplicate Yes events.
struct DustConfirmation {
    core: CoreId,
    consumed: Cell<bool>,
}

impl DustConfirmation {
    /// Send exactly the captured core once, and only while the caller's live authority holds.
    fn confirm<E>(
        &self,
        authorized: bool,
        send: impl FnOnce(CoreId) -> Result<(), E>,
    ) -> Result<(), E> {
        if authorized && !self.consumed.replace(true) {
            send(self.core)?;
        }
        Ok(())
    }
}

/// Compact natural-width action; its label never inherits a fixed English-only width.
fn dust_button(core: CoreId, token: &str) -> MoonButton {
    MoonButton::new(gpui::SharedString::from(format!("assets-dust-{core}")))
        .size(MoonSize::Xs)
        .variant(MoonButtonVariant::Soft)
        .label(t!("assets.dust.button", token = token).to_string())
}

/// Core identity yields to the action at narrow widths; wrapping keeps the complete button visible.
fn core_header(name: String, venue: String, button: MoonButton, cx: &App) -> gpui::Div {
    let p = MoonPalette::active(cx);
    h_flex()
        .w_full()
        .flex_wrap()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .px(design::ui_px(cx, 8.0))
        .py(design::ui_px(cx, 3.0))
        .bg(rgb(p.shell_high))
        .child(
            div()
                .flex_1()
                .min_w(px(80.0))
                .overflow_hidden()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(format!("{name} · {venue}")),
        )
        .child(div().flex_none().child(button.render()))
}

impl AssetsView {
    /// Render one header per supported core, even when the dust threshold hides all its coins.
    /// A bounded vertical list preserves table space in a large fleet and all Assets host shapes.
    pub(super) fn dust_core_headers(&self, cx: &Context<Self>) -> impl IntoElement {
        let b = self.backend.read(cx);
        let mut rows = v_flex().w_full();
        let mut count = 0;
        for (core, name) in self.query_cores(b) {
            let venue = b.session.core_venues().get(&core);
            let Some(token) = dust_target(venue) else {
                continue;
            };
            let venue_label = crate::controls::venue_section_label(venue);
            let ready = b
                .session
                .store()
                .core(core)
                .is_some_and(|data| data.status == ConnStatus::Ready);
            let view = cx.entity();
            let button = dust_button(core, token)
                .disabled(!ready)
                .tooltip(t!("assets.dust.hint", token = token).to_string())
                .on_click(move |_, window, app| open_confirmation(view.clone(), core, window, app));
            rows = rows.child(core_header(name, venue_label, button, cx));
            count += 1;
        }
        div()
            .id("assets-dust-core-headers")
            .w_full()
            .flex_none()
            .max_h(design::ui_px(cx, 120.0))
            .overflow_y_scroll()
            .children((count > 0).then_some(rows))
    }

    /// Re-read scope, readiness and venue at confirmation time, never retargeting a stale dialog.
    fn dust_authorized(&self, core: CoreId, cx: &App) -> bool {
        let b = self.backend.read(cx);
        self.query_cores(b).iter().any(|(id, _)| *id == core)
            && dust_target(b.session.core_venues().get(&core)).is_some()
            && b.session
                .store()
                .core(core)
                .is_some_and(|data| data.status == ConnStatus::Ready)
    }
}

/// Only Yes dispatches: opening, cancelling, and dismissing the dialog send no command.
fn open_confirmation(view: Entity<AssetsView>, core: CoreId, window: &mut Window, app: &mut App) {
    let (name, venue, token) = {
        let panel = view.read(app);
        if !panel.dust_authorized(core, app) {
            return;
        }
        let b = panel.backend.read(app);
        let Some((_, name)) = panel.query_cores(b).into_iter().find(|(id, _)| *id == core) else {
            return;
        };
        let venue = b.session.core_venues().get(&core);
        let Some(token) = dust_target(venue) else {
            return;
        };
        (name, crate::controls::venue_section_label(venue), token)
    };
    let confirmation = Rc::new(DustConfirmation {
        core,
        consumed: Cell::new(false),
    });
    window.open_unique_moon_dialog("assets-dust-confirm", app, move |dialog, _, cx| {
        let confirm_view = view.clone();
        let confirmation = confirmation.clone();
        let question = t!(
            "assets.dust.question",
            core = name.as_str(),
            venue = venue.as_str(),
            token = token
        )
        .to_string();
        crate::panels::order_dialog_chrome(dialog, 380.0, t!("assets.dust.title").to_string(), cx)
            .content(move |content, _, _| content.child(div().w_full().child(question.clone())))
            .footer(
                h_flex()
                    .w_full()
                    .gap_2()
                    .justify_end()
                    .child(
                        MoonButton::new("assets-dust-no")
                            .outline()
                            .label(t!("dialogs.no").to_string())
                            .on_click(|_, window, cx| window.close_dialog(cx))
                            .render(),
                    )
                    .child(
                        MoonButton::new("assets-dust-yes")
                            .variant(MoonButtonVariant::Danger)
                            .label(t!("dialogs.yes").to_string())
                            .on_click(move |_, window, app| {
                                let panel = confirm_view.read(app);
                                let authorized = panel.dust_authorized(core, app);
                                let result = confirmation.confirm(authorized, |id| {
                                    panel.backend.read(app).session.convert_dust(id)
                                });
                                if !authorized {
                                    window.push_notification(
                                        MoonNotification::warning(
                                            t!("assets.dust.unavailable").to_string(),
                                        ),
                                        app,
                                    );
                                } else if let Err(error) = result {
                                    log::warn!("assets convert dust failed: {error:#}");
                                    window.push_notification(
                                        MoonNotification::warning(
                                            t!("assets.dust.failed").to_string(),
                                        ),
                                        app,
                                    );
                                }
                                window.close_dialog(app);
                            })
                            .render(),
                    ),
            )
    });
}

#[cfg(test)]
mod tests;
