//! Explicit address-based station core comparison, updates and confirmed removals.

use std::rc::Rc;

use gpui::*;
use moon_remote::ssh::Target;
use moon_ui::{
    MoonButton, MoonDisclosure, MoonDisclosureDirection, MoonGroupBox, MoonInput, MoonInputEvent,
    MoonInputState, MoonPalette, MoonSegmentItem, MoonSegmentedControl, MoonTag, MoonTone, h_flex,
    rgba_from, v_flex,
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

/// Keep the full identity visible at any width, with natural multiline height and hover text.
fn core_name_cell(index: usize, name: String) -> Stateful<Div> {
    div()
        .id(("station-core-name", index))
        .w_full()
        .min_w_0()
        .whitespace_normal()
        .tooltip(text_tooltip_value(name.clone()))
        .child(name)
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

/// Where a core's actions are drawn: a differing core shows them both open and in the full list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActionSurface {
    Card,
    Row,
}

/// A row action's element id, unique per row and surface even for rows without a station uid.
fn action_id(verb: &str, surface: ActionSurface, index: usize) -> SharedString {
    let surface = match surface {
        ActionSurface::Card => "card",
        ActionSurface::Row => "row",
    };
    SharedString::from(format!("station-core-{verb}-{surface}-{index}"))
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

/// Choose a table only when its measured controls and readable text columns all fit.
fn table_fits(width: f32, action: f32, state: f32, name: f32, address: f32, gaps: f32) -> bool {
    width >= action + state + name + address + gaps
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

/// One plain sentence on why a differing core is listed.
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

/// Measured columns of the full list and whether a row fits on one line.
#[derive(Clone, Copy, Debug)]
pub(super) struct TableLayout {
    pub(super) wide: bool,
    pub(super) address: f32,
    pub(super) state: f32,
    pub(super) action: f32,
}

/// Measure the widest action (the armed removal pair included), state word and address.
pub(super) fn table_layout(rows: &[Row], width: f32, cx: &App) -> TableLayout {
    let metrics = design::CONTROL_TIER.control_metrics();
    let text_width = |key: &str| {
        design::ui_text_width_zoomed(cx, t!(key).as_ref(), metrics.font_size, 400.0, false)
    };
    let button_padding = 2.0 * design::ui_value(cx, metrics.pad_x + 1.0);
    let cancel = text_width("dialogs.cancel") + button_padding;
    let action = [
        "telegram.server.cores_add",
        "telegram.server.cores_name",
        "telegram.server.cores_key",
    ]
    .into_iter()
    .map(|key| text_width(key) + button_padding)
    .chain(std::iter::once(
        text_width("telegram.server.cores_remove_confirm")
            + button_padding
            + cancel
            + design::ui_value(cx, 6.0),
    ))
    .fold(0.0_f32, f32::max)
    .ceil();
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
        .fold(
            design::ui_body_text_width(cx, t!("telegram.server.cores_col_state").as_ref(), 400.0),
            f32::max,
        );
    let state = state.ceil();
    let address = rows
        .iter()
        .map(|row| {
            design::mono_body_text_width(cx, row.address.as_deref().unwrap_or("\u{2014}"), 400.0)
        })
        .fold(
            design::ui_body_text_width(cx, t!("telegram.server.cores_col_address").as_ref(), 400.0),
            f32::max,
        )
        .ceil();
    let wide = table_fits(
        width - design::ui_value(cx, 2.0 * TABLE_PAD_X + 2.0),
        action,
        state,
        design::ui_value(cx, 100.0),
        address,
        design::ui_value(cx, 24.0),
    );
    TableLayout {
        wide,
        address,
        state,
        action,
    }
}

/// Diameter of a full-list state dot, unscaled.
const STATE_DOT: f32 = 7.0;
/// Horizontal padding of a full-list row, unscaled.
const TABLE_PAD_X: f32 = 10.0;
/// Tallest the full list grows before it scrolls, unscaled.
pub(super) const TABLE_MAX_H: f32 = 340.0;

/// Green "all N match" or amber "K differ" beside its one plain sentence.
pub(super) fn summary_el(summary: CoresSummary, cx: &App) -> AnyElement {
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
            t!(
                &plural_key(
                    "telegram.server.cores_differ_hint",
                    (summary.total - summary.differ) as u64
                ),
                x = summary.total - summary.differ,
                n = summary.total
            ),
        )
    };
    h_flex()
        .w_full()
        .min_w_0()
        .flex_wrap()
        .items_center()
        .gap(design::ui_px(cx, 8.0))
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
        .into_any_element()
}

/// A differing core, open: full wrapping name, address, state, reason and its own actions.
pub(super) fn differ_card(index: usize, row: &Row, actions: AnyElement, cx: &App) -> AnyElement {
    let palette = MoonPalette::active(cx);
    let muted = rgba_from(palette.text_muted, 1.0);
    let (key, tone) = state_label(row.state);
    let mut name = v_flex()
        .w_full()
        .min_w_0()
        .child(core_name_cell(index, row.name.clone()).font_family(design::ui_font()));
    if row.state == RowState::NameDiffers {
        name = name.child(
            div()
                .text_color(muted)
                .font_family(design::mono())
                .w_full()
                .min_w_0()
                .whitespace_normal()
                .child(
                    t!(
                        "telegram.server.cores_was",
                        name = row.station_name.as_deref().unwrap_or_default()
                    )
                    .to_string(),
                ),
        );
    }
    let reason = reason_key(row.state).map(|key| t!(key).to_string());
    h_flex()
        .w_full()
        .min_w_0()
        .flex_wrap()
        .items_center()
        .gap_x(design::ui_px(cx, 12.0))
        .gap_y(design::ui_px(cx, 6.0))
        .px(design::ui_px(cx, 10.0))
        .py(design::ui_px(cx, 7.0))
        .rounded(design::ui_px(cx, 5.0))
        .bg(rgba_from(tone.color(palette), 0.10))
        .debug_selector(move || format!("station-core-card-{index}"))
        .child(name)
        .child(
            core_address_cell(row.address.clone())
                .text_color(muted)
                .debug_selector(move || format!("station-core-card-addr-{index}")),
        )
        .child(
            div().flex_none().child(
                MoonTag::new().tone(tone).mono(false).child(
                    div()
                        .font_family(design::ui_font())
                        .child(t!(key).to_string()),
                ),
            ),
        )
        .children(reason.map(|reason| {
            div()
                .flex_1()
                .flex_basis(design::ui_px(cx, 220.0))
                .min_w_0()
                .font_family(design::ui_font())
                .child(reason)
        }))
        .child(
            div()
                .flex_none()
                .ml_auto()
                .debug_selector(move || format!("station-core-card-act-{index}"))
                .child(actions),
        )
        .into_any_element()
}

/// The compact list's state: a tone dot and one word.
fn state_word(state: RowState, cx: &App) -> Div {
    let (key, tone) = state_label(state);
    let color = rgba_from(tone.color(MoonPalette::active(cx)), 1.0);
    h_flex()
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
        )
}

/// A truncating core name with the full name on hover; the row that owns the width holds it.
fn compact_name_cell(index: usize, name: &str) -> Stateful<Div> {
    div()
        .id(("station-core-row-name", index))
        .flex_1()
        .min_w_0()
        .truncate()
        .font_family(design::ui_font())
        .tooltip(text_tooltip_value(name.to_owned()))
        .child(name.to_owned())
}

/// One full-list row: a single line when the table fits, else name+state over address+action.
pub(super) fn compact_row(
    index: usize,
    row: &Row,
    actions: AnyElement,
    layout: TableLayout,
    cx: &App,
) -> AnyElement {
    let palette = MoonPalette::active(cx);
    let muted = rgba_from(palette.text_muted, 1.0);
    let address = core_address_cell(row.address.clone())
        .text_color(muted)
        .debug_selector(move || format!("station-core-addr-{index}"));
    let actions = div()
        .flex_none()
        .debug_selector(move || format!("station-core-act-{index}"))
        .child(actions);
    let shell = div()
        .w_full()
        .min_w_0()
        .px(design::ui_px(cx, TABLE_PAD_X))
        .py(design::ui_px(cx, 4.0))
        .border_t_1()
        .border_color(rgba_from(palette.border, 1.0));
    if layout.wide {
        shell
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .child(compact_name_cell(index, &row.name))
                    .child(address.w(px(layout.address)))
                    .child(state_word(row.state, cx).w(px(layout.state)))
                    .child(actions.w(px(layout.action))),
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
                            .child(compact_name_cell(index, &row.name))
                            .child(state_word(row.state, cx)),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .gap(design::ui_px(cx, 8.0))
                            .child(address)
                            .child(actions),
                    ),
            )
            .into_any_element()
    }
}

/// The caret line that opens the full list.
pub(super) fn show_all_el(
    open: bool,
    total: usize,
    on_toggle: Rc<dyn Fn(&mut Window, &mut App)>,
    cx: &App,
) -> AnyElement {
    let palette = MoonPalette::active(cx);
    let caret_toggle = on_toggle.clone();
    h_flex()
        .id("station-cores-show-all")
        .flex_none()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .cursor_pointer()
        .text_color(rgba_from(palette.text_muted, 1.0))
        .debug_selector(|| "station-cores-show-all".into())
        .on_click(move |_, window, app| on_toggle(window, app))
        .child(
            MoonDisclosure::button("station-cores-show-all-caret", open)
                .direction(MoonDisclosureDirection::RightDown)
                .size(design::DISCLOSURE_GLYPH)
                .box_size(design::DISCLOSURE_BOX)
                .hover_color(palette.text)
                .on_toggle(move |_, window, app| {
                    caret_toggle(window, app);
                    app.stop_propagation();
                }),
        )
        .child(
            div()
                .flex_none()
                .whitespace_nowrap()
                .font_family(design::ui_font())
                .child(
                    t!(
                        &plural_key("telegram.server.cores_show_all", total as u64),
                        n = total
                    )
                    .to_string(),
                ),
        )
        .into_any_element()
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

/// The full list: a column header when one line fits, rows in a bounded scroll of their own.
pub(super) fn table_el(
    layout: TableLayout,
    rows: Vec<AnyElement>,
    scroll: &ScrollHandle,
    cx: &App,
) -> AnyElement {
    let palette = MoonPalette::active(cx);
    let muted = rgba_from(palette.text_muted, 1.0);
    let head = layout.wide.then(|| {
        h_flex()
            .w_full()
            .min_w_0()
            .gap(design::ui_px(cx, 8.0))
            .px(design::ui_px(cx, TABLE_PAD_X))
            .py(design::ui_px(cx, 5.0))
            .bg(rgba_from(palette.head_row, 1.0))
            .text_color(muted)
            .font_family(design::ui_font())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
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
                    .w(px(layout.state))
                    .flex_none()
                    .child(t!("telegram.server.cores_col_state").to_string()),
            )
            .child(div().w(px(layout.action)).flex_none())
    });
    let body = if rows.is_empty() {
        div()
            .w_full()
            .p(design::ui_px(cx, 14.0))
            .text_color(muted)
            .font_family(design::ui_font())
            .flex()
            .justify_center()
            .child(t!("telegram.server.cores_nothing_found").to_string())
            .into_any_element()
    } else {
        v_flex()
            .w_full()
            .min_w_0()
            .children(rows)
            .into_any_element()
    };
    v_flex()
        .w_full()
        .min_w_0()
        .border_1()
        .border_color(rgba_from(palette.border, 1.0))
        .rounded(design::ui_px(cx, 5.0))
        .overflow_hidden()
        .children(head)
        .child(
            div()
                .id("station-cores-table")
                .w_full()
                .min_w_0()
                .max_h(design::ui_px(cx, TABLE_MAX_H))
                .overflow_y_scroll()
                .track_scroll(scroll)
                .debug_selector(|| "station-cores-table".into())
                .child(body),
        )
        .into_any_element()
}

/// A row, its index in the comparison and the surface its actions are drawn on.
struct RowAt<'a>(usize, &'a Row, ActionSurface);

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

    /// Lead with the verdict and the differing cores; the full list stays folded behind a caret.
    pub(super) fn station_cores_block(
        &self,
        target: &Target,
        width: f32,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let backend = self.backend.read(cx);
        let seen = backend.station.cores_seen.as_ref()?;
        let palette = MoonPalette::active(cx);
        let muted = rgba_from(palette.text_muted, 1.0);
        let section = MoonGroupBox::new("station-cores")
            .child(crate::settings::common::group_title(
                t!("telegram.server.cores_title").to_string(),
                cx,
            ))
            .padding(12.0)
            .gap(10.0);
        if seen.is_none() {
            return Some(
                section
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
        let summary = cores_summary(rows);
        let busy = backend.station.busy();
        let eligible: Vec<_> = ed
            .cores_cache
            .local_revision
            .iter()
            .map(|core| core.uid)
            .collect();
        let mut section = MoonGroupBox::new("station-cores")
            .padding(12.0)
            .gap(10.0)
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .flex_wrap()
                    .justify_between()
                    .gap(design::ui_px(cx, 8.0))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("telegram.server.cores_title").to_string()),
                    )
                    .child(
                        div().font_family(design::mono()).text_color(muted).child(
                            t!(
                                "telegram.server.cores_count",
                                station = counts.on_station,
                                here = counts.here
                            )
                            .to_string(),
                        ),
                    ),
            );
        if rows.is_empty() {
            section = section.child(
                div()
                    .text_color(muted)
                    .child(t!("telegram.server.cores_empty").to_string()),
            );
        } else {
            section = section.child(summary_el(summary, cx));
            for (index, row) in rows.iter().enumerate() {
                if row.state.differs() {
                    let actions = self.station_core_actions(
                        RowAt(index, row, ActionSurface::Card),
                        counts.on_station,
                        target,
                        &eligible,
                        busy,
                        cx,
                    );
                    section = section.child(differ_card(index, row, actions, cx));
                }
            }
            let view = cx.entity();
            section = section.child(show_all_el(
                ed.cores_all_open,
                summary.total,
                Rc::new(move |_, app| {
                    view.update(app, |this, cx| {
                        this.telegram.server.cores_all_open ^= true;
                        cx.notify();
                    })
                }),
                cx,
            ));
            if ed.cores_all_open {
                let view = cx.entity();
                let layout = table_layout(rows, width - design::ui_value(cx, 24.0), cx);
                let query = ed.cores_query.read(cx).value().to_string();
                let shown = filter_rows(rows, &query, ed.cores_filter)
                    .into_iter()
                    .map(|(index, row)| {
                        let actions = self.station_core_actions(
                            RowAt(index, row, ActionSurface::Row),
                            counts.on_station,
                            target,
                            &eligible,
                            busy,
                            cx,
                        );
                        compact_row(index, row, actions, layout, cx)
                    })
                    .collect();
                section = section
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
                    .child(table_el(layout, shown, &ed.cores_scroll, cx));
            }
        }
        // The send button lives in the page's bar; the station-only note stays with the list.
        if counts.pushable != 0 && counts.only_station != 0 {
            section = section.child(
                div()
                    .text_color(muted)
                    .child(t!("telegram.server.cores_send_all_hint").to_string()),
            );
        }
        for el in result_els(backend.station.cores_result.clone(), cx) {
            section = section.child(el);
        }
        Some(section.into_any_element())
    }

    /// Bind row actions to station identities; removal needs an explicit second click.
    fn station_core_actions(
        &self,
        RowAt(index, row, surface): RowAt,
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
                MoonButton::new(action_id("send", surface, index))
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
                MoonButton::new(action_id("remove", surface, index))
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
