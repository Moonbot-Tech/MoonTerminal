//! The axis's "Report" button: the anonymous diagnostic report of the model over the current
//! scope (`moon_core::db::tuner::ticks::diag`), copied to the clipboard and saved to the
//! Downloads folder in one press.
//!
//! The report exists for a user whose model shares are far below the developer's: they press it
//! and post the text, and the developer compares it with their own segment by segment. It holds
//! no coin, price, date, size, profit or name — the module doc of `diag` lists what it does hold
//! — so a user can post it in a public group. Both deliveries at once, because which one fits
//! depends on the size: a few segments fit a chat message, a fleet of cores does not, and the
//! user should not have to guess which before pressing.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::*;
use moon_core::db::tuner::ticks::diag::{self, CoreFacts, ReportInput, ReportRow, TapeClass};
use moon_core::market::trade_replay::TickStatus;
use moon_ui::{MoonButton, MoonButtonVariant, MoonNotification, MoonWindowExt as _};
use rust_i18n::t;

use super::super::super::AnalyticsView;
use super::state::{DealRow, TapeStatus, TicksData};

impl AnalyticsView {
    /// The button, once the axis holds rows; `None` before. It stays while a reload is pending
    /// — every closed trade marks the rows stale on a busy fleet — and the report names the
    /// period the shown rows were read under (`TicksState::loaded_period`).
    pub(super) fn ticks_report_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let rows = self.ticks.data.data().is_some_and(|d| !d.rows.is_empty());
        rows.then(|| {
            MoonButton::new("an-ticks-report-btn")
                .label(t!("analytics.ticks.report_btn").to_string())
                .variant(MoonButtonVariant::Soft)
                .tooltip(t!("analytics.ticks.report_tip").to_string())
                .on_click(cx.listener(|this, _, window, cx| this.ticks_make_report(window, cx)))
                .render()
                .into_any_element()
        })
    }

    /// Build the report, copy it, save it, and say where it went.
    ///
    /// The write is a few kilobytes on a press, not on a frame, so it runs here: a background
    /// task would only move the notice a frame later.
    fn ticks_make_report(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.ticks_report_text(cx) else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        let note = match save(&text) {
            Ok(path) => MoonNotification::success(
                t!(
                    "analytics.ticks.report_done",
                    path = path.display().to_string()
                )
                .to_string(),
            ),
            Err(error) => MoonNotification::warning(
                t!("analytics.ticks.report_copied_only", error = error).to_string(),
            ),
        };
        window.push_notification(note, cx);
    }

    /// The report's text over the loaded rows; `None` before a load.
    fn ticks_report_text(&self, cx: &App) -> Option<String> {
        let data = self.ticks.data.data()?;
        // UTC seconds; `to` is exclusive and `from` negative for an open period.
        let period_days = self
            .ticks
            .loaded_period
            .filter(|&(from, to)| from >= 0 && to > from)
            .map(|(from, to)| (to - from) as f64 / 86_400.0);
        let input = ReportInput {
            build: build_label(),
            period_days,
            without_ms: data.without_ms,
            service: data.service,
            untunable: data.untunable,
            rows: data.rows.iter().map(|row| report_row(row, data)).collect(),
            cores: core_facts(&data.rows, self, cx),
            model: super::model_cfg::current(),
        };
        Some(diag::render(&input))
    }
}

/// One table row as the report reads it.
fn report_row<'a>(row: &'a DealRow, data: &'a TicksData) -> ReportRow<'a> {
    let deal = &row.deal;
    // A strategy's fields are read per core, or once for a strategy the load could not tie to
    // one (`unmodelled_map`).
    let outside = data
        .unmodelled
        .get(&(deal.strategy_id, Some(deal.core_uid)))
        .or_else(|| data.unmodelled.get(&(deal.strategy_id, None)))
        .map_or(&[][..], Vec::as_slice);
    ReportRow {
        deal,
        venue: row.address.as_ref().map(|a| venue_name(a.venue)),
        tape: tape_class(row.tape),
        verdict: row.verdict.as_ref(),
        outside_model: outside,
    }
}

/// The report's narrowing of a row's tape status. Every `TickStatus` is named, so a new one
/// fails to compile here rather than landing under a wrong name.
fn tape_class(tape: TapeStatus) -> TapeClass {
    match tape {
        TapeStatus::Covered => TapeClass::Covered,
        TapeStatus::Missing => TapeClass::Missing,
        TapeStatus::NoAddress => TapeClass::NoAddress,
        TapeStatus::Fetching => TapeClass::Fetching,
        TapeStatus::Refused(status) => TapeClass::Refused(match status {
            TickStatus::Pending => "Pending",
            TickStatus::Streaming => "Streaming",
            TickStatus::AwaitingCore => "AwaitingCore",
            TickStatus::ContextUnavailable => "ContextUnavailable",
            TickStatus::NoRoute => "NoRoute",
            TickStatus::Disabled => "Disabled",
            TickStatus::OutOfRetention { .. } => "OutOfRetention",
            TickStatus::NoTrades => "NoTrades",
            TickStatus::RateLimited { .. } => "RateLimited",
            TickStatus::Failed => "Failed",
            TickStatus::Served => "Served",
        }),
    }
}

/// A venue as the report names it: the brand's own display name and the market in English. Not
/// `venue_label`, which localizes the market — the report is one text for every reader — and
/// not `Debug`, whose spelling follows the variant names rather than the brand's.
fn venue_name(venue: moon_core::venue::Venue) -> String {
    use moon_core::venue::MarketKind;
    let market = match venue.kind {
        MarketKind::Spot => "Spot",
        MarketKind::Futures => "Futures",
        MarketKind::Quarterly => "Quarterly",
    };
    format!("{} {market}", venue.brand.display())
}

/// Each core's build and time zone, as the session holds them now. A core the session does not
/// hold is left out, and the report says it is not connected.
fn core_facts(rows: &[DealRow], view: &AnalyticsView, cx: &App) -> HashMap<u64, CoreFacts> {
    let backend = view.backend.read(cx);
    let store = backend.session.store();
    let mut out = HashMap::new();
    for row in rows {
        let uid = row.deal.core_uid;
        if out.contains_key(&uid) {
            continue;
        }
        if let Some(core) = store.core(uid) {
            out.insert(
                uid,
                CoreFacts {
                    // The store keeps a configured core while it is offline, and drops its
                    // build on any status but Ready.
                    connected: core.status == moon_core::feed::ConnStatus::Ready,
                    version: core.server_version,
                    tz_offset_secs: core.time_offset.offset_secs,
                },
            );
        }
    }
    out
}

/// The terminal's build as the status bar's tooltip states it: the release base and the commit.
fn build_label() -> String {
    format!(
        "{} ({})",
        option_env!("MOONTERMINAL_RELEASE_BASE").unwrap_or("unknown"),
        option_env!("MOONTERMINAL_GIT_REV").unwrap_or("unknown")
    )
}

/// Write the report to the Downloads folder under a UTC-stamped name, so two reports never
/// overwrite each other; the logs folder when the platform names no Downloads folder.
fn save(text: &str) -> Result<PathBuf, String> {
    let now = moon_core::util::now_unix_ms_i64();
    let stamp = chrono::DateTime::from_timestamp_millis(now)
        .map(|t| t.format("%Y%m%d-%H%M%S").to_string())
        .unwrap_or_else(|| now.to_string());
    let name = format!("moonterminal-tuner-report-{stamp}.txt");
    let path = moon_core::config::paths::user_download_path(&name)
        .unwrap_or_else(|| moon_core::config::paths::logs_dir().join(&name));
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    Ok(path)
}
