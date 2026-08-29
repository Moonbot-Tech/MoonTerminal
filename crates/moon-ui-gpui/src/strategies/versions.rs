//! Versions pane of the Strategies window, between the tree and sections: history for the selected
//! strategy from strategies.sqlite. A row states its identity by clock stamp, its position
//! relative to the live strategy by a slot badge derived from `valid_to` (never from row index),
//! and always carries a restore affordance that stages the whole snapshot into the live strategy
//! through `stage_version_into_current` — the one path also reachable from the params pane and the
//! right-click menu. Profit comes from the lazy version_stats cache (`strat_db::stats`) computed
//! on the background executor. The Live row represents live editable mode; selecting any persisted
//! snapshot row, including the current-version snapshot, supplies its fields to the read-only
//! sections and parameters panes.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBadge, MoonBadgeSize, MoonBadgeVariant, MoonButton, MoonButtonIconSlot, MoonButtonSize,
    MoonContextMenuWindowExt as _, MoonMenuItem, MoonPalette, MoonTone, MoonWindowExt as _, h_flex,
    v_flex,
};
use rust_i18n::t;

use super::version_facts::{self, VersionSlot};
use super::{Key, StrategiesView, logic};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::feed::{SchemaFieldUi, StrategyRow};
use moon_core::strat_db::stats::VersionInfo;
use moon_core::util::display_time;

/// Horizontal padding on ONE side of the versions pane's own container (design units, matching the
/// `.px(design::ui_px(cx, VERSIONS_PANE_PADDING))` call below). The two width budgets below must
/// subtract this same value, scaled, on BOTH sides so they never drift from the pane's real content
/// area again.
const VERSIONS_PANE_PADDING: f32 = 8.0;

/// Horizontal padding on ONE side of a version row's own container (design units, matching the
/// `.px(design::ui_px(cx, VERSION_ROW_PADDING))` call on the row below). `budget_chars` must
/// subtract this same value, scaled, on BOTH sides too — plus the row's own `border_1()`, which is
/// a raw, unscaled 1px per side (`border_style_methods!` in moon-gpui hard-codes `px(1.)`, it is
/// not a design unit) — or the budget over-states the row's real content box and the head picks a
/// wider layout than the row can actually hold.
const VERSION_ROW_PADDING: f32 = 6.0;

/// State of the versions pane.
#[derive(Default)]
pub(super) struct VersionsState {
    pub list: Vec<VersionInfo>,
    /// Strategy for which `list` was loaded.
    pub key: Option<Key>,
    /// strat_db generation at load time; a new record triggers a reload.
    pub db_gen: u64,
    pub inflight: bool,
    /// `valid_from` of the selected persisted snapshot; None selects live editable mode.
    pub sel: Option<i64>,
    /// Synthetic persisted-snapshot row whose fields come from raw_json for the panes on the right.
    pub row: Option<(Key, i64, StrategyRow)>,
    /// Fields changed in the selected version, mapped from lowercase name to the name as dumped and
    /// the previous display value; an empty value means the field did not exist. Empty means no diff.
    pub changed: std::collections::HashMap<String, (String, String)>,
    /// Section in snapshot view: with a nonempty diff, None selects the synthetic "All" section of
    /// changed fields across every section, while Some(i) filters one schema section.
    pub section: Option<usize>,
    /// Select the LATEST version once the list finishes loading.
    ///
    /// This follows a click on a deleted strategy, which has no live mode and opens directly on its
    /// final parameters.
    pub pending_latest: bool,
    /// Whether the pane is collapsed left into a narrow strip showing only the version count.
    pub collapsed: bool,
    /// Confirmation of the last "restore into current": which strategy it belongs to, what the
    /// restore actually did, and from which version, so the params pane can say what just happened
    /// after the bounce back to live mode.
    ///
    /// Keyed by strategy because this state outlives the selection that produced it: without the
    /// key, a note from strategy A would keep showing over strategy B once the selection moves on.
    pub staged_note: Option<(Key, StagedOutcome, i64)>,
}

/// What a "restore into current" actually did, so the pane never states a number that disagrees
/// with the Apply button beside it.
///
/// The count in `Staged` is what Apply will SEND — never the wider set of fields the restore
/// touched. A restore that clears three stale drafts and stages one field changes one field in the
/// core, and reporting three next to an "Apply 1" button is the same two-disagreeing-figures defect
/// this pane exists to remove.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StagedOutcome {
    /// `n` fields are staged and Apply will send exactly those.
    Staged(usize),
    /// Nothing is left to apply, but `n` stale drafts were discarded to get there — a real effect
    /// that must not be reported as "nothing to change".
    ClearedOnly(usize),
    /// The version already equalled live and no draft was touched. A real state, not an absence:
    /// without it the click reads as a no-op.
    Identical,
}

impl VersionsState {
    /// Drop everything tied to the CURRENT selection, so no fact about the strategy just left can
    /// survive onto whatever the pane describes next.
    ///
    /// Every site that changes which strategy this pane is about goes through here, and that is the
    /// whole point. The three of them — a strategy switch, a workspace-scope move, and the
    /// multi-selection guard — each used to clear their own overlapping subset by hand, so a field
    /// added to this struct could be forgotten in one of them. The failure mode is not a crash: it
    /// is a silent cross-strategy leak, which is exactly the bug class `staged_note` had to be
    /// keyed to close in the first place (plan amendment A3).
    ///
    /// Returns:
    ///     Nothing; all fields that can describe the previous selection are cleared.
    pub(super) fn clear_selection(&mut self) {
        self.sel = None;
        self.row = None;
        self.changed.clear();
        self.section = None;
        self.staged_note = None;
    }
}

impl StrategiesView {
    /// Return whether a persisted snapshot is being viewed, making the parameter panes read-only.
    pub(super) fn viewing_version(&self) -> bool {
        self.versions.sel.is_some()
    }

    /// Return the selected version's synthetic row when it belongs to the current selection.
    pub(super) fn version_override(&self) -> Option<(Key, &StrategyRow)> {
        let vf = self.versions.sel?;
        let (key, row_vf, row) = self.versions.row.as_ref()?;
        (Some(*key) == logic::selected_key(self) && *row_vf == vf).then_some((*key, row))
    }

    /// Return the changed-fields-only filter when viewing a persisted snapshot with a nonempty diff.
    pub(super) fn version_changed_filter(
        &self,
    ) -> Option<&std::collections::HashMap<String, (String, String)>> {
        if self.viewing_version() && !self.versions.changed.is_empty() {
            Some(&self.versions.changed)
        } else {
            None
        }
    }

    /// Fire-and-forget reload of the version list after the selection or DB generation changes.
    fn ensure_versions(&mut self, cx: &mut Context<Self>) {
        let key = logic::selected_key(self);
        let db_gen = moon_core::strat_db::generation();
        if self.versions.key == key && (self.versions.db_gen == db_gen || self.versions.inflight) {
            return;
        }
        if self.versions.key != key {
            // A different strategy invalidates the selected version and cached list.
            self.versions.list.clear();
            self.versions.clear_selection();
        }
        self.versions.key = key;
        self.versions.db_gen = db_gen;
        let Some((core, id)) = key else { return };
        self.versions.inflight = true;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let list = executor
                .spawn(
                    async move { moon_core::strat_db::stats::versions_with_stats(core, id as i64) },
                )
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.versions.inflight = false;
                    if this.versions.key == Some((core, id)) {
                        this.versions.list = list;
                        // A deleted strategy has no live mode, so open its latest known version immediately.
                        if this.versions.pending_latest {
                            this.versions.pending_latest = false;
                            if let Some(vf) = this.versions.list.first().map(|v| v.valid_from) {
                                this.select_version(Some(vf), cx);
                            }
                        }
                        cx.notify();
                    }
                });
            });
        })
        .detach();
    }

    /// Load deleted strategies for the tree's Deleted folder in the background when the strat_db
    /// generation changes; soft deletion is recorded on a FullSet snapshot.
    pub(super) fn ensure_deleted(&mut self, cx: &mut Context<Self>) {
        let db_gen = moon_core::strat_db::generation();
        if self.deleted_gen == db_gen || self.deleted_inflight {
            return;
        }
        self.deleted_gen = db_gen;
        self.deleted_inflight = true;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let heads = executor
                .spawn(async move { moon_core::strat_db::stats::deleted_heads() })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.deleted_inflight = false;
                    let mut map: std::collections::HashMap<
                        moon_core::session::CoreId,
                        Vec<moon_core::strat_db::stats::HeadRow>,
                    > = std::collections::HashMap::new();
                    for h in heads {
                        map.entry(h.core_uid).or_default().push(h);
                    }
                    if this.deleted != map {
                        this.deleted = map;
                        // The tree signature cannot see this map, so it reads this counter instead.
                        this.deleted_rev = this.deleted_rev.wrapping_add(1);
                        cx.notify();
                    }
                });
            });
        })
        .detach();
    }

    /// Stage a FULL persisted snapshot into the live strategy through "Restore to current".
    ///
    /// Every schema field receives its version value; fields absent from that version fall back to
    /// the schema default, resetting fields added later. Only values that genuinely differ from the
    /// current strategy are staged, and cosmetic fields on the ignore list remain untouched. The
    /// Apply button submits the changes and creates a new version.
    ///
    /// Args:
    ///     vf: Persisted version timestamp identifying the snapshot to stage.
    ///     cx: View context used to load, stage, and repaint the selected strategy.
    ///
    /// Returns:
    ///     Nothing; the asynchronous load exits without staging when the selection is no longer live.
    pub(super) fn stage_version_into_current(&mut self, vf: i64, cx: &mut Context<Self>) {
        let Some((core, id)) = logic::selected_key(self) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let payload = executor
                .spawn(async move {
                    let view = moon_core::strat_db::stats::version_view(core, id as i64, vf)?;
                    let ignore: std::collections::HashSet<String> =
                        moon_core::config::storage::load()
                            .strategies
                            .ignore_fields
                            .into_iter()
                            .collect();
                    Some((view.fields, ignore))
                })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    let Some((fields, ignore)) = payload else {
                        return;
                    };
                    if logic::selected_key(this) != Some((core, id)) {
                        return;
                    }
                    let vmap: std::collections::HashMap<String, String> =
                        fields.into_iter().collect();
                    // Diff against live values across the kind's ENTIRE schema.
                    let edits: Vec<(String, String)> = {
                        let b = this.backend.read(cx);
                        let store = b.session.store();
                        let Some(live) = logic::row(store, core, id) else {
                            return;
                        };
                        let Some(kind) = store
                            .core(core)
                            .and_then(|cd| cd.schema.as_ref())
                            .and_then(|sch| {
                                sch.kinds.iter().find(|k| k.ordinal == live.kind_ordinal)
                            })
                        else {
                            return;
                        };
                        let mut seen = std::collections::HashSet::new();
                        let mut out = Vec::new();
                        for sec in &kind.sections {
                            for f in &sec.fields {
                                if !seen.insert(f.name.to_lowercase()) || ignore.contains(&f.name) {
                                    continue;
                                }
                                let mut target = vmap
                                    .get(&f.name)
                                    .cloned()
                                    .or_else(|| f.default.clone())
                                    .unwrap_or_default();
                                // Match field_value semantics: an empty numeric field is `0`.
                                if target.is_empty()
                                    && f.type_name != "String"
                                    && matches!(f.ui, SchemaFieldUi::Edit)
                                {
                                    target = "0".to_string();
                                }
                                let cur = logic::field_value(live, f);
                                if !logic::values_equal(&cur, &target) {
                                    out.push((f.name.clone(), target));
                                }
                            }
                        }
                        out
                    };
                    // "Restore this version" means the WHOLE version: drop every stale draft for
                    // THIS strategy first, or a field the snapshot agrees with live on survives
                    // untouched in `field_edits` and still reaches the core on Apply. Only this
                    // strategy's entries are touched — drafts for other strategies must survive
                    // (`discard_field_edits` in actions.rs preserves drafts outside scope the same
                    // way).
                    let mut cleared_names: std::collections::HashSet<String> =
                        std::collections::HashSet::new();
                    this.field_edits.retain(|(c, i, name), _| {
                        let hit = *c == core && *i == id;
                        if hit {
                            cleared_names.insert(name.clone());
                        }
                        !hit
                    });
                    let cleared_count = cleared_names.len();
                    let diff_n = edits.len();
                    // Three distinct outcomes, and each gets its own wording. The count reported
                    // for a staged restore is `diff_n` — what Apply will SEND — never the wider
                    // set of fields the restore touched: a cleared draft ends up matching the
                    // version without re-entering `field_edits`, so counting it here would print a
                    // figure the Apply button beside it contradicts.
                    let outcome = if diff_n > 0 {
                        StagedOutcome::Staged(diff_n)
                    } else if cleared_count > 0 {
                        StagedOutcome::ClearedOnly(cleared_count)
                    } else {
                        StagedOutcome::Identical
                    };
                    if outcome == StagedOutcome::Identical {
                        // A real state, not silence: the version is identical to live and nothing
                        // was cleared, so the user must be told the click was not a no-op. No mode
                        // switch — there is nothing to go back to live FOR.
                        this.versions.staged_note = Some(((core, id), outcome, vf));
                        cx.notify();
                        return;
                    }
                    for (name, v) in edits {
                        this.field_edits.insert((core, id, name), v);
                    }
                    log::info!(
                        "version {vf} -> current: {cleared_count} stale drafts cleared, {diff_n} fields staged"
                    );
                    // Return to live view with yellow dirty markers and the Apply button, even when
                    // the diff itself was empty: clearing stale drafts is a real effect and must not
                    // be reported as "nothing to change".
                    // Order matters: select_version(None, ..) does not clear staged_note (its own
                    // clear is guarded on `vf.is_some()`), so this write survives it.
                    this.select_version(None, cx);
                    this.versions.staged_note = Some(((core, id), outcome, vf));
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Restore a deleted strategy under its OLD id through the context-menu Restore action.
    ///
    /// The head and latest-version fields load in the background before `RestoreStrategy` is sent
    /// to the core. The echoed snapshot revives the head: unchanged content reopens the latest
    /// version, while changed restored content may create a new version. Profit history rejoins
    /// through the unchanged id.
    pub(super) fn restore_deleted_strategy(
        &mut self,
        core: moon_core::session::CoreId,
        id: u64,
        cx: &mut Context<Self>,
    ) {
        let backend = self.backend.clone();
        cx.spawn(async move |_this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let payload = executor
                .spawn(async move {
                    let head = moon_core::strat_db::stats::head_row(core, id as i64)?;
                    let fields =
                        moon_core::strat_db::stats::latest_version_fields(core, id as i64)?;
                    Some((head, fields))
                })
                .await;
            let _ = cx.update(|cx| {
                let Some((head, fields)) = payload else {
                    log::warn!("восстановление {id}: нет head/версий в strat_db");
                    return;
                };
                let hidden = backend
                    .read(cx)
                    .singleton_workspace()
                    .is_some_and(|workspace| {
                        !backend
                            .read(cx)
                            .effective_workspace_scope(
                                &workspace.group,
                                crate::workspace::RetainedCoreScope::All,
                            )
                            .contains(core)
                    });
                if hidden {
                    return;
                }
                if let Err(e) = backend.read(cx).session.restore_strategy(
                    core,
                    id,
                    head.kind_ordinal,
                    head.folder_path,
                    fields,
                ) {
                    log::warn!("restore strategy {id} failed: {e}");
                }
            });
        })
        .detach();
    }

    /// Select a DELETED strategy from the tree's Deleted folder and automatically open its latest
    /// version because it has no live parameters.
    pub(super) fn select_deleted_strategy(&mut self, key: Key, cx: &mut Context<Self>) {
        self.focus_strategy(key);
        if self.versions.key == Some(key) && !self.versions.list.is_empty() {
            let vf = self.versions.list[0].valid_from;
            self.versions.pending_latest = false;
            self.select_version(Some(vf), cx);
        } else {
            self.versions.pending_latest = true;
        }
        self.persist_session(cx);
        cx.notify();
    }

    /// Select None for live editable mode or Some for a persisted snapshot loaded in the background.
    fn select_version(&mut self, vf: Option<i64>, cx: &mut Context<Self>) {
        if self.versions.sel == vf {
            return;
        }
        self.versions.sel = vf;
        self.versions.row = None;
        self.versions.changed.clear();
        self.versions.section = None; // A loaded nonempty diff interprets None as "All".
        // Guarded on `vf.is_some()` only: an unguarded clear would erase the note
        // `stage_version_into_current` is about to set through its own `select_version(None, ..)`.
        if vf.is_some() {
            self.versions.staged_note = None;
        }
        cx.notify();
        let (Some((core, id)), Some(vf)) = (logic::selected_key(self), vf) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let (view, head) = executor
                .spawn(async move {
                    (
                        moon_core::strat_db::stats::version_view(core, id as i64, vf),
                        moon_core::strat_db::stats::head_row(core, id as i64),
                    )
                })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    let Some(view) = view else { return };
                    if this.versions.sel != Some(vf)
                        || logic::selected_key(this) != Some((core, id))
                    {
                        return; // The selection has already moved elsewhere.
                    }
                    // Build a synthetic row from the live snapshot's kind/folder/check state plus
                    // the version fields. Take the name from the version because it may have been
                    // renamed. Deleted strategies are absent from the store, so use the DB head row.
                    let live = {
                        let b = this.backend.read(cx);
                        let store = b.session.store();
                        logic::row(store, core, id).cloned()
                    };
                    let base = live.or_else(|| {
                        head.map(|h| StrategyRow {
                            id,
                            name: h.name,
                            kind: h.kind,
                            kind_ordinal: h.kind_ordinal,
                            folder_path: h.folder_path,
                            checked: false,
                            is_short: h.is_short,
                            fields: Vec::new(),
                        })
                    });
                    if let Some(mut r) = base {
                        if let Some((_, n)) = view.fields.iter().find(|(k, _)| k == "StrategyName")
                        {
                            if !n.is_empty() {
                                r.name = n.clone();
                            }
                        }
                        r.fields = view.fields;
                        this.versions.changed = view
                            .changed
                            .into_iter()
                            .map(|(k, old)| (k.to_lowercase(), (k, old)))
                            .collect();
                        this.versions.row = Some(((core, id), vf, r));
                        cx.notify();
                    }
                });
            });
        })
        .detach();
    }

    /// Render the Versions pane between the tree and sections.
    ///
    /// It collapses left into a narrow strip showing an arrow and the selected strategy's version count.
    ///
    /// Args:
    ///     cx: View context used to resolve current state and build pane controls.
    ///
    /// Returns:
    ///     The collapsed strip or the full Versions-pane element for the current selection.
    pub(super) fn versions_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = MoonPalette::active(cx);
        let border = moon(p.border);
        // Taken ONCE: two rows built off different `now_ms` values in the same frame could land
        // on opposite sides of a minute boundary.
        let now_ms = moon_core::util::now_unix_ms_i64();
        let zone = self.display_zone;
        let effective = logic::selected_keys(self);
        let single = logic::selected_key(self).is_some() && effective.len() <= 1;
        if self.versions.collapsed {
            if single {
                self.ensure_versions(cx); // Keep the count on the collapsed strip current.
            }
            let count = single.then(|| self.versions.list.len()).filter(|n| *n > 0);
            return v_flex()
                .id("versions-collapsed")
                .w(design::ui_px(cx, 22.0))
                .flex_none()
                .h_full()
                .bg(moon(p.shell_high))
                .border_r_1()
                .border_color(border)
                .items_center()
                .pt(design::ui_px(cx, 12.0))
                .gap(design::ui_px(cx, 6.0))
                .cursor_pointer()
                .font_family(design::mono())
                .text_size(design::t_body(cx))
                .text_color(moon(p.text_muted))
                .hover(move |s| s.bg(moon_alpha(p.panel, 0.74)))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.versions.collapsed = false;
                    this.save_panels(cx);
                    cx.notify();
                }))
                .child(div().child("▸"))
                .when_some(count, |s, n| {
                    s.child(div().text_size(design::t_caption(cx)).child(n.to_string()))
                })
                .into_any_element();
        }
        let mut col = v_flex()
            .w(px(self.panels.versions_w))
            .flex_none()
            .h_full()
            .bg(moon(p.shell_high))
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .line_height(design::line_px(cx, 14.0))
            .border_r_1()
            .border_color(border)
            .px(design::ui_px(cx, VERSIONS_PANE_PADDING))
            .py(design::ui_px(cx, 12.0))
            .gap(design::ui_px(cx, 7.0))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("strat.versions").to_string()),
                    )
                    // Collapse the pane left, leaving a narrow strip with the count.
                    .child(
                        div()
                            .id("versions-collapse")
                            .px(design::ui_px(cx, 4.0))
                            .rounded(design::ui_px(cx, 3.0))
                            .cursor_pointer()
                            .text_color(moon(p.text_muted))
                            .hover(move |s| s.bg(moon_alpha(p.panel, 0.74)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.versions.collapsed = true;
                                this.save_panels(cx);
                                cx.notify();
                            }))
                            .child("◂"),
                    ),
            )
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .child(t!("strat.versions_order").to_string()),
            )
            .child(div().w_full().h(px(1.0)).bg(border));

        let hint = |s: String| div().mt_2().text_color(moon(p.text_muted)).child(s);
        if logic::selected_key(self).is_none() {
            return col
                .child(hint(t!("strat.no_selection").to_string()))
                .into_any_element();
        }
        // Versions are unavailable for multi-selection because the panes show merged live values.
        if effective.len() > 1 {
            // A note keyed to the strategy the primary selection left behind would otherwise
            // survive on top of the merged multi-selection view it does not belong to.
            self.versions.clear_selection();
            return col
                .child(hint(t!("strat.versions_multi").to_string()))
                .into_any_element();
        }
        self.ensure_versions(cx);
        if self.versions.list.is_empty() {
            let text = if self.versions.inflight {
                "…".to_string()
            } else {
                t!("strat.versions_empty").to_string()
            };
            return col.child(hint(text)).into_any_element();
        }

        // A deleted strategy exists in the DB but not the live store, so it has no live mode: omit
        // the Current row and show only its history.
        let live_exists = self
            .selected
            .map(|(c, id)| {
                let b = self.backend.read(cx);
                logic::row(b.session.store(), c, id).is_some()
            })
            .unwrap_or(false);
        let mut list = v_flex().w_full().gap_0();
        // Current is the default live mode with every field editable. A separate row below represents
        // the persisted current-version snapshot and opens its DIFF and profit like other snapshot rows.
        if live_exists {
            let live_on = self.versions.sel.is_none();
            let live_date = self
                .versions
                .list
                .first()
                .map(|v| display_time::format_chart_clock(v.valid_from, zone, false, now_ms))
                .unwrap_or_default();
            let mut live_row = h_flex()
                .id("ver-live")
                .w_full()
                .h(design::fit_h_px(cx, 24.0, 14.0, 5.0))
                .px(design::ui_px(cx, 6.0))
                .rounded(design::ui_px(cx, 3.0))
                .border_1()
                .border_color(moon_alpha(p.border, 0.0))
                .items_center()
                .gap_1()
                .overflow_hidden()
                .cursor_pointer()
                .tooltip(crate::panels::common::text_tooltip(
                    t!("strat.versions_live_tip").to_string(),
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(moon(p.text))
                        .child(t!("strat.versions_live").to_string()),
                )
                // Show the latest version's date, currently in the core, dimmed on the right.
                .child(
                    div()
                        .flex_none()
                        .text_color(moon(p.text_muted))
                        .child(live_date),
                )
                .on_click(cx.listener(|this, _, _, cx| this.select_version(None, cx)));
            if live_on {
                live_row = live_row
                    .bg(moon_alpha(p.amber, 0.16))
                    .border_color(moon_alpha(p.amber, 0.55));
            } else {
                live_row = live_row.hover(move |s| s.bg(moon_alpha(p.panel, 0.74)));
            }
            // The visual break between "live" and "the snapshots of live" (defect 3).
            list = list
                .child(live_row)
                .child(div().w_full().h(px(1.0)).bg(border))
                .child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_muted))
                        .child(t!("strat.versions_history").to_string()),
                );
        }
        // Monospace character budget for one row line inside the pane's own horizontal padding —
        // how `version_row_facts` decides the narrow-pane degrade without needing a GPUI context.
        let char_w = design::mono_body_text_width(cx, "0", 400.0).max(1.0);
        // Two sides of the pane's own `.px(...)` padding above, UI-scaled from the same design-unit
        // constant so this budget can never drift from the padding it is meant to exclude.
        let pane_padding_px = design::ui_value(cx, VERSIONS_PANE_PADDING * 2.0);
        // Two sides of the version row's OWN `.px(...)` inset, applied inside the pane padding above
        // (§ the head is rendered inside an individual row, not directly inside the pane).
        let row_padding_px = design::ui_value(cx, VERSION_ROW_PADDING * 2.0);
        // Two sides of the row's `border_1()` — a raw, unscaled 1px per side, not a design unit, so
        // it is added directly rather than through `design::ui_value`.
        let row_border_px = 2.0;
        let budget_chars =
            ((self.panels.versions_w - pane_padding_px - row_padding_px - row_border_px) / char_w)
                .floor()
                .max(0.0) as usize;
        let n_versions = self.versions.list.len();
        // Resolve a fact's colour role to an actual colour. `MoonTone::Positive`/`Negative`/
        // `Danger` are intercepted first because the light theme substitutes darker `*_text`
        // variants for legibility that MoonTone's own resolver does not know about; every other
        // tone falls through to it.
        let tone_color = |tone: MoonTone| match tone {
            MoonTone::Positive => design::positive_color(p),
            MoonTone::Negative | MoonTone::Danger => design::danger_color(p),
            other => other.color(p),
        };
        // Two saves that render the same stamp must be told apart, or the bare `HH:MM` text reads
        // as one save that silently vanished (owner decision R6). Keyed on the RENDERED stamp
        // string — what `format_chart_clock` actually produces, not a raw millisecond minute
        // bucket — because that is what the user compares, and it is also what makes a DST fold
        // (two different instants, same local wall clock) fall out correctly. One pass over the
        // WHOLE list rather than neighbours only: a third version sorting between two same-minute
        // rows must not hide the collision from either of them.
        let bare_stamps: Vec<String> = self
            .versions
            .list
            .iter()
            .map(|v| display_time::format_chart_clock(v.valid_from, zone, false, now_ms))
            .collect();
        let mut stamp_counts: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for s in &bare_stamps {
            *stamp_counts.entry(s.as_str()).or_insert(0) += 1;
        }
        let mut previous_taken = false;
        for i in 0..n_versions {
            let v = &self.versions.list[i];
            let slot = if v.valid_to.is_none() {
                VersionSlot::InEffect
            } else if !previous_taken {
                previous_taken = true;
                VersionSlot::Previous
            } else {
                VersionSlot::Older
            };
            let with_seconds = stamp_counts
                .get(bare_stamps[i].as_str())
                .copied()
                .unwrap_or(0)
                > 1;

            let facts = version_facts::version_row_facts(
                v,
                slot,
                zone,
                now_ms,
                with_seconds,
                budget_chars,
                bare_stamps[i].as_str(),
            );
            let tip = version_facts::row_tooltip(&facts);
            let on = self.versions.sel == Some(v.valid_from);
            let vf = v.valid_from;

            let fact_el = |f: version_facts::VersionFact| -> AnyElement {
                let inner: AnyElement = if f.badge {
                    MoonBadge::new(f.text)
                        .variant(MoonBadgeVariant::Soft)
                        .size(MoonBadgeSize::Status)
                        .tone(f.tone)
                        .render()
                        .into_any_element()
                } else {
                    div()
                        .text_color(moon(tone_color(f.tone)))
                        .child(f.text)
                        .into_any_element()
                };
                div().flex_none().child(inner).into_any_element()
            };

            let mut head_col = v_flex().flex_none().gap(px(1.0));
            for line in facts.head {
                let mut line_row = h_flex().flex_none().gap_2();
                for f in line {
                    line_row = line_row.child(fact_el(f));
                }
                head_col = head_col.child(line_row);
            }
            let mut tail_row = h_flex().flex_1().min_w_0().overflow_hidden().gap_2();
            for f in facts.tail {
                tail_row = tail_row.child(fact_el(f));
            }

            let mut row = v_flex()
                .id(SharedString::from(format!("ver-{vf}")))
                .w_full()
                // Baseline sized for the usual two-line row (§0a); `min_h` rather than `h` lets
                // the rare row whose head itself needed two lines grow to three without clipping.
                .min_h(design::fit_h_px(cx, 38.0, 14.0, 5.0))
                .px(design::ui_px(cx, VERSION_ROW_PADDING))
                .py(design::ui_px(cx, 2.0))
                .rounded(design::ui_px(cx, 3.0))
                .border_1()
                .border_color(moon_alpha(p.border, 0.0))
                .gap(px(1.0))
                .cursor_pointer()
                .tooltip(crate::panels::common::text_tooltip(tip))
                .child(head_col)
                .child(tail_row)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.select_version(Some(vf), cx);
                }))
                // Right-click to stage the FULL version in place of the current one.
                .when(live_exists, |r| {
                    r.on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            let pos = e.position;
                            let view = cx.entity();
                            let item = MoonMenuItem::with_key(
                                "version-restore-current",
                                t!("strat.version_restore").to_string(),
                            )
                            .on_click(move |_, window, app| {
                                window.close_context_menu(app);
                                view.update(app, |this, cx| {
                                    this.stage_version_into_current(vf, cx);
                                });
                            });
                            let _ = this;
                            window.open_moon_context_menu(
                                cx,
                                "strat-version-menu",
                                pos,
                                vec![item],
                                200.0,
                            );
                        }),
                    )
                });
            if on {
                row = row
                    .bg(moon_alpha(p.amber, 0.16))
                    .border_color(moon_alpha(p.amber, 0.55));
            } else {
                row = row.hover(move |s| s.bg(moon_alpha(p.panel, 0.74)));
            }
            list = list.child(row);
        }
        col = col.child(
            div()
                .id("strat-versions-scroll")
                .flex_1()
                .w_full()
                .overflow_y_scroll()
                .child(list),
        );
        // The pinned restore bar: `flex_none` so the scrolling list above can never squeeze it
        // away (defect 5, and it must be unlosable).
        if let (Some(vf), true) = (self.versions.sel, live_exists) {
            let compact =
                design::mono_body_text_width(cx, &t!("strat.version_restore").to_string(), 400.0)
                    + 40.0
                    > self.panels.versions_w - design::ui_value(cx, VERSIONS_PANE_PADDING * 2.0);
            col = col.child(
                div()
                    .flex_none()
                    .w_full()
                    .pt(design::ui_px(cx, 6.0))
                    .child(self.version_restore_button(vf, compact, cx)),
            );
        }
        col.into_any_element()
    }

    /// The always-visible restore affordance, shown while a persisted snapshot is selected and a
    /// live strategy exists. `compact` drops the label to icon-only when the caller has measured
    /// that the label will not fit, which is what keeps it usable in a 90 px pane.
    ///
    /// Args:
    ///     vf: Persisted version timestamp staged when the affordance is activated.
    ///     compact: Whether available pane width requires an icon-only label.
    ///     cx: View context used to wire the restore callback.
    ///
    /// Returns:
    ///     The restore button in its measured compact or labelled form.
    pub(super) fn version_restore_button(
        &self,
        vf: i64,
        compact: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let mut btn = MoonButton::new("strat-version-restore")
            .outline()
            .size(MoonButtonSize::Micro)
            .leading_icon(MoonButtonIconSlot::new("icons/undo-2.svg"))
            .tooltip(t!("strat.version_restore_tip").to_string())
            .on_click(cx.listener(move |this, _, _, cx| this.stage_version_into_current(vf, cx)));
        if !compact {
            btn = btn.label(t!("strat.version_restore").to_string());
        }
        btn.render().into_any_element()
    }
}
