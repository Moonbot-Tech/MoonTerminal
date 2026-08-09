//! Versions pane of the Strategies window, between the tree and sections: history for the selected
//! strategy from strategies.sqlite with `dd.mm (changed)(profit$)` statistics. Profit comes from
//! the lazy version_stats cache (`strat_db::stats`) computed on the background executor. The top
//! Current row represents live editable mode; selecting any persisted snapshot row, including the
//! current-version snapshot, supplies its fields to the read-only sections and parameters panes.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonContextMenuWindowExt as _, MoonMenuItem, MoonPalette, MoonWindowExt as _, h_flex, v_flex,
};
use rust_i18n::t;

use super::{Key, StrategiesView, logic};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::feed::{SchemaFieldUi, StrategyRow};
use moon_core::strat_db::stats::{VersionInfo, short_date};

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
            self.versions.sel = None;
            self.versions.row = None;
            self.versions.changed.clear();
            self.versions.section = None;
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
                    if edits.is_empty() {
                        return;
                    }
                    let n = edits.len();
                    for (name, v) in edits {
                        this.field_edits.insert((core, id, name), v);
                    }
                    log::info!("версия {vf} → в текущую: {n} полей застейджено");
                    // Return to live view with yellow dirty markers and the Apply button.
                    this.select_version(None, cx);
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
    pub(super) fn versions_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = MoonPalette::active(cx);
        let border = moon(p.border);
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
            .px(design::ui_px(cx, 8.0))
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
            .child(div().w_full().h(px(1.0)).bg(border));

        let hint = |s: String| div().mt_2().text_color(moon(p.text_muted)).child(s);
        if logic::selected_key(self).is_none() {
            return col
                .child(hint(t!("strat.no_selection").to_string()))
                .into_any_element();
        }
        // Versions are unavailable for multi-selection because the panes show merged live values.
        if effective.len() > 1 {
            self.versions.sel = None;
            self.versions.row = None;
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
                .cursor_pointer()
                .child(
                    div()
                        .flex_1()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(moon(p.text))
                        .child(t!("strat.versions_live").to_string()),
                )
                // Show the latest version's date, currently in the core, dimmed on the right.
                .child(
                    div().flex_none().text_color(moon(p.text_muted)).child(
                        self.versions
                            .list
                            .first()
                            .map(|v| short_date(v.valid_from, self.display_zone))
                            .unwrap_or_default(),
                    ),
                )
                .on_click(cx.listener(|this, _, _, cx| this.select_version(None, cx)));
            if live_on {
                live_row = live_row
                    .bg(moon_alpha(p.amber, 0.16))
                    .border_color(moon_alpha(p.amber, 0.55));
            } else {
                live_row = live_row.hover(move |s| s.bg(moon_alpha(p.panel, 0.74)));
            }
            list = list.child(live_row);
        }
        for (i, v) in self.versions.list.iter().enumerate() {
            let is_current = i == 0;
            let on = self.versions.sel == Some(v.valid_from);
            let profit_col = if v.profit > 0.0 {
                p.green
            } else if v.profit < 0.0 {
                p.orange
            } else {
                p.text_muted
            };
            let profit = format!(
                "({}{}$)",
                if v.profit > 0.0 { "+" } else { "" },
                moon_core::util::fmt::compact(v.profit, 2)
            );
            let vf = v.valid_from;
            let mut row = h_flex()
                .id(SharedString::from(format!("ver-{vf}")))
                .w_full()
                .h(design::fit_h_px(cx, 24.0, 14.0, 5.0))
                .px(design::ui_px(cx, 6.0))
                .rounded(design::ui_px(cx, 3.0))
                .border_1()
                .border_color(moon_alpha(p.border, 0.0))
                .items_center()
                .gap_1()
                .cursor_pointer()
                .child(
                    div()
                        .text_color(moon(p.text))
                        .child(short_date(v.valid_from, self.display_zone)),
                )
                .child(
                    div()
                        .text_color(moon(p.text_soft))
                        .child(format!("({})", v.n_changed)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(moon(profit_col))
                        .child(profit),
                )
                .when(is_current && live_exists, |r| {
                    r.child(
                        div()
                            .flex_none()
                            .text_size(design::t_caption(cx))
                            .text_color(moon(p.text_muted))
                            .child(t!("strat.versions_current").to_string()),
                    )
                })
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
        col.into_any_element()
    }
}
