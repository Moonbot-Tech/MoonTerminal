//! Frame of the core-settings popover opened by the gear button beside the header core selector.
//!
//! The popup has two halves with deliberately different contracts. Above the tab strip sit the
//! ACTIONS — restart, emulator, cancel entries on exchange, cancel all orders — which act on the
//! core the moment they are pressed (the two cancellations after one confirming click), because
//! none of them is a value to be reviewed before committing. Below it sit the
//! tabs, [`general`] and [`autostart`], which edit ONE staged draft and reach the core only when OK
//! is pressed, the way the Moonbot settings window they reproduce does.
//!
//! Every edit goes to the core the popup was opened over, resolved through
//! `core_settings::resolve_core_settings_write` at event time. The immediate actions may not fall
//! back to `active_trade_core(group)` alone: each acts on a value read at RENDER time, which belongs
//! to the core that was active then — and the stacked repaint throttles allow that to differ from
//! the core active when the click lands. Default Alert Strategy is the exception by construction: it
//! captures the core rendered into its own row.

pub(crate) mod autostart;
mod general;
mod widgets;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonVariant, MoonCheckbox, MoonInputState, MoonPalette, MoonSliderState,
    MoonTabItem, MoonTabStrip, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use moon_core::feed::{ClientSettingsEdit, CoreConfig};
use moon_core::session::CoreId;

use crate::panels::popup_title;
use crate::shell::Shell;
use crate::shell::core_settings::armed::{ArmedAction, ExchangeCancelOffer};
use crate::shell::core_settings::{
    BlacklistReveal, exchange_cancel_offer, group_core_name, resolve_core_settings_write,
};
use crate::{Backend, design};

/// Unscaled content width shared with the popover host.
///
/// [`core_settings_content`] applies the font scale to this value; the terminal chrome uses the
/// same scaled width when sizing the surrounding `MoonPopover`.
/// Wide enough for the two-column tabs: the AutoStart page pairs a trade-window loss cap with an
/// hourly one, and stacking those into one column pushes the watchdogs off the screen.
pub const CONTENT_W: f32 = 720.0;

/// Tabs of the core-settings popup, in strip order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum CoreSettingsTab {
    /// Exit rules, risk limits and leverage — Moonbot's "General" page.
    #[default]
    General,
    /// Moonbot's AutoStart page.
    AutoStart,
}

impl CoreSettingsTab {
    pub(crate) const ALL: [CoreSettingsTab; 2] = [Self::General, Self::AutoStart];

    fn title(self) -> String {
        match self {
            Self::General => t!("core_settings.tab_general").to_string(),
            Self::AutoStart => t!("core_settings.tab_autostart").to_string(),
        }
    }
}

/// Addressing and palette shared by every tab.
pub(crate) struct TabCtx<'a> {
    pub(crate) backend: &'a Entity<Backend>,
    pub(crate) group: &'a str,
    /// Core the popup was seeded from; the only core it may write to.
    pub(crate) seeded: Option<CoreId>,
    pub(crate) p: MoonPalette,
}

/// Retained text editors the General tab renders, all owned by Shell.
pub(crate) struct TextEditors<'a> {
    pub(crate) input: &'a Entity<MoonInputState>,
    pub(crate) area: &'a Entity<MoonInputState>,
}

/// One numeric editor already created for this render, paired with the width it should occupy.
pub(crate) struct NumField {
    pub(crate) state: Entity<MoonInputState>,
    pub(crate) id: &'static str,
    pub(crate) width: f32,
}

/// One slider already created for this render.
pub(crate) struct SliderRow {
    pub(crate) state: Entity<MoonSliderState>,
    pub(crate) id: &'static str,
}

/// Editors and sliders prepared by Shell for one render of the popup.
#[derive(Default)]
pub(crate) struct SettingsWidgets {
    pub(crate) fields: Vec<NumField>,
    pub(crate) sliders: Vec<SliderRow>,
}

impl SettingsWidgets {
    /// Look one editor up by the id it was created with.
    ///
    /// A missing id means the caller listed a row in the render pass but not in the spec pass; the
    /// row then renders without its control rather than panicking mid-frame.
    fn field(&self, id: &'static str) -> Option<&NumField> {
        self.fields.iter().find(|f| f.id == id)
    }

    fn slider(&self, id: &'static str) -> Option<&SliderRow> {
        self.sliders.iter().find(|s| s.id == id)
    }
}

/// Every numeric editor both tabs need, in the order Shell should create them.
///
/// Each entry is `(id, current value, staging function, field width)`.
#[allow(clippy::type_complexity)]
pub(crate) fn field_specs(
    draft: &CoreConfig,
) -> Vec<(&'static str, String, fn(&mut CoreConfig, &str), f32)> {
    let mut specs = general::field_specs(draft);
    specs.extend(autostart::field_specs(draft));
    specs
}

/// Every slider both tabs need, as `(id, bounds, current value, staging function, mirror editor)`.
///
/// The mirror names the numeric editor showing the same value, if the row has one; a row that
/// prints its value in its own caption has none.
#[allow(clippy::type_complexity)]
pub(crate) fn slider_specs(
    draft: &CoreConfig,
) -> Vec<(
    &'static str,
    (f32, f32, f32),
    f32,
    fn(&mut CoreConfig, f32),
    Option<&'static str>,
)> {
    let mut specs = general::slider_specs(draft);
    specs.extend(autostart::slider_specs(draft));
    specs
}

/// The popup's tab strip, built by the CALLER and handed in.
///
/// It is not built inside [`core_settings_content`] because it is rendered through a lifted
/// palette: MoonUI keys an inactive tab label off `text_muted`, which sits under the body contrast
/// floor in both stock themes, and `render_with_theme` is the only way to hand it a different
/// palette — and that needs `&mut Window` and `&mut App`, which the content builder does not hold.
/// Reuses the main window's chart-tab control, as the Settings window's hotkey groups do.
///
/// Args:
///     tab: Core-settings tab that the strip marks as selected.
///     p: Active palette whose muted label tone is lifted for the strip.
///     view: Shell entity updated when the user selects another tab.
///     window: Window that owns the strip's persistent overflow state.
///     cx: Application context used to render the themed strip.
///
/// Returns:
///     The core-settings tab strip in its fixed-height wrapper.
pub(crate) fn core_settings_tab_strip(
    tab: CoreSettingsTab,
    p: MoonPalette,
    view: &Entity<Shell>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let view = view.clone();
    let items: Vec<MoonTabItem> = CoreSettingsTab::ALL
        .iter()
        .map(|t| MoonTabItem::new(t.title()).selected(tab == *t))
        .collect();
    let strip_h = design::tab_strip_h(cx);
    let strip = MoonTabStrip::new("core-settings-tabs")
        .gap(4.0)
        .items(items)
        .on_click(move |ix, _event, _window, app| {
            let Some(next) = CoreSettingsTab::ALL.get(ix).copied() else {
                return;
            };
            view.update(app, |this, cx| this.set_core_settings_tab(next, cx));
        });
    let strip = design::chrome_tab_strip(strip, p, window, cx);
    div().w_full().h(strip_h).child(strip).into_any_element()
}

/// Builds core-settings popover content.
///
/// Args:
///     ctx: Popup-wide addressing and palette.
///     tab: Selected tab.
///     draft: Staged settings, absent until the core's configuration arrives.
///     widgets: Editors and sliders prepared by Shell for this render.
///     editors: Retained blacklist and strategy-filter editors.
///     blacklist_expanded: Whether to render the multiline blacklist editor.
///     armed: The destructive action awaiting its confirming click, if any.
///     bl_reveal: Coin highlighted on the blacklist, when a reveal is showing.
///     view: Shell entity used by tab switching, staging, OK, and Cancel.
///     tab_strip: Pre-rendered contrast-safe tab strip, built where a mutable window is available.
///     cx: Application context used to read state and render controls.
///     on_armed: Callback for a click on either two-click action (Cancel All Orders, Cancel
///         Entries On Exchange), told which one was clicked.
///     on_toggle_blacklist: Callback that toggles the blacklist editor mode.
///
/// Returns:
///     The complete core-settings popover content.
#[allow(clippy::too_many_arguments)]
pub(crate) fn core_settings_content(
    ctx: &TabCtx<'_>,
    tab: CoreSettingsTab,
    draft: Option<&CoreConfig>,
    widgets: &SettingsWidgets,
    editors: &TextEditors<'_>,
    blacklist_expanded: bool,
    armed: Option<ArmedAction>,
    bl_reveal: Option<BlacklistReveal<'_>>,
    view: &Entity<Shell>,
    tab_strip: AnyElement,
    cx: &App,
    on_armed: impl Fn(ArmedAction, &mut Window, &mut App) + 'static,
    on_toggle_blacklist: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    let TabCtx {
        backend, group, p, ..
    } = *ctx;
    let b = backend.read(cx);
    let core = b.active_trade_core(group);
    let cd = core.and_then(|c| b.session.store().core(c));
    let cs = cd.and_then(|d| d.client_settings.clone());
    let profit = cd.and_then(|d| d.profit_state);
    // Show labeled Running and Auto Detect dots for the active core. They moved from the header,
    // where the unlabeled dots were cramped beside the gear button.
    let run = core
        .map(|c| b.session.core_run_state(c))
        .unwrap_or_default();

    // Chrome is MoonPopover's; see `popover_contents_do_not_paint_a_second_surface`.
    let root = v_flex()
        .id("core-settings-popup")
        .w(design::font_w_px(cx, CONTENT_W))
        .gap(design::ui_px(cx, 8.0))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 8.0))
                .child(popup_title(t!("core_settings.title"), p, cx))
                .child(widgets::runtime_status(run, p, cx)),
        )
        // Above the "no core" branch below: expert mode is a preference about the GEAR, not about
        // any one core, and a trader who has not connected yet must still be able to switch faces.
        .child(expert_switch(ctx, view, cx));

    // Show a placeholder when the core or its settings snapshot is unavailable.
    let Some(cs) = cs else {
        return root
            .child(
                div()
                    .text_color(rgb(p.text_muted))
                    .child(t!("core_settings.no_core").to_string()),
            )
            .into_any_element();
    };

    let actions = action_row(&cs, armed, ctx, cx, on_armed);

    let body = match draft {
        Some(draft) => match tab {
            CoreSettingsTab::General => general::general_tab(
                ctx,
                draft,
                widgets,
                editors,
                view,
                blacklist_expanded,
                bl_reveal,
                cx,
                on_toggle_blacklist,
            ),
            CoreSettingsTab::AutoStart => {
                autostart::autostart_tab(ctx, draft, profit, widgets, view, cx)
            }
        },
        // The runtime fetches the full configuration in the background after Ready and retries
        // until it lands, so this is a wait, not a failure: no request of ours is missing.
        None => div()
            .text_color(rgb(p.text_muted))
            .child(t!("core_settings.as_waiting").to_string())
            .into_any_element(),
    };

    root.child(actions)
        .child(tab_strip)
        .child(body)
        .when(draft.is_some(), |r| r.child(footer(view, cx)))
        .into_any_element()
}

/// The expert-mode switch, directly under this popup's title.
///
/// Ticking it is a THREE-part action and all three belong together: the preference is persisted so
/// the gear keeps opening the expert window, this popup closes (its draft discarded, as dismissing
/// it always discards), and the expert window opens over the same core straight away — the click
/// that ticked the box is the same click the user would otherwise have to repeat on the gear.
///
/// Args:
///     ctx: Popup-wide addressing and palette.
///     view: Shell entity used to close this popup.
///     cx: Application context used for font-scaled geometry.
///
/// Returns:
///     The switch row.
fn expert_switch(ctx: &TabCtx<'_>, view: &Entity<Shell>, cx: &App) -> impl IntoElement {
    let TabCtx { backend, group, .. } = *ctx;
    // Read rather than assumed: this popup is only drawn while the preference is off, but a
    // hardcoded tick is how the two faces would come to disagree if that ever stopped holding.
    let checked = backend.read(cx).core_settings_expert();
    let backend = backend.clone();
    let group = group.to_string();
    let view = view.clone();
    h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 8.0))
        .child(
            MoonCheckbox::new("core-settings-expert")
                .label(t!("core_settings.expert").to_string())
                .description(t!("core_settings.expert_hint").to_string())
                .checked(checked)
                .on_change(move |value, window, app| {
                    if !*value {
                        return;
                    }
                    backend.update(app, |b, bcx| b.set_core_settings_expert(true, bcx));
                    view.update(app, |shell, cx| shell.leave_popup_for_expert(window, cx));
                    let owner_display = crate::window::windowing::window_display_id(window, app);
                    crate::core_expert::open(
                        backend.clone(),
                        Some(window.window_handle()),
                        owner_display,
                        group.clone(),
                        app,
                    );
                }),
        )
}

/// Immediate actions above the tab strip: restart, emulator, cancel entries on exchange, cancel all
/// orders.
///
/// The two cancellations share one confirmation slot (`core_settings::armed`): each asks once on
/// its own button, and the exchange one also names the core it addresses in a banner while armed,
/// because its reach — orders on the exchange that this terminal may never have seen — is not
/// visible anywhere in the popup.
fn action_row(
    cs: &moon_core::feed::ClientSettings,
    armed: Option<ArmedAction>,
    ctx: &TabCtx<'_>,
    cx: &App,
    on_armed: impl Fn(ArmedAction, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let TabCtx {
        backend,
        group,
        seeded,
        p,
    } = *ctx;
    let on_armed = std::rc::Rc::new(on_armed);
    // A RENDER-time read of the core to draw the exchange button for, resolved through the same
    // guard the click re-runs: only the core the popup was opened over, and only while it is still
    // the active one. Spelled `ctx.seeded` and `(group)` because it is a read, not a handler — the
    // contract test counts the handler shape `(&group)` against `resolve_core_settings_write(seeded`.
    let (exchange_offer, exchange_core_name) = {
        let b = backend.read(cx);
        let target = resolve_core_settings_write(ctx.seeded, b.active_trade_core(group));
        (
            exchange_cancel_offer(b, target),
            target.map(|core| group_core_name(b, group, core)),
        )
    };
    let restart_btn = {
        let backend = backend.clone();
        let group = group.to_string();
        MoonButton::new("core-restart")
            .label(t!("core_settings.restart").to_string())
            .variant(MoonButtonVariant::Blue)
            .padding_x(12.0)
            .on_click(move |_, _w, app| {
                let active = backend.read(app).active_trade_core(&group);
                // Guarded like Cancel All beside it: the popup's two destructive actions must not
                // disagree about which core they act on.
                if let Some(core) = resolve_core_settings_write(seeded, active) {
                    // Through the shared run control, so this button and the ones in the Profit
                    // Monitor register the same pending intent instead of each tracking its own.
                    crate::controls::core_run::restart(&backend, core, app);
                }
            })
            .render()
    };
    let emu_check = widgets::cs_checkbox(
        "core-emu",
        t!("core_settings.emu").to_string(),
        cs.emu_mode,
        backend,
        group,
        seeded,
        ClientSettingsEdit::EmuMode,
    );
    // Reset Session and Reset All live on the AutoStart tab beside the counters they clear, which
    // is where Moonbot puts them and the only place their effect is visible.
    let cancel_all_armed = armed == Some(ArmedAction::CancelAll);
    let cancel_all = {
        let on_armed = on_armed.clone();
        MoonButton::new("core-cancel-all")
            .label(if cancel_all_armed {
                t!("core_settings.cancel_all_confirm").to_string()
            } else {
                t!("core_settings.cancel_all").to_string()
            })
            .variant(MoonButtonVariant::Danger)
            .selected(cancel_all_armed)
            .padding_x(12.0)
            .on_click(move |_, window, app| on_armed(ArmedAction::CancelAll, window, app))
            .render()
    };
    // Armed only while it can still be sent. `Shell::reconcile_core_settings_popup` clears the slot
    // once the core drops or loses N1; this guard covers the frame before that reconcile runs.
    let exchange_armed = armed == Some(ArmedAction::ExchangeEntries) && exchange_offer.available();
    let exchange_cancel = {
        let tooltip = match exchange_offer {
            ExchangeCancelOffer::Offered => t!("core_settings.exch_cancel_hint"),
            ExchangeCancelOffer::NeedsNewerCore => t!("core_settings.exch_cancel_needs_core"),
            ExchangeCancelOffer::Offline => t!("core_settings.exch_cancel_offline"),
        };
        let button = MoonButton::new("core-cancel-exchange-entries")
            .label(if exchange_armed {
                t!("core_settings.exch_cancel_confirm").to_string()
            } else {
                t!("core_settings.exch_cancel").to_string()
            })
            .variant(MoonButtonVariant::Danger)
            .selected(exchange_armed)
            .disabled(!exchange_offer.available())
            .padding_x(12.0)
            .tooltip(tooltip.to_string());
        if exchange_offer.available() {
            let on_armed = on_armed.clone();
            button
                .on_click(move |_, window, app| on_armed(ArmedAction::ExchangeEntries, window, app))
                .render()
        } else {
            button.render()
        }
    };
    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 6.0))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 10.0))
                .child(restart_btn)
                .child(emu_check)
                .child(div().flex_1())
                .child(exchange_cancel)
                .child(cancel_all),
        )
        // The confirmation's question: which account, and what the core will reach. Shown only
        // while armed, so an unarmed popup carries no extra line.
        .when_some(
            exchange_armed.then_some(exchange_core_name).flatten(),
            |this, core| {
                let danger = design::danger_color(p);
                this.child(
                    div()
                        .w_full()
                        .px(design::ui_px(cx, 6.0))
                        .py(design::ui_px(cx, 3.0))
                        .rounded(design::r_button(cx))
                        .bg(rgba_from(danger, 0.14))
                        .border_1()
                        .border_color(rgb(danger))
                        .text_color(rgb(p.text))
                        .text_size(design::t_caption(cx))
                        .child(
                            t!("core_settings.exch_cancel_armed", core = core.as_str()).to_string(),
                        ),
                )
            },
        )
        // Display a prominent warning banner while emulator mode is enabled.
        .when(cs.emu_mode, |this| {
            this.child(
                div()
                    .w_full()
                    .px(design::ui_px(cx, 6.0))
                    .py(design::ui_px(cx, 3.0))
                    .rounded(design::r_button(cx))
                    .bg(rgba_from(p.amber, 0.18))
                    .border_1()
                    .border_color(rgb(p.amber))
                    .text_color(rgb(p.amber))
                    .text_size(design::t_caption(cx))
                    .child(t!("core_settings.emu_on").to_string()),
            )
        })
}

/// OK and Cancel for the staged tabs.
fn footer(view: &Entity<Shell>, cx: &App) -> impl IntoElement {
    let ok_view = view.clone();
    let cancel_view = view.clone();
    h_flex()
        .w_full()
        .items_center()
        .justify_end()
        .gap(design::ui_px(cx, 8.0))
        .child(
            MoonButton::new("core-settings-cancel")
                .label(t!("core_settings.cancel").to_string())
                .variant(MoonButtonVariant::Soft)
                .padding_x(14.0)
                .on_click(move |_, _w, app| {
                    cancel_view.update(app, |this, cx| this.cancel_core_draft(cx));
                })
                .render(),
        )
        .child(
            MoonButton::new("core-settings-ok")
                .label(t!("core_settings.ok").to_string())
                .variant(MoonButtonVariant::Blue)
                .padding_x(18.0)
                .on_click(move |_, _w, app| {
                    ok_view.update(app, |this, cx| this.commit_core_draft(cx));
                })
                .render(),
        )
}
