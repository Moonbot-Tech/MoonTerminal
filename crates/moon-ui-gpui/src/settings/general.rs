//! General page editor for personal and machine settings in `settings.toml`, plus the section,
//! box and draft-bound number helpers the rebuilt pages share.
//! Changes remain in `Backend.preview`. Control zones and the Main-window idle timeout are
//! consumed live from that draft and roll back when Settings closes unsaved; other settings take
//! effect after saving and reconciling the relevant runtime state. UI theme and zoom moved to the
//! Appearance page; the zoom slider's builder stays here beside its range.

use gpui::prelude::FluentBuilder;
use gpui::*;
use std::rc::Rc;

use moon_core::config::AppConfig;
use moon_ui::{
    MoonButtonSize, MoonInputEvent, MoonInputState, MoonNumberInput, MoonPalette, MoonSelect,
    MoonSliderState, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::SettingsView;
use super::common::{Action, hint_row};
use crate::{Backend, design};

/// Zoom endpoints shared by the slider state and its displayed captions.
///
/// The range the slider offers, not a bound on the setting: a hand-edited value outside it stays
/// valid (`repair_ui_scale` touches only values that cannot mean anything).
pub(super) const UI_ZOOM_RANGE: std::ops::RangeInclusive<f32> = 0.50..=2.00;

/// One MoonUI select at a fixed width, the General page's control for an enum setting.
///
/// The width is passed once and reaches both the trigger box and the menu: written out per row,
/// those two drift, and the menu ends up narrower or wider than the control that opened it.
fn select_control<T: Clone + PartialEq + 'static>(
    state: &Entity<moon_ui::MoonSelectState<T>>,
    width: f32,
    cx: &App,
) -> AnyElement {
    div()
        .w(design::ui_px(cx, width))
        .child(
            MoonSelect::new(state)
                .trigger_size(MoonButtonSize::density(cx))
                .menu_width(design::font_w(cx, width)),
        )
        .into_any_element()
}

/// A page section: a small heading over one bordered box holding its rows.
///
/// Args:
///     id: Debug selector of the whole section, for layout probes.
///     title: Section heading.
///     rows: The box's rows, top to bottom.
///     cx: Application context for theme and scale.
pub(super) fn page_section(
    id: &'static str,
    title: String,
    rows: Vec<AnyElement>,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    v_flex()
        .w_full()
        .min_w_0()
        .gap(design::ui_px(cx, 6.0))
        .debug_selector(move || id.to_string())
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgba_from(p.text, 1.0))
                .child(title),
        )
        .child(page_box(rows, cx))
        .into_any_element()
}

/// The bordered, raised box a page groups its rows in.
pub(super) fn page_box(rows: Vec<AnyElement>, cx: &App) -> Div {
    let p = MoonPalette::active(cx);
    v_flex()
        .w_full()
        .min_w_0()
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 4.0))
        .border_1()
        .border_color(rgba_from(p.border, 1.0))
        .rounded(design::r_container(cx))
        .bg(rgba_from(p.shell_high, 1.0))
        // A hairline between rows, none above the first.
        .children(rows.into_iter().enumerate().map(|(i, row)| {
            div()
                .w_full()
                .when(i > 0, |d| {
                    d.border_t_1().border_color(rgba_from(p.border, 0.5))
                })
                .child(row)
        }))
}

/// [`num_input`](super::common::num_input) that can be switched off: a field whose setting is
/// disabled by a checkbox beside it stays visible, greyed, and keeps its value.
pub(super) fn num_input_on(
    id: &'static str,
    state: &Entity<MoonInputState>,
    range: std::ops::RangeInclusive<f64>,
    step: f64,
    unit: Option<String>,
    enabled: bool,
) -> AnyElement {
    super::common::NumField {
        id,
        state: state.clone(),
        range,
        step,
        unit,
        enabled,
    }
    .into_any_element()
}

/// A number field bound to the Settings draft.
///
/// A typed or stepped value goes through `apply`, which clamps, writes `Backend.preview` and says
/// whether anything changed. Whenever the view repaints for any other reason (revert, paste, a
/// theme switch) the field is re-read from the draft through `read`, unless it has focus (the
/// user is typing) or `read` returns `None` (the setting is off and the field keeps its number).
///
/// Args:
///     init: Value shown first.
///     window: Window the field lives in.
///     cx: Settings context the subscriptions are owned by.
///     read: The draft value the field should show, or `None` to leave it alone.
///     apply: Writes a parsed value into the draft; true when the draft changed.
///
/// Returns:
///     The field's input state, for [`num_input`](super::common::num_input).
pub(super) fn draft_number(
    init: f64,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    read: impl Fn(&AppConfig) -> Option<f64> + 'static,
    apply: impl Fn(&mut AppConfig, f64) -> bool + 'static,
) -> Entity<MoonInputState> {
    let state = super::common::number_state(init, window, cx);
    cx.subscribe(&state, move |this, field, ev: &MoonInputEvent, cx| {
        if !matches!(ev, MoonInputEvent::Change) {
            return;
        }
        let Some(value) = MoonNumberInput::read_value(&field, cx) else {
            return;
        };
        let changed = this.backend.update(cx, |b, bcx| {
            let changed = b.preview.as_mut().is_some_and(|p| apply(p, value));
            if changed {
                bcx.notify();
            }
            changed
        });
        if changed {
            cx.notify();
        }
    })
    .detach();
    let field = state.clone();
    let view = cx.entity();
    cx.observe_in(&view, window, move |this, _, window, cx| {
        let want = {
            let b = this.backend.read(cx);
            read(b.preview.as_ref().unwrap_or(&b.config))
        };
        let Some(want) = want else {
            return;
        };
        if field.read(cx).focus_handle(cx).is_focused(window) {
            return;
        }
        if MoonNumberInput::read_value(&field, cx) != Some(want) {
            field.update(cx, |s, cx| s.set_value(want.to_string(), window, cx));
        }
    })
    .detach();
    state
}

/// The controls of the General page, built by the view and laid out by [`general_body`].
pub(super) struct GeneralControls {
    pub(super) language: AnyElement,
    pub(super) valuation: AnyElement,
    pub(super) tab_per_core: AnyElement,
    pub(super) book_pan: AnyElement,
    pub(super) idle_on: AnyElement,
    pub(super) idle_secs: AnyElement,
    pub(super) log_files: AnyElement,
    pub(super) retention: AnyElement,
}

/// The General page body: language and money, charts, logs.
///
/// Args:
///     c: The page's controls.
///     more: Open state and toggle of a row's long help, by row id.
///     cx: Application context for theme and scale.
pub(super) fn general_body(
    c: GeneralControls,
    more: &dyn Fn(&'static str) -> (bool, Action),
    cx: &App,
) -> AnyElement {
    let s = |key: &str| t!(key).to_string();
    let row = |id: &'static str,
               label: String,
               hint: String,
               long: Option<String>,
               control: AnyElement| {
        let (open, toggle) = more(id);
        div()
            .debug_selector(move || id.to_string())
            .child(hint_row(
                id,
                label,
                Some(hint),
                long,
                control,
                open,
                toggle,
                cx,
            ))
            .into_any_element()
    };
    let idle = h_flex()
        .flex_wrap()
        .gap(design::ui_px(cx, 8.0))
        .items_center()
        .child(c.idle_on)
        .child(
            div()
                .text_color(rgba_from(MoonPalette::active(cx).text_muted, 1.0))
                .child(s("general.idle_close.after")),
        )
        .child(c.idle_secs)
        .into_any_element();
    v_flex()
        .w_full()
        .min_w_0()
        .gap(design::ui_px(cx, 16.0))
        .child(page_section(
            "general-sec-lang",
            s("general.sec.lang_money"),
            vec![
                row(
                    "general-language",
                    s("general.language"),
                    s("general.language.hint"),
                    None,
                    c.language,
                ),
                row(
                    "general-valuation",
                    s("general.valuation.label"),
                    s("general.valuation.hint"),
                    Some(s("general.valuation.more")),
                    c.valuation,
                ),
            ],
            cx,
        ))
        .child(page_section(
            "general-sec-charts",
            s("general.sec.charts"),
            vec![
                row(
                    "general-tab-per-core",
                    s("general.charts_split_by_core"),
                    s("general.tab_per_core.hint"),
                    Some(s("general.charts_split_by_core_hint")),
                    c.tab_per_core,
                ),
                row(
                    "general-book-pan",
                    s("general.book_pan.label"),
                    s("general.book_pan.hint"),
                    Some(s("general.book_pan.more")),
                    c.book_pan,
                ),
                row(
                    "general-idle",
                    s("general.idle_close.label"),
                    s("general.idle_close.hint"),
                    Some(s("general.main_idle_close_hint")),
                    idle,
                ),
            ],
            cx,
        ))
        .child(page_section(
            "general-sec-logs",
            s("general.sec.logs"),
            vec![
                row(
                    "general-log-files",
                    s("general.log_files.label"),
                    s("general.log_files.hint"),
                    Some(s("general.log_to_file_hint")),
                    c.log_files,
                ),
                row(
                    "general-retention",
                    s("general.log_retention.label"),
                    s("general.log_retention.hint"),
                    Some(s("general.log_retention_hint")),
                    c.retention,
                ),
            ],
            cx,
        ))
        .into_any_element()
}

/// Every row id of the General page, top to bottom.
#[cfg(test)]
const GENERAL_ROWS: [&str; 7] = [
    "general-language",
    "general-valuation",
    "general-tab-per-core",
    "general-book-pan",
    "general-idle",
    "general-log-files",
    "general-retention",
];

/// Remembered enabled idle timeout, or the default when none is valid.
fn idle_restore(last: u32) -> u32 {
    if last >= 5 {
        last
    } else {
        SettingsView::IDLE_DEFAULT_SECS
    }
}

impl SettingsView {
    /// Fallback used when idle closing is re-enabled without a valid `idle_last_secs` value.
    /// A remembered valid timeout is restored before this 120-second default is considered.
    const IDLE_DEFAULT_SECS: u32 = 120;

    /// A "more" disclosure under a group of buttons: what each one does, folded by default.
    ///
    /// Args:
    ///     key: Stable id; also the open-state key.
    ///     items: `(button caption, what it does)` in the buttons' order.
    ///     cx: Settings context for the open state.
    pub(super) fn more_note(
        &self,
        key: &'static str,
        items: Vec<(String, String)>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let (open, toggle) = self.more_toggle(key, cx);
        let text = items
            .into_iter()
            .map(|(caption, what)| format!("{caption} — {what}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        super::common::hint_row(
            key,
            String::new(),
            None,
            Some(text),
            div().into_any_element(),
            open,
            toggle,
            cx,
        )
    }

    /// Open state of a row's long help and the action that folds or unfolds it.
    pub(super) fn more_toggle(&self, key: &'static str, cx: &Context<Self>) -> (bool, Action) {
        let open = self.more_open.contains(key);
        let this = cx.entity().downgrade();
        let toggle: Action = Rc::new(move |_, cx| {
            let _ = this.update(cx, |view, cx| {
                if !view.more_open.remove(key) {
                    view.more_open.insert(key);
                }
                cx.notify();
            });
        });
        (open, toggle)
    }

    /// Build the General page body: locale, the USDT valuation, chart grouping, book-zone pan,
    /// Main-window idle closing, and file logging with its retention.
    ///
    /// Args:
    ///     cx: Settings context that supplies the active draft and palette.
    ///
    /// Returns:
    ///     The assembled General-page content.
    pub(super) fn general_tab(&self, cx: &Context<Self>) -> AnyElement {
        let (split, scz, idle_secs, logf) = {
            let b = self.backend.read(cx);
            let d = b.preview.as_ref().unwrap_or(&b.config);
            (
                d.charts_split_by_core,
                d.separate_control_zones,
                d.main_idle_close_secs,
                d.log_to_file,
            )
        };
        // Remember the last valid enabled timeout and restore it when the checkbox is re-enabled.
        if idle_secs >= 5 {
            self.idle_last_secs.set(idle_secs);
        }
        let restore = idle_restore(self.idle_last_secs.get());
        let ui = &self.general_ui;
        let controls = GeneralControls {
            language: select_control(&self.lang, 220.0, cx),
            // Which rate converts quote money to USDT, for Report and Analytics.
            valuation: select_control(&self.valuation, 260.0, cx),
            tab_per_core: self
                .draft_checkbox(cx, "split", split, |p, v| {
                    if p.charts_split_by_core != v {
                        p.charts_split_by_core = v;
                        true
                    } else {
                        false
                    }
                })
                .into_any_element(),
            // Grant chart panning inside the order-book zone; order gestures stay in that zone.
            book_pan: self
                .draft_checkbox(cx, "separate-zones", scz, |p, v| {
                    if p.separate_control_zones != v {
                        p.separate_control_zones = v;
                        true
                    } else {
                        false
                    }
                })
                .into_any_element(),
            // Zero disables the timeout; enabling restores the last remembered value.
            idle_on: self
                .draft_checkbox(cx, "idle-close", idle_secs > 0, move |p, v| {
                    let target = match (v, p.main_idle_close_secs) {
                        (false, _) => 0,
                        (true, 0) => restore,
                        (true, kept) => kept,
                    };
                    if p.main_idle_close_secs != target {
                        p.main_idle_close_secs = target;
                        true
                    } else {
                        false
                    }
                })
                .into_any_element(),
            idle_secs: num_input_on(
                "general-idle-secs",
                &ui.main_idle_close_secs,
                5.0..=3600.0,
                10.0,
                Some(t!("general.unit.sec").to_string()),
                idle_secs > 0,
            ),
            log_files: self
                .draft_checkbox(cx, "logf", logf, |p, v| {
                    if p.log_to_file != v {
                        p.log_to_file = v;
                        true
                    } else {
                        false
                    }
                })
                .into_any_element(),
            // Retention means nothing while file logging is off.
            retention: num_input_on(
                "general-retention-days",
                &ui.log_retention_days,
                0.0..=365.0,
                1.0,
                Some(t!("general.unit.days").to_string()),
                logf,
            ),
        };
        general_body(controls, &|key| self.more_toggle(key, cx), cx)
    }

    /// The Main page: language, money, charts and logs, MoonBot import in its header.
    pub(super) fn general_page(&self, cx: &Context<Self>) -> super::frame::PageParts {
        let body = self.general_tab(cx);
        let import = moon_ui::MoonButton::new("moonbot-import")
            .variant(moon_ui::MoonButtonVariant::Neutral)
            .label(t!("import.button").to_string())
            .on_click(cx.listener(|this, _, _, cx| this.start_moonbot_import(cx)))
            .render()
            .into_any_element();
        let import = super::nav::tipped(
            "moonbot-import-tip".into(),
            t!("import.hint").to_string(),
            import,
        );
        self.page_parts(super::Tab::General, body, vec![import], cx)
    }
}

/// Format a UI scale as a whole percentage, hiding floating-point step noise.
pub(super) fn zoom_text(value: f32) -> String {
    format!("{:.0}%", value * 100.0)
}

/// Build the zoom slider with a 5% step and a draft theme preview applied only on release.
///
/// During drag, the slider state supplies the live percentage caption without rescaling the app.
/// Initializing the thumb does not rewrite an existing hand-edited scale outside the UI range.
pub(super) fn build_zoom(
    backend: &Entity<Backend>,
    cx: &mut Context<SettingsView>,
) -> Entity<MoonSliderState> {
    let cur = {
        let b = backend.read(cx);
        b.preview.as_ref().unwrap_or(&b.config).ui_scale
    };
    super::common::draft_slider_on(
        cx,
        *UI_ZOOM_RANGE.start(),
        *UI_ZOOM_RANGE.end(),
        0.05,
        cur,
        super::common::DraftSliderApplyOn::Release,
        |p, value, bcx| {
            if p.ui_scale == value {
                return false;
            }
            p.ui_scale = value;
            crate::install_moon_theme_for_config(p, bcx);
            true
        },
    )
}

/// UI state of the General page that the draft does not hold. Branch-owned: fields are added here,
/// never on `SettingsView`.
pub(super) struct GeneralUi {
    /// Number field for `main_idle_close_secs`; keeps its number while idle closing is off.
    pub(super) main_idle_close_secs: Entity<MoonInputState>,
    /// Number field for `log_retention_days`.
    pub(super) log_retention_days: Entity<MoonInputState>,
}

impl GeneralUi {
    /// Fields seeded from the draft `config` and bound to the draft.
    pub(super) fn init(
        config: &AppConfig,
        window: &mut Window,
        cx: &mut Context<SettingsView>,
    ) -> Self {
        let idle = match config.main_idle_close_secs {
            0 => SettingsView::IDLE_DEFAULT_SECS,
            secs => secs,
        };
        Self {
            main_idle_close_secs: draft_number(
                f64::from(idle),
                window,
                cx,
                |p| (p.main_idle_close_secs > 0).then_some(f64::from(p.main_idle_close_secs)),
                |p, v| {
                    // Off stays off: the field is disabled then, and a value must not enable it.
                    if p.main_idle_close_secs == 0 {
                        return false;
                    }
                    let v = v.round().clamp(5.0, 3600.0) as u32;
                    let changed = p.main_idle_close_secs != v;
                    p.main_idle_close_secs = v;
                    changed
                },
            ),
            log_retention_days: draft_number(
                f64::from(config.log_retention_days),
                window,
                cx,
                |p| Some(f64::from(p.log_retention_days)),
                |p, v| {
                    let v = v.round().clamp(0.0, 365.0) as u32;
                    let changed = p.log_retention_days != v;
                    p.log_retention_days = v;
                    changed
                },
            ),
        }
    }
}

#[cfg(test)]
pub(super) mod probe_host;
#[cfg(test)]
mod tests;
