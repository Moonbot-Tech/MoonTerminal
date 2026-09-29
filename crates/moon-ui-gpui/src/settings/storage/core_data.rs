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
    MoonButton, MoonButtonVariant, MoonCheckbox, MoonDisclosure, MoonDisclosureDirection,
    MoonPalette, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::super::connections::{
    CONN_INDENT_MARGIN, CONN_INDENT_PAD, CONN_TABLE_INSET, ConnEntry, EntryLabels, ServerRowMeta,
    conn_row_h_value, flatten_entries, group_count, group_icon, group_name, group_pill,
    sorted_group_rows, subsection_header_row,
};
use super::super::{SettingsView, section};
use crate::design;
use moon_core::config::paths;

mod delete;

#[cfg(test)]
mod tests;

/// How long a delete waits for each writer to confirm before reporting it as not confirmed.
const WRITER_WAIT: Duration = Duration::from_secs(30);

const DIALOG_ID: &str = "storage-core-data-delete";

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
pub(in crate::settings) struct CoreDataEd {
    /// Collapsed by default: the list is a maintenance tool, not something read every visit.
    pub(super) expanded: bool,
    /// The counts, or why they could not be read; `None` until the first count lands.
    pub(super) counts: Option<Result<Vec<CoreCounts>, String>>,
    pub(super) inflight: bool,
    pub(super) selected: BTreeSet<u64>,
    pub(super) kinds: Kinds,
}

impl Default for CoreDataEd {
    fn default() -> Self {
        Self {
            expanded: false,
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
enum Group {
    /// Configured and ready: its reports download again after a delete.
    Connected,
    /// Configured, not ready now: its reports download on its next connection.
    Offline,
    /// Not in Connections: whatever is deleted is gone.
    Archive,
}

/// One core as the list shows it.
#[derive(Clone, Debug)]
struct CoreLine {
    counts: CoreCounts,
    name: String,
    group: Group,
    /// The running report catch-up's percent, `Some` while one runs on a ready core.
    syncing: Option<String>,
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
enum Line {
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
fn lines(
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
fn head_row(p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .w_full()
        .items_center()
        .pl(px(CONN_TABLE_INSET))
        .pr(design::ui_px(cx, design::MOON_SCROLLBAR_OVERLAY_W))
        .text_size(design::t_caption(cx))
        .text_color(rgb(p.text_muted))
        .child(
            div()
                .flex_1()
                .min_w_0()
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

    /// Render the section: a caret-headed title, and while expanded the toolbar, list and hint.
    pub(super) fn core_data_section(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = MoonPalette::active(cx);
        let muted = rgba_from(p.text_muted, 1.0);
        let expanded = self.storage.core_data.expanded;
        if expanded && self.storage.core_data.counts.is_none() {
            self.core_data_refresh(cx);
        }
        let caret = MoonDisclosure::button("core-data-caret", expanded)
            .direction(MoonDisclosureDirection::DownUp)
            .size(design::DISCLOSURE_GLYPH)
            .box_size(design::DISCLOSURE_BOX)
            .hover_color(p.text)
            .on_toggle(cx.listener(|this, open: &bool, _, cx| {
                this.storage.core_data.expanded = *open;
                if *open {
                    this.core_data_refresh(cx);
                }
                cx.notify();
            }));
        let header = h_flex()
            .id("core-data-header")
            .gap(design::ui_px(cx, 6.0))
            .items_center()
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                let open = !this.storage.core_data.expanded;
                this.storage.core_data.expanded = open;
                if open {
                    this.core_data_refresh(cx);
                }
                cx.notify();
            }))
            .child(caret)
            .child(section(&t!("storage.core_data_title"), p, cx));
        let mut body = v_flex().w_full().gap_1().child(header);
        if !expanded {
            return body
                .child(
                    div()
                        .text_color(muted)
                        .child(t!("storage.core_data_collapsed_hint").to_string()),
                )
                .into_any_element();
        }
        let lines = match &self.storage.core_data.counts {
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
            Some(Ok(_)) => Rc::new(self.core_data_lines(cx)),
        };
        let selectable: Vec<u64> = lines
            .iter()
            .filter_map(|l| match l {
                Line::Core(c) if c.selectable() => Some(c.counts.uid),
                _ => None,
            })
            .collect();
        let selected = self.storage.core_data.selected.clone();
        let all_selected =
            !selectable.is_empty() && selectable.iter().all(|u| selected.contains(u));
        let kinds = self.storage.core_data.kinds;
        let busy = self.storage.busy;
        let can_delete = !busy && kinds.any() && !selected.is_empty();

        let kind_box = |id: &'static str, label: String, on: bool, set: fn(&mut Kinds, bool)| {
            MoonCheckbox::new(id)
                .checked(on)
                .label(label)
                .on_change(cx.listener(move |this, v: &bool, _, cx| {
                    set(&mut this.storage.core_data.kinds, *v);
                    cx.notify();
                }))
        };
        let all_uids = selectable.clone();
        let toolbar = h_flex()
            .w_full()
            .gap(design::ui_px(cx, 12.0))
            .items_center()
            .flex_wrap()
            .child(
                MoonCheckbox::new("core-data-all")
                    .checked(all_selected)
                    .disabled(selectable.is_empty())
                    .label(t!("storage.core_data_select_all").to_string())
                    .on_change(cx.listener(move |this, v: &bool, _, cx| {
                        this.storage.core_data.selected.clear();
                        this.core_data_set(&all_uids, *v, cx);
                    })),
            )
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
                    .label(format!("  {}  ", t!("storage.core_data_delete")))
                    .disabled(!can_delete)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.core_data_confirm(window, cx);
                    }))
                    .render(),
            );

        // Group icons, loaded first: `texture` needs `&mut self`, and the rows below are built
        // while `cx` is borrowed as `&App`.
        let mut icon_tex: HashMap<u32, Option<std::sync::Arc<RenderImage>>> = HashMap::new();
        for line in lines.iter() {
            if let Line::Group { icon, .. } = line {
                icon_tex
                    .entry(*icon)
                    .or_insert_with(|| self.icons.texture(*icon));
            }
        }
        // Every line at full height, scrolled by the page: GPUI hands a wheel event to EVERY
        // scrolling hitbox under the pointer (`should_handle_scroll` is a hit test, not "topmost"),
        // so a list with a scroll of its own inside this scrolling page would move both at once.
        // Connections avoids that with a bounded, non-scrolling page (`settings/render.rs`); this
        // section shares its page with the rest of the Storage tab, so it does not scroll by itself.
        let row_h = conn_row_h_value(cx);
        let weak = cx.entity().downgrade();
        let app: &App = cx;
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
                    } => subsection_header_row(
                        id.clone(),
                        caption.clone(),
                        *count,
                        *highlighted,
                        p,
                        app,
                    )
                    .into_any_element(),
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

        body = body
            .child(toolbar)
            .child(head_row(p, cx))
            .child(if lines.is_empty() {
                div()
                    .text_color(muted)
                    .child(t!("storage.core_data_empty").to_string())
                    .into_any_element()
            } else {
                v_flex().w_full().children(rows).into_any_element()
            })
            .child(
                div()
                    .text_color(muted)
                    .child(t!("storage.core_data_hint").to_string()),
            );
        body.into_any_element()
    }
}
