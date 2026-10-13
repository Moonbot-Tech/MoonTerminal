//! The Hotkeys page as free builders over a row model, so the layout probe can draw it without a
//! backend: the conflict banner, the search filter, the category strip, the command table and
//! the collapsed built-in combos. `tab.rs` fills the model from the draft and hands in the live
//! editors; nothing here reads or writes the configuration.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonAlert, MoonButton, MoonButtonSize, MoonButtonVariant, MoonDisclosure,
    MoonDisclosureDirection, MoonPalette, MoonTag, MoonTooltipView, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::clash::Severity;
use super::registry::HotkeyGroup;
use crate::design;
use crate::settings::common::Action;

/// The least the Команда column gets; it shares the rest with the Проблема column.
const NAME_MIN_W: f32 = 120.0;
/// Width of the Где работает column when the body has room; narrower bodies shrink it, the
/// caption truncated with its full text in a tooltip.
const SCOPE_W: f32 = 76.0;
/// Width of the Клавиша column: `MoonHotkeyInput` keeps a minimum of 176 of its own.
pub(super) const KEY_W: f32 = 184.0;
/// Logical width of an editor inside the key cell.
pub(super) const EDITOR_W: f32 = 176.0;
/// Width of the last column, which holds "+ жест".
const ACTION_W: f32 = 64.0;
/// Gap between the columns.
const GAP: f32 = 8.0;

/// What the search and the conflict filter read of one row.
#[derive(Clone, Debug, Default)]
pub(super) struct RowText {
    /// The command's name in the user's language.
    pub name: String,
    /// The command's MoonBot name, when it differs.
    pub moonbot: Option<String>,
    /// The bound keys and gestures, as stored or as labelled.
    pub keys: Vec<String>,
    /// The row carries a conflict or a bare-key warning.
    pub problem: bool,
}

/// Lowercase, without blanks, with `-` read as `+`: "Alt+Z", "alt-z" and "alt + z" compare equal.
fn normalized(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| if c == '-' { '+' } else { c })
        .flat_map(char::to_lowercase)
        .collect()
}

/// The rows the page shows: those matching `query` (by name, MoonBot name or bound key, ignoring
/// case, blanks and the `-`/`+` spelling) and, with `conflicts_only`, only the problem rows.
///
/// Args:
///     rows: Every candidate row.
///     query: The search text; blank matches everything.
///     conflicts_only: Keep only the rows with a conflict or a bare-key warning.
///
/// Returns:
///     Indices into `rows`, in order.
pub(super) fn filter_rows(rows: &[RowText], query: &str, conflicts_only: bool) -> Vec<usize> {
    let query = normalized(query);
    rows.iter()
        .enumerate()
        .filter(|(_, row)| !conflicts_only || row.problem)
        .filter(|(_, row)| {
            query.is_empty()
                || std::iter::once(&row.name)
                    .chain(row.moonbot.iter())
                    .chain(row.keys.iter())
                    .any(|text| normalized(text).contains(&query))
        })
        .map(|(ix, _)| ix)
        .collect()
}

/// Whether a key row shows its mouse-gesture editor: always on the Mouse category, elsewhere
/// only once a gesture is set or the user asked for one with "+ жест".
pub(super) fn gesture_shown(mouse_category: bool, gesture_set: bool, opened: bool) -> bool {
    mouse_category || gesture_set || opened
}

/// One table row, ready to draw.
pub(super) struct HkRow {
    /// Element id fragment, unique on the page.
    pub id: String,
    pub name: String,
    pub moonbot: Option<String>,
    /// The row's explanation, shown as the name's tooltip.
    pub hint: Option<String>,
    /// Where the command acts.
    pub scope: String,
    /// Conflict captions with their severity.
    pub problems: Vec<(String, Severity)>,
    /// The row follows another (the short move rows under the mirror switch).
    pub muted: bool,
    /// The editors, stacked in the key cell: the key, then the gesture and its parameter.
    pub editors: Vec<AnyElement>,
    /// "+ жест", for a key row whose gesture editor is hidden.
    pub add_gesture: Option<Action>,
}

/// One line of muted text that wraps.
/// One line that gives way in a narrow body: truncated, its full text in a tooltip.
///
/// Args:
///     id: Element id, for the tooltip's hover state.
///     text: The caption.
///     color: Its colour.
///     cx: Application context.
fn shrinking_line(id: String, text: String, color: Hsla, cx: &App) -> Stateful<Div> {
    let full = text.clone();
    div()
        .id(SharedString::from(id))
        .min_w_0()
        .truncate()
        .text_size(design::t_caption(cx))
        .text_color(color)
        .tooltip(move |_window, cx| {
            cx.new(|_| MoonTooltipView::new(full.clone()).max_width(380.0))
                .into()
        })
        .child(text)
}

fn muted(text: impl Into<SharedString>, cx: &App) -> Div {
    let p = MoonPalette::active(cx);
    div()
        .min_w_0()
        .text_size(design::t_caption(cx))
        .text_color(rgba_from(p.text_muted, 1.0))
        .child(text.into())
}

/// The conflict banner: how many commands never fire and the filter switch.
///
/// Args:
///     conflicts: Rows another binding shadows.
///     conflicts_only: The filter is on; the button offers to show everything.
///     on_toggle: Flips the filter.
///     cx: Application context.
///
/// Returns:
///     The banner, or `None` when nothing conflicts and the filter is off.
pub(super) fn banner(
    conflicts: u32,
    conflicts_only: bool,
    on_toggle: Action,
    cx: &App,
) -> Option<AnyElement> {
    if conflicts == 0 && !conflicts_only {
        return None;
    }
    let title = super::super::common::count_label("hotkeys.banner.conflicts", u64::from(conflicts));
    let label = if conflicts_only {
        t!("hotkeys.banner.show_all")
    } else {
        t!("hotkeys.banner.show_conflicts")
    };
    Some(
        h_flex()
            .id("hotkeys-banner")
            .debug_selector(|| "hotkeys-banner".into())
            .w_full()
            .flex_wrap()
            .gap(design::ui_px(cx, GAP))
            .items_center()
            .child(
                div().flex_1().min_w(design::ui_px(cx, 200.0)).child(
                    MoonAlert::warning(
                        "hotkeys-banner-alert",
                        t!("hotkeys.banner.body").to_string(),
                    )
                    .title(title.to_string())
                    .render(),
                ),
            )
            .child(
                MoonButton::new("hotkeys-banner-toggle")
                    .variant(moon_ui::MoonButtonVariant::Neutral)
                    .size(MoonButtonSize::density(cx))
                    .label(label.to_string())
                    .on_click(move |_, window, cx| on_toggle(window, cx))
                    .render(),
            )
            .into_any_element(),
    )
}

/// The category strip: one button per category, the active one filled, each with its conflict
/// count. Wraps: seven Russian category names do not fit one line of the default body.
///
/// Args:
///     active: The category shown.
///     conflicts: Conflicts per category, in [`HotkeyGroup::ALL`] order.
///     on_pick: Called with the category a click selects.
///     cx: Application context.
///
/// Returns:
///     The strip.
pub(super) fn categories(
    active: HotkeyGroup,
    conflicts: &[u32],
    on_pick: std::rc::Rc<dyn Fn(HotkeyGroup, &mut Window, &mut App)>,
    cx: &App,
) -> AnyElement {
    h_flex()
        .id("hotkeys-categories")
        .debug_selector(|| "hotkeys-categories".into())
        .w_full()
        .flex_wrap()
        .gap(design::ui_px(cx, 6.0))
        .children(HotkeyGroup::ALL.iter().enumerate().map(|(ix, group)| {
            let group = *group;
            let pick = on_pick.clone();
            let count = conflicts.get(ix).copied().unwrap_or(0);
            h_flex()
                .gap(design::ui_px(cx, 4.0))
                .items_center()
                .child(
                    MoonButton::new(SharedString::from(format!("hotkeys-cat-{ix}")))
                        .size(MoonButtonSize::density(cx))
                        .variant(if group == active {
                            MoonButtonVariant::Blue
                        } else {
                            MoonButtonVariant::Neutral
                        })
                        .label(group.title())
                        .on_click(move |_, window, cx| pick(group, window, cx))
                        .render(),
                )
                .when(count > 0, |el| {
                    el.child(
                        MoonTag::danger()
                            .mono(true)
                            .label(count.to_string())
                            .render(),
                    )
                })
        }))
        .into_any_element()
}

/// The table: a header and one line per row, or `empty` when no row is left.
///
/// Args:
///     rows: The rows, filtered.
///     mouse: The Mouse category: the key column is headed "Жест мыши".
///     empty: What an empty table says.
///     cx: Application context.
///
/// Returns:
///     The table.
pub(super) fn table(rows: Vec<HkRow>, mouse: bool, empty: String, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    let head = |text: String| muted(text, cx);
    let header = h_flex()
        .w_full()
        .gap(design::ui_px(cx, GAP))
        .pb(design::ui_px(cx, 4.0))
        .border_b_1()
        .border_color(rgba_from(p.border, 1.0))
        .child(
            head(t!("hotkeys.col.command").to_string())
                .flex_1()
                .min_w(design::ui_px(cx, NAME_MIN_W)),
        )
        .child(
            head(t!("hotkeys.col.scope").to_string())
                .flex_shrink(1.0)
                .min_w_0()
                .truncate()
                .w(design::ui_px(cx, SCOPE_W)),
        )
        .child(
            head(if mouse {
                t!("hotkeys.col.gesture").to_string()
            } else {
                t!("hotkeys.col.key").to_string()
            })
            .flex_none()
            .w(design::ui_px(cx, KEY_W)),
        )
        .child(
            head(t!("hotkeys.col.problem").to_string())
                .flex_1()
                .min_w_0()
                .truncate(),
        )
        .child(div().flex_none().w(design::ui_px(cx, ACTION_W)));
    let body: Vec<AnyElement> = if rows.is_empty() {
        vec![
            muted(empty, cx)
                .debug_selector(|| "hotkeys-empty".into())
                .w_full()
                .py(design::ui_px(cx, 16.0))
                .text_center()
                .into_any_element(),
        ]
    } else {
        rows.into_iter().map(|row| table_row(row, cx)).collect()
    };
    v_flex()
        .id("hotkeys-table")
        .debug_selector(|| "hotkeys-table".into())
        .w_full()
        .child(header)
        .children(body)
        .into_any_element()
}

/// One line of the table.
fn table_row(row: HkRow, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    let HkRow {
        id,
        name,
        moonbot,
        hint,
        scope,
        problems,
        muted: row_muted,
        editors,
        add_gesture,
    } = row;
    let name_cell = v_flex()
        .id(SharedString::from(format!("hk-name-{id}")))
        .debug_selector({
            let sel = format!("hk-name-{id}");
            move || sel.clone()
        })
        .flex_1()
        .min_w(design::ui_px(cx, NAME_MIN_W))
        .child(
            div()
                .min_w_0()
                .text_size(design::t_body(cx))
                .text_color(rgba_from(
                    if row_muted { p.text_muted } else { p.text },
                    1.0,
                ))
                .child(name),
        )
        .children(moonbot.map(|mb| {
            div()
                .min_w_0()
                .font_family(design::mono())
                .text_size(design::t_caption(cx))
                .text_color(rgba_from(p.text_muted, 1.0))
                .child(mb)
        }))
        .when_some(hint, |el, hint| {
            el.tooltip(move |_window, cx| {
                cx.new(|_| MoonTooltipView::new(hint.clone()).max_width(380.0))
                    .into()
            })
        });
    let problem_cell = v_flex()
        .debug_selector({
            let sel = format!("hk-problem-{id}");
            move || sel.clone()
        })
        .flex_1()
        .min_w_0()
        .gap(design::ui_px(cx, 2.0))
        .children(
            problems
                .into_iter()
                .enumerate()
                .map(|(i, (text, severity))| {
                    let color = match severity {
                        Severity::Shadowed => p.red_text,
                        Severity::Shares | Severity::Bare => p.amber,
                    };
                    shrinking_line(
                        format!("hk-problem-{id}-{i}"),
                        text.to_string(),
                        rgba_from(color, 1.0),
                        cx,
                    )
                }),
        );
    let action_cell = div()
        .debug_selector({
            let sel = format!("hk-action-{id}");
            move || sel.clone()
        })
        .flex_none()
        .w(design::ui_px(cx, ACTION_W))
        .flex()
        .justify_end()
        .children(add_gesture.map(|on| {
            MoonButton::new(SharedString::from(format!("hk-add-gesture-{id}")))
                .ghost()
                .size(MoonButtonSize::density(cx))
                .label(t!("hotkeys.add_gesture").to_string())
                .on_click(move |_, window, cx| on(window, cx))
                .render()
        }));
    h_flex()
        .debug_selector({
            let sel = format!("hk-row-{id}");
            move || sel.clone()
        })
        .w_full()
        .gap(design::ui_px(cx, GAP))
        .py(design::ui_px(cx, 5.0))
        .items_start()
        .border_b_1()
        .border_color(rgba_from(p.border, 0.5))
        .child(name_cell)
        .child(
            div()
                .debug_selector({
                    let sel = format!("hk-scope-{id}");
                    move || sel.clone()
                })
                .flex_shrink(1.0)
                .min_w_0()
                .w(design::ui_px(cx, SCOPE_W))
                .child(shrinking_line(
                    format!("hk-scope-text-{id}"),
                    scope.to_string(),
                    rgba_from(p.text_muted, 1.0),
                    cx,
                )),
        )
        .child(
            v_flex()
                .flex_none()
                .w(design::ui_px(cx, KEY_W))
                .gap(design::ui_px(cx, 4.0))
                .children(editors),
        )
        .child(problem_cell)
        .child(action_cell)
        .into_any_element()
}

/// The built-in combos: a collapsed disclosure, then one line per combo — the keys in mono, what
/// they do muted.
///
/// Args:
///     open: The block is unfolded.
///     on_toggle: Folds or unfolds it.
///     cx: Application context.
///
/// Returns:
///     The block.
pub(super) fn builtin(open: bool, on_toggle: Action, cx: &App) -> AnyElement {
    const LINES: [&str; 11] = [
        "hotkeys.builtin.wheel_zoom",
        "hotkeys.builtin.wheel_pan",
        "hotkeys.builtin.wheel_super_zoom",
        "hotkeys.builtin.x_sync",
        "hotkeys.builtin.cancel_hover",
        "hotkeys.builtin.draw_click",
        "hotkeys.builtin.fig_menu",
        "hotkeys.builtin.order_menu",
        "hotkeys.builtin.esc_close",
        "hotkeys.builtin.close_all",
        "hotkeys.builtin.reset_windows",
    ];
    let p = MoonPalette::active(cx);
    let toggle = on_toggle.clone();
    let head = h_flex()
        .id("hotkeys-builtin-head")
        .debug_selector(|| "hotkeys-builtin-head".into())
        .w_full()
        .flex_wrap()
        .gap(design::ui_px(cx, 6.0))
        .items_center()
        .py(design::ui_px(cx, 6.0))
        .px(design::ui_px(cx, 8.0))
        .border_1()
        .border_color(rgba_from(p.border, 1.0))
        .rounded(design::r_container(cx))
        .cursor_pointer()
        .on_click(move |_, window, cx| toggle(window, cx))
        .child(
            MoonDisclosure::button("hotkeys-builtin-caret", open)
                .direction(MoonDisclosureDirection::RightDown)
                .size(design::DISCLOSURE_GLYPH)
                .box_size(design::DISCLOSURE_BOX)
                .on_toggle(move |_, window, cx| on_toggle(window, cx)),
        )
        // Short labels kept whole: a content-sized text in this row is painted from its
        // min-content measure, one word a line.
        .child(
            div()
                .flex_none()
                .whitespace_nowrap()
                .text_color(rgba_from(p.text, 1.0))
                .child(t!("hotkeys.builtin.title").to_string()),
        )
        .child(
            muted(t!("hotkeys.builtin.count", n = LINES.len()).to_string(), cx)
                .flex_none()
                .whitespace_nowrap(),
        );
    let lines = open.then(|| {
        v_flex()
            .debug_selector(|| "hotkeys-builtin-body".into())
            .w_full()
            .gap(design::ui_px(cx, 4.0))
            .pt(design::ui_px(cx, 6.0))
            .children(LINES.iter().map(|key| {
                let line = t!(*key).to_string();
                let (keys, what) = split_combo(&line);
                h_flex()
                    .w_full()
                    .gap(design::ui_px(cx, 10.0))
                    .items_start()
                    .children(keys.map(|keys| {
                        div()
                            .flex_none()
                            .font_family(design::mono())
                            .text_size(design::t_caption(cx))
                            .text_color(rgba_from(p.text, 1.0))
                            .child(keys)
                    }))
                    .child(muted(what, cx).flex_1())
            }))
    });
    v_flex()
        .w_full()
        .child(head)
        .children(lines)
        .into_any_element()
}

/// A built-in line `"[Keys] what it does"` split into its keys and the rest; a line without the
/// bracket is all description.
pub(super) fn split_combo(line: &str) -> (Option<String>, String) {
    if let Some(rest) = line.strip_prefix('[')
        && let Some((keys, what)) = rest.split_once(']')
    {
        return (Some(keys.to_string()), what.trim().to_string());
    }
    (None, line.to_string())
}

#[cfg(test)]
mod tests;
