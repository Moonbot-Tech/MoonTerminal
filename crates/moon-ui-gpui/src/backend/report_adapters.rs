//! Warning display settings and report valuation adapters.

use crate::Backend;
use gpui::Context;
use moon_core::db::valuation::ValuationMode;
use moon_core::session::CoreId;
use moon_core::session::core_order::CoreOrder;
use moon_core::session::core_order::OrderedCores;

impl Backend {
    /// The core-warning axis toggles (CPU / memory / connectivity / ping).
    pub(crate) fn warn_axes(&self) -> moon_core::config::layout::WarnAxesCfg {
        self.layout.warn_axes
    }

    /// Store the core-warning axis toggles from the Core Status gear popup and mark layout dirty.
    ///
    /// The engine reads these at the next tick, so a disabled axis stops opening episodes at once;
    /// the read paths also filter its persisted history out, so it also disappears from the charts.
    pub(crate) fn set_warn_axes(&mut self, axes: moon_core::config::layout::WarnAxesCfg) {
        if self.layout.warn_axes != axes {
            self.layout.warn_axes = axes;
            self.layout_dirty = true;
            // A toggle shifts which episodes charts should draw without opening/closing one, so push
            // the revision forward to invalidate the cached marks on every chart.
            self.warn.bump_rev();
        }
    }

    /// The group's active trading core as a SURFACE may address it: `None` in Auto Overview.
    ///
    /// [`Self::active_trade_core`] falls through to the group's FIRST core in Overview, where the
    /// header draws no per-core cluster at all — an address the user never chose and no popup,
    /// settings page or bulk command may write to. Every surface that resolves a core for a write
    /// asks this rather than the bare method, so the gate is one line rather than a copy at some
    /// of the call sites.
    pub(crate) fn scoped_trade_core(&self, group: &str) -> Option<CoreId> {
        if self.is_auto_overview_scope(group) {
            None
        } else {
            self.active_trade_core(group)
        }
    }

    /// Return the group's cores in canonical order for the header selector.
    pub(crate) fn group_cores(&self, group: &str) -> OrderedCores {
        CoreOrder::new(&self.config).from_sessions(self.session.sessions(), |s| s.group == group)
    }

    /// Which conversion every quote-money surface currently applies.
    ///
    /// One application-wide setting rather than one per panel: the many simultaneous Report hosts —
    /// a docked tab per group window, a detached window per group, the scoped standalone window,
    /// and Analytics — must not present the same period under two conversions. It also makes the
    /// worker's demand signal exact: one setting, one flag, no reference counting to leak.
    ///
    /// Returns:
    ///     The saved valuation mode.
    pub(crate) fn valuation_mode(&self) -> ValuationMode {
        self.config.report_valuation_mode
    }

    /// Current name of every configured core ([`moon_core::db::CoreNames::from_servers`]).
    pub(crate) fn report_core_names(&self) -> moon_core::db::CoreNames {
        moon_core::db::CoreNames::from_servers(&self.config.servers)
    }

    /// The time axis every replicated report timestamp is DISPLAYED on.
    ///
    /// Built from the retained per-core snapshots rather than by reading `reports.sqlite`, because
    /// this is called while rendering: a database read per frame would put a file-system round trip
    /// inside the paint path, and the numbers it would return are already in memory — the writer
    /// publishes `FeedMsg::TimeOffset` only AFTER its transaction commits, so a core whose snapshot
    /// carries an offset is a core whose segment is already durable.
    ///
    /// This axis carries ONE segment per core, the one in force. The read paths load the full
    /// segment history from the table inside their own pinned snapshot; they need it because they
    /// convert instants across all of history, while a panel only ever renders what is current.
    /// The two therefore agree on exactly the thing they share, which is the CURRENT offset.
    ///
    /// A core with no measurement contributes no segment, so it converts as the identity -- the
    /// same answer the uncorrected terminal gave, which is the only assumption that cannot make an
    /// honest UTC core worse.
    ///
    /// # Known limitation: ONE segment, so no offset HISTORY
    ///
    /// The durable table keeps an append-only segment list and picks the segment covering each
    /// row's own instant. This axis carries only the CURRENT segment per core, because it is built
    /// from the retained live snapshot rather than from SQLite -- and `ReportAxis` extends its
    /// earliest segment backward without bound, so every historical row is corrected by the offset
    /// in force TODAY.
    ///
    /// For a core whose offset never moves -- which is every fixed-offset fleet, and every case
    /// the reported bug was about -- that is exactly correct. It goes wrong only once a core's
    /// offset actually CHANGES, i.e. across a DST transition or a machine that moved zone: rows
    /// from before the change then render one delta out, and the Report's period bounds shift with
    /// them, so the grid and the filter stay consistent with each other while both sit an hour off
    /// for the older half of the year. The error is bounded by the offset delta and never
    /// compounds.
    ///
    /// Fixing it means publishing the DURABLE axis as a backend-level artifact refreshed off the
    /// database thread, since this function is called from render paths and must not read SQLite.
    /// That is a deliberate follow-up, not an oversight.
    ///
    /// Args:
    ///     zone: The user's selected display zone.
    ///
    /// Returns:
    ///     The axis to render and to build period bounds on.
    pub(crate) fn report_axis(&self, zone: chrono_tz::Tz) -> moon_core::db::ReportAxis {
        let store = self.session.store();
        let measured = self
            .config
            .servers
            .iter()
            .filter_map(|server| {
                let core = store.core(server.id)?;
                let offset_secs = core.time_offset.offset_secs?;
                Some((
                    server.uid,
                    vec![moon_core::db::OffsetSegment {
                        from_utc: core.time_offset.observed_at_utc.div_euclid(1_000),
                        offset_secs,
                    }],
                ))
            })
            .collect();
        moon_core::db::ReportAxis::from_measured(measured, zone)
    }

    /// Activate the valuation mode a Settings save just committed.
    ///
    /// The value itself is already in `config`, written by the save. Two things do not follow from
    /// that on their own:
    ///
    /// * the worker only fetches current rates while the mode demands them, so this activation
    ///   point sets the demand flag rather than leaving it to each view;
    /// * every open surface reads the mode without polling it. They observe the report revision,
    ///   which nothing else would move here — a mode switch changes no rows, so neither generation
    ///   advances. Without this wake they would keep rendering the previous mode's numbers under
    ///   the new mode's label until some unrelated data change happened along.
    ///
    /// Args:
    ///     cx: Backend context used to publish the revision the other surfaces observe.
    pub(crate) fn apply_valuation_mode(&mut self, cx: &mut Context<Self>) {
        if let Some(valuation) = &self.valuation {
            valuation.set_current_wanted(self.valuation_mode() == ValuationMode::Current);
        }
        self.report_revision.update(cx, |_, cx| cx.notify());
    }
}
