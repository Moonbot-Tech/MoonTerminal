//! `Backend` methods for the figure and alert layer: toggling the selected figure's Alert flag,
//! removing a figure while disarming its core alert, and re-upserting an alert after an edit.
//! This child module keeps the implementation separate from `main.rs` while retaining access to
//! `Backend`'s private fields.

use std::collections::HashMap;

use moon_core::alert_blob;
use moon_core::figures::{Figure, FigureKey};
use moon_core::session::CoreId;

use crate::Backend;

/// Encodes a figure for a chart-alert upsert, using the figure ID as `obj_uid`.
fn figure_blob(fig: &Figure) -> Vec<u8> {
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
                upsert_blob = Some(figure_blob(fig));
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
                    continue;
                };
                server
                    .entry((core, market.clone()))
                    .or_default()
                    .push(Figure {
                        id: *obj_uid,
                        kind: d.kind,
                        color: d.color,
                        thickness: d.thickness,
                        line_kind: d.line_kind,
                        created_ms: d.created_ms as i64,
                        alert: true,
                        strategy_id: d.strategy_id,
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

    /// Re-upserts an armed figure with its current data after an edit. Drag handling calls this on
    /// mouse-up rather than on every movement, and strategy changes reuse the same path. Missing or
    /// unarmed figures do nothing; command-send errors are ignored.
    pub(crate) fn reupsert_figure_alert(&mut self, core: CoreId, market: &str, id: u64) {
        let blob = {
            let store = self.figures.borrow();
            store
                .get(core, market, id)
                .filter(|f| f.alert)
                .map(figure_blob)
        };
        if let Some(blob) = blob {
            let _ = self
                .session
                .chart_alert_upsert(core, market.to_string(), id, blob);
        }
    }
}
