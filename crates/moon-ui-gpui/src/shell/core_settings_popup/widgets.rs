//! Controls shared by the core-settings popup's tabs.
//!
//! Every write here is addressed through [`resolve_core_settings_write`] for the reason the module
//! header of the parent states: a control is drawn from the core that was active at RENDER time, and
//! the stacked repaint throttles allow the active core to move before the click lands.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonDropdown, MoonInput,
    MoonMenuSize, MoonPalette, MoonSlider, h_flex, v_flex,
};
use rust_i18n::t;

use moon_core::feed::{ClientSettingsEdit, CoreConfig};
use moon_core::session::{CoreId, CoreRunState};

use crate::panels::common::{RadioMark, radio_items, sound_preview_button};
use crate::shell::Shell;
use crate::shell::core_settings::resolve_core_settings_write;
use crate::{Backend, design};

use super::SettingsWidgets;

/// Ordinal of the Alerts strategy kind, matching MoonProto `StrategyKindId::ALERTS = 22`.
const ALERTS_KIND: u8 = 22;

/// Side of the sound-preview square, in font-scaled units: the height of the Action-sized
/// dropdown it sits beside, so the pair reads as one control.
const SOUND_PLAY_SIDE: f32 = 22.0;

/// Builds the rendered core's Default Alert Strategy row from that core's Alerts strategies. A
/// selection updates `Backend::default_alert_strategy[core]`, which is persisted in server config.
/// Enabling an alert applies this default only when the alert's existing `strategy_id` is zero.
///
/// A `MoonDropdown`, like the sound pickers below it and the filters everywhere else. It used to be
/// a hand-built inline list with its own search field, for one reason only: a menu overlay inside
/// this popover read as an outside click and shut the popup before the pick landed. The popover now
/// switches that dismissal off (see the core-settings gear in `chrome::terminal_chrome`), so the
/// bespoke list — and the retained input behind it — are gone.
///
/// No search box comes back with it. The one searchable picker in this application
/// (`panels::report`'s `MoonCombobox`) needs a state entity built with a `Window` and a per-frame
/// sync flush, which is a great deal of machinery for a list already narrowed to ONE strategy kind;
/// the menu scrolls instead. A core that really does carry hundreds of Alerts strategies is the
/// signal to trade up, and the swap is contained to this function.
///
/// Args:
///     core: Core whose default this row edits, or `None` when no core is active.
///     backend: Application state read for the strategy list and written on selection.
///     p: Active palette.
///     cx: Application context used for font-scaled geometry.
///
/// Returns:
///     The row, or `None` when there is no core to edit.
pub(super) fn def_alert_strategy_row(
    core: Option<CoreId>,
    backend: &Entity<Backend>,
    p: MoonPalette,
    cx: &App,
) -> Option<AnyElement> {
    let core = core?;
    let b = backend.read(cx);
    let cur = b.alert_def_strategy(core);
    // The em dash means "no strategy" and always leads the list.
    let mut options: Vec<(u64, SharedString)> = vec![(0u64, SharedString::from("—"))];
    options.extend(
        b.session
            .store()
            .core(core)?
            .strategies
            .iter()
            .filter(|st| st.kind_ordinal == ALERTS_KIND)
            .map(|st| (st.id, SharedString::from(st.name.clone()))),
    );
    let label = options
        .iter()
        .find(|(id, _)| *id == cur)
        .map(|(_, name)| name.to_string())
        // A default pointing at a strategy this core no longer lists: show the id rather than
        // silently reading as "no strategy", which is what an unmatched dropdown would look like.
        .unwrap_or_else(|| format!("#{cur}"));
    let backend = backend.clone();
    let items = radio_items(
        options
            .into_iter()
            .map(|(id, name)| (id, SharedString::from(format!("core-def-strat-{id}")), name)),
        cur,
        RadioMark::Check,
        move |app, id| {
            backend.update(app, |bk, cx| {
                bk.set_alert_def_strategy(core, id);
                cx.notify();
            });
        },
    );
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
                MoonDropdown::new("core-def-strategy")
                    .label(label)
                    .trigger_caret(true)
                    .trigger_variant(MoonButtonVariant::Soft)
                    .trigger_size(MoonButtonSize::Action)
                    // Fits the chosen name between a readable floor and the popup's own
                    // width, so a long strategy name is not clipped to a fixed trigger.
                    .fit_trigger_width(120.0, 240.0)
                    .menu_width_scaled(240.0)
                    .menu_max_height_ui(220.0)
                    .menu_size(MoonMenuSize::Compact)
                    .items(items),
            )
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

/// Build the sound picker for one price-approach alert: Moonbot's list in a dropdown, with the
/// square play button that sounds what is selected.
///
/// The same pair the core-warning settings already use (`panels::core_status::config_popup`),
/// built from the same two helpers, so the two sound pickers in this application cannot drift
/// apart. Picking does NOT play; the preview button does, exactly as it does there.
///
/// A `MoonDropdown` can live here only because the gear popover switches outside-click dismissal
/// off; see the core-settings gear in `chrome::terminal_chrome`.
///
/// A core holding an ordinal this build has no name for shows that NUMBER rather than a guess,
/// and the preview goes dim: naming it `Alarm` would write that guess back on the next OK.
///
/// Args:
///     id: Element-id prefix, unique per row.
///     current: The 1-based ordinal the draft holds.
///     view: Shell entity the selection stages into.
///     set: Writes the picked ordinal into the draft.
///     p: Active palette.
///     cx: Application context used for font-scaled geometry.
///
/// Returns:
///     The dropdown and its preview button.
pub(super) fn sound_cell(
    id: &'static str,
    current: i32,
    view: &Entity<Shell>,
    set: fn(&mut CoreConfig, i32),
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    let name = crate::media::sound::mb_sound_name(current);
    let label = name.map_or_else(|| format!("#{current}"), str::to_string);
    let view = view.clone();
    let options = crate::media::sound::MB_SOUNDS
        .iter()
        .enumerate()
        .map(|(i, n)| {
            // The wire ordinal is 1-based; see `media::sound::MB_SOUNDS`.
            let ordinal = i as i32 + 1;
            (
                ordinal,
                SharedString::from(format!("{id}-{ordinal}")),
                SharedString::from(*n),
            )
        });
    let items = radio_items(options, current, RadioMark::Check, move |app, ordinal| {
        view.update(app, |this, cx| {
            this.edit_core_draft(|draft| set(draft, ordinal), cx);
        });
    });
    h_flex()
        .items_center()
        .gap(design::ui_px(cx, 4.0))
        .child(
            MoonDropdown::new(SharedString::from(id))
                .label(label)
                .trigger_caret(true)
                .trigger_variant(MoonButtonVariant::Soft)
                .trigger_size(MoonButtonSize::Action)
                .trigger_width_scaled(94.0)
                .menu_width_scaled(128.0)
                .menu_size(MoonMenuSize::Compact)
                .items(items),
        )
        .child(sound_preview_button(
            SharedString::from(format!("{id}-play")),
            name,
            design::ui_px(cx, SOUND_PLAY_SIDE),
            p,
            cx,
        ))
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
