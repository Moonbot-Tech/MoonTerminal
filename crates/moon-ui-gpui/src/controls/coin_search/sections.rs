//! Rendering grouped lists into the scrolling result list.

use super::*;

/// Renders one grouped list into `list`, preceded by `heading` when there is something to show.
///
/// Three row kinds, every one a FIXED-HEIGHT DIRECT CHILD so [`direct_row_count`] can size the
/// viewport: an exchange heading (only when the list spans more than one), a COIN row, and — while
/// that coin row is open — one child row per core offering it.
///
/// Args:
///     list: Stateful scrolling list that receives the rows.
///     id: Stable popup identity used to derive row and checkbox IDs.
///     section: Stable section identity that keeps IDs unique across suggestion groups.
///     heading: Optional localized heading, omitted for ordinary query results.
///     sections: The grouped list, as [`group_hits`] produced it.
///     selected: Markets currently accumulated for multi-select.
///     toggled: Coin rows the host has recorded a caret click on.
///     multi_select: Whether rows include selection checkboxes.
///     show_server_per_row: Whether a coin row names the core it would open on.
///     active_core: Core the window is addressing, which decides what a coin row opens.
///     p: Active palette used by row text and hover states.
///     cx: Application context used to resolve scaled design tokens.
///     on_pick: Callback for opening a row's market.
///     on_toggle: Callback for changing a checkbox selection.
///     on_expand: Callback for a caret click, carrying the coin row's identity.
///
/// Returns:
///     The same stateful list with this section appended, or unchanged when `sections` is empty.
#[allow(clippy::too_many_arguments)]
pub(super) fn push_section<F, G, E>(
    mut list: Stateful<Div>,
    id: &'static str,
    section: &'static str,
    heading: Option<String>,
    sections: Vec<CoinSection>,
    selected: &HashSet<(CoreId, String)>,
    toggled: &HashSet<CoinGroupKey>,
    multi_select: bool,
    show_server_per_row: bool,
    active_core: Option<CoreId>,
    p: MoonPalette,
    cx: &App,
    on_pick: F,
    on_toggle: G,
    on_expand: E,
) -> Stateful<Div>
where
    F: Fn(CoreId, String, &mut Window, &mut App) + Clone + 'static,
    G: Fn(CoreId, String, &mut App) + Clone + 'static,
    E: Fn(CoinGroupKey, &mut App) + Clone + 'static,
{
    if sections.is_empty() {
        return list;
    }
    let row_h = coin_row_h(cx);
    let hover_bg = rgb(p.shell_high);
    let show_sections = shows_sections(&sections);
    if let Some(heading) = heading {
        list = list.child(
            div()
                .w_full()
                .h(px(row_h))
                .flex_none()
                .flex()
                .items_center()
                .px(design::ui_px(cx, 8.0))
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(heading),
        );
    }
    // One running index across every section, so an element id stays unique when a venue's caption
    // repeats or a coin appears under two exchanges.
    let mut i = 0usize;
    for venue_section in sections {
        if show_sections {
            list = list.child(
                div()
                    .w_full()
                    .h(px(row_h))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px(design::ui_px(cx, 8.0))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(p.text_muted))
                    .child(crate::controls::venue_section_label(
                        venue_section.venue.as_ref(),
                    )),
            );
        }
        for group in venue_section.groups {
            let members = group.members.len();
            let trading = group.members.iter().filter(|hit| hit.in_trade).count();
            let open = group_is_open(&group.key, members, toggled);
            // The coin row stands for the instrument; this is the core it would actually open, and
            // it is NAMED on the row so the choice is never hidden.
            let Some(picked) = pick_core(&group.members, active_core) else {
                continue;
            };
            let pick_core_id = picked.core;
            let pick_market = picked.market.clone();
            let pick_server = picked.server.clone();
            let checked = selected.contains(&(pick_core_id, pick_market.clone()));

            let on_pick_row = on_pick.clone();
            let market_pick = pick_market.clone();
            let on_toggle_row = on_toggle.clone();
            let market_toggle = pick_market.clone();
            let on_expand_row = on_expand.clone();
            let caret_key = group.key.clone();
            let pair = group.pair.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("{id}-{section}-row-{i}")))
                    .w_full()
                    .h(px(row_h))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px(design::ui_px(cx, 8.0))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover_bg))
                    .child(
                        h_flex()
                            .w_full()
                            .gap(design::ui_px(cx, 6.0))
                            .items_center()
                            // Clicking a multi-select checkbox does not open the market. The
                            // wrapper below needs no stop_propagation because MoonCheckbox does not
                            // trigger the row's on_pick handler.
                            .when(multi_select, |row| {
                                row.child(
                                    MoonCheckbox::new(SharedString::from(format!(
                                        "{id}-{section}-cb-{i}"
                                    )))
                                    .checked(checked)
                                    .on_change(
                                        move |_v: &bool, _w, app| {
                                            on_toggle_row(pick_core_id, market_toggle.clone(), app);
                                            app.stop_propagation();
                                        },
                                    ),
                                )
                            })
                            // A caret only where there is something under the row. A single-core
                            // coin renders exactly the row it always did.
                            .when(members > 1, |row| {
                                row.child(
                                    MoonDisclosure::button(
                                        SharedString::from(format!("{id}-{section}-caret-{i}")),
                                        open,
                                    )
                                    .direction(MoonDisclosureDirection::DownUp)
                                    .size(design::DISCLOSURE_GLYPH)
                                    .box_size(design::DISCLOSURE_BOX)
                                    .hover_color(p.text)
                                    .on_toggle(
                                        move |_v: &bool, _w, app| {
                                            on_expand_row(caret_key.clone(), app);
                                            app.stop_propagation();
                                        },
                                    ),
                                )
                            })
                            // Clicking the row text opens the coin on the picked core.
                            .child(
                                h_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(design::ui_px(cx, 6.0))
                                    .items_baseline()
                                    .on_mouse_down(MouseButton::Left, move |_, window, app| {
                                        on_pick_row(pick_core_id, market_pick.clone(), window, app);
                                        app.stop_propagation();
                                    })
                                    // The instrument never yields; the optional core name does.
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_size(design::t_body(cx))
                                            .text_color(rgb(p.text))
                                            .child(pair),
                                    )
                                    // A lone core carries its marker on the coin row itself;
                                    // a group states the count and marks its child rows.
                                    .when(members == 1 && trading == 1, |row| {
                                        row.child(in_trade_dot(
                                            SharedString::from(format!("{id}-{section}-trade-{i}")),
                                            p,
                                            cx,
                                        ))
                                    })
                                    .when(members > 1, |row| {
                                        let caption = match trading {
                                            0 => t!("chart.coin.cores", n = members.to_string()),
                                            k => t!(
                                                "chart.coin.cores_in_trade",
                                                n = members.to_string(),
                                                k = k.to_string()
                                            ),
                                        };
                                        row.child(
                                            div()
                                                .flex_none()
                                                .text_size(design::t_caption(cx))
                                                .text_color(rgb(p.text_muted))
                                                .child(caption.to_string()),
                                        )
                                    })
                                    .when(show_server_per_row, |row| {
                                        row.child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .text_size(design::t_caption(cx))
                                                .text_color(rgb(p.text_muted))
                                                // `@core` distinguishes the server qualifier from
                                                // the instrument symbol.
                                                .child(format!("@{pick_server}")),
                                        )
                                    }),
                            ),
                    ),
            );
            i += 1;
            if !open || members <= 1 {
                continue;
            }
            // Cores already trading the coin first; the row's pick above used canonical order.
            for member in in_trade::in_trade_first(&group.members) {
                let CoinHit {
                    core,
                    market,
                    server,
                    in_trade,
                    ..
                } = member.clone();
                let checked = selected.contains(&(core, market.clone()));
                let on_pick_child = on_pick.clone();
                let market_pick = market.clone();
                let on_toggle_child = on_toggle.clone();
                let market_toggle = market.clone();
                list = list.child(
                    div()
                        .id(SharedString::from(format!("{id}-{section}-row-{i}")))
                        .w_full()
                        .h(px(row_h))
                        .flex_none()
                        .flex()
                        .items_center()
                        .px(design::ui_px(cx, 8.0))
                        .pl(design::ui_px(cx, 22.0))
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover_bg))
                        // On the ROW, which is the stateful element: a plain `Div` carries no
                        // tooltip. A core name wider than the popup clips, and this is how the
                        // whole name — never a shortened one — stays reachable.
                        .tooltip(crate::panels::common::text_tooltip(server.clone()))
                        .child(
                            h_flex()
                                .w_full()
                                .gap(design::ui_px(cx, 6.0))
                                .items_center()
                                .when(multi_select, |row| {
                                    row.child(
                                        MoonCheckbox::new(SharedString::from(format!(
                                            "{id}-{section}-cb-{i}"
                                        )))
                                        .checked(checked)
                                        .on_change(
                                            move |_v: &bool, _w, app| {
                                                on_toggle_child(core, market_toggle.clone(), app);
                                                app.stop_propagation();
                                            },
                                        ),
                                    )
                                })
                                // The row-level tooltip keeps a clipped core name available without
                                // widening the popup for one unusually long configured name.
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .whitespace_nowrap()
                                        .overflow_hidden()
                                        .truncate()
                                        .text_size(design::t_caption(cx))
                                        .text_color(rgb(p.text_soft))
                                        .on_mouse_down(MouseButton::Left, move |_, window, app| {
                                            on_pick_child(core, market_pick.clone(), window, app);
                                            app.stop_propagation();
                                        })
                                        .child(server),
                                )
                                .when(in_trade, |row| {
                                    row.child(in_trade_dot(
                                        SharedString::from(format!("{id}-{section}-trade-{i}")),
                                        p,
                                        cx,
                                    ))
                                }),
                        ),
                );
                i += 1;
            }
        }
    }
    list
}

/// The "already trading here" marker: a small accent dot whose tooltip says what it means.
///
/// A dot rather than a word tag: the popup is [`COIN_POPUP_W`] wide and a core name already
/// competes for that width, so the marker must never be the thing that clips it.
fn in_trade_dot(id: SharedString, p: MoonPalette, cx: &App) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .text_size(design::t_caption(cx))
        .text_color(rgb(p.accent))
        .tooltip(crate::panels::common::text_tooltip(SharedString::from(
            t!("chart.coin.in_trade_tip").to_string(),
        )))
        .child("●")
}

/// Hand the keyboard back to the window when the coin search is finished with.
///
/// The field keeps focus after a pick — nothing takes it away, and the terminal deliberately does
/// not blur an input just because something else was clicked. Visually the search is over and the
/// user is back on the chart; in fact every keystroke still belongs to a text field, which eats the
/// editing shortcuts outright: Ctrl+Z is Undo there, Ctrl+X is Cut, and both are perfectly ordinary
/// things to bind New Long and New Short to. The hotkey then does nothing with no symptom at all.
///
/// The mechanism is [`crate::hotkeys::release_field_focus`], which is also why this is conditional
/// on `field` actually holding the focus — not a nicety. Some exits are not clicks on the field at
/// all: the header ticker's list closes when the pointer merely leaves it, which happens perfectly
/// often while the user is typing somewhere else entirely. An unconditional blur there would reach
/// across the window and empty the caret out of whatever field they were in.
///
/// Kept as its own name rather than calling that one directly at every site: the contract test
/// `every_coin_search_exit_releases_the_keyboard` counts these calls per host, so a coin search
/// growing a new way out has to say so here.
pub(crate) fn release_focus(field: &Entity<MoonInputState>, window: &mut Window, cx: &App) {
    crate::hotkeys::release_field_focus(field, window, cx);
}
