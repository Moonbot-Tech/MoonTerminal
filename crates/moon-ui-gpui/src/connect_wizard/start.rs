//! The start screen: what the wizard is going to do. Two variants — no core yet, where it opened
//! by itself, and cores already there, where the user opened it.

use gpui::*;
use moon_ui::{
    MoonBadge, MoonButton, MoonPalette, MoonSelectableCard, MoonTone, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::render::{Screen, note};
use super::{Branch, ConnectWizardView};
use crate::design;

impl ConnectWizardView {
    pub(super) fn start_screen(&mut self, cx: &mut Context<Self>) -> Screen {
        let cores = self.backend.read(cx).config.servers.len();
        let first_run = cores == 0;
        let mut screen = if first_run {
            Screen::new(t!("connect_wizard.start.first_title"))
                .sub(t!("connect_wizard.start.first_sub"))
        } else {
            Screen::new(t!("connect_wizard.start.title"))
                .sub(t!("connect_wizard.start.sub", n = cores))
        };
        screen = screen
            .child(self.option(
                Branch::NewCore,
                t!("connect_wizard.start.new_title").to_string(),
                if first_run {
                    t!("connect_wizard.start.new_first").to_string()
                } else {
                    t!("connect_wizard.start.new_more").to_string()
                },
                first_run.then(|| {
                    (
                        t!("connect_wizard.start.recommended").to_string(),
                        MoonTone::Positive,
                    )
                }),
                cx,
            ))
            .child(move_option(first_run, cx))
            .child(self.option(
                Branch::ByKey,
                t!("connect_wizard.start.key_title").to_string(),
                t!("connect_wizard.start.key_desc").to_string(),
                None,
                cx,
            ));
        #[cfg(windows)]
        {
            screen = screen.child(self.download_block(first_run, cx));
        }
        screen
            .left(
                MoonButton::new("cw-later")
                    .size(design::CONTROL_TIER)
                    .label(if first_run {
                        t!("connect_wizard.later").to_string()
                    } else {
                        t!("connect_wizard.cancel").to_string()
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.close(window, cx)))
                    .render(),
            )
            .right(
                MoonButton::new("cw-next")
                    .primary()
                    .size(design::CONTROL_TIER)
                    .label(t!("connect_wizard.next").to_string())
                    .on_click(cx.listener(|this, _, _, cx| this.next_from_start(cx)))
                    .render(),
            )
    }

    /// One way to connect a core, as a card the user picks.
    fn option(
        &self,
        branch: Branch,
        title: String,
        description: String,
        badge: Option<(String, MoonTone)>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = match branch {
            Branch::NewCore => "cw-opt-new",
            Branch::ByKey => "cw-opt-key",
        };
        MoonSelectableCard::new(id)
            .selected(self.choice == branch)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.choice = branch;
                cx.notify();
            }))
            .child(option_body(title, description, badge, cx))
    }
}

/// The move card. Disabled while there is no core to move; the move itself joins the wizard in
/// its own change, and until then it stays on the core's card in Settings → Cores.
fn move_option(first_run: bool, cx: &App) -> impl IntoElement {
    let badge = if first_run {
        t!("connect_wizard.start.move_no_cores").to_string()
    } else {
        t!("connect_wizard.start.move_from_card").to_string()
    };
    MoonSelectableCard::new("cw-opt-move")
        .disabled(true)
        .child(option_body(
            t!("connect_wizard.start.move_title").to_string(),
            t!("connect_wizard.start.move_desc").to_string(),
            Some((badge, MoonTone::Muted)),
            cx,
        ))
}

/// A card's title, its badge and its description.
fn option_body(
    title: String,
    description: String,
    badge: Option<(String, MoonTone)>,
    cx: &App,
) -> impl IntoElement {
    v_flex()
        .gap(design::ui_px(cx, 4.0))
        .child(
            h_flex()
                .flex_wrap()
                .items_center()
                .gap(design::ui_px(cx, 8.0))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgba_from(MoonPalette::active(cx).text, 1.0))
                        .child(title),
                )
                .children(badge.map(|(label, tone)| MoonBadge::new(label).tone(tone).render())),
        )
        .child(note(description, cx))
}
