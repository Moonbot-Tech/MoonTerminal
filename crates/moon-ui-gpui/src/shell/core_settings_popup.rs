//! Frame of the core-settings popover opened by the gear button beside the header core selector.
//!
//! The popup has two halves with deliberately different contracts. Above the tab strip sit the
//! ACTIONS — restart, emulator, cancel all orders — which act on the core the moment they are
//! pressed, because none of them is a value to be reviewed before committing. Below it sit the
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
    h_flex, rgba_from, v_flex, MoonButton, MoonButtonSize, MoonButtonVariant, MoonInputState,
    MoonPalette, MoonSliderState, MoonTabItem, MoonTabStrip,
};
use rust_i18n::t;

use moon_core::feed::{ClientSettingsEdit, CoreConfig};
use moon_core::session::CoreId;

use crate::panels::popup_title;
use crate::shell::core_settings::resolve_core_settings_write;
use crate::shell::Shell;
use crate::{design, Backend};

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
    pub(crate) def_strategy: &'a Entity<MoonInputState>,
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

/// Builds core-settings popover content.
///
/// Args:
///     ctx: Popup-wide addressing and palette.
///     tab: Selected tab.
///     draft: Staged settings, absent until the core's configuration arrives.
///     widgets: Editors and sliders prepared by Shell for this render.
///     editors: Retained blacklist and strategy-filter editors.
///     blacklist_expanded: Whether to render the multiline blacklist editor.
///     cancel_confirm: Whether Cancel All Orders is awaiting confirmation.
///     view: Shell entity used by tab switching, staging, OK, and Cancel.
///     cx: Application context used to read state and render controls.
///     on_cancel_all: Callback for the staged Cancel All Orders action.
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
    cancel_confirm: bool,
    view: &Entity<Shell>,
    cx: &App,
    on_cancel_all: impl Fn(&mut App) + 'static,
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
        );

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

    let actions = action_row(&cs, cancel_confirm, ctx, cx, on_cancel_all);

    // Reuse the main window's chart-tab control, as the Settings window's hotkey groups do.
    let strip = {
        let view = view.clone();
        let items: Vec<MoonTabItem> = CoreSettingsTab::ALL
            .iter()
            .map(|t| MoonTabItem::new(t.title()).selected(tab == *t))
            .collect();
        div()
            .w_full()
            .h(design::fit_h_px(cx, 28.0, 13.0, 7.5))
            .child(
                MoonTabStrip::new("core-settings-tabs")
                    .gap(4.0)
                    .items(items)
                    .on_click(move |ix, _event, _window, app| {
                        let Some(next) = CoreSettingsTab::ALL.get(ix).copied() else {
                            return;
                        };
                        view.update(app, |this, cx| this.set_core_settings_tab(next, cx));
                    })
                    .render(),
            )
    };

    let body = match draft {
        Some(draft) => match tab {
            CoreSettingsTab::General => general::general_tab(
                ctx,
                draft,
                widgets,
                editors,
                view,
                blacklist_expanded,
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
        .child(strip)
        .child(body)
        .when(draft.is_some(), |r| r.child(footer(view, cx)))
        .into_any_element()
}

/// Immediate actions above the tab strip: restart, emulator, cancel all orders.
fn action_row(
    cs: &moon_core::feed::ClientSettings,
    cancel_confirm: bool,
    ctx: &TabCtx<'_>,
    cx: &App,
    on_cancel_all: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    let TabCtx {
        backend,
        group,
        seeded,
        p,
    } = *ctx;
    let restart_btn = {
        let backend = backend.clone();
        let group = group.to_string();
        MoonButton::new("core-restart")
            .label(t!("core_settings.restart").to_string())
            .size(MoonButtonSize::Action)
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
    let cancel_all = MoonButton::new("core-cancel-all")
        .label(if cancel_confirm {
            t!("core_settings.cancel_all_confirm").to_string()
        } else {
            t!("core_settings.cancel_all").to_string()
        })
        .size(MoonButtonSize::Action)
        .variant(MoonButtonVariant::Danger)
        .selected(cancel_confirm)
        .padding_x(12.0)
        .on_click(move |_, _w, app| on_cancel_all(app))
        .render();
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
                .child(cancel_all),
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
                .size(MoonButtonSize::Action)
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
                .size(MoonButtonSize::Action)
                .variant(MoonButtonVariant::Blue)
                .padding_x(18.0)
                .on_click(move |_, _w, app| {
                    ok_view.update(app, |this, cx| this.commit_core_draft(cx));
                })
                .render(),
        )
}
