//! `Backend` methods for the figure and alert layer: toggling the selected figure's Alert flag,
//! removing a figure while disarming its core alert, and re-upserting an alert after an edit.
//! This child module keeps the implementation separate from `main.rs` while retaining access to
//! `Backend`'s private fields.

use std::collections::HashMap;

use moon_core::alert_blob;
use moon_core::figures::{DrawStyle, Figure, FigureKey, FigureTool, ToolSetting, ToolSettings};
use moon_core::session::CoreId;

use crate::Backend;

/// Encodes a figure for a chart-alert upsert, using the figure ID as `obj_uid`.
///
/// `None` for a tool the core has no chart-object type for; such a figure is drawn locally and is
/// never armed (`Figure::can_alert`).
fn figure_blob(fig: &Figure) -> Option<Vec<u8>> {
    alert_blob::encode(
        &fig.kind,
        fig.color,
        fig.thickness,
        fig.line_kind,
        fig.created_ms as f64,
        fig.strategy_id,
        fig.id,
    )
}

impl Backend {
    /// The style the next figure of `tool` will be drawn in.
    ///
    /// A tool that has never been styled reads the shared default rather than an empty value, so a
    /// first figure looks like every other tool's first figure.
    pub(crate) fn fig_style(&self, tool: FigureTool) -> DrawStyle {
        self.fig_styles
            .get(tool.def().key)
            .copied()
            .unwrap_or_default()
    }

    /// The style of `tool`, for editing. Materialises the default on first use.
    pub(crate) fn fig_style_mut(&mut self, tool: FigureTool) -> &mut DrawStyle {
        self.fig_styles.entry(tool.def().key).or_default()
    }

    /// The switch defaults for `tool`, as a settings surface shows them.
    pub(crate) fn tool_settings(&self, tool: FigureTool) -> Vec<ToolSetting> {
        moon_core::figures::settings_of(tool, self.tool_switches(tool))
    }

    /// The raw stored overrides for `tool`, for snapshotting into a draft. Empty when the tool has
    /// never been touched, which is also what "everything at its own default" means.
    pub(crate) fn tool_switches(&self, tool: FigureTool) -> &ToolSettings {
        static NONE: std::sync::LazyLock<ToolSettings> =
            std::sync::LazyLock::new(ToolSettings::new);
        match self.fig_tool_settings.get(tool.def().key) {
            Some(map) => map,
            // Only the miss pays for the shared empty map; a hit never touches the lock.
            None => &NONE,
        }
    }

    /// Sets one switch default for `tool`, returning whether it changed.
    ///
    /// Stored SPARSELY: a value the tool would give anyway removes the entry instead of recording
    /// it. Otherwise a switch added to the tool later would be pinned by a map written before it
    /// existed, and "reset" would have no meaning. A key the tool does not offer is refused
    /// outright rather than stored, so nothing can accumulate that `settings_of` will never render
    /// and no map is allocated for a write that stores nothing.
    pub(crate) fn set_tool_setting(&mut self, tool: FigureTool, key: &str, on: bool) -> bool {
        let default = moon_core::figures::settings_of(tool, &ToolSettings::new());
        let Some(def_on) = default.iter().find(|s| s.key == key).map(|s| s.on) else {
            return false;
        };
        if def_on == on {
            let Some(map) = self.fig_tool_settings.get_mut(tool.def().key) else {
                return false;
            };
            let changed = map.remove(key).is_some();
            if map.is_empty() {
                self.fig_tool_settings.remove(tool.def().key);
            }
            return changed;
        }
        self.fig_tool_settings
            .entry(tool.def().key)
            .or_default()
            .insert(key.to_string(), on)
            != Some(on)
    }

    /// Returns the core's default Alerts-strategy ID from `ServerConfig`, or zero when the core is
    /// absent. The setting is persisted as per-server metadata in `cfg/settings.toml`.
    pub(crate) fn alert_def_strategy(&self, core: CoreId) -> u64 {
        self.config
            .servers
            .iter()
            .find(|s| s.id == core)
            .map(|s| s.default_alert_strategy)
            .unwrap_or(0)
    }

    /// Sets the core's default Alerts-strategy ID. A changed value marks the configuration for the
    /// coordination loop's routine save; an unknown core or unchanged value is ignored.
    pub(crate) fn set_alert_def_strategy(&mut self, core: CoreId, strategy_id: u64) {
        if let Some(s) = self.config.servers.iter_mut().find(|s| s.id == core) {
            if s.default_alert_strategy != strategy_id {
                s.default_alert_strategy = strategy_id;
                self.config_dirty = true;
            }
        }
    }

    /// Toggles the selected figure's Alert flag, upserting the chart alert when enabling it and
    /// deleting the core alert when disabling it. A newly armed figure with no strategy inherits
    /// the core's nonzero default strategy. Returns `false` if no selected figure exists in the
    /// store; command-send errors do not roll back the local edit.
    pub(crate) fn toggle_selected_figure_alert(&mut self) -> bool {
        let Some((core, market, id)) = self.fig_selected.clone() else {
            return false;
        };
        // Arming is refused for a figure the core cannot represent — a tool it has no type for, or
        // a figure shared across cores, which has no single core to arm it on. Disarming always
        // runs: it only deletes whatever alert is out there.
        let Some((armed, can_alert)) = self
            .figures
            .borrow()
            .get(core, &market, id)
            .map(|f| (f.alert, f.can_alert()))
        else {
            return false;
        };
        if !armed && !can_alert {
            return false;
        }
        let def_strategy = self.alert_def_strategy(core);
        let mut upsert_blob = None;
        let mut disarm = false;
        let changed = self.figures.borrow_mut().edit(core, &market, id, |fig| {
            fig.alert = !fig.alert;
            if fig.alert {
                // Apply the core's default strategy when arming a figure that has none.
                if fig.strategy_id == 0 && def_strategy != 0 {
                    fig.strategy_id = def_strategy;
                }
                upsert_blob = figure_blob(fig);
            } else {
                disarm = true;
            }
            true
        });
        if !changed {
            return false;
        }
        if let Some(blob) = upsert_blob {
            let _ = self.session.chart_alert_upsert(core, market, id, blob);
        } else if disarm {
            let _ = self.session.chart_alert_delete(core, market, id);
        }
        true
    }

    /// Removes a figure and deletes its core alert if it was armed. The matching global selection
    /// is cleared even if the figure was already absent; command-send errors are ignored.
    pub(crate) fn remove_figure(&mut self, core: CoreId, market: &str, id: u64) {
        let removed = self.figures.borrow_mut().remove(core, market, id);
        if let Some(fig) = removed {
            if fig.alert {
                let _ = self
                    .session
                    .chart_alert_delete(core, market.to_string(), id);
            }
        }
        if self
            .fig_selected
            .as_ref()
            .is_some_and(|(c, m, i)| *c == core && m == market && *i == id)
        {
            self.fig_selected = None;
        }
    }

    /// Shares a figure with every core on its market, or takes the sharing back.
    ///
    /// A shared figure is drawn on this market's chart whichever core the chart belongs to, while
    /// still being owned — and persisted — by the core it was drawn on. Refused for an armed or
    /// server-owned figure; see `Figure::can_share`.
    pub(crate) fn set_figure_shared(&mut self, core: CoreId, market: &str, id: u64, shared: bool) {
        if !self
            .figures
            .borrow_mut()
            .set_shared(core, market, id, shared)
        {
            return;
        }
        // Un-sharing from a chart that does not own the figure makes it vanish there. Drop the
        // selection with it, or the Delete/Alert hotkeys would keep pointing at a figure this
        // chart can no longer see and silently do nothing.
        let still_visible = self
            .figures
            .borrow()
            .visible(core, market)
            .any(|f| f.id == id);
        if !still_visible
            && self
                .fig_selected
                .as_ref()
                .is_some_and(|(c, m, i)| *c == core && m == market && *i == id)
        {
            self.fig_selected = None;
        }
    }

    /// Reconciles chart alerts created by a core or Moonbot into the render store. It decodes the
    /// current blobs from every core into a fresh set of `from_server` figures, skipping unsupported
    /// or malformed blobs. `FigureStore::set_server_figures` then preserves local figures and omits
    /// a server duplicate whose `obj_uid` matches a local figure ID. The coordination loop calls this
    /// only after `chart_alerts_activity` changes.
    pub(crate) fn sync_remote_alerts(&mut self) {
        let mut server: HashMap<FigureKey, Vec<Figure>> = HashMap::new();
        for (core, data) in self.session.store().cores() {
            for ((market, obj_uid), blob) in &data.chart_alerts {
                let Some(d) = alert_blob::decode(blob) else {
                    // The raw bytes of every incoming object are already logged as hex by the feed
                    // (`feed::live`, for exactly this reverse engineering), so an undecodable one
                    // needs no second copy here — only skipping.
                    continue;
                };
                server
                    .entry((core, market.clone()))
                    .or_default()
                    .push(Figure {
                        id: *obj_uid,
                        kind: d.kind,
                        color: d.color,
                        // The core's chart objects carry no fill; the blob has no field for one.
                        fill: [0; 4],
                        thickness: d.thickness,
                        line_kind: d.line_kind,
                        // ROUNDED, not truncated. The store keeps whole milliseconds while a
                        // TDateTime carries a fraction of one, and `as i64` truncates toward zero:
                        // an object Moonbot stamped on an exact millisecond came back a whole
                        // millisecond early roughly a third of the time, so every later edit
                        // re-upserted a blob whose @22 differed from the one it arrived as.
                        created_ms: d.created_ms.round() as i64,
                        alert: true,
                        strategy_id: d.strategy_id,
                        shared: false,
                        from_server: true,
                    });
            }
        }
        self.figures.borrow_mut().set_server_figures(server);
    }

    /// Sets a figure's Alerts-strategy ID, where zero means no strategy. A changed value marks the
    /// figure store for persistence and, for an armed figure, re-upserts a blob carrying the ID at
    /// offset 32. A missing figure or unchanged value is ignored.
    pub(crate) fn set_figure_strategy(
        &mut self,
        core: CoreId,
        market: &str,
        id: u64,
        strategy_id: u64,
    ) {
        let changed = self.figures.borrow_mut().edit(core, market, id, |f| {
            if f.strategy_id == strategy_id {
                false
            } else {
                f.strategy_id = strategy_id;
                true
            }
        });
        if changed {
            self.reupsert_figure_alert(core, market, id);
        }
    }

    /// Edits one figure in place — its style or its tool's own switches — and puts the result
    /// everywhere it has to go: the store marks itself dirty for `figures.json`, and an ARMED
    /// figure re-upserts its blob so the core's copy does not keep the old look.
    ///
    /// The single write path for the per-figure settings, so no caller can change a figure and
    /// forget one of those two. `edit` returns whether anything actually changed; an unchanged
    /// figure costs no save and no round trip to the core.
    ///
    /// A figure that came FROM the core is edited like any other, because dragging one already
    /// works that way: the edit goes back as a re-encoded blob. That round trip keeps only the
    /// fields `alert_blob` decodes, which is a property of the format work being unfinished rather
    /// than of this call — and refusing it here while the drag path does it anyway would only make
    /// the two disagree. One field is not merely dropped but rewritten: a thickness the wire could
    /// not have meant is repaired on decode and the repair goes back with the next edit. What such a figure must NOT be offered is a fill: the blob has no field
    /// for one, so it would be reverted by the next reconcile; the settings panel drops that row.
    pub(crate) fn edit_figure(
        &mut self,
        core: CoreId,
        market: &str,
        id: u64,
        f: impl FnOnce(&mut moon_core::figures::Figure) -> bool,
    ) -> bool {
        if !self.figures.borrow_mut().edit(core, market, id, f) {
            return false;
        }
        self.reupsert_figure_alert(core, market, id);
        true
    }

    /// Re-upserts an armed figure with its current data after an edit. Drag handling calls this on
    /// mouse-up rather than on every movement, and strategy changes reuse the same path. Missing or
    /// unarmed figures do nothing; command-send errors are ignored.
    pub(crate) fn reupsert_figure_alert(&mut self, core: CoreId, market: &str, id: u64) {
        let blob = {
            let store = self.figures.borrow();
            store
                .get(core, market, id)
                .filter(|f| f.alert)
                .and_then(figure_blob)
        };
        if let Some(blob) = blob {
            let _ = self
                .session
                .chart_alert_upsert(core, market.to_string(), id, blob);
        }
    }
}
