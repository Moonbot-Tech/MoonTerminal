//! Explicit address-based station core comparison, updates and confirmed removals.

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::config::CoreGroup;
use moon_remote::ssh::Target;
use moon_ui::{
    MoonButton, MoonInput, MoonInputEvent, MoonInputState, MoonPalette, MoonSegmentItem,
    MoonSegmentedControl, MoonTag, MoonTone, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::super::SettingsView;
use super::super::frame::{RemoteBar, SaveBar};
use super::server_tape::plural_key;
use crate::backend::station::{
    cores_sync::{self, Counts, Row, RowState},
    job::{CoreStart, Job},
};
use crate::design;
use crate::panels::common::text_tooltip_value;

/// The page's one card: the station cores card is the page, so it carries no title of its own.
/// A `MoonGroupBox` cannot take `flex_1`, and the card must grow to the page height for its
/// table to; this draws the same frame on a plain `div`.
fn card(id: &'static str, cx: &App) -> Stateful<Div> {
    let p = MoonPalette::active(cx);
    div()
        .id(id)
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .rounded(design::ui_px(cx, 5.0))
        .border(design::ui_px(cx, 1.0))
        .border_color(rgba_from(p.border, 1.0))
        .bg(rgba_from(p.shell_high, 1.0))
        .p(design::ui_px(cx, 12.0))
        .gap(design::ui_px(cx, 10.0))
}

/// The names of the saved groups holding the terminal core `uid`, or a dash.
fn group_label(groups: &[CoreGroup], uid: Option<u64>) -> String {
    let names: Vec<&str> = uid
        .map(|uid| {
            groups
                .iter()
                .filter(|group| group.cores.contains(&uid))
                .map(|group| group.name.as_str())
                .collect()
        })
        .unwrap_or_default();
    if names.is_empty() {
        "\u{2014}".into()
    } else {
        names.join(", ")
    }
}

/// Reserve the address's intrinsic width so the name yields space instead of hiding the port.
fn core_address_cell(address: Option<String>) -> Div {
    div()
        .flex_none()
        .whitespace_nowrap()
        .font_family(design::mono())
        .child(address.unwrap_or_else(|| "\u{2014}".into()))
}

/// Return the state's localization key and semantic tone independently of row actions.
fn state_label(state: RowState) -> (&'static str, MoonTone) {
    match state {
        RowState::Same => ("telegram.server.cores_state_same", MoonTone::Positive),
        RowState::OnlyHere => ("telegram.server.cores_state_only_here", MoonTone::Info),
        RowState::OnlyOnStation => ("telegram.server.cores_state_only_station", MoonTone::Danger),
        RowState::NameDiffers => ("telegram.server.cores_state_name", MoonTone::Warning),
        RowState::KeyDiffers => ("telegram.server.cores_state_key", MoonTone::Warning),
    }
}

impl RowState {
    /// Anything but an exact match differs: it is listed open and counted against the verdict.
    fn differs(self) -> bool {
        self != RowState::Same
    }
}

/// A row action's element id, unique per row even for rows without a station uid.
fn action_id(verb: &str, index: usize) -> SharedString {
    SharedString::from(format!("station-core-{verb}-row-{index}"))
}

/// How many rows differ between the terminal and the station: the page's count and its bar's.
pub(super) fn diverging(rows: &[Row]) -> usize {
    rows.iter().filter(|row| row.state.differs()).count()
}

/// Offer removal only for station-only entries when another station core remains.
fn removable(row: &Row, station_count: usize) -> bool {
    row.state == RowState::OnlyOnStation && row.station_uid.is_some() && station_count > 1
}

/// A vanished or no-longer-removable row cannot retain a destructive confirmation.
fn retained_arm(armed: Option<RemovalArm>, rows: &[Row]) -> Option<RemovalArm> {
    let count = Counts::from_rows(rows).on_station;
    armed.filter(|(uid, address, name)| {
        rows.iter().any(|row| {
            row.station_uid == Some(*uid)
                && &row.address == address
                && &row.name == name
                && removable(row, count)
        })
    })
}

/// Confirmation binds every visible part of the identity, not just a reused uid.
pub(super) type RemovalArm = (u64, Option<String>, String);

/// Whether a removal confirmed NOW may run, checked at the moment of "Yes" rather than when the
/// dialog opened: the station must be idle and the row still listed, unchanged and removable.
///
/// Returns:
///     `Err` with the locale key of the reason the job does not start.
pub(super) fn removal_check(
    busy: bool,
    arm: &RemovalArm,
    rows: &[Row],
) -> Result<(), &'static str> {
    if busy {
        return Err("telegram.mini_error_busy_station");
    }
    if retained_arm(Some(arm.clone()), rows).is_none() {
        return Err("telegram.server.cores_changed");
    }
    Ok(())
}

/// Comparison cache invalidates on the listing and the secret-free local config revision.
#[derive(Default)]
pub(super) struct CoreCache {
    seen: Option<Option<Vec<moon_core::station_api::ListedCore>>>,
    high_water: Option<u64>,
    local_revision: Vec<cores_sync::LocalCore>,
    pub(super) rows: Vec<Row>,
}
impl CoreCache {
    /// Reconcile only when inputs change; render uses the retained rows.
    fn update(
        &mut self,
        seen: &Option<Option<Vec<moon_core::station_api::ListedCore>>>,
        local: Vec<cores_sync::LocalCore>,
        high_water: Option<u64>,
    ) {
        if &self.seen != seen || self.local_revision != local || self.high_water != high_water {
            self.rows = seen
                .as_ref()
                .and_then(Option::as_ref)
                .map(|listing| cores_sync::reconcile(&local, listing, high_water))
                .unwrap_or_default();
            self.seen = seen.clone();
            self.high_water = high_water;
            self.local_revision = local;
        }
    }
}

/// Choose a table only when its measured columns, the widest action and the gaps all fit.
fn table_fits(width: f32, columns: &[f32], gaps: f32) -> bool {
    width >= columns.iter().sum::<f32>() + gaps
}

/// Which rows the full core list shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum CoresFilter {
    #[default]
    All,
    Differ,
}

/// The block's one-line verdict: every compared core and the ones that differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CoresSummary {
    pub(super) total: usize,
    pub(super) differ: usize,
}

/// Count every row of the union; anything but an exact match differs.
fn cores_summary(rows: &[Row]) -> CoresSummary {
    CoresSummary {
        total: rows.len(),
        differ: diverging(rows),
    }
}

/// Rows of the full list matching the filter and a case-insensitive name or address query.
fn filter_rows<'a>(rows: &'a [Row], query: &str, filter: CoresFilter) -> Vec<(usize, &'a Row)> {
    let query = query.trim().to_lowercase();
    let hit = |text: Option<&str>| text.is_some_and(|text| text.to_lowercase().contains(&query));
    rows.iter()
        .enumerate()
        .filter(|(_, row)| filter == CoresFilter::All || row.state.differs())
        .filter(|(_, row)| {
            query.is_empty()
                || hit(Some(&row.name))
                || hit(row.station_name.as_deref())
                || hit(row.address.as_deref())
        })
        .collect()
}

/// One plain sentence on why a differing core is listed: its state's hover text.
fn reason_key(state: RowState) -> Option<&'static str> {
    match state {
        RowState::Same => None,
        RowState::OnlyHere => Some("telegram.server.cores_reason_only_here"),
        RowState::OnlyOnStation => Some("telegram.server.cores_reason_only_station"),
        RowState::NameDiffers => Some("telegram.server.cores_reason_name"),
        RowState::KeyDiffers => Some("telegram.server.cores_reason_key"),
    }
}

/// Re-render the list on every keystroke of its search.
pub(super) fn watch_query<T: 'static>(query: &Entity<MoonInputState>, cx: &mut Context<T>) {
    cx.subscribe(query, |_, _, ev: &MoonInputEvent, cx| {
        if matches!(ev, MoonInputEvent::Change) {
            cx.notify();
        }
    })
    .detach();
}

/// Measured columns of the core table and whether a row fits on one line.
///
/// Ядро, Адрес and Группа take their content's width (the name capped at [`NAME_MAX_W`]);
/// Состояние takes the rest and holds the state word and the row's action.
#[derive(Clone, Copy, Debug)]
pub(super) struct TableLayout {
    pub(super) wide: bool,
    pub(super) name: f32,
    pub(super) address: f32,
    pub(super) group: f32,
}

/// Measure the name, address and group columns from the rows and their headers, and decide
/// whether the widest state word and action still fit beside them.
///
/// Args:
///     rows: The comparison rows, all of them, so a filter never moves the columns.
///     groups: Each row's group label, in row order.
///     width: Width of the card's content box.
///     cx: Application context for scale, zoom and locale.
pub(super) fn table_layout(rows: &[Row], groups: &[String], width: f32, cx: &App) -> TableLayout {
    let metrics = design::CONTROL_TIER.control_metrics();
    let text_width = |key: &str| {
        design::ui_text_width_zoomed(cx, t!(key).as_ref(), metrics.font_size, 400.0, false)
    };
    let header = |key: &str| design::ui_body_text_width(cx, t!(key).as_ref(), 400.0);
    let mono = |text: &str| design::mono_body_text_width(cx, text, 400.0);
    let button_padding = 2.0 * design::ui_value(cx, metrics.pad_x + 1.0);
    let action = [
        "telegram.server.cores_add",
        "telegram.server.cores_name",
        "telegram.server.cores_key",
        "telegram.server.cores_remove",
    ]
    .into_iter()
    .map(|key| text_width(key) + button_padding)
    .fold(0.0_f32, f32::max);
    let state = design::ui_value(cx, STATE_DOT + 6.0)
        + [
            RowState::Same,
            RowState::OnlyHere,
            RowState::OnlyOnStation,
            RowState::NameDiffers,
            RowState::KeyDiffers,
        ]
        .into_iter()
        .map(|state| text_width(state_label(state).0))
        .fold(header("telegram.server.cores_col_state"), f32::max);
    let name = rows
        .iter()
        .map(|row| mono(&row.name))
        .fold(header("telegram.server.cores_col_core"), f32::max)
        .min(design::ui_value(cx, NAME_MAX_W))
        .ceil();
    let address = rows
        .iter()
        .map(|row| mono(row.address.as_deref().unwrap_or("\u{2014}")))
        .fold(header("telegram.server.cores_col_address"), f32::max)
        .ceil();
    let group = groups
        .iter()
        .map(|label| design::ui_body_text_width(cx, label, 400.0))
        .fold(header("telegram.server.cores_col_group"), f32::max)
        .min(design::ui_value(cx, GROUP_MAX_W))
        .ceil();
    let wide = table_fits(
        width - design::ui_value(cx, 2.0 * TABLE_PAD_X + 2.0),
        &[name, address, group, state, action],
        design::ui_value(cx, 4.0 * COL_GAP),
    );
    TableLayout {
        wide,
        name,
        address,
        group,
    }
}

/// Diameter of a state dot, unscaled.
const STATE_DOT: f32 = 7.0;
/// Horizontal padding of a table row, unscaled.
const TABLE_PAD_X: f32 = 12.0;
/// Gap between table columns, unscaled.
const COL_GAP: f32 = 12.0;
/// Widest the name column grows before a long name truncates, unscaled.
const NAME_MAX_W: f32 = 260.0;
/// Widest the group column grows before its names truncate, unscaled.
const GROUP_MAX_W: f32 = 180.0;
/// Shortest the table gets in a low window before the page clips, unscaled: a header and a few
/// rows stay readable.
pub(super) const TABLE_MIN_H: f32 = 120.0;

/// The card's first line: green "all N match" or amber "K differ", one plain sentence, and the
/// counts on both sides at the right.
pub(super) fn summary_el(summary: CoresSummary, counts: &Counts, cx: &App) -> AnyElement {
    let (tone, tag, sentence) = if summary.differ == 0 {
        (
            MoonTone::Positive,
            t!(
                &plural_key("telegram.server.cores_all_match", summary.total as u64),
                n = summary.total
            ),
            t!("telegram.server.cores_all_match_hint"),
        )
    } else {
        (
            MoonTone::Warning,
            t!(
                &plural_key("telegram.server.cores_differ_tag", summary.differ as u64),
                k = summary.differ
            ),
            t!("telegram.server.cores_differ_lead"),
        )
    };
    h_flex()
        .w_full()
        .min_w_0()
        .flex_wrap()
        .items_center()
        .gap(design::ui_px(cx, 10.0))
        .debug_selector(|| "station-cores-summary".into())
        .child(
            div().flex_none().child(
                MoonTag::new()
                    .tone(tone)
                    .mono(false)
                    .child(div().font_family(design::ui_font()).child(tag.to_string())),
            ),
        )
        .child(
            div()
                .flex_1()
                .flex_basis(design::ui_px(cx, 200.0))
                .min_w_0()
                .font_family(design::ui_font())
                .child(sentence.to_string()),
        )
        .child(counts_el(counts, cx))
        .into_any_element()
}

/// "на станции N · здесь M", muted and monospaced.
fn counts_el(counts: &Counts, cx: &App) -> Div {
    div()
        .flex_none()
        .whitespace_nowrap()
        .font_family(design::mono())
        .text_color(rgba_from(MoonPalette::active(cx).text_muted, 1.0))
        .debug_selector(|| "station-cores-counts".into())
        .child(
            t!(
                "telegram.server.cores_count",
                station = counts.on_station,
                here = counts.here
            )
            .to_string(),
        )
}

/// The state: a tone dot and one word, the reason a differing core is listed on hover.
fn state_word(index: usize, row: &Row, cx: &App) -> Stateful<Div> {
    let (key, tone) = state_label(row.state);
    let color = rgba_from(tone.color(MoonPalette::active(cx)), 1.0);
    let word = h_flex()
        .id(("station-core-state", index))
        .flex_none()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .text_color(color)
        .child(
            div()
                .flex_none()
                .size(design::ui_px(cx, STATE_DOT))
                .rounded_full()
                .bg(color),
        )
        .child(
            div()
                .flex_none()
                .whitespace_nowrap()
                .font_family(design::ui_font())
                .child(t!(key).to_string()),
        );
    match reason_key(row.state) {
        Some(reason) => word.tooltip(text_tooltip_value(t!(reason).to_string())),
        None => word,
    }
}

/// A truncating monospaced core name with the full name on hover — and the station's name too
/// when the two differ.
fn compact_name_cell(index: usize, row: &Row) -> Stateful<Div> {
    let hover = match (&row.station_name, row.state) {
        (Some(was), RowState::NameDiffers) => format!(
            "{}\n{}",
            row.name,
            t!("telegram.server.cores_was", name = was)
        ),
        _ => row.name.clone(),
    };
    div()
        .id(("station-core-row-name", index))
        .min_w_0()
        .truncate()
        .font_family(design::mono())
        .tooltip(text_tooltip_value(hover))
        .child(row.name.clone())
}

/// One table row: one line in the columns when the table fits, else name+state over
/// address, group and action.
pub(super) fn compact_row(
    index: usize,
    row: &Row,
    group: &str,
    actions: AnyElement,
    layout: TableLayout,
    cx: &App,
) -> AnyElement {
    let palette = MoonPalette::active(cx);
    let muted = rgba_from(palette.text_muted, 1.0);
    let address = core_address_cell(row.address.clone())
        .text_color(muted)
        .debug_selector(move || format!("station-core-addr-{index}"));
    let group = div()
        .min_w_0()
        .truncate()
        .font_family(design::ui_font())
        .when(group == "\u{2014}", |el| el.text_color(muted))
        .debug_selector(move || format!("station-core-group-{index}"))
        .child(group.to_owned());
    let actions = div()
        .flex_none()
        .debug_selector(move || format!("station-core-act-{index}"))
        .child(actions);
    let shell = div()
        .w_full()
        .min_w_0()
        .px(design::ui_px(cx, TABLE_PAD_X))
        .py(design::ui_px(cx, 3.0))
        .border_b_1()
        .border_color(rgba_from(palette.border, 1.0))
        .debug_selector(move || format!("station-core-row-{index}"));
    if layout.wide {
        shell
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .min_h(design::ui_px(cx, 21.0))
                    .items_center()
                    .gap(design::ui_px(cx, COL_GAP))
                    .child(
                        compact_name_cell(index, row)
                            .flex_none()
                            .w(px(layout.name))
                            .debug_selector(move || format!("station-core-name-{index}")),
                    )
                    .child(address.w(px(layout.address)))
                    .child(group.flex_none().w(px(layout.group)))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(design::ui_px(cx, 10.0))
                            .child(state_word(index, row, cx))
                            .child(actions),
                    ),
            )
            .into_any_element()
    } else {
        shell
            .child(
                v_flex()
                    .w_full()
                    .min_w_0()
                    .gap(design::ui_px(cx, 4.0))
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .items_center()
                            .gap(design::ui_px(cx, 8.0))
                            .child(
                                compact_name_cell(index, row)
                                    .flex_1()
                                    .debug_selector(move || format!("station-core-name-{index}")),
                            )
                            .child(state_word(index, row, cx)),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .flex_wrap()
                            .items_center()
                            .gap(design::ui_px(cx, 8.0))
                            .child(address)
                            .child(group.flex_1())
                            .child(actions),
                    ),
            )
            .into_any_element()
    }
}

/// The block's tail: the last result only when there is one, then the sync note.
pub(super) fn result_els(result: Option<String>, cx: &App) -> Vec<AnyElement> {
    let palette = MoonPalette::active(cx);
    let muted = rgba_from(palette.text_muted, 1.0);
    let mut els = Vec::new();
    if let Some(result) = result.filter(|result| !result.is_empty()) {
        els.push(
            div()
                .w_full()
                .min_w_0()
                .font_family(design::mono())
                .text_color(muted)
                .min_h(design::ui_px(cx, 28.0))
                .p(design::ui_px(cx, 6.0))
                .rounded(design::ui_px(cx, 5.0))
                .bg(rgba_from(palette.shell_high, 1.0))
                .truncate()
                .child(result)
                .into_any_element(),
        );
    }
    els.push(
        div()
            .text_color(muted)
            .debug_selector(|| "station-cores-sync-hint".into())
            .child(t!("telegram.server.cores_sync_hint").to_string())
            .into_any_element(),
    );
    els
}

/// Search beside the All / Differ chips; wraps under the search when narrow.
pub(super) fn list_controls_el(
    query: &Entity<MoonInputState>,
    filter: CoresFilter,
    summary: CoresSummary,
    on_filter: Rc<dyn Fn(CoresFilter, &mut Window, &mut App)>,
    cx: &App,
) -> AnyElement {
    let filters = [CoresFilter::All, CoresFilter::Differ];
    let chips = MoonSegmentedControl::new("station-cores-filter")
        .items(filters.map(|item| {
            let label = match item {
                CoresFilter::All => t!(
                    &plural_key("telegram.server.cores_filter_all", summary.total as u64),
                    n = summary.total
                ),
                CoresFilter::Differ => {
                    t!(
                        &plural_key("telegram.server.cores_filter_differ", summary.differ as u64),
                        k = summary.differ
                    )
                }
            };
            MoonSegmentItem::new("", label.to_string())
                .fit_width(cx, 48.0, 320.0)
                .selected(item == filter)
        }))
        .on_click(move |index, _, window, app| {
            if let Some(item) = filters.get(index) {
                on_filter(*item, window, app);
            }
        })
        .render();
    h_flex()
        .w_full()
        .min_w_0()
        .flex_wrap()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .child(
            div()
                .flex_1()
                .flex_basis(design::ui_px(cx, 180.0))
                .min_w_0()
                .max_w(design::ui_px(cx, 260.0))
                .child(
                    MoonInput::new("station-cores-search")
                        .state(query)
                        .size(design::INPUT_SIZE)
                        .placeholder(t!("telegram.server.cores_search_placeholder").to_string()),
                ),
        )
        .child(
            div()
                .flex_none()
                .debug_selector(|| "station-cores-chips".into())
                .child(chips),
        )
        .into_any_element()
}
/// The core table: it takes the card's remaining height, its header stays put and its rows
/// scroll inside it. An empty filter result is a line inside the table, never a collapsed table.
///
/// Args:
///     layout: The measured columns.
///     rows: The rows the filter and search left.
///     empty_key: Locale key of the line shown when `rows` is empty.
///     scroll: The rows' scroll position.
///     cx: Application context for theme and scale.
pub(super) fn table_el(
    layout: TableLayout,
    rows: Vec<AnyElement>,
    empty_key: &str,
    scroll: &ScrollHandle,
    cx: &App,
) -> AnyElement {
    let palette = MoonPalette::active(cx);
    let muted = rgba_from(palette.text_muted, 1.0);
    let head = layout.wide.then(|| {
        h_flex()
            .flex_none()
            .w_full()
            .min_w_0()
            .gap(design::ui_px(cx, COL_GAP))
            .px(design::ui_px(cx, TABLE_PAD_X))
            .py(design::ui_px(cx, 6.0))
            .bg(rgba_from(palette.head_row, 1.0))
            .border_b_1()
            .border_color(rgba_from(palette.border, 1.0))
            .text_color(muted)
            .font_family(design::ui_font())
            .debug_selector(|| "station-cores-head".into())
            .child(
                div()
                    .w(px(layout.name))
                    .flex_none()
                    .truncate()
                    .child(t!("telegram.server.cores_col_core").to_string()),
            )
            .child(
                div()
                    .w(px(layout.address))
                    .flex_none()
                    .child(t!("telegram.server.cores_col_address").to_string()),
            )
            .child(
                div()
                    .w(px(layout.group))
                    .flex_none()
                    .truncate()
                    .child(t!("telegram.server.cores_col_group").to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(t!("telegram.server.cores_col_state").to_string()),
            )
    });
    let body = if rows.is_empty() {
        div()
            .w_full()
            .p(design::ui_px(cx, 28.0))
            .text_color(muted)
            .font_family(design::ui_font())
            .flex()
            .justify_center()
            .text_center()
            .debug_selector(|| "station-cores-empty".into())
            .child(t!(empty_key).to_string())
            .into_any_element()
    } else {
        v_flex()
            .w_full()
            .min_w_0()
            .children(rows)
            .into_any_element()
    };
    v_flex()
        .flex_1()
        .min_h(design::ui_px(cx, TABLE_MIN_H))
        .w_full()
        .min_w_0()
        .border_1()
        .border_color(rgba_from(palette.border, 1.0))
        .rounded(design::ui_px(cx, 4.0))
        .overflow_hidden()
        .debug_selector(|| "station-cores-table-area".into())
        .children(head)
        .child(
            div()
                .id("station-cores-table")
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .min_w_0()
                .overflow_y_scroll()
                .track_scroll(scroll)
                .debug_selector(|| "station-cores-table".into())
                .child(body),
        )
        .into_any_element()
}

/// What the Station cores card says when it has no comparison rows to list.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum CoresNote {
    /// The station's listing is being read.
    Loading,
    /// The read ended without a listing; the reason when the read failed.
    NotRead(Option<String>),
    /// The station answered with no cores and the terminal has none to send either.
    Empty,
}

impl CoresNote {
    /// The note for a station whose listing is unread.
    ///
    /// Args:
    ///     running: Whether a station job (the page's own read included) is running.
    ///     error: Why the last read failed, if it did.
    ///
    /// Returns:
    ///     `Loading` while a job runs, else `NotRead` with the error.
    pub(super) fn unread(running: bool, error: Option<String>) -> Self {
        if running {
            Self::Loading
        } else {
            Self::NotRead(error)
        }
    }
}

/// The card's body when it has no rows: never blank. A listing that is not read yet shows the
/// read in progress, or what went wrong and a button to read again; an empty listing says so and
/// where cores come from.
///
/// Args:
///     note: Which state to show.
///     retry: Reads the station again; drawn only for `NotRead`.
///     cx: Application context for theme and scale.
///
/// Returns:
///     The note, under the selector `station-cores-note`.
pub(super) fn cores_note_el(
    note: CoresNote,
    retry: crate::settings::common::Action,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    let muted = rgba_from(p.text_muted, 1.0);
    let text = |key: &str| {
        crate::settings::common::prose(
            "station-cores-note-text".into(),
            t!(key).to_string(),
            false,
            cx,
        )
        .text_color(muted)
    };
    let body = v_flex()
        .debug_selector(|| "station-cores-note".into())
        .w_full()
        .min_w_0()
        .items_start()
        .gap(design::ui_px(cx, 8.0));
    match note {
        CoresNote::Loading => body.child(text("telegram.server.cores_loading")),
        CoresNote::Empty => body.child(text("telegram.server.cores_empty")),
        CoresNote::NotRead(error) => body
            .child(text("telegram.server.cores_not_read"))
            .children(error.map(|reason| {
                div()
                    .min_w_0()
                    .text_size(design::t_caption(cx))
                    .text_color(rgba_from(p.red_text, 1.0))
                    .child(reason)
            }))
            .child(
                div().debug_selector(|| "station-cores-read".into()).child(
                    MoonButton::new("station-cores-read")
                        .size(design::CONTROL_TIER)
                        .label(t!("telegram.server.cores_read_again").to_string())
                        .on_click(move |_, window, cx| retry(window, cx))
                        .render(),
                ),
            ),
    }
    .into_any_element()
}

/// A row and its index in the comparison.
struct RowAt<'a>(usize, &'a Row);

impl SettingsView {
    /// How many cores differ between the terminal and the station, for the Station cores
    /// page's navigation count; `None` while that is not known. Phase 4 fills it in.
    pub(in crate::settings) fn station_diverging_count(&self, cx: &App) -> Option<u32> {
        let seen = self.backend.read(cx).station.cores_seen.as_ref()?;
        seen.as_ref()?;
        Some(diverging(&self.telegram.server.cores_cache.rows) as u32)
    }

    /// The Station cores page's one send bar: how many cores differ, and the button that sends
    /// every terminal core the station lacks or holds differently. Station-only cores are never
    /// sent; they leave the station only through their own confirmed removal.
    pub(in crate::settings) fn station_cores_bar(&self, cx: &Context<Self>) -> SaveBar {
        let Some(target) = self.telegram.server.known().cloned() else {
            return SaveBar::None;
        };
        let backend = self.backend.read(cx);
        if !matches!(backend.station.cores_seen, Some(Some(_))) {
            return SaveBar::None;
        }
        let ed = &self.telegram.server;
        let rows = &ed.cores_cache.rows;
        let n = diverging(rows);
        let upsert = cores_sync::bulk(rows);
        let busy = backend.station.busy();
        let msg = if n == 0 {
            t!("settings.mode.remote").to_string()
        } else {
            t!(&plural_key("station.cores.diverge_n", n as u64), n = n).to_string()
        };
        let eligible: Vec<_> = ed
            .cores_cache
            .local_revision
            .iter()
            .map(|core| core.uid)
            .collect();
        let dirty = !upsert.is_empty() && !busy;
        let upsert_n = upsert.len();
        let view = cx.entity().downgrade();
        SaveBar::Remote(RemoteBar {
            msg,
            dirty,
            apply: Rc::new(move |_, app| {
                if upsert.is_empty() {
                    return;
                }
                let job = Job::Cores {
                    target: target.clone(),
                    upsert: upsert.clone(),
                    eligible: eligible.clone(),
                    started: CoreStart::Bulk,
                };
                let _ = view.update(app, |this, cx| this.server_bot_run(Ok(job), cx));
            }),
            revert: None,
            label: if upsert_n == 0 {
                t!("telegram.server.access_apply").to_string()
            } else {
                t!(
                    &plural_key("station.cores.send_n", upsert_n as u64),
                    n = upsert_n
                )
                .to_string()
            },
        })
    }

    /// Refresh the comparison rows after a listing or local config change.
    pub(super) fn station_cores_sync(&mut self, cx: &App) {
        let backend = self.backend.read(cx);
        self.telegram.server.cores_cache.update(
            &backend.station.cores_seen,
            cores_sync::local_cores(&backend.config),
            backend.station.core_uid_high_water,
        );
    }

    /// The page's one card: the verdict line, search and filter, then the table, always open,
    /// taking the page's remaining height; the station-only note, the last result and the sync
    /// note under it. Unread, unsupported and empty listings get a short card of their own.
    pub(super) fn station_cores_block(
        &self,
        target: &Target,
        width: f32,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let backend = self.backend.read(cx);
        let Some(seen) = backend.station.cores_seen.as_ref() else {
            // Unread: the page asked for it on open. A blank card is never a state.
            let note =
                CoresNote::unread(backend.station.running, backend.station.bot_error.clone());
            let view = cx.entity().downgrade();
            let retry: crate::settings::common::Action = Rc::new(move |_, app| {
                let _ = view.update(app, |this, cx| {
                    this.backend.update(cx, |b, bcx| b.station_refresh_bot(bcx));
                    cx.notify();
                });
            });
            return Some(
                card("station-cores", cx)
                    .child(cores_note_el(note, retry, cx))
                    .into_any_element(),
            );
        };
        let palette = MoonPalette::active(cx);
        let muted = rgba_from(palette.text_muted, 1.0);
        if seen.is_none() {
            return Some(
                card("station-cores", cx)
                    .child(
                        div()
                            .text_color(muted)
                            .child(t!("telegram.server.cores_unsupported").to_string()),
                    )
                    .into_any_element(),
            );
        }
        let ed = &self.telegram.server;
        let rows = &ed.cores_cache.rows;
        let counts = Counts::from_rows(rows);
        if rows.is_empty() {
            return Some(
                card("station-cores", cx)
                    .child(
                        h_flex()
                            .w_full()
                            .justify_end()
                            .child(counts_el(&counts, cx)),
                    )
                    .child(cores_note_el(CoresNote::Empty, Rc::new(|_, _| {}), cx))
                    .children(result_els(backend.station.cores_result.clone(), cx))
                    .into_any_element(),
            );
        }
        let summary = cores_summary(rows);
        let busy = backend.station.busy();
        let eligible: Vec<_> = ed
            .cores_cache
            .local_revision
            .iter()
            .map(|core| core.uid)
            .collect();
        let groups: Vec<String> = rows
            .iter()
            .map(|row| group_label(&backend.config.core_groups, row.terminal_uid))
            .collect();
        // The card's content box: the page column less the card's padding and border.
        let content_w = super::page_column_w(width, cx) - design::ui_value(cx, 2.0 * 12.0 + 2.0);
        let layout = table_layout(rows, &groups, content_w, cx);
        let query = ed.cores_query.read(cx).value().to_string();
        let shown: Vec<AnyElement> = filter_rows(rows, &query, ed.cores_filter)
            .into_iter()
            .map(|(index, row)| {
                let actions = self.station_core_actions(
                    RowAt(index, row),
                    counts.on_station,
                    target,
                    &eligible,
                    busy,
                    cx,
                );
                compact_row(index, row, &groups[index], actions, layout, cx)
            })
            .collect();
        let empty_key = if ed.cores_filter == CoresFilter::Differ && query.trim().is_empty() {
            "telegram.server.cores_no_differ"
        } else {
            "telegram.server.cores_nothing_found"
        };
        let view = cx.entity();
        let section = card("station-cores", cx)
            .flex_1()
            .min_h(px(0.0))
            .child(summary_el(summary, &counts, cx))
            .child(list_controls_el(
                &ed.cores_query,
                ed.cores_filter,
                summary,
                Rc::new(move |filter, _, app| {
                    view.update(app, |this, cx| {
                        this.telegram.server.cores_filter = filter;
                        cx.notify();
                    })
                }),
                cx,
            ))
            .child(table_el(layout, shown, empty_key, &ed.cores_scroll, cx))
            // The send button lives in the page's bar; the station-only note stays with the list.
            .when(counts.pushable != 0 && counts.only_station != 0, |el| {
                el.child(
                    div()
                        .flex_none()
                        .text_color(muted)
                        .child(t!("telegram.server.cores_send_all_hint").to_string()),
                )
            })
            .children(result_els(backend.station.cores_result.clone(), cx));
        Some(section.into_any_element())
    }

    /// Bind row actions to station identities; removal needs an explicit second click.
    fn station_core_actions(
        &self,
        RowAt(index, row): RowAt,
        station_count: usize,
        target: &Target,
        eligible: &[u64],
        busy: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let mut actions = h_flex()
            .w_full()
            .min_w_0()
            .flex_wrap()
            .gap(design::ui_px(cx, 6.0));
        let id = row.station_uid.unwrap_or_default();
        let changes = cores_sync::bulk(std::slice::from_ref(row));
        let eligible = eligible.to_vec();
        let action = match row.state {
            RowState::OnlyHere => Some(("telegram.server.cores_add", CoreStart::Add)),
            RowState::NameDiffers => Some(("telegram.server.cores_name", CoreStart::Name)),
            RowState::KeyDiffers => Some(("telegram.server.cores_key", CoreStart::Key)),
            _ => None,
        };
        if let Some((key, started)) = action {
            let target = target.clone();
            actions = actions.child(
                MoonButton::new(action_id("send", index))
                    .size(design::CONTROL_TIER)
                    .label(t!(key).to_string())
                    .disabled(busy || changes.is_empty())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.server_bot_run(
                            Ok(Job::Cores {
                                target: target.clone(),
                                upsert: changes.clone(),
                                eligible: eligible.clone(),
                                started,
                            }),
                            cx,
                        );
                    }))
                    .render(),
            );
        } else if removable(row, station_count) {
            let target = target.clone();
            let name = row.name.clone();
            let address = row.address.clone();
            actions = actions.child(
                MoonButton::new(action_id("remove", index))
                    .danger()
                    .size(design::CONTROL_TIER)
                    .label(t!("telegram.server.cores_remove").to_string())
                    .disabled(busy)
                    .on_click(cx.listener(move |_, _, window, cx| {
                        let view = cx.entity().downgrade();
                        // The dialog runs the removal at most once.
                        let job = std::cell::Cell::new(Some(Job::CoresRemove {
                            target: target.clone(),
                            uids: vec![id],
                            names: vec![name.clone()],
                            addresses: vec![address.clone()],
                            eligible: eligible.clone(),
                        }));
                        let arm: RemovalArm = (id, address.clone(), name.clone());
                        let body = match &address {
                            Some(address) => format!("{name} · {address}"),
                            None => name.clone(),
                        };
                        super::confirm_danger(
                            window,
                            cx,
                            format!("{}?", t!("telegram.server.cores_remove")),
                            body,
                            t!("telegram.server.cores_remove_confirm").to_string(),
                            Rc::new(move |_, app| {
                                let Some(job) = job.take() else {
                                    return;
                                };
                                let _ = view.update(app, |this, cx| {
                                    this.station_cores_sync(cx);
                                    let busy = this.backend.read(cx).station.busy();
                                    let rows = &this.telegram.server.cores_cache.rows;
                                    match removal_check(busy, &arm, rows) {
                                        Ok(()) => this.server_bot_run(Ok(job), cx),
                                        Err(key) => {
                                            this.server_bot_run(Err(t!(key).to_string()), cx)
                                        }
                                    }
                                });
                            }),
                        );
                    }))
                    .render(),
            );
        }
        actions.into_any_element()
    }
}

#[cfg(test)]
mod tests;
