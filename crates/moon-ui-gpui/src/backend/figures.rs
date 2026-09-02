//! `Backend` methods for the figure and alert layer: toggling the selected figure's Alert flag,
//! removing a figure while disarming its core alert, and re-upserting an alert after an edit.
//! This child module keeps the implementation separate from `main.rs` while retaining access to
//! `Backend`'s private fields.

use std::collections::HashMap;

use moon_core::alert_blob;
use moon_core::figures::{
    DEFAULT_FILL_ALPHA, DrawStyle, Figure, FigureKey, FigureTool, ToolSetting, ToolSettings,
};
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
        fig.created_ms,
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

    /// Whether `tool` takes part in the `switch_figure` cycle — Moonbot's `HotKey` checkbox.
    ///
    /// Reads the EXCLUSION list in `hotkeys.toml`, so a tool nobody has switched off answers `true`
    /// without an entry anywhere; see `HotkeysConfig::switch_figure_skip`.
    pub(crate) fn tool_in_cycle(&self, tool: FigureTool) -> bool {
        !self
            .config
            .hotkeys
            .switch_figure_skip
            .iter()
            .any(|key| key == tool.def().key)
    }

    /// Puts `tool` into the cycle or takes it out, returning whether anything changed.
    ///
    /// Marks the configuration for the coordination loop's routine save, like every other setting
    /// written from a panel rather than from the Settings window.
    pub(crate) fn set_tool_in_cycle(&mut self, tool: FigureTool, on: bool) -> bool {
        if self.tool_in_cycle(tool) == on {
            return false;
        }
        let key = tool.def().key;
        let edit = |skip: &mut Vec<String>| {
            if on {
                skip.retain(|held| held != key);
            } else if !skip.iter().any(|held| held == key) {
                skip.push(key.to_string());
            }
        };
        edit(&mut self.config.hotkeys.switch_figure_skip);
        // The Settings draft takes the same edit, for the reason
        // `settings::hotkeys::tab::confirm_core_pull` states about its own write: a later
        // "Settings > Save" starts from `preview` and copies it over `config`, so a draft that
        // never learned about this switch would silently roll it back to whatever the Settings
        // window held when it opened.
        if let Some(preview) = self.preview.as_mut() {
            edit(&mut preview.hotkeys.switch_figure_skip);
        }
        self.config_dirty = true;
        true
    }

    /// The tool the `switch_figure` hotkey advances to, honouring the exclusions.
    pub(crate) fn next_fig_tool(&self) -> FigureTool {
        self.fig_tool.next_allowed(|tool| self.tool_in_cycle(tool))
    }

    /// Deletes the last figure DRAWN on this chart — Moonbot's Ctrl+Z.
    ///
    /// A chart holding only the core's own alerts deletes nothing: they are not ours to remove, and
    /// removing one here would remove it on the core. Whether the KEY is then consumed is the
    /// caller's decision, not this answer — see `HotkeyAction::FigUndo`.
    ///
    /// The borrow is taken and released on its own line: `remove_figure` borrows the same `RefCell`
    /// mutably, and a `Ref` still alive across that call is a panic inside the frame loop.
    ///
    /// Args:
    ///     core: Core owning the chart.
    ///     market: Market the chart is showing.
    ///
    /// Returns:
    ///     Whether a figure was deleted.
    pub(crate) fn undo_last_figure(&mut self, core: CoreId, market: &str) -> bool {
        let last = self.figures.borrow().last_local(core, market);
        let Some(id) = last else {
            return false;
        };
        self.remove_figure(core, market, id);
        true
    }

    /// Whether the Sells-to-zone drawing mode is armed.
    pub(crate) fn sells_zone_armed(&self) -> bool {
        self.sells_zone_arm.is_some()
    }

    /// Arms the Sells-to-zone drawing mode, or ends it when it is already armed.
    ///
    /// Arming selects the Zone tool and enables drawing, so each band is placed by the ordinary
    /// two-click figure path. It STAYS armed while band after band is drawn — spreading sells is
    /// aiming work, and the first band is rarely the last. The previous tool and drawing mode are
    /// remembered and put back by [`Self::disarm_sells_zone`], whichever way the mode ends.
    pub(crate) fn toggle_sells_zone_arm(&mut self) {
        if self.sells_zone_arm.is_some() {
            self.disarm_sells_zone();
            return;
        }
        self.sells_zone_arm = Some((self.fig_tool, self.fig_draw_mode));
        self.fig_tool = FigureTool::Channel;
        self.fig_draw_mode = true;
    }

    /// Ends the Sells-to-zone drawing mode, restoring the tool and drawing mode it interrupted.
    ///
    /// A no-op when nothing is armed, so every place that ends the mode — the repeated hotkey,
    /// Escape, a tool picked from the toolbar — can call it unconditionally.
    pub(crate) fn disarm_sells_zone(&mut self) {
        if let Some((tool, draw_mode)) = self.sells_zone_arm.take() {
            self.fig_tool = tool;
            self.fig_draw_mode = draw_mode;
        }
    }

    /// Selects a drawing tool and enters drawing, as the toolbar and the per-tool hotkeys do.
    ///
    /// The single place that pair is written, so a caller cannot select a tool while leaving the
    /// Sells-to-zone mode armed — which would hand the next band drawn to the core as a live
    /// bulk move.
    pub(crate) fn select_fig_tool(&mut self, tool: FigureTool) {
        self.disarm_sells_zone();
        self.fig_tool = tool;
        self.fig_draw_mode = true;
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

    /// Toggles the selected figure's Alert flag. See [`Self::set_figure_alert`] for what arming and
    /// disarming actually do; this is the hotkey's entry point and only resolves WHICH figure.
    /// Returns `false` when nothing is selected.
    pub(crate) fn toggle_selected_figure_alert(&mut self) -> bool {
        let Some((core, market, id)) = self.fig_selected.clone() else {
            return false;
        };
        let Some(armed) = self
            .figures
            .borrow()
            .get(core, &market, id)
            .map(|f| f.alert)
        else {
            return false;
        };
        self.set_figure_alert(core, &market, id, !armed)
    }

    /// Sets a figure's Alert flag: upserting the chart alert when arming it and deleting the core
    /// alert when disarming it. A newly armed figure with no strategy inherits the core's nonzero
    /// default strategy.
    ///
    /// The single place the flag is written AND the rule it must obey — so the hotkey over a chart
    /// and the Alerts panel's checkbox cannot arm a figure two different ways, and a surface that
    /// greys its control is only DRAWING a refusal this function makes. Returns whether anything
    /// changed; a missing figure, an unchanged flag and every refusal return `false`.
    pub(crate) fn set_figure_alert(
        &mut self,
        core: CoreId,
        market: &str,
        id: u64,
        on: bool,
    ) -> bool {
        // Arming is refused for a figure the core cannot represent — a tool it has no type for, or
        // a figure shared across cores, which has no single core to arm it on.
        let Some((armed, strategy_before, can_alert)) = self
            .figures
            .borrow()
            .get(core, market, id)
            .map(|f| (f.alert, f.strategy_id, f.can_alert()))
        else {
            return false;
        };
        if armed == on || (on && !can_alert) {
            return false;
        }
        // BOTH directions are refused while the core cannot be commanded. A chart-alert command is
        // attempted once by the core's worker and never retried, so a flag flipped now would simply
        // disagree with Moonbot: an arm nobody received, or a disarm that leaves the core firing an
        // alert this side believes is gone.
        if !self.core_can_command(core) {
            return false;
        }
        let market = market.to_string();
        let def_strategy = self.alert_def_strategy(core);
        let mut upsert_blob = None;
        let now_ms = moon_chart::paint::now_unix_ms();
        let changed = self.figures.borrow_mut().edit(core, &market, id, |fig| {
            fig.alert = on;
            // Stamped on the way OUT, cleared on the way back: `reconcile_local_alerts` reads it to
            // tell an upsert still in flight from an object the core does not have.
            fig.alert_sent_ms = if on { now_ms } else { 0.0 };
            if on {
                // Apply the core's default strategy when arming a figure that has none.
                if fig.strategy_id == 0 && def_strategy != 0 {
                    fig.strategy_id = def_strategy;
                }
                upsert_blob = figure_blob(fig);
            }
            true
        });
        if !changed {
            return false;
        }
        let sent = match (on, upsert_blob) {
            (true, Some(blob)) => self
                .session
                .chart_alert_upsert(core, market.clone(), id, blob),
            (false, _) => self.session.chart_alert_delete(core, market.clone(), id),
            // Arming produced no blob: a tool the encoder has no chart-object type for. Nothing was
            // sent and nothing can be, so the flag it just set is a lie and rolls back below.
            (true, None) => Err(anyhow::anyhow!("figure {id} has no chart-object encoding")),
        };
        // A command that never left the terminal must not leave the figure claiming otherwise. The
        // flag is what the Alerts panel's checkbox and the chart's badge both read, and nothing else
        // ever reconciles a LOCAL figure's flag against the core — an unsent arm would sit there
        // ticked, and an unsent disarm would hide an alert the core is still firing.
        if let Err(err) = sent {
            log::warn!("chart alert not sent for figure {id} on {market}: {err}");
            self.figures.borrow_mut().edit(core, &market, id, |fig| {
                fig.alert = armed;
                fig.strategy_id = strategy_before;
                fig.alert_sent_ms = 0.0;
                true
            });
            return false;
        }
        true
    }

    /// Whether a core is in a state that can carry a command it will not be asked twice for.
    ///
    /// `Ready` is the same test the Analytics purge and the tuner's core menu apply before offering
    /// an action; chart alerts need it because the worker attempts the call once, logs a failure and
    /// moves on.
    pub(crate) fn core_can_command(&self, core: CoreId) -> bool {
        self.session
            .store()
            .core(core)
            .is_some_and(|c| c.status == moon_core::feed::ConnStatus::Ready)
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
        // What the cores actually hold, and which of them may be believed about it — see
        // `FigureStore::reconcile_local_alerts`. A core is believed once it is connected AND has
        // reported its chart-alert set at least once (`chart_alerts_rev`), because it is asked for
        // a full snapshot on connect; before that, an empty set means "has not said" rather than
        // "holds nothing".
        let mut held: std::collections::HashSet<(CoreId, u64)> = std::collections::HashSet::new();
        let mut authoritative: std::collections::HashSet<CoreId> = std::collections::HashSet::new();
        for (core, data) in self.session.store().cores() {
            if data.chart_alerts_rev > 0 && data.status == moon_core::feed::ConnStatus::Ready {
                authoritative.insert(core);
            }
            for ((market, obj_uid), blob) in &data.chart_alerts {
                held.insert((core, *obj_uid));
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
                        // The blob carries no fill — every sampled length is accounted for by the
                        // header and the geometry — so a tool that FILLS gets one derived from its
                        // line colour, which is what Moonbot itself draws. Deterministic on purpose:
                        // the next reconcile rebuilds this figure from the same bytes and must
                        // arrive at the same fill, which is also why the settings panel offers no
                        // fill row for a server figure.
                        fill: match d.kind.tool().def().fills {
                            true => [d.color[0], d.color[1], d.color[2], DEFAULT_FILL_ALPHA],
                            false => [0; 4],
                        },
                        kind: d.kind,
                        color: d.color,
                        thickness: d.thickness,
                        line_kind: d.line_kind,
                        // Kept as it arrived, to the last bit: this is what a re-upsert writes
                        // back into Moonbot's own object, and rounding it away made Moonbot delete
                        // the object on the first drag.
                        created_ms: d.created_ms,
                        alert: true,
                        strategy_id: d.strategy_id,
                        shared: false,
                        // Nothing is in flight for a figure the core just handed us.
                        alert_sent_ms: 0.0,
                        from_server: true,
                    });
            }
        }
        self.figures.borrow_mut().set_server_figures(server);
        // AFTER the server set is in place: a figure Moonbot dropped is already gone from it, and
        // this is what stops a LOCAL figure from claiming an alert the core no longer has.
        self.figures.borrow_mut().reconcile_local_alerts(
            &held,
            &authoritative,
            moon_chart::paint::now_unix_ms(),
        );
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
