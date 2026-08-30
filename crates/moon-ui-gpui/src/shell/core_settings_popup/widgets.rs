//! Controls shared by the core-settings popup's tabs.
//!
//! Every write here is addressed through [`resolve_core_settings_write`] for the reason the module
//! header of the parent states: a control is drawn from the core that was active at RENDER time, and
//! the stacked repaint throttles allow the active core to move before the click lands.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonCheckbox, MoonCheckboxSize, MoonInput, MoonInputState, MoonPalette, MoonSlider, h_flex,
    rgba_from, v_flex,
};
use rust_i18n::t;

use moon_core::feed::{ClientSettingsEdit, CoreConfig};
use moon_core::session::{CoreId, CoreRunState};

use crate::shell::Shell;
use crate::shell::core_settings::resolve_core_settings_write;
use crate::{Backend, design};

use super::SettingsWidgets;

/// Ordinal of the Alerts strategy kind, matching MoonProto `StrategyKindId::ALERTS = 22`.
const ALERTS_KIND: u8 = 22;

/// Builds the rendered core's Default Alert Strategy row from that core's Alerts strategies. A
/// selection updates `Backend::default_alert_strategy[core]`, which is persisted in server config.
/// Enabling an alert applies this default only when the alert's existing `strategy_id` is zero.
pub(super) fn def_alert_strategy_row(
    core: Option<CoreId>,
    filter_input: &Entity<MoonInputState>,
    backend: &Entity<Backend>,
    p: MoonPalette,
    cx: &App,
) -> Option<AnyElement> {
    let core = core?;
    let b = backend.read(cx);
    let cur = b.alert_def_strategy(core);
    let filter = filter_input.read(cx).value().trim().to_lowercase();
    // Filter this core's Alerts strategies by the query. The em dash meaning no strategy always
    // remains first and is never filtered out.
    let mut options: Vec<(u64, String)> = vec![(0u64, "—".to_string())];
    options.extend(
        b.session
            .store()
            .core(core)?
            .strategies
            .iter()
            .filter(|s| s.kind_ordinal == ALERTS_KIND)
            .filter(|s| filter.is_empty() || s.name.to_lowercase().contains(&filter))
            .map(|s| (s.id, s.name.clone())),
    );
    // Render inline instead of using MoonDropdown because a nested menu overlay inside MoonPopover
    // is treated as an outside click and closes the popover before selection. Search and a
    // height-capped scroller keep hundreds of strategies compact, shrink for short lists, and keep
    // clicks inside the content.
    let mut list = v_flex().w_full().gap(design::ui_px(cx, 2.0));
    for (id, name) in options {
        let selected = id == cur;
        let backend2 = backend.clone();
        list = list.child(
            h_flex()
                .id(SharedString::from(format!("def-strat-{id}")))
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .px(design::ui_px(cx, 6.0))
                .py(design::ui_px(cx, 3.0))
                .rounded(design::r_button(cx))
                .cursor_pointer()
                .when(selected, |e| e.bg(rgba_from(p.accent, 0.16)))
                .hover(|e| e.bg(rgba_from(p.text, 0.06)))
                .child(
                    div()
                        .w(design::ui_px(cx, 12.0))
                        .text_color(rgb(p.accent))
                        .child(if selected { "✓" } else { "" }),
                )
                .child(
                    div()
                        .flex_1()
                        .text_color(rgb(if selected { p.text } else { p.text_soft }))
                        .child(name),
                )
                .on_click(move |_, _w, app| {
                    backend2.update(app, |bk, bcx| {
                        bk.set_alert_def_strategy(core, id);
                        bcx.notify();
                    });
                }),
        );
    }
    Some(
        v_flex()
            .w_full()
            .gap(design::ui_px(cx, 4.0))
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(p.text_muted))
                    .child(t!("core_settings.def_strategy").to_string()),
            )
            .child(
                MoonInput::new("core-def-strategy-filter")
                    .state(filter_input)
                    .small(),
            )
            .child(
                div()
                    .id("core-def-strategy-list")
                    .w_full()
                    .max_h(design::ui_px(cx, 150.0))
                    .overflow_y_scroll()
                    .child(list),
            )
            .into_any_element(),
    )
}

/// Builds the per-core manual-config opt-in checkbox for the popup's core.
///
/// UNLIKE [`cs_checkbox`] and every other control on this tab, this writes through
/// `Backend::set_core_manual_enabled` rather than a `ClientSettings`/core-config command: the flag
/// is per-core LOCAL terminal config, not something the core itself needs to acknowledge or the OK
/// button needs to stage. It therefore addresses `seeded` directly — the popup's own core — rather
/// than resolving through [`resolve_core_settings_write`], which exists to protect a value read at
/// RENDER time from the active trade core. Both rules answer "which core", but from different
/// premises: this checkbox must follow the popup it is drawn in, never the chart, or a user could
/// open a popup for core A, watch the active core change to B mid-render, and have the checkbox
/// silently start describing B.
pub(super) fn core_manual_checkbox(
    seeded: Option<CoreId>,
    backend: &Entity<Backend>,
    cx: &App,
) -> Option<AnyElement> {
    let core = seeded?;
    let checked = backend.read(cx).core_manual_enabled(core);
    let backend = backend.clone();
    Some(
        MoonCheckbox::new("core-manual-config")
            .label(t!("conn.use_core_manual_config").to_string())
            .checked(checked)
            .size(MoonCheckboxSize::Compact)
            .on_change(move |ch: &bool, _w, app| {
                let on = *ch;
                backend.update(app, |bk, _| bk.set_core_manual_enabled(core, on));
            })
            .into_any_element(),
    )
}

/// Builds a `ClientSettings` checkbox for the popup's core. `edit` constructs its boolean variant.
///
/// The box the user clicks was drawn from `checked`, read at RENDER time from the core that was
/// active then, so the toggled value belongs to that core and nowhere else — hence the same
/// [`resolve_core_settings_write`] guard the retained editors use.
pub(super) fn cs_checkbox(
    id: &str,
    label: String,
    checked: bool,
    backend: &Entity<Backend>,
    group: &str,
    seeded: Option<CoreId>,
    edit: fn(bool) -> ClientSettingsEdit,
) -> impl IntoElement {
    let backend = backend.clone();
    let group = group.to_string();
    MoonCheckbox::new(SharedString::from(id.to_string()))
        .label(label)
        .checked(checked)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| {
            let on = *ch;
            let b = backend.read(app);
            if let Some(core) = resolve_core_settings_write(seeded, b.active_trade_core(&group)) {
                if let Err(e) = b.session.edit_client_settings(core, edit(on)) {
                    log::warn!("core settings edit failed: {e:#}");
                }
            }
        })
}

/// Builds labeled Running (`is_started`) and Auto Detect (`auto_detect_active`) status dots for the
/// active core. Enabled states are green; inactive Auto Detect on a running core is amber; other
/// inactive states are gray. These moved from unlabeled dots beside the header gear button.
///
/// The state comes from the shared `CoreRunState` projection rather than from the raw store entry,
/// so this popup and the Profit Monitor's run column cannot disagree about an offline core: there,
/// both halves read as unknown however much the store still retains.
pub(super) fn runtime_status(run: CoreRunState, p: MoonPalette, cx: &App) -> impl IntoElement {
    let ok = design::positive_color(p);
    // Muted covers BOTH "reported as stopped" and "never reported": this popup has room for two
    // dots and no third colour to spare, and the run column in the Profit Monitor is where the
    // three states are told apart. What must not happen is a green dot for a core that said
    // nothing, which is what reading the raw field with `unwrap_or(false)` used to risk.
    let started_color = if run.started == Some(true) {
        ok
    } else {
        p.text_muted
    };
    let auto_color = match (run.auto_detect, run.started) {
        (Some(true), _) => ok,
        (Some(false), Some(true)) => p.amber,
        _ => p.text_muted,
    };
    // Both dots come from the same command, so one reconnect leaves both unconfirmed: the core came
    // back, MoonProto repeats no resync, and until it reports again this is last connection's
    // answer. Drawn faded and named in the tooltip — the same language the Profit Monitor's run
    // column uses.
    let faded = run.started.is_some() && !run.started_confirmed;
    let hint = if faded {
        Some(t!("core_run.unconfirmed_short").to_string())
    } else {
        None
    };
    let labeled = move |id: &'static str, color: u32, label: String, cx: &App| {
        h_flex()
            // A stable literal, never the localized label: an id built from user-facing text
            // changes under a live locale switch and can collide between two translations.
            .id(id)
            .items_center()
            .gap(design::ui_px(cx, 4.0))
            .when_some(hint.clone(), |row, hint| {
                row.tooltip(crate::panels::common::text_tooltip(hint))
            })
            .child(if faded {
                design::status_dot_stale(color, cx).into_any_element()
            } else {
                design::status_dot(color, cx).into_any_element()
            })
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(p.text_soft))
                    .child(label),
            )
    };
    h_flex()
        .items_center()
        .gap(design::ui_px(cx, 10.0))
        .child(labeled(
            "core-runtime-started",
            started_color,
            t!("core_settings.runtime_started").to_string(),
            cx,
        ))
        .child(labeled(
            "core-runtime-auto",
            auto_color,
            t!("core_settings.runtime_auto").to_string(),
            cx,
        ))
}

/// Build a draft-editing checkbox.
///
/// `set` is a plain function pointer rather than a closure so the handler can be moved into the
/// event callback without capturing the draft, which lives in `Shell` and must be read at event
/// time, never at render time.
pub(super) fn flag(
    id: &'static str,
    label: String,
    checked: bool,
    view: &Entity<Shell>,
    set: fn(&mut CoreConfig, bool),
) -> impl IntoElement {
    let view = view.clone();
    MoonCheckbox::new(SharedString::from(id))
        .label(label)
        .checked(checked)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| {
            let on = *ch;
            view.update(app, |this, cx| {
                this.edit_core_draft(|draft| set(draft, on), cx);
            });
        })
}

/// A caption in the popup's muted body colour.
pub(super) fn caption(text: String, p: MoonPalette, cx: &App) -> impl IntoElement {
    div()
        .text_size(design::t_caption(cx))
        .text_color(rgb(p.text_soft))
        .child(text)
}

/// Render one numeric editor, or nothing when its state was not prepared.
pub(super) fn num(widgets: &SettingsWidgets, id: &'static str, cx: &App) -> Option<AnyElement> {
    let f = widgets.field(id)?;
    Some(
        div()
            .w(design::font_w_px(cx, f.width))
            .child(
                MoonInput::new(SharedString::from(id))
                    .state(&f.state)
                    .small(),
            )
            .into_any_element(),
    )
}

/// Render one full-width slider, or nothing when its state was not prepared.
pub(super) fn slider(widgets: &SettingsWidgets, id: &'static str) -> Option<AnyElement> {
    let s = widgets.slider(id)?;
    Some(
        div()
            .w_full()
            .child(MoonSlider::new(&s.state).id(id).height(18.0))
            .into_any_element(),
    )
}

/// Render one editor that fills the row instead of taking a fixed width.
///
/// Used by the leverage "Config" line, whose rules text is long and has no natural column width.
pub(super) fn stretch_field(widgets: &SettingsWidgets, id: &'static str) -> Option<AnyElement> {
    let f = widgets.field(id)?;
    Some(
        div()
            .flex_1()
            .child(
                MoonInput::new(SharedString::from(id))
                    .state(&f.state)
                    .small(),
            )
            .into_any_element(),
    )
}
