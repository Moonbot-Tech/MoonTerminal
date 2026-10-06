//! Auto rail row rendering and the shared dock host.

use super::*;

/// Separator between the rail summary's three rendered segments.
///
/// Must match `workspace.summary`'s own spacing byte for byte (space, U+00B7 MIDDLE DOT, space):
/// the tooltip still renders that original one-string value, so a mismatch would put two
/// different-looking separators on screen at once.
pub(super) const SUMMARY_SEP: &str = " · ";

/// Render the absolute-fill host used by both Classic and Auto around the one DockArea.
///
/// Args:
///     dock: Shared group dock entity.
///
/// Returns:
///     Full-height flexible clipped body that does not create another dock or panel instance. The
///     explicit cross-axis height is required because MoonUI's horizontal flex centers children
///     and the absolute DockArea child contributes no intrinsic height of its own.
pub(super) fn dock_host(dock: Entity<moon_ui::DockArea>) -> impl IntoElement {
    div()
        .relative()
        .flex_1()
        .h_full()
        .w_full()
        .min_h_0()
        .overflow_hidden()
        .child(
            div()
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .left_0()
                .child(dock),
        )
}

/// Render one virtualized Overview, group, or configured-core row.
///
/// Args:
///     item: Flattened roster item.
///     density: Current responsive rail rung.
///     run: Slots and exchange-heading preference resolved once per rail render.
///     p: Active Moon palette.
///     backend: Shared state used by click actions.
///     current_group: Group window that owns this rail.
///     cx: Application context used for scaled geometry and callbacks.
///
/// Returns:
///     Complete fixed-height row with localized status and tooltip behavior.
pub(super) fn render_rail_item(
    item: RailItem,
    density: WorkspaceRailDensity,
    run: RailRun,
    p: MoonPalette,
    backend: Entity<Backend>,
    current_group: String,
    cx: &mut App,
) -> AnyElement {
    match item {
        RailItem::Overview { selected } => {
            let label = t!("workspace.overview").to_string();
            let tooltip = t!("workspace.overview_tip", group = current_group.clone()).to_string();
            let visible = match density {
                WorkspaceRailDensity::Icon => label.chars().next().unwrap_or('?').to_string(),
                WorkspaceRailDensity::Full | WorkspaceRailDensity::Compact => label,
            };
            let click_backend = backend.clone();
            let click_group = current_group.clone();
            rail_row_base("workspace-overview", selected, true, 9.0, 7.0, p, cx)
                // This row is a MODE, not a core: it aggregates every core at once, while every
                // row below it selects exactly one. Weight, a step up in size and a rule beneath
                // separate it from the list it sits on top of, without giving it a surface of its
                // own — the rail already reads as one recessed pane. ONE step up, not `t_title`:
                // the virtual list's row height is fixed and does not track the legacy font-delta
                // channel, so a three-step jump clips its own text at the top of that channel's
                // range.
                .text_size(design::t_body_lg(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .border_b_1()
                .border_color(rgb(p.border_soft))
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .justify_center()
                        .gap(design::ui_px(cx, 7.0))
                        .child(design::status_dot_sized(p.accent, 7.0, cx))
                        .child(div().min_w_0().truncate().child(visible)),
                )
                .tooltip(move |_window, cx| {
                    cx.new(|_| MoonTooltipView::new(tooltip.clone())).into()
                })
                .on_click(move |_, _, cx| {
                    click_backend.update(cx, |backend, backend_cx| {
                        backend.select_auto_workspace_core(&click_group, None, backend_cx);
                    });
                })
                .into_any_element()
        }
        RailItem::Exchange {
            venue,
            logo,
            section,
            cores,
        } => {
            let label = crate::controls::venue_section_label(venue.as_ref());
            let tooltip = label.clone();
            // Keyed on the venue IDENTITY, never on the caption: an element id built from rendered
            // text changes with the locale and with a core build's spelling, which makes GPUI treat
            // the same heading as a different element and drop its hover and tooltip state.
            let row_id = SharedString::from(match venue.as_ref() {
                Some(venue) => format!("workspace-exchange-{}-{}", venue.id.code, venue.id.dex),
                None => "workspace-exchange-unknown".to_string(),
            });
            let compact_label = match density {
                WorkspaceRailDensity::Icon if logo.is_none() => {
                    Some(label.chars().next().unwrap_or('?').to_string())
                }
                WorkspaceRailDensity::Icon => None,
                WorkspaceRailDensity::Full | WorkspaceRailDensity::Compact => Some(label),
            };
            let controls = (run.exchange_controls
                && density != WorkspaceRailDensity::Icon
                && !cores.is_empty())
            .then_some(RunScope {
                key: RunKey::Section(section),
                cores,
                reserve: run.slots,
                offers: run.slots,
            })
            .and_then(|scope| run_cell(&scope, &backend, p, cx));
            // The gap is paid out of this 30-unit cell (`design::RAIL_SECTION_GAP`), which is why
            // it is 6 and not 8: at font +6 the caption line box is ~18 px, leaving room inside
            // the remaining 23 units. The outer element stays transparent so the rail's own
            // `p.gutter` shows through as the section gap; the inner row alone paints the solid
            // one-step-up background, so the heading itself reads as elevated, not the gap above
            // it.
            div()
                .id(row_id)
                .size_full()
                .min_w_0()
                .pt(design::ui_px(cx, design::RAIL_SECTION_GAP))
                .child(
                    h_flex()
                        .flex_1()
                        .h_full()
                        .items_center()
                        .gap(design::ui_px(cx, 6.0))
                        .px(design::ui_px(cx, 8.0))
                        .min_w_0()
                        .text_size(design::t_caption(cx))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(p.text_muted))
                        .bg(rgb(p.panel_high))
                        .border_b_1()
                        .border_color(rgb(p.border_soft))
                        .children(controls)
                        .when_some(logo, |row, logo| {
                            row.child(
                                img(logo)
                                    .flex_none()
                                    .w(design::ui_px(cx, 13.0))
                                    .h(design::ui_px(cx, 13.0))
                                    .rounded(design::ui_px(cx, 2.0)),
                            )
                        })
                        .when_some(compact_label, |row, label| {
                            row.child(div().min_w_0().truncate().child(label))
                        }),
                )
                .tooltip(move |_window, cx| {
                    cx.new(|_| MoonTooltipView::new(tooltip.clone())).into()
                })
                .into_any_element()
        }
        RailItem::Core {
            row,
            is_last_in_section,
            cores,
        } => {
            let status = workspace_status_text(row.status);
            let tooltip = workspace_core_tooltip(&row);
            let dot = workspace_status_color(row.status, p);
            let name = match density {
                WorkspaceRailDensity::Icon => row.name.chars().next().unwrap_or('?').to_string(),
                WorkspaceRailDensity::Full | WorkspaceRailDensity::Compact => row.name.clone(),
            };
            let metrics = core_rail_metrics(density);
            // Enlarges for every status the summary's `problem` count includes (`Unavailable` as
            // well as `Problem`, see `workspace.rs`'s roster tally), so a counted row is always
            // findable. Colour still separates severity: `Unavailable` keeps `p.amber` and
            // `Problem` keeps the danger colour via `workspace_status_color`, untouched below —
            // only the pill's danger treatment stays `Problem`-only.
            let dot_size = if matches!(
                row.status,
                WorkspaceCoreStatus::Unavailable | WorkspaceCoreStatus::Problem
            ) {
                metrics.dot_size + design::RAIL_PROBLEM_DOT_STEP
            } else {
                metrics.dot_size
            };
            let vertical_stem = div()
                .absolute()
                .left_0()
                .top_0()
                .w(px(1.0))
                .bg(rgb(p.border_soft))
                .when(is_last_in_section, |stem| stem.h(design::ui_px(cx, 14.5)))
                .when(!is_last_in_section, |stem| stem.bottom_0());
            let scope = RunScope {
                key: RunKey::Core(row.core),
                cores,
                reserve: run.slots,
                // A row that is not Ready shows its CONNECTION dot in the status slot (below), so
                // it offers no runtime status of its own there — and therefore no restart button
                // either.
                offers: RunSlots {
                    status: run.slots.status && row.status == WorkspaceCoreStatus::Ready,
                    ..run.slots
                },
            };
            let ready = row.status == WorkspaceCoreStatus::Ready;
            let mut content = h_flex()
                .size_full()
                .min_w_0()
                .gap(design::ui_px(cx, metrics.gap))
                .child(
                    div()
                        .relative()
                        .flex_none()
                        .h_full()
                        .w(design::ui_px(cx, metrics.connector_width))
                        .child(vertical_stem)
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .top(design::ui_px(cx, 14.5))
                                .w(design::ui_px(cx, metrics.connector_elbow_width))
                                .h(px(1.0))
                                .bg(rgb(p.border_soft)),
                        ),
                );
            if run.slots.status {
                // The rail's own connection colour outranks the runtime dot: "cannot reach the
                // core" is the fact a stopped-runtime dot would otherwise hide, and it keeps its
                // enlarged Problem size.
                let connection = (!ready)
                    .then(|| design::status_dot_sized(dot, dot_size, cx).into_any_element());
                content =
                    content.children(run_cell_with_status(&scope, connection, &backend, p, cx));
            } else {
                content = content.child(design::status_dot_sized(dot, dot_size, cx));
                content = content.children(run_cell(&scope, &backend, p, cx));
            }
            content = content.child(div().flex_1().min_w_0().truncate().child(name));
            if workspace_status_label_visible(row.status, density) {
                // The pill stays `Problem`-only, deliberately narrower than the dot above: it is
                // the danger treatment, and `Unavailable` is not danger — it already gets its own
                // plain label below via `workspace_status_label_visible`. Do not widen this back to
                // match the dot; the two signals answer different questions on purpose.
                //
                // `AnyElement` boxing here is load-bearing, not incidental: `rail_problem_pill` now
                // returns `MoonBadge` (`impl IntoElement`) while the other arm returns `Div` — the
                // two arms genuinely differ in type, so this is where the divergence gets erased.
                let label_child: AnyElement = if row.status == WorkspaceCoreStatus::Problem {
                    rail_problem_pill(status, p).into_any_element()
                } else {
                    div()
                        .flex_none()
                        .text_size(design::t_caption(cx))
                        .text_color(rgb(p.text_muted))
                        .child(status)
                        .into_any_element()
                };
                content = content.child(label_child);
            }
            let action = crate::workspace::plan_workspace_navigation(&current_group, &row);
            let selectable = action.is_some();
            let row_id = format!("workspace-core-{}", row.core);
            let mut root = rail_row_base(
                row_id,
                row.selected,
                selectable,
                metrics.horizontal_padding,
                metrics.gap,
                p,
                cx,
            )
            .child(content)
            .tooltip(move |_window, cx| {
                cx.new(|_| MoonTooltipView::new(tooltip.clone()).max_width(440.0))
                    .into()
            });
            if let Some(action) = action {
                root = root.on_click(move |_, _, cx| {
                    execute_workspace_navigation(&backend, action.clone(), cx);
                });
            }
            root.into_any_element()
        }
    }
}
