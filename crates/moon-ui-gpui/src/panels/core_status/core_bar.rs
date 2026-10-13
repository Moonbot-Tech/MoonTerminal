//! Core Status scope selector and presentation toolbar.

use std::collections::HashSet;

use super::ip_cell::mask_affordance;
use super::table;
use super::{CoreStatusMode, CoreStatusView};
use crate::design;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonInput, MoonMenuItem,
    MoonPalette, MoonSegmentItem, MoonSegmentSelection, MoonSegmentedControl, MoonTone, h_flex,
};
use rust_i18n::t;

impl CoreStatusView {
    /// Render the effective core scope and the view toolbar.
    ///
    /// Classic mode exposes the retained multi-selector and exchange batch toggles. Auto mode
    /// renders a non-interactive pinned scope chip naming the workspace label instead, without
    /// changing retained Classic state. The rest of the bar is the segmented view switcher, the
    /// search field, and the IP, column, and warnings controls. The row wraps when the dock is
    /// narrow; it does not clip.
    ///
    /// Args:
    ///     scope: Memoized toolbar scope: picker cores, effective selection and pinned label.
    ///     hidden: Column ids unchecked in the Columns menu, read once for this frame.
    ///     cx: View context used to read exchanges and wire selection callbacks.
    ///
    /// Returns:
    ///     The toolbar row.
    pub(super) fn core_bar(
        &self,
        scope: &super::frame_cache::ScopeModel,
        hidden: &HashSet<String>,
        segments: &super::health::SegmentCounts,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let cores = &scope.cores;
        let workspace_owned = scope.workspace_owned;
        let selection = if workspace_owned {
            &scope.effective_selection
        } else {
            &self.sel_cores
        };
        let combo: AnyElement = if workspace_owned {
            let p = MoonPalette::active(cx);
            let label = scope.owned_label.clone().unwrap_or_default();
            let width = px(crate::controls::pinned_scope_width(
                cx,
                &label,
                crate::controls::CORE_COMBO_TRIGGER_W,
                crate::controls::PINNED_SCOPE_TRIGGER_MAX_W,
            ));
            pinned_scope_flat(label, width, p, cx)
        } else {
            let view = cx.entity();
            let exchange_view = view.clone();
            let venues = self.backend.read(cx).session.core_venues();
            let extras =
                crate::controls::core_combo_extras(!workspace_owned, &view, &self.backend, cx);
            crate::controls::core_combo(
                "core-status-core",
                cores,
                venues,
                selection,
                crate::controls::CoreAllRowMode::ImplicitOrComplete,
                t!("core_status.all_cores").to_string(),
                |n| t!("core_status.cores_n", n = n).to_string(),
                170.0,
                extras,
                cx,
                move |id, app| {
                    view.update(app, |t, c| t.toggle_core(id, c));
                },
                move |exchange_cores, app| {
                    exchange_view.update(app, |t, c| {
                        t.toggle_exchange_cores(exchange_cores, c);
                    });
                },
            )
            .into_any_element()
        };
        let bar = toolbar_row(
            combo,
            self.mode_switch(segments, cx),
            self.search_field(cx),
            self.toolbar_tail(hidden, cx),
            cx,
        );
        let weak = cx.entity().downgrade();
        div()
            .w_full()
            .min_w(px(0.0))
            .relative()
            .child(
                canvas(
                    move |bounds, _, app| {
                        let w = f32::from(bounds.size.width);
                        let Some(view) = weak.upgrade() else {
                            return;
                        };
                        if (view.read(app).toolbar_measured_w - w).abs() <= 0.5 {
                            return;
                        }
                        let weak = view.downgrade();
                        app.defer(move |app| {
                            let Some(view) = weak.upgrade() else {
                                return;
                            };
                            view.update(app, |this, cx| {
                                if (this.toolbar_measured_w - w).abs() > 0.5 {
                                    this.toolbar_measured_w = w;
                                    cx.notify();
                                }
                            });
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(bar)
    }

    /// Segmented view switcher with the counts the mockup puts on each view.
    ///
    /// Args:
    ///     segments: Counts derived from the strip's own figures ([`super::health::segment_counts`]).
    ///     cx: View context used to wire the click.
    ///
    /// Returns:
    ///     A filled joined strip. Alarms shows the 24-hour count. Updates shows its count at zero.
    fn mode_switch(
        &self,
        segments: &super::health::SegmentCounts,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let weak_view = cx.entity().downgrade();
        mode_switch_control(
            segments.cores,
            segments.servers,
            segments.problems,
            segments.alarms,
            segments.updates,
            self.mode,
            self.toolbar_measured_w,
            move |index, _, app| {
                let Some(view) = weak_view.upgrade() else {
                    return;
                };
                let mode = match index {
                    0 => CoreStatusMode::Flat,
                    1 => CoreStatusMode::Servers,
                    2 => CoreStatusMode::Problems,
                    3 => CoreStatusMode::Warnings,
                    _ => CoreStatusMode::Updates,
                };
                view.update(app, |this, cx| this.set_mode(mode, cx));
            },
            cx,
        )
    }

    /// Toolbar search. [`toolbar_row`] clamps the slot between 140 and 240 density-scaled pixels.
    ///
    /// Args:
    ///     cx: View context used for the density scale.
    ///
    /// Returns:
    ///     The search field, without a width of its own.
    fn search_field(&self, cx: &Context<Self>) -> impl IntoElement {
        let enabled = matches!(self.mode, CoreStatusMode::Flat | CoreStatusMode::Servers);
        let muted = design::moon(MoonPalette::active(cx).text_muted);
        MoonInput::new("core-status-search")
            .state(&self.search_input)
            .placeholder(t!("core_status.search.placeholder").to_string())
            .size(design::INPUT_SIZE)
            .cleanable(true)
            .disabled(!enabled)
            .prefix(
                moon_ui::MoonIcon::empty()
                    .path("icons/search.svg")
                    .text_color(muted)
                    .w(px(14.0))
                    .h(px(14.0)),
            )
    }

    /// IP mask, column menu, and the warnings gear, packed to the trailing edge.
    ///
    /// Args:
    ///     hidden: Column ids unchecked in the Columns menu, read once for this frame.
    ///     cx: View context used to build the controls.
    ///
    /// Returns:
    ///     The trailing group. [`toolbar_row`] places it on the trailing edge. The IP button
    ///     id is not the Servers header's id.
    fn toolbar_tail(&self, hidden: &HashSet<String>, cx: &Context<Self>) -> impl IntoElement {
        let weak_view = cx.entity().downgrade();
        toolbar_tail_el(
            self.ip_masked,
            self.columns_menu(hidden, cx),
            self.warn_gear(cx),
            move |_, _, app| {
                let Some(view) = weak_view.upgrade() else {
                    return;
                };
                view.update(app, |this, cx| this.toggle_ip_mask(cx));
            },
            cx,
        )
    }

    /// Checkbox menu over every Flat column, persisted with that table's widths.
    ///
    /// Args:
    ///     hidden: Column ids unchecked in the Columns menu.
    ///     cx: View context used to wire each toggle.
    ///
    /// Returns:
    ///     A dropdown that stays open while columns are checked. The last checked column
    ///     cannot be cleared.
    fn columns_menu(&self, hidden: &HashSet<String>, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        columns_menu_el(
            hidden,
            move |column, app| {
                view.update(app, |this, cx| this.toggle_flat_column(&column, cx));
            },
            cx,
        )
    }
}

/// The pinned Auto-mode scope as a flat label: a filled box with no border and no dot glyph,
/// so it never reads as a member of the view switch beside it.
///
/// Core Status only. The other panels keep the shared [`crate::panels::pinned_scope_host`] chip.
/// It is not clickable, exactly like that chip, and carries the same tooltip.
///
/// Args:
///     label: The pinned scope's name, such as the workspace overview.
///     width: Rendered width, already scaled by the caller.
///     p: Active palette.
///     cx: Application context used for font-scaled geometry.
///
/// Returns:
///     The tooltip-hosting flat label.
fn pinned_scope_flat(label: String, width: Pixels, p: MoonPalette, cx: &App) -> AnyElement {
    div()
        .id("core-status-core-tip")
        .flex_none()
        .tooltip(crate::panels::common::text_tooltip(
            t!("workspace.scope.pinned_hint", scope = label.clone()).to_string(),
        ))
        .child(
            h_flex()
                .id("core-status-core")
                .debug_selector(|| "cs-scope-flat".into())
                .flex_none()
                .w(width)
                .h(design::action_control_h_px(cx))
                .items_center()
                .px(design::ui_px(cx, design::PINNED_SCOPE_PAD_X))
                .rounded(design::r_button(cx))
                .bg(rgb(p.panel_head))
                .font_family(design::mono())
                .text_size(design::ui_px(
                    cx,
                    design::CONTROL_TIER.control_metrics().font_size,
                ))
                .text_color(rgb(design::chrome_label_color(p)))
                .child(div().min_w_0().truncate().child(label)),
        )
        .into_any_element()
}

/// Space between the scope control and its separating rule, in design pixels.
const SCOPE_RULE_PAD: f32 = 8.0;

/// Toolbar row. The four slots wrap onto a second line; nothing is clipped to stay on one.
///
/// Args:
///     scope: Core-scope combo or pinned chip.
///     modes: Segmented view switch.
///     search: Search field. This function clamps its slot.
///     tail: IP mask, column menu, and warnings gear.
///     cx: Application context used for the density scale.
///
/// Returns:
///     The wrapping row. The tail keeps the trailing edge of whichever line it lands on.
pub(super) fn toolbar_row(
    scope: impl IntoElement,
    modes: impl IntoElement,
    search: impl IntoElement,
    tail: impl IntoElement,
    cx: &App,
) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    h_flex()
        .id("core-status-toolbar")
        .debug_selector(|| "cs-toolbar".into())
        .w_full()
        .min_w(px(0.0))
        .flex_wrap()
        .flex_none()
        .items_center()
        .gap(design::ui_px(cx, 8.0))
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 8.0))
        .bg(rgb(p.panel_high))
        .font_family(design::ui_font())
        // The scope control filters which cores the panel covers; it is not a view. A rule on
        // its right edge keeps it from reading as a sixth segment of the switch beside it.
        .child(
            div()
                .debug_selector(|| "cs-toolbar-scope".into())
                .flex_none()
                .pr(design::ui_px(cx, SCOPE_RULE_PAD))
                .border_r_1()
                .border_color(rgb(p.border))
                .child(scope),
        )
        .child(
            div()
                .debug_selector(|| "cs-toolbar-modes".into())
                .flex_none()
                .max_w_full()
                .overflow_hidden()
                .child(modes),
        )
        .child(
            div()
                .debug_selector(|| "cs-toolbar-search".into())
                .flex_1()
                .min_w(design::ui_px(cx, 140.0))
                .max_w(design::ui_px(cx, 240.0))
                .child(search),
        )
        .child(
            div()
                .debug_selector(|| "cs-toolbar-tail".into())
                .ml_auto()
                .flex_none()
                .child(tail),
        )
}

/// Segmented view switcher with the counts the mockup puts on each view.
///
/// Args:
///     flat_n: Cores in the flat snapshot.
///     server_n: Server cards.
///     problem_n: Distinct cores needing attention, the verdict's own figure.
///     alarms: Alarms in the last 24 hours. Shown on the Alarms segment, including zero.
///     lagging: Cores behind the fleet build. Shown on Updates, including zero.
///     mode: The view on screen.
///     available: Measured toolbar width in pixels. `0` has not been measured and keeps the
///         full labels with counts, so the first frame does not flash the short forms.
///     on_click: Segment index, in the same order as the items.
///     cx: Application context used to fit each segment.
///
/// Returns:
///     A filled joined strip. A narrow dock drops counts, then switches to the short labels,
///     then lowers the segment maximum so all five stay inside.
pub(super) fn mode_switch_control(
    flat_n: usize,
    server_n: usize,
    problem_n: usize,
    alarms: usize,
    lagging: usize,
    mode: CoreStatusMode,
    available: f32,
    on_click: impl Fn(usize, &ClickEvent, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let (items, tier) = fitted_mode_items(
        flat_n, server_n, problem_n, alarms, lagging, mode, available, cx,
    );
    let tier_id = match tier {
        ModeLabel::Full => "cs-mode-tier-full",
        ModeLabel::Plain => "cs-mode-tier-plain",
        ModeLabel::Short => "cs-mode-tier-short",
    };
    div()
        .debug_selector(move || tier_id.into())
        .flex_none()
        .child(
            MoonSegmentedControl::new("core-status-mode")
                .selection(MoonSegmentSelection::Filled)
                .items(items)
                .on_click(move |index, event, _, app| on_click(index, event, app)),
        )
}

/// How much of each mode name the strip is allowed to draw.
#[derive(Clone, Copy)]
enum ModeLabel {
    /// Translated name plus the count.
    Full,
    /// Translated name, no count.
    Plain,
    /// Short name, no count.
    Short,
}

/// The five modes in strip order.
const MODE_ORDER: [CoreStatusMode; 5] = [
    CoreStatusMode::Flat,
    CoreStatusMode::Servers,
    CoreStatusMode::Problems,
    CoreStatusMode::Warnings,
    CoreStatusMode::Updates,
];

/// Build the five segments, degrading until they fit `available`.
///
/// The minimum is the label plus the control's own padding. There is no 64 px floor.
fn fitted_mode_items(
    flat_n: usize,
    server_n: usize,
    problem_n: usize,
    alarms: usize,
    lagging: usize,
    mode: CoreStatusMode,
    available: f32,
    cx: &App,
) -> (Vec<MoonSegmentItem>, ModeLabel) {
    // Toolbar padding is 12 design px on each side. `available` is the measured row.
    let pad = f32::from(design::ui_px(cx, 12.0)) * 2.0;
    let budget = if available <= 0.5 {
        f32::MAX
    } else {
        (available - pad).max(0.0)
    };
    for label in [ModeLabel::Full, ModeLabel::Plain, ModeLabel::Short] {
        let items = mode_items(
            flat_n, server_n, problem_n, alarms, lagging, mode, label, 0.0, 168.0, cx,
        );
        if items_width(&items) <= budget + 0.5 {
            return (items, label);
        }
    }
    // 64 here is only the probe's design width, so a rendered pixel converts back to a
    // design pixel. It is not a floor on the segments.
    let probe = MoonSegmentItem::new("", "X").fit_width(cx, 64.0, 64.0);
    let scaled = probe.resolved_width().max(1.0);
    let design_max = ((budget / 5.0) / (scaled / 64.0)).max(1.0);
    (
        mode_items(
            flat_n,
            server_n,
            problem_n,
            alarms,
            lagging,
            mode,
            ModeLabel::Short,
            0.0,
            design_max,
            cx,
        ),
        ModeLabel::Short,
    )
}

fn mode_items(
    flat_n: usize,
    server_n: usize,
    problem_n: usize,
    alarms: usize,
    lagging: usize,
    mode: CoreStatusMode,
    label: ModeLabel,
    min_w: f32,
    max_w: f32,
    cx: &App,
) -> Vec<MoonSegmentItem> {
    MODE_ORDER
        .into_iter()
        .map(|item| {
            let count = match (label, item) {
                (ModeLabel::Full, CoreStatusMode::Flat) => Some(flat_n),
                (ModeLabel::Full, CoreStatusMode::Servers) => Some(server_n),
                (ModeLabel::Full, CoreStatusMode::Problems) => Some(problem_n),
                (ModeLabel::Full, CoreStatusMode::Warnings) => Some(alarms),
                (ModeLabel::Full, CoreStatusMode::Updates) => Some(lagging),
                _ => None,
            };
            counted_segment(
                cx,
                mode_label(item, matches!(label, ModeLabel::Short)),
                count,
                mode_count_tone(item),
                item == mode,
                min_w,
                max_w,
            )
        })
        .collect()
}

fn items_width(items: &[MoonSegmentItem]) -> f32 {
    items.iter().map(MoonSegmentItem::resolved_width).sum()
}

/// Translated mode name. `short` selects the narrow-dock key.
fn mode_label(mode: CoreStatusMode, short: bool) -> String {
    let key = match (mode, short) {
        (CoreStatusMode::Flat, false) => "core_status.mode.flat",
        (CoreStatusMode::Flat, true) => "core_status.mode.flat_short",
        (CoreStatusMode::Servers, false) => "core_status.mode.servers",
        (CoreStatusMode::Servers, true) => "core_status.mode.servers_short",
        (CoreStatusMode::Problems, false) => "core_status.mode.problems",
        (CoreStatusMode::Problems, true) => "core_status.mode.problems_short",
        (CoreStatusMode::Warnings, false) => "core_status.mode.warnings",
        (CoreStatusMode::Warnings, true) => "core_status.mode.warnings_short",
        (CoreStatusMode::Updates, false) => "core_status.mode.updates",
        (CoreStatusMode::Updates, true) => "core_status.mode.updates_short",
    };
    t!(key).to_string()
}

/// IP mask, column menu, and the warnings gear.
///
/// Args:
///     ip_masked: Whether addresses are hidden. Chooses the eye icon.
///     menu: Columns dropdown, already built.
///     gear: Warnings control. The closed gear is a button; the popover is the caller's.
///     on_ip: Toggles the mask.
///     cx: Application context used for the density scale.
///
/// Returns:
///     The trailing group. [`toolbar_row`] owns the trailing-edge margin.
pub(super) fn toolbar_tail_el(
    ip_masked: bool,
    menu: impl IntoElement,
    gear: impl IntoElement,
    on_ip: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let (icon, tooltip) = mask_affordance(ip_masked);
    h_flex()
        .flex_none()
        .items_center()
        .gap(design::ui_px(cx, 4.0))
        .child(
            MoonButton::new("core-status-toolbar-ip")
                .size(MoonButtonSize::density(cx))
                .ghost()
                .icon(icon)
                .tooltip(t!(tooltip).to_string())
                .on_click(on_ip),
        )
        .child(menu)
        .child(gear)
}

/// Checkbox menu over every Flat column, persisted with that table's widths.
///
/// Args:
///     hidden: Column ids unchecked in the Columns menu.
///     on_toggle: Column id the operator clicked. A disabled last column does not fire.
///     cx: Application context used to size the trigger.
///
/// Returns:
///     A dropdown that stays open while columns are checked. The last checked column
///     cannot be cleared.
pub(super) fn columns_menu_el<T: Fn(String, &mut App) + Clone + 'static>(
    hidden: &HashSet<String>,
    on_toggle: T,
    cx: &App,
) -> impl IntoElement + use<T> {
    let keys = table::order_keys();
    let visible_count = keys.iter().filter(|key| !hidden.contains(**key)).count();
    let mut menu = MoonDropdown::new("core-status-columns")
        .label(t!("core_status.columns.menu").to_string())
        .trigger_variant(MoonButtonVariant::Ghost)
        .trigger_size(MoonButtonSize::density(cx))
        .trigger_caret(true)
        .fit_trigger_width(72.0, 140.0)
        .fit_menu_width(140.0, 240.0)
        .close_on_select(false);
    for key in keys {
        let shown = !hidden.contains(*key);
        let last = shown && visible_count == 1;
        let on_toggle = on_toggle.clone();
        let column = (*key).to_string();
        menu = menu.item(
            MoonMenuItem::with_key(*key, column_label(key))
                .checked(shown)
                .disabled(last)
                .on_click(move |_, _, app| on_toggle(column.clone(), app)),
        );
    }
    menu
}

/// One segment cell. A count is drawn as a pill after the name.
///
/// Args:
///     cx: View context used to fit the cell.
///     label: Translated view name.
///     count: Cores or servers to show. `None` draws the name alone, including a zero the
///         caller chose to omit.
///     tone: Pill tone for a non-zero count. A zero count stays neutral.
///     selected: Whether this view is the one on screen.
///     min_w: Minimum fitted width in design pixels. The control scales it.
///     max_w: Maximum fitted width in design pixels.
///
/// Returns:
///     A fitted segment whose width reserves the pill.
fn counted_segment(
    cx: &App,
    label: String,
    count: Option<usize>,
    tone: MoonTone,
    selected: bool,
    min_w: f32,
    max_w: f32,
) -> MoonSegmentItem {
    let mut item = MoonSegmentItem::new("", label);
    if let Some(n) = count {
        // `count` goes before `fit_width`, so the fitted width reserves the pill.
        item = item
            .count(n.to_string())
            .count_tone(if n == 0 { MoonTone::Default } else { tone });
    }
    item.fit_width(cx, min_w, max_w.max(min_w))
        .selected(selected)
}

/// Pill tone of a view's count: Problems red, Alarms and Updates amber, the rest neutral.
///
/// [`counted_segment`] keeps a zero count neutral whatever this returns.
fn mode_count_tone(mode: CoreStatusMode) -> MoonTone {
    match mode {
        CoreStatusMode::Problems => MoonTone::Danger,
        CoreStatusMode::Warnings | CoreStatusMode::Updates => MoonTone::Warning,
        CoreStatusMode::Flat | CoreStatusMode::Servers => MoonTone::Default,
    }
}

/// Translated Flat column heading for the menu.
///
/// Args:
///     key: Canonical column id.
///
/// Returns:
///     The column's existing heading. An unknown id is returned unchanged.
fn column_label(key: &str) -> String {
    match key {
        "server" => t!("core_status.col.server").to_string(),
        "core" => t!("core_status.col.core").to_string(),
        "version" => t!("core_status.col.version").to_string(),
        "cpu_proc" => t!("core_status.col.cpu_proc").to_string(),
        "cpu_sys" => t!("core_status.col.cpu_sys").to_string(),
        "mem_used" => t!("core_status.col.mem_used").to_string(),
        "free_phys" => t!("core_status.col.free_phys").to_string(),
        "ping" => t!("core_status.col.ping").to_string(),
        "ping_exch" => t!("core_status.col.ping_exch").to_string(),
        "cpus" => t!("core_status.col.cpus").to_string(),
        "api_key" => t!("core_status.col.api_key").to_string(),
        "api_quota" => t!("core_status.col.api_quota").to_string(),
        "startup" => t!("core_status.col.startup").to_string(),
        "tz_off" => t!("core_status.col.tz_off").to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests;
