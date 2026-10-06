//! The coin search result popup.

use super::*;

/// Renders the result dropdown: query matches, or whichever list the open tab asks for — the
/// empty-field suggestions, the marked markets, or the cores' temporary bans — with multi-selection
/// checkboxes and an Open in New Tab button where the rows can carry them. Clicking outside a checkbox calls the
/// owner-defined `on_pick`; a checkbox calls `on_toggle`; the footer button calls `on_open_new` for
/// the accumulated selection. `selected` contains the currently checked markets. In single-selection
/// mode, `on_toggle` and `on_open_new` are never called.
///
/// Args:
///     id: Stable popup identity used for the scroll container and child controls.
///     results: Query matches, or the list the open tab resolved.
///     selected: Markets currently accumulated for multi-select.
///     toggled: Coin rows the host has recorded a caret click on; see [`group_is_open`].
///     multi_select: Whether checkboxes and the Open in New Tab footer are enabled at all; a list
///         whose rows carry no checkbox turns them off whatever the host asked for.
///     active_core: Core the window is addressing, which decides what a coin row opens.
///     server_context: Sole server named once above the rows, or `None` to label every row.
///     tabs: The host's tab strip, or `None` for a query-only host (the header ticker, the Report
///         token filter) which shows no tabs and can produce none of their lists.
///     p: Active palette used by the dropdown.
///     cx: Application context used to resolve scaled design tokens.
///     on_pick: Callback for opening a row's market.
///     on_toggle: Callback for changing a checkbox selection.
///     on_expand: Callback for a caret click, carrying the coin row's identity.
///     on_open_new: Callback for opening the accumulated selection in a new tab.
///
/// Returns:
///     A stateful dropdown element whose list can retain scroll position.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_popup<F, G, H, E>(
    id: &'static str,
    results: CoinResults,
    selected: &HashSet<(CoreId, String)>,
    toggled: &HashSet<CoinGroupKey>,
    multi_select: bool,
    active_core: Option<CoreId>,
    server_context: Option<String>,
    tabs: Option<CoinTabsCfg>,
    p: MoonPalette,
    cx: &App,
    on_pick: F,
    on_toggle: G,
    on_expand: E,
    on_open_new: H,
) -> Stateful<Div>
where
    F: Fn(CoreId, String, &mut Window, &mut App) + Clone + 'static,
    G: Fn(CoreId, String, &mut App) + Clone + 'static,
    H: Fn(&mut Window, &mut App) + Clone + 'static,
    E: Fn(CoinGroupKey, &mut App) + Clone + 'static,
{
    let selected_count = selected.len();
    let show_server_per_row = server_context.is_none();
    let row_h = coin_row_h(cx);
    let visible_slots = whole_row_slots(COIN_LIST_RAW_CAP, row_h);
    let list_cap = whole_row_cap(COIN_LIST_RAW_CAP, row_h);
    // Grouped ONCE, here, and handed to both the arithmetic and the renderer: counting one shape
    // while drawing another is exactly how a viewport cap starts lying.
    let grouped = match results {
        CoinResults::Query(hits) => GroupedResults::Query(group_hits(hits)),
        CoinResults::Suggest { recent, volatile } => GroupedResults::Suggest {
            recent: group_hits(recent),
            volatile: group_hits(volatile),
        },
        CoinResults::Favorites(rows) => GroupedResults::Marked {
            list: tabs::MarkedList::Favorites,
            rows,
        },
        CoinResults::Banned(rows) => GroupedResults::Marked {
            list: tabs::MarkedList::Banned,
            rows,
        },
    };
    // Whether a selection can be accumulated at all is a property of the ROWS on screen, not of the
    // tab that asked for them: an acting list draws its own button where the others draw
    // checkboxes, so the hint row and the footer would count markets none of its rows shows. Asked
    // of the grouped value, which is the one thing both the arithmetic and the renderer read.
    let multi_select = multi_select && !matches!(grouped, GroupedResults::Marked { .. });
    let result_rows = match &grouped {
        GroupedResults::Query(sections) => {
            if sections.is_empty() {
                1
            } else {
                direct_row_count(sections, toggled)
            }
        }
        GroupedResults::Suggest { recent, volatile } => {
            if recent.is_empty() && volatile.is_empty() {
                1
            } else {
                direct_row_count(recent, toggled)
                    + usize::from(!recent.is_empty())
                    + direct_row_count(volatile, toggled)
                    + usize::from(!volatile.is_empty())
            }
        }
        // One fixed-height row per entry, or the one row the empty note occupies.
        GroupedResults::Marked { rows, .. } => rows.len().max(1),
    };
    let direct_child_count =
        result_rows + usize::from(server_context.is_some()) + usize::from(multi_select);
    let list_overflows = direct_child_count > visible_slots;
    // `.id(..)` makes the container stateful so `overflow_y_scroll` can let GPUI track wheel
    // scrolling by ID. The integral cap keeps its final visible row whole at every font scale.
    let mut list = div()
        .id(SharedString::from(format!("{id}-list")))
        .flex()
        .flex_col()
        .w_full()
        .max_h(px(list_cap))
        .overflow_y_scroll();

    if let Some(server) = server_context {
        let context = t!("chart.coin.server_context", server = server).to_string();
        let tooltip = context.clone();
        list = list.child(
            div()
                .id(SharedString::from(format!("{id}-server-context")))
                .w_full()
                .h(px(row_h))
                .flex_none()
                .flex()
                .items_center()
                .px(design::ui_px(cx, 8.0))
                .whitespace_nowrap()
                .overflow_hidden()
                .truncate()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .tooltip(crate::panels::common::text_tooltip(tooltip))
                .child(context),
        );
    }

    // A one-line note on what the checkboxes accumulate toward, so the footer button is not the
    // first explanation of the mode — and only where checkboxes actually exist. It must stay ONE
    // line: wrapped, it doubles the popup's header and pushes the first result out of view. The
    // dictionary values are short enough to fit at the stock font scale; `whitespace_nowrap`
    // makes a future longer translation clip instead of silently folding.
    if multi_select {
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
                .child(t!("chart.coin.multi_hint").to_string()),
        );
    }

    let empty_note = |list: Stateful<Div>, text: String| {
        list.child(
            div()
                .w_full()
                .h(px(row_h))
                .flex_none()
                .flex()
                .items_center()
                .px(design::ui_px(cx, 8.0))
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(text),
        )
    };

    match grouped {
        GroupedResults::Query(sections) => {
            if sections.is_empty() {
                list = empty_note(list, t!("chart.coin.no_results").to_string());
            } else {
                list = push_section(
                    list,
                    id,
                    "q",
                    None,
                    sections,
                    selected,
                    toggled,
                    multi_select,
                    show_server_per_row,
                    active_core,
                    p,
                    cx,
                    on_pick.clone(),
                    on_toggle.clone(),
                    on_expand.clone(),
                );
            }
        }
        GroupedResults::Suggest { recent, volatile } => {
            if recent.is_empty() && volatile.is_empty() {
                list = empty_note(list, t!("chart.coin.no_suggestions").to_string());
            } else {
                list = push_section(
                    list,
                    id,
                    "recent",
                    Some(t!("chart.coin.recent").to_string()),
                    recent,
                    selected,
                    toggled,
                    multi_select,
                    show_server_per_row,
                    active_core,
                    p,
                    cx,
                    on_pick.clone(),
                    on_toggle.clone(),
                    on_expand.clone(),
                );
                list = push_section(
                    list,
                    id,
                    "volatile",
                    Some(t!("chart.coin.top_volatile").to_string()),
                    volatile,
                    selected,
                    toggled,
                    multi_select,
                    show_server_per_row,
                    active_core,
                    p,
                    cx,
                    on_pick.clone(),
                    on_toggle.clone(),
                    on_expand.clone(),
                );
            }
        }
        GroupedResults::Marked { list: which, rows } => {
            if rows.is_empty() {
                list = empty_note(list, t!(which.empty_key()).to_string());
            } else {
                list = tabs::push_marked_rows(
                    list,
                    id,
                    which,
                    rows,
                    show_server_per_row,
                    p,
                    cx,
                    on_pick.clone(),
                    // Only a tabbed host can be showing this list at all, so the command is always
                    // in hand here; taken through the option anyway rather than unwrapped, which
                    // would turn a future caller's mistake into a panic in the frame loop.
                    tabs.as_ref().map(|cfg| cfg.row_action(which)),
                );
            }
        }
    }

    // The fade is anchored outside the scroll content and has no input handlers, so it signals
    // continuation without becoming another row or taking wheel/click interaction from the list.
    let list = div()
        .relative()
        .flex_none()
        .w_full()
        .child(list)
        .when(list_overflows, |wrapper| {
            wrapper.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(design::ui_px(cx, COIN_LIST_FADE_H))
                    .bg(linear_gradient(
                        180.0,
                        linear_color_stop(design::moon_alpha(p.panel_high, 0.0), 0.0),
                        linear_color_stop(design::moon_alpha(p.panel_high, 1.0), 1.0),
                    )),
            )
        });

    // Show the Open in New Tab footer only in multi-select mode and enable it for a nonempty
    // selection. Keep it outside the scroller so it remains visible, with the selected count in
    // its label.
    let footer = multi_select.then(|| {
        let label = if selected_count > 0 {
            format!("{} ({selected_count})", t!("chart.coin.open_new_tab"))
        } else {
            t!("chart.coin.open_new_tab").to_string()
        };
        div()
            .w_full()
            .px(design::ui_px(cx, 6.0))
            .py(design::ui_px(cx, 6.0))
            .border_t_1()
            .border_color(rgb(p.border))
            .child(
                MoonButton::new(SharedString::from(format!("{id}-open-new")))
                    .label(label)
                    .variant(if selected_count > 0 {
                        MoonButtonVariant::Blue
                    } else {
                        MoonButtonVariant::Soft
                    })
                    .disabled(selected_count == 0)
                    .on_click(move |_, window, app| {
                        on_open_new(window, app);
                        app.stop_propagation();
                    })
                    .render(),
            )
    });

    div()
        .id(id)
        .flex()
        .flex_col()
        .w(design::font_w_px(cx, COIN_POPUP_W))
        .bg(rgb(p.panel_high))
        .border_1()
        .border_color(rgb(p.border))
        .rounded(design::r_button(cx))
        // Intercept mouse_down across the popup. A checkbox reacts on_change rather than
        // mouse_down, so the event would otherwise reach the dismiss layer underneath and close
        // the list. A pick row has its own earlier mouse-down handler with stop_propagation.
        .on_mouse_down(MouseButton::Left, |_, _window, app| app.stop_propagation())
        // Claim the wheel — and the pointer generally — for whatever sits UNDER this popup. The
        // chart reads the wheel through an ordinary gpui handler gated on its own hitbox, so
        // without this a scroll over the results list also rescaled the chart behind them. The
        // inner list keeps scrolling: its hitbox is pushed after this one and so is still hit
        // before the traversal stops here.
        .occlude()
        // Above the list and outside it: the viewport cap counts the list's own fixed-height rows.
        .children(
            tabs.as_ref()
                .map(|cfg| tabs::render_tab_strip(id, cfg, p, cx)),
        )
        .child(list)
        .children(footer)
}
