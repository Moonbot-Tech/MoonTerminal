//! "Data by core" in the Storage tab: whose data the local databases hold, and deleting it.
//!
//! The list comes from the databases, not from Connections: a core deleted from Connections keeps
//! its rows (#553), and that is exactly the core a user comes here to remove. The cores that ARE
//! in Connections sit on that tab's own tree (window group → exchange → core, `flatten_entries`),
//! and the rest follow under a section of their own. Deleting a connected core's reports makes the
//! report writer download its history again, which is the repair for a replica with a hole in it
//! (#665). Strategies, order traces and warnings are the only copy and cannot come back, so only
//! reports are ticked by default.

use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::time::Duration;

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonVariant, MoonCheckbox, MoonPalette, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::super::SettingsView;
use super::super::common::{Action, hint_row};
use super::super::connections::{
    CONN_INDENT_MARGIN, CONN_INDENT_PAD, CONN_TABLE_INSET, ConnEntry, EntryLabels, ServerRowMeta,
    conn_row_h_value, flatten_entries, group_count, group_icon, group_name, group_pill,
    sorted_group_rows, subsection_header_row,
};
use super::super::telegram::plural_key;
use crate::design;
use moon_core::config::paths;

mod delete;

#[cfg(test)]
mod tests;

/// How long a delete waits for each writer to confirm before reporting it as not confirmed.
const WRITER_WAIT: Duration = Duration::from_secs(30);

/// Widths of the count columns, in unscaled pixels: reports, strategies, traces, warnings, sync.
const COUNT_COLS: [f32; 5] = [90.0, 110.0, 80.0, 80.0, 80.0];

/// What the local stores hold for one core.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct CoreCounts {
    pub uid: u64,
    /// The newest name a store recorded for this core, empty when none did.
    pub stored_name: String,
    pub reports: u64,
    /// Strategy heads, live and deleted.
    pub strategies: u64,
    pub versions: u64,
    pub traces: u64,
    pub warnings: u64,
}

/// Which stores a delete touches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Kinds {
    pub reports: bool,
    pub strategies: bool,
    pub traces: bool,
    pub warnings: bool,
}

impl Kinds {
    fn any(self) -> bool {
        self.reports || self.strategies || self.traces || self.warnings
    }
}

/// Section state, kept on the Storage tab for the life of the Settings window.
///
/// The table is always on the page. There is no collapsed state.
pub(in crate::settings) struct CoreDataEd {
    /// The counts, or why they could not be read; `None` until the first count lands.
    pub(super) counts: Option<Result<Vec<CoreCounts>, String>>,
    pub(super) inflight: bool,
    pub(super) selected: BTreeSet<u64>,
    pub(super) kinds: Kinds,
}

impl Default for CoreDataEd {
    fn default() -> Self {
        Self {
            counts: None,
            inflight: false,
            selected: BTreeSet::new(),
            kinds: Kinds {
                reports: true,
                strategies: false,
                traces: false,
                warnings: false,
            },
        }
    }
}

/// Count every per-core store, off the UI thread.
///
/// Measured on a 608k-row replica with 29 cores: 16 ms for the reports, 1-10 ms for each other
/// store. A replica that does not exist yet counts as empty; one that cannot be read is an error,
/// never a list of zeros.
fn collect() -> Result<Vec<CoreCounts>, String> {
    let reports = match moon_core::db::report_rows_by_core() {
        Ok(rows) => rows,
        Err(moon_core::db::ReadFail::NotReady) => Vec::new(),
        Err(e) => return Err(e.to_string()),
    };
    let strategies = moon_core::strat_db::counts_by_core().map_err(|e| e.to_string())?;
    let traces = moon_core::db::order_traces::answers_by_core().map_err(|e| e.to_string())?;
    let warnings =
        crate::backend::core_warn::store::episodes_by_core(&paths::core_warnings_db_path())
            .map_err(|e| e.to_string())?;
    Ok(merge(reports, strategies, traces, warnings))
}

/// Merge the per-store counts into one row per core, newest report name winning.
fn merge(
    reports: Vec<(u64, String, u64)>,
    strategies: HashMap<u64, moon_core::strat_db::CoreStratCounts>,
    traces: HashMap<u64, u64>,
    warnings: HashMap<u64, u64>,
) -> Vec<CoreCounts> {
    fn row(out: &mut HashMap<u64, CoreCounts>, uid: u64) -> &mut CoreCounts {
        out.entry(uid).or_insert_with(|| CoreCounts {
            uid,
            ..CoreCounts::default()
        })
    }
    let mut out: HashMap<u64, CoreCounts> = HashMap::new();
    for (uid, name, n) in reports {
        let r = row(&mut out, uid);
        r.reports = n;
        r.stored_name = name;
    }
    for (uid, c) in strategies {
        let r = row(&mut out, uid);
        r.strategies = c.heads;
        r.versions = c.versions;
        if r.stored_name.is_empty() {
            r.stored_name = c.name;
        }
    }
    for (uid, n) in traces {
        row(&mut out, uid).traces = n;
    }
    for (uid, n) in warnings {
        row(&mut out, uid).warnings = n;
    }
    let mut rows: Vec<CoreCounts> = out.into_values().collect();
    rows.sort_by_key(|r| r.uid);
    rows
}

/// Where a core stands relative to Connections, which is what decides what deleting it means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Group {
    /// Configured and ready: its reports download again after a delete.
    Connected,
    /// Configured, not ready now: its reports download on its next connection.
    Offline,
    /// Not in Connections: whatever is deleted is gone.
    Archive,
}

/// One core as the list shows it.
#[derive(Clone, Debug)]
pub(super) struct CoreLine {
    pub(super) counts: CoreCounts,
    pub(super) name: String,
    pub(super) group: Group,
    /// The running report catch-up's percent, `Some` while one runs on a ready core.
    pub(super) syncing: Option<String>,
}

impl CoreLine {
    /// A core whose catch-up is running cannot be selected: a page requested before the wipe
    /// would land after it and leave a hole below the new download.
    fn selectable(&self) -> bool {
        self.syncing.is_none()
    }
}

/// A list line: the Connections tree, then the cores that are not in Connections.
#[derive(Clone, Debug)]
pub(super) enum Line {
    /// A window-group branch, with its cores for the group's checkbox.
    Group {
        name: String,
        icon: u32,
        uids: Vec<u64>,
    },
    /// An exchange section inside a group, or the section of cores not in Connections.
    Section {
        id: SharedString,
        caption: String,
        count: usize,
        highlighted: bool,
    },
    Core(CoreLine),
}

/// Lay the cores out on the Connections tree, then list the rest under `archive_caption`.
///
/// `entries` is the Connections list's own flattening (`flatten_entries`), so groups, exchange
/// sections and the core order are exactly the ones that tab shows.
pub(super) fn lines(
    entries: &[ConnEntry],
    mut cores: HashMap<u64, CoreLine>,
    archive_caption: &str,
) -> Vec<Line> {
    let mut out = Vec::with_capacity(entries.len() + cores.len() + 1);
    let mut group: Option<usize> = None;
    for entry in entries {
        match entry {
            ConnEntry::GroupHeader { name, icon, .. } => {
                group = Some(out.len());
                out.push(Line::Group {
                    name: name.clone(),
                    icon: *icon,
                    uids: Vec::new(),
                });
            }
            ConnEntry::ExchangeHeader {
                group_index,
                exchange_index,
                caption,
                member_count,
                identified,
            } => out.push(Line::Section {
                id: SharedString::from(format!("core-data-ex-{group_index}-{exchange_index}")),
                caption: caption.clone(),
                count: *member_count,
                highlighted: *identified,
            }),
            ConnEntry::CoreRow { uid, .. } => {
                if let Some(core) = cores.remove(uid) {
                    if let Some(Line::Group { uids, .. }) = group.and_then(|g| out.get_mut(g)) {
                        uids.push(*uid);
                    }
                    out.push(Line::Core(core));
                }
            }
            // Only an unsaved row is pending, and this list reads the saved configuration.
            ConnEntry::PendingHeader { .. } => {}
        }
    }
    let mut archive: Vec<CoreLine> = cores.into_values().collect();
    if !archive.is_empty() {
        archive.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.counts.uid.cmp(&b.counts.uid))
        });
        out.push(Line::Section {
            id: SharedString::from("core-data-archive"),
            caption: archive_caption.to_string(),
            count: archive.len(),
            highlighted: false,
        });
        out.extend(archive.into_iter().map(Line::Core));
    }
    out
}

/// A count as a cell: blank for zero, so the stores a core has nothing in stand out.
fn count_text(n: u64) -> String {
    if n == 0 { String::new() } else { n.to_string() }
}

/// The count cells of one row, or the column captions: right-aligned, fixed widths.
fn count_cells(texts: [String; 5], color: u32, cx: &App) -> Div {
    let mut row = h_flex().flex_shrink_0().items_center();
    for (text, w) in texts.into_iter().zip(COUNT_COLS) {
        row = row.child(
            div()
                .w(design::ui_px(cx, w))
                .flex()
                .justify_end()
                .text_color(rgb(color))
                .child(text),
        );
    }
    row
}

/// The column captions above the list, aligned with the rows below.
///
/// Args:
///     select_all: Header checkbox that selects every selectable core. Its label is the short
///         "All" string, so the caption row stays one line.
///     p: Active palette.
///     cx: Application context for scale.
///
/// Returns:
///     The caption row.
fn head_row(select_all: impl IntoElement, p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .w_full()
        .items_center()
        .pl(px(CONN_TABLE_INSET))
        .pr(design::ui_px(cx, design::MOON_SCROLLBAR_OVERLAY_W))
        .text_size(design::t_caption(cx))
        .text_color(rgb(p.text_muted))
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap(design::ui_px(cx, 6.0))
                .items_center()
                .child(select_all)
                .child(t!("storage.core_data_col_core").to_string()),
        )
        .child(count_cells(
            [
                t!("storage.core_data_col_reports").to_string(),
                t!("storage.core_data_col_strategies").to_string(),
                t!("storage.core_data_col_traces").to_string(),
                t!("storage.core_data_col_warnings").to_string(),
                t!("storage.core_data_col_sync").to_string(),
            ],
            p.text_muted,
            cx,
        ))
}

/// A window-group branch: the checkbox selects every selectable core in it.
fn group_row(
    weak: &WeakEntity<SettingsView>,
    name: &str,
    icon: Option<std::sync::Arc<RenderImage>>,
    member_count: usize,
    selectable: Vec<u64>,
    all_selected: bool,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let weak = weak.clone();
    let disabled = selectable.is_empty();
    group_pill(p, cx)
        .child(
            MoonCheckbox::new(SharedString::from(format!("core-data-grp-{name}")))
                .checked(all_selected)
                .disabled(disabled)
                .on_change(move |on: &bool, _, cx| {
                    let on = *on;
                    let uids = selectable.clone();
                    let _ = weak.update(cx, |this, cx| this.core_data_set(&uids, on, cx));
                }),
        )
        .child(group_icon(icon, cx))
        .child(group_name(name))
        .child(group_count(member_count, p, cx))
        .into_any_element()
}

/// One core: its checkbox, name and uid, and what each store holds for it.
fn core_row(
    weak: &WeakEntity<SettingsView>,
    core: &CoreLine,
    selected: bool,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let c = &core.counts;
    let uid = c.uid;
    let weak = weak.clone();
    let strategies = if c.strategies == 0 && c.versions == 0 {
        String::new()
    } else {
        format!("{} / {}", c.strategies, c.versions)
    };
    let row = h_flex()
        .w_full()
        .h_full()
        .gap_2()
        .items_center()
        .pr(design::ui_px(cx, design::MOON_SCROLLBAR_OVERLAY_W))
        .child(
            MoonCheckbox::new(SharedString::from(format!("core-data-sel-{uid}")))
                .checked(selected)
                .disabled(!core.selectable())
                .on_change(move |on: &bool, _, cx| {
                    let on = *on;
                    let _ = weak.update(cx, |this, cx| this.core_data_set(&[uid], on, cx));
                }),
        )
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_2()
                .child(div().min_w_0().truncate().child(core.name.clone()))
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(rgb(p.text_muted))
                        .child(format!("#{uid}")),
                ),
        )
        .child(count_cells(
            [
                count_text(c.reports),
                strategies,
                count_text(c.traces),
                count_text(c.warnings),
                core.syncing.clone().unwrap_or_default(),
            ],
            if core.syncing.is_some() {
                p.amber
            } else {
                p.text
            },
            cx,
        ));
    // The Connections tab's branch guide, so the two lists read as one.
    div()
        .h_full()
        .ml(px(CONN_INDENT_MARGIN))
        .pl(px(CONN_INDENT_PAD))
        .border_l_1()
        .border_color(rgb(p.border))
        .child(row)
        .into_any_element()
}

/// One core a delete may touch. `group` decides whether its reports come back.
struct DeleteTarget {
    uid: u64,
    group: Group,
}

/// Selectable cores whose uid is in `selected`, in list order.
///
/// A core whose catch-up is running is left out even when its uid is in the set: selecting it
/// would let a page from the old download land under the new one.
///
/// Args:
///     lines: The table, including group and section rows.
///     selected: Uids the checkboxes have on.
///
/// Returns:
///     The cores the delete will name.
fn deletion_targets(lines: &[Line], selected: &BTreeSet<u64>) -> Vec<DeleteTarget> {
    lines
        .iter()
        .filter_map(|line| match line {
            Line::Core(core) if core.selectable() && selected.contains(&core.counts.uid) => {
                Some(DeleteTarget {
                    uid: core.counts.uid,
                    group: core.group,
                })
            }
            _ => None,
        })
        .collect()
}

/// One per-core store the table can delete from, in column order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Reports,
    Strategies,
    Traces,
    Warnings,
}

/// Kinds that are on, in column order: reports, strategies, traces, warnings.
///
/// Args:
///     kinds: Which stores the request names.
///
/// Returns:
///     The on kinds. Empty when every box is off.
fn active_kinds(kinds: Kinds) -> Vec<Kind> {
    [
        (kinds.reports, Kind::Reports),
        (kinds.strategies, Kind::Strategies),
        (kinds.traces, Kind::Traces),
        (kinds.warnings, Kind::Warnings),
    ]
    .into_iter()
    .filter_map(|(on, kind)| on.then_some(kind))
    .collect()
}

/// The delete request: each selected selectable core crossed with each kind that is on.
///
/// Args:
///     lines: The table the checkboxes were drawn from.
///     selected: Uids the user ticked, including ones a catch-up has since made untouchable.
///     kinds: Which stores the request names.
///
/// Returns:
///     `(uid, kind)` in list order, then kind order. A syncing core contributes nothing.
fn deletion_pairs(lines: &[Line], selected: &BTreeSet<u64>, kinds: Kinds) -> Vec<(u64, Kind)> {
    let kinds = active_kinds(kinds);
    deletion_targets(lines, selected)
        .into_iter()
        .flat_map(|target| kinds.iter().copied().map(move |kind| (target.uid, kind)))
        .collect()
}

/// "N cores selected" in the active language's plural form.
///
/// Args:
///     n: How many selectable cores are in the selection.
///
/// Returns:
///     The localized count line.
fn selected_label(n: usize) -> String {
    t!(&plural_key("storage.core_data.selected_n", n as u64), n = n).to_string()
}

impl SettingsView {
    /// Count the per-core stores in the background; a count already running is not repeated.
    pub(super) fn core_data_refresh(&mut self, cx: &mut Context<Self>) {
        if self.storage.core_data.inflight {
            return;
        }
        self.storage.core_data.inflight = true;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let counts = executor.spawn(async move { collect() }).await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    let ed = &mut this.storage.core_data;
                    ed.inflight = false;
                    // A core that left the stores leaves the selection too.
                    if let Ok(rows) = &counts {
                        ed.selected.retain(|uid| rows.iter().any(|r| r.uid == *uid));
                    }
                    ed.counts = Some(counts);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// The list's lines: stored cores plus configured ones that store nothing yet, laid out on
    /// the Connections tree, with the live facts only the backend knows — group, running
    /// catch-up.
    fn core_data_lines(&self, cx: &App) -> Vec<Line> {
        let Some(Ok(counts)) = &self.storage.core_data.counts else {
            return Vec::new();
        };
        let b = self.backend.read(cx);
        let store = b.session.store();
        let venues = b.session.core_venues();
        let config = &b.config;
        let mut by_uid: HashMap<u64, CoreCounts> =
            counts.iter().map(|c| (c.uid, c.clone())).collect();
        for server in &config.servers {
            by_uid.entry(server.uid).or_insert_with(|| CoreCounts {
                uid: server.uid,
                ..CoreCounts::default()
            });
        }
        let cores: HashMap<u64, CoreLine> = by_uid
            .into_values()
            .map(|counts| {
                let server = config.servers.iter().find(|s| s.uid == counts.uid);
                let core = store.core(counts.uid);
                let ready = core.is_some_and(|d| d.status == moon_core::feed::ConnStatus::Ready);
                let group = match (server, ready) {
                    (Some(_), true) => Group::Connected,
                    (Some(_), false) => Group::Offline,
                    (None, _) => Group::Archive,
                };
                let name = server
                    .map(|s| s.name.clone())
                    .filter(|n| !n.is_empty())
                    .or_else(|| {
                        (!counts.stored_name.is_empty()).then(|| counts.stored_name.clone())
                    })
                    .unwrap_or_else(|| {
                        t!("storage.core_data_unnamed", uid = counts.uid).to_string()
                    });
                let syncing = if ready {
                    core.and_then(|d| d.report_sync).map(|p| match p.percent() {
                        Some(pct) => format!("{pct}%"),
                        None => "…".to_string(),
                    })
                } else {
                    None
                };
                (
                    counts.uid,
                    CoreLine {
                        counts,
                        name,
                        group,
                        syncing,
                    },
                )
            })
            .collect();
        // The Connections tab's own tree, built from the saved configuration.
        let servers: Vec<ServerRowMeta> = config
            .servers
            .iter()
            .map(|s| {
                (
                    s.id,
                    s.uid,
                    s.active,
                    s.group.clone(),
                    venues.get(&s.id).cloned(),
                )
            })
            .collect();
        let groups = sorted_group_rows(&servers, &config.groups);
        let order = moon_core::session::core_order::CoreOrder::new(config);
        let labels = EntryLabels {
            pending: "",
            exchange: &|venue| crate::controls::venue_section_label(venue),
        };
        let entries = flatten_entries(&servers, &groups, &order, labels);
        lines(
            &entries,
            cores,
            t!("storage.core_data_group_archive").as_ref(),
        )
    }

    /// Put cores in the selection, or take them out.
    fn core_data_set(&mut self, uids: &[u64], on: bool, cx: &mut Context<Self>) {
        let sel = &mut self.storage.core_data.selected;
        for uid in uids {
            if on {
                sel.insert(*uid);
            } else {
                sel.remove(uid);
            }
        }
        cx.notify();
    }

    /// The core table, always on the page: selection, kind checkboxes and a confirmed delete.
    /// Gathers the table's state; the layout is [`core_section_el`]'s.
    ///
    /// Args:
    ///     cx: Settings view context.
    ///
    /// Returns:
    ///     The section. A missing count shows the loading line and starts a refresh.
    pub(super) fn core_data_section(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.storage.core_data.counts.is_none() {
            self.core_data_refresh(cx);
        }
        let lines = match &self.storage.core_data.counts {
            None => None,
            Some(Err(e)) => Some(Err(e.clone())),
            Some(Ok(_)) => Some(Ok(Rc::new(self.core_data_lines(cx)))),
        };
        // Group icons, loaded here: `texture` needs `&mut self`.
        let mut icons: HashMap<u32, Option<std::sync::Arc<RenderImage>>> = HashMap::new();
        if let Some(Ok(lines)) = &lines {
            for line in lines.iter() {
                if let Line::Group { icon, .. } = line {
                    icons
                        .entry(*icon)
                        .or_insert_with(|| self.icons.texture(*icon));
                }
            }
        }
        let view = CoreView {
            lines,
            selected: self.storage.core_data.selected.clone(),
            kinds: self.storage.core_data.kinds,
            busy: self.storage.busy,
            icons,
        };
        let open = self.more_open.clone();
        let ctx = super::StorageCtx {
            weak: cx.entity().downgrade(),
            open: &open,
        };
        core_section_el(&view, &ctx, cx)
    }
}

/// What the core table draws, gathered by [`SettingsView::core_data_section`].
pub(super) struct CoreView {
    /// The table's lines; `None` while counting, `Err` when the count failed.
    pub(super) lines: Option<Result<Rc<Vec<Line>>, String>>,
    pub(super) selected: BTreeSet<u64>,
    pub(super) kinds: Kinds,
    /// A maintenance operation runs: Delete is disabled.
    pub(super) busy: bool,
    /// Group icon textures by icon id.
    pub(super) icons: HashMap<u32, Option<std::sync::Arc<RenderImage>>>,
}

/// The core table: its title and help, the header with select-all, one line per group, section
/// and core, and the footer with the kind boxes and Delete.
pub(super) fn core_section_el(core: &CoreView, s: &super::StorageCtx, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    let muted = rgba_from(p.text_muted, 1.0);
    let open = s.open.contains("storage-core-hint");
    let view = s.weak.clone();
    let toggle: Action = Rc::new(move |_, app| {
        let _ = view.update(app, |this, cx| {
            if !this.more_open.remove("storage-core-hint") {
                this.more_open.insert("storage-core-hint");
            }
            cx.notify();
        });
    });
    let title = hint_row(
        "storage-core-hint",
        t!("storage.core_data.title").to_string(),
        None,
        Some(t!("storage.core_data_hint").to_string()),
        div().into_any_element(),
        open,
        toggle,
        cx,
    );
    let body = v_flex()
        .id("storage-core")
        .debug_selector(|| "storage-core".into())
        .w_full()
        .gap(design::ui_px(cx, 6.0))
        .child(title);
    let lines = match &core.lines {
        None => {
            return body
                .child(
                    div()
                        .text_color(muted)
                        .child(t!("storage.core_data_loading").to_string()),
                )
                .into_any_element();
        }
        Some(Err(e)) => {
            return body
                .child(
                    div()
                        .text_color(rgb(p.red))
                        .child(t!("storage.core_data_failed", err = e.clone()).to_string()),
                )
                .into_any_element();
        }
        Some(Ok(lines)) => lines.clone(),
    };
    let selectable: Vec<u64> = lines
        .iter()
        .filter_map(|l| match l {
            Line::Core(c) if c.selectable() => Some(c.counts.uid),
            _ => None,
        })
        .collect();
    let selected = core.selected.clone();
    let chosen = deletion_targets(&lines, &selected);
    let all_selected = !selectable.is_empty() && selectable.iter().all(|u| selected.contains(u));
    let kinds = core.kinds;
    let busy = core.busy;
    let can_delete = !busy && kinds.any() && !chosen.is_empty();

    let kind_box = |id: &'static str, label: String, on: bool, set: fn(&mut Kinds, bool)| {
        MoonCheckbox::new(id).checked(on).label(label).on_change({
            let weak = s.weak.clone();
            move |v: &bool, _, app| {
                let _ = weak.update(app, |this, cx| {
                    set(&mut this.storage.core_data.kinds, *v);
                    cx.notify();
                });
            }
        })
    };
    let all_uids = selectable.clone();
    let select_all = MoonCheckbox::new("core-data-all")
        .checked(all_selected)
        .disabled(selectable.is_empty())
        .label(t!("storage.core_data_select_all").to_string())
        .on_change({
            let weak = s.weak.clone();
            move |v: &bool, _, app| {
                let _ = weak.update(app, |this, cx| {
                    this.storage.core_data.selected.clear();
                    this.core_data_set(&all_uids, *v, cx);
                });
            }
        });
    let footer = h_flex()
        .w_full()
        .gap(design::ui_px(cx, 12.0))
        .items_center()
        .flex_wrap()
        .child(div().text_color(muted).child(selected_label(chosen.len())))
        .child(
            div()
                .text_color(muted)
                .child(t!("storage.core_data_what").to_string()),
        )
        .child(kind_box(
            "core-data-kind-reports",
            t!("storage.core_data_col_reports").to_string(),
            kinds.reports,
            |k, v| k.reports = v,
        ))
        .child(kind_box(
            "core-data-kind-strategies",
            t!("storage.core_data_col_strategies").to_string(),
            kinds.strategies,
            |k, v| k.strategies = v,
        ))
        .child(kind_box(
            "core-data-kind-traces",
            t!("storage.core_data_col_traces").to_string(),
            kinds.traces,
            |k, v| k.traces = v,
        ))
        .child(kind_box(
            "core-data-kind-warnings",
            t!("storage.core_data_col_warnings").to_string(),
            kinds.warnings,
            |k, v| k.warnings = v,
        ))
        .child(
            MoonButton::new("core-data-delete")
                .variant(MoonButtonVariant::Danger)
                .mono(false)
                .label(format!("  {}  ", t!("storage.core_data.delete")))
                .disabled(!can_delete)
                .on_click(s.click(|this, window, cx| {
                    this.core_data_confirm(window, cx);
                }))
                .render(),
        );
    let header = head_row(select_all, p, cx);

    // Every line at full height, scrolled by the page: GPUI hands a wheel event to EVERY
    // scrolling hitbox under the pointer (`should_handle_scroll` is a hit test, not "topmost"),
    // so a list with a scroll of its own inside this scrolling page would move both at once.
    // Connections avoids that with a bounded, non-scrolling page (`settings/render.rs`); this
    // section shares its page with the rest of the Storage tab, so it does not scroll by itself.
    let row_h = conn_row_h_value(cx);
    let weak = s.weak.clone();
    let app: &App = cx;
    let icon_tex = &core.icons;
    let rows: Vec<AnyElement> = lines
        .iter()
        .map(|line| {
            let p = MoonPalette::active(app);
            let el = match line {
                Line::Group { name, icon, uids } => {
                    let group_selectable: Vec<u64> = uids
                        .iter()
                        .copied()
                        .filter(|u| selectable.contains(u))
                        .collect();
                    let all = !group_selectable.is_empty()
                        && group_selectable.iter().all(|u| selected.contains(u));
                    group_row(
                        &weak,
                        name,
                        icon_tex.get(icon).cloned().flatten(),
                        uids.len(),
                        group_selectable,
                        all,
                        p,
                        app,
                    )
                }
                Line::Section {
                    id,
                    caption,
                    count,
                    highlighted,
                } => {
                    subsection_header_row(id.clone(), caption.clone(), *count, *highlighted, p, app)
                        .into_any_element()
                }
                Line::Core(core) => {
                    core_row(&weak, core, selected.contains(&core.counts.uid), p, app)
                }
            };
            // Clipped like a virtual-list item: a narrow window cuts the count columns at the
            // row's edge rather than letting them spill past it.
            div()
                .relative()
                .overflow_hidden()
                .w_full()
                .h(px(row_h))
                .child(el)
                .into_any_element()
        })
        .collect();

    body.child(header)
        .child(if lines.is_empty() {
            div()
                .text_color(muted)
                .child(t!("storage.core_data_empty").to_string())
                .into_any_element()
        } else {
            v_flex().w_full().children(rows).into_any_element()
        })
        .child(footer)
        .into_any_element()
}
