//! The trade-tape cleanup of the Storage tab: what `trades.sqlite` may lose, counted for the
//! preview under the button and cut for real on a press — or on the terminal's own initiative
//! once the cores are up ([`startup`]).
//!
//! The file knows markets and stretches of time, not trades (`trade_cache::trim`), so a cleanup
//! is built the other way round: the report's rows are resolved to the markets the file spells —
//! through the live catalog like a trade window, or, for a core that is not connected, by the
//! coin's name against the markets the file already holds — and each row claims the stretches
//! its window would ask for as prints (`ReplayWindow::focus_spans`) at the margin in force.
//! The union of every claim per market is the [`KeepMap`]; everything outside it goes. Two
//! rows whose claims overlap claim the ground between them too, so only the outer edges of a
//! run of trades are ever cut.
//!
//! Only the tuner's rows claim ([`TapeOwner::is_tunable`]), each at the margin the tuner reads
//! it with: what a wider margin left behind, the prints of manual sells, funding rows,
//! container strategies and of trades the report no longer knows are what one press removes.
//!
//! Everything here runs on the background executor: the replica scan, the catalog lookups and
//! the trim itself all block. The one thing taken on the UI thread is the [`CleanupContext`] —
//! handles and settings, not state.

pub(crate) mod startup;

use std::collections::HashMap;
use std::sync::Arc;

use gpui::*;
use moon_ui::{MoonButton, MoonPalette, h_flex, rgba_from, v_flex};
use rust_i18n::t;

use super::super::SettingsView;
use crate::Backend;
use crate::design;
use moon_core::db::ReportAxis;
use moon_core::db::ReportStamp;
use moon_core::db::tape_owners::{TapeOwner, read_tape_owners};
use moon_core::db::tuner::ticks::{ORDER_WAIT_CAP_MS, model_window_at, order_open_at};
use moon_core::market::MarketDataSource;
use moon_core::market::trade_replay::trade_cache::{self, Inventory, KeepMap, TrimReport};
use moon_core::market::trade_replay::{Coverage, long_position_ms, model_margin_ms, worker};
use moon_core::symbol::Exchange;

/// What one pass found — the preview's numbers, or the apply's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::settings) struct CleanupPreview {
    pub report: TrimReport,
    /// Report rows inside the file's time range that were let claim.
    pub claimants: usize,
    /// Of those, rows whose core is not connected and that were matched by the coin's name.
    pub by_name: usize,
    /// Of those, rows that matched no market of the file either way; their prints, if any,
    /// count as nobody's.
    pub unresolved: usize,
}

/// How far past the file's own time range the replica is read, in seconds, before the margin's
/// own reach is added ([`Margins::reach_s`]): a core's clock offset moves a row's stamps
/// against the file's UTC ones by at most that.
const OWNER_SLACK_S: i64 = moon_core::db::MAX_OFFSET_SECS as i64;

/// The handles and settings a pass needs, taken on the UI thread.
#[derive(Clone)]
pub(super) struct CleanupContext {
    source: MarketDataSource,
    /// `core_uid` to the core's quote (`ServerConfig::market`), for spelling a coin into a
    /// market — the connected cores' and the offline ones' alike.
    quotes: HashMap<u64, String>,
    /// The core clocks, for lifting the report's stamps onto the file's UTC.
    axis: ReportAxis,
}

/// What a claim is sized with, read once per pass so every row of it is judged alike whatever
/// the Storage tab does meanwhile: the margin the tuner's fetch and the close-time capture ask
/// for (the chart's `[trade_replay] margin_s` floored to the model's two pads), and the length
/// from which a position is walked as its two ends.
#[derive(Clone, Copy, Debug)]
struct Margins {
    model_ms: i64,
    long_position_ms: i64,
}

impl Margins {
    fn live() -> Self {
        Self {
            model_ms: model_margin_ms(),
            long_position_ms: long_position_ms(),
        }
    }

    /// How far, in seconds, a row's claim can reach past its own stamps — the margin, and before
    /// the entry the entry order's life on top of it (`ORDER_WAIT_CAP_MS`), rounded up. A row
    /// that opened this much after the file's last print still claims prints inside the file, so
    /// the replica is read that much wider than the file's range. The order's life reaches only
    /// before the entry: on the other bound the reach is wider than any claim, and the few rows
    /// it adds claim nothing.
    fn reach_s(self) -> i64 {
        (self.model_ms + ORDER_WAIT_CAP_MS).div_euclid(1_000) + 1
    }
}

impl CleanupContext {
    pub(super) fn of(backend: &Backend) -> Self {
        Self {
            source: backend.session.market_source(),
            quotes: backend
                .config
                .servers
                .iter()
                .map(|s| (s.uid, s.market.clone()))
                .collect(),
            axis: backend.report_axis(chrono_tz::UTC),
        }
    }

    /// Count what a cleanup would remove, or remove it (`apply`) and compact the file.
    /// Blocking; call from the background executor.
    ///
    /// Args:
    ///     apply: `false` for the preview, `true` for the press.
    pub(super) fn run(&self, apply: bool) -> anyhow::Result<CleanupPreview> {
        // No file to maintain — the switch off and nothing ever written — reads as nothing to
        // remove, not as a failure; the handle refuses to create the file for a count. With
        // the switch on the handle is asked whatever the disk shows: a worker just started by
        // the replay path creates the file on its own thread a moment later.
        if !trade_cache::is_enabled() && !moon_core::config::paths::trades_db_path().exists() {
            return Ok(CleanupPreview::default());
        }
        let cache = trade_cache::maintenance_handle()
            .ok_or_else(|| anyhow::anyhow!("trade cache worker is not running"))?;
        let inventory = cache
            .inventory()
            .ok_or_else(|| anyhow::anyhow!("trade cache did not answer"))??;
        let Some((from_ms, to_ms)) = inventory.range_ms else {
            return Ok(CleanupPreview::default());
        };
        let margins = Margins::live();
        let slack_s = OWNER_SLACK_S + margins.reach_s();
        let owners = read_tape_owners(
            from_ms.div_euclid(1_000) - slack_s,
            to_ms.div_euclid(1_000) + slack_s,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        let claimants: Vec<&TapeOwner> = owners.iter().filter(|o| o.is_tunable()).collect();
        let (keep, mut preview) = build_keep(
            &inventory,
            &claimants,
            &self.axis,
            &self.quotes,
            margins,
            |owner| self.live_address(owner),
        );
        let report = cache
            .trim(Arc::new(keep), apply)
            .ok_or_else(|| anyhow::anyhow!("trade cache did not answer"))??;
        if apply {
            // The tiles in memory still hold what the file just lost; a held-data query would
            // go on answering from them until a restart.
            worker::forget_tiles();
            log::info!(
                "[x] trades cleanup: {} claimant row(s), {} by name, {} unresolved; {} span(s) dropped, {} cut, {} print(s) / {} bytes gone of {}",
                preview.claimants,
                preview.by_name,
                preview.unresolved,
                report.spans_dropped,
                report.spans_cut,
                report.prints_dropped,
                report.bytes_dropped,
                report.bytes_total
            );
        }
        preview.report = report;
        Ok(preview)
    }

    /// Where a row's prints live by the live catalog, the way a trade window resolves it;
    /// `None` when the core is not connected or its catalog does not spell the coin.
    fn live_address(&self, owner: &TapeOwner) -> Option<(String, String)> {
        let quote = self
            .quotes
            .get(&owner.core_uid)
            .map(String::as_str)
            .unwrap_or_default();
        let address = self.source.replay_address(owner.core_uid).ok()?;
        let market = self
            .source
            .resolve_market(owner.core_uid, quote, &owner.coin)?;
        Some((address.exchange_key, market))
    }
}

/// The union of every claimant's stretches, per market of the file.
///
/// Args:
///     inventory: The file's markets, for the name fallback.
///     claimants: The rows let claim.
///     axis: The core clocks.
///     quotes: Each core's quote, for spelling a coin by name.
///     margins: The margins in force.
///     live: The catalog's answer for a row, or `None` — see [`CleanupContext::live_address`].
fn build_keep(
    inventory: &Inventory,
    claimants: &[&TapeOwner],
    axis: &ReportAxis,
    quotes: &HashMap<u64, String>,
    margins: Margins,
    mut live: impl FnMut(&TapeOwner) -> Option<(String, String)>,
) -> (KeepMap, CleanupPreview) {
    let by_name = NameIndex::of(inventory);
    let mut keep = KeepMap::new();
    let mut addresses: Addresses = HashMap::new();
    let mut preview = CleanupPreview {
        claimants: claimants.len(),
        ..Default::default()
    };
    for owner in claimants {
        let (keys, by_name_only) = addresses
            .entry((owner.core_uid, owner.coin.clone()))
            .or_insert_with(|| match live(owner) {
                Some(key) => (vec![key], false),
                None => {
                    let quote = quotes
                        .get(&owner.core_uid)
                        .map(String::as_str)
                        .unwrap_or_default();
                    (by_name.matches(&owner.coin, quote), true)
                }
            });
        if keys.is_empty() {
            preview.unresolved += 1;
            continue;
        }
        if *by_name_only {
            preview.by_name += 1;
        }
        for stamp in stamps(axis, owner) {
            // The tuner's own window (`model_window_at`): from the entry order's creation where
            // the row carries it, so the lead the tuner fetched for the order's life is claimed
            // with the trade — split by the pass's own threshold, not one read a moment later.
            let Some(window) = model_window_at(
                order_open_at(stamp.buy_ms, stamp.buy_set_ms),
                stamp.buy_ms,
                stamp.close_ms,
                margins.model_ms,
                margins.long_position_ms,
            ) else {
                continue;
            };
            let focus = window.focus_spans();
            for key in keys.iter() {
                let coverage = keep.entry(key.clone()).or_insert_with(Coverage::none);
                for &span in focus.spans() {
                    coverage.add(span);
                }
            }
        }
    }
    (keep, preview)
}

/// Per distinct `(core, coin)`: the markets it claims on, and whether the name had to stand in
/// for the catalog.
type Addresses = HashMap<(u64, String), (Vec<(String, String)>, bool)>;

/// One claim's stamps on one clock: the entry, the exit, and the entry order's creation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ClaimStamps {
    buy_ms: i64,
    close_ms: i64,
    buy_set_ms: Option<i64>,
}

/// The row's stamps on the file's clock: through the core's measured offset, the way the trade
/// window and the close-time capture stamp their requests — and, when both stamps are
/// milliseconds, the raw values too, the way the tuner's fetch stamps its. Two claims where the
/// clocks disagree, so neither path's tape is cut as the other's excess. The order's creation is
/// a millisecond stamp on the entry's clock and moves with it.
fn stamps(axis: &ReportAxis, owner: &TapeOwner) -> Vec<ClaimStamps> {
    let (buy_ms, close_ms) = axis.stamp_pair_to_utc_ms(owner.buy, owner.close, owner.core_uid);
    let raw_buy_ms = match owner.buy {
        ReportStamp::Millis(buy) => Some(buy),
        ReportStamp::Seconds(_) => None,
    };
    let lifted = ClaimStamps {
        buy_ms,
        close_ms,
        buy_set_ms: owner
            .buy_set_ms
            .zip(raw_buy_ms)
            .map(|(set, raw)| set + (buy_ms - raw)),
    };
    let mut out = vec![lifted];
    if let (ReportStamp::Millis(buy), ReportStamp::Millis(close)) = (owner.buy, owner.close)
        && (buy, close) != (buy_ms, close_ms)
    {
        out.push(ClaimStamps {
            buy_ms: buy,
            close_ms: close,
            buy_set_ms: owner.buy_set_ms,
        });
    }
    out
}

/// The file's markets by their upper-cased name, for the name fallback.
struct NameIndex {
    markets: HashMap<String, Vec<(String, String)>>,
}

impl NameIndex {
    fn of(inventory: &Inventory) -> Self {
        let mut markets: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for key in &inventory.keys {
            markets
                .entry(key.1.to_ascii_uppercase())
                .or_default()
                .push(key.clone());
        }
        Self { markets }
    }

    /// Every market of the file one of the coin's spellings names, on any exchange: the coin
    /// as stored (old rows hold the full market), then each family's own shape. Wider than the
    /// catalog would answer — a spelling shared by two exchanges claims on both — and that is
    /// the safe direction: a claim too wide keeps prints, never loses them.
    fn matches(&self, coin: &str, quote: &str) -> Vec<(String, String)> {
        // The setting is a bare quote (`USDT`) on most cores; `resolve_quote_on` reads a bare
        // quote as a market with no base and answers nothing, so the setting itself stands
        // in then, upper-cased like every spelling here.
        let quote = match moon_core::symbol::resolve_quote_on(quote, Exchange::Unknown) {
            resolved if resolved.is_empty() => quote.to_ascii_uppercase(),
            resolved => resolved,
        };
        let mut out: Vec<(String, String)> = Vec::new();
        let candidates = std::iter::once(coin.to_string()).chain(
            moon_core::symbol::parse::market_names_for(coin, &quote, Exchange::Unknown),
        );
        for candidate in candidates {
            if let Some(keys) = self.markets.get(&candidate.to_ascii_uppercase()) {
                for key in keys {
                    if !out.contains(key) {
                        out.push(key.clone());
                    }
                }
            }
        }
        out
    }
}

/// What a cleanup removed, for the status line: prints and the packed bytes they took.
fn removal_detail(report: &TrimReport) -> String {
    t!(
        "storage.trades_cleanup_done",
        prints = report.prints_dropped,
        size = super::fmt_size(report.bytes_dropped.max(0) as u64)
    )
    .to_string()
}

impl SettingsView {
    /// Count what the cleanup button would remove, on the background executor, and show it
    /// under the button. One count at a time: an ask that lands while one runs marks it dirty,
    /// and the running one asks again when it finishes — the margin may have moved twice while
    /// the replica was being read.
    pub(super) fn storage_cleanup_refresh(&mut self, cx: &mut Context<Self>) {
        if self.storage.cleanup_inflight {
            self.storage.cleanup_dirty = true;
            return;
        }
        self.storage.cleanup_inflight = true;
        self.storage.cleanup_dirty = false;
        // Pending, not the last count: the margin may have moved, and a stale count would
        // leave the button armed on numbers the press would no longer match.
        self.storage.cleanup = None;
        let context = CleanupContext::of(self.backend.read(cx));
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let result = executor.spawn(async move { context.run(false) }).await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.storage.cleanup_inflight = false;
                    this.storage.cleanup = Some(result.map_err(|e| e.to_string()));
                    if this.storage.cleanup_dirty {
                        this.storage_cleanup_refresh(cx);
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Run the cleanup for real through the tab's maintenance path; the status line names
    /// what went, and the snapshot and the count are taken again afterwards.
    fn trades_cleanup_press(&mut self, cx: &mut Context<Self>) {
        let context = CleanupContext::of(self.backend.read(cx));
        self.storage_op(cx, "storage.op_cleanup", move || {
            context
                .run(true)
                .map(|preview| Some(removal_detail(&preview.report)))
        });
    }

    /// The cleanup button with the count beside it.
    pub(super) fn trades_cleanup_controls(
        &self,
        cx: &mut Context<Self>,
        p: MoonPalette,
        busy: bool,
    ) -> impl IntoElement {
        let muted = rgba_from(p.text_muted, 1.0);
        let preview = self.storage.cleanup.clone();
        let line = match &preview {
            None => t!("storage.trades_cleanup_pending").to_string(),
            Some(Err(err)) => t!("storage.trades_cleanup_failed", err = err).to_string(),
            Some(Ok(preview)) if preview.report.is_empty() => {
                t!("storage.trades_cleanup_empty").to_string()
            }
            Some(Ok(preview)) => t!(
                "storage.trades_cleanup_preview",
                prints = preview.report.prints_dropped,
                size = super::fmt_size(preview.report.bytes_dropped.max(0) as u64)
            )
            .to_string(),
        };
        // Pressable only with something counted to remove: an empty count is "nothing to do",
        // a pending one may still be a large file being read.
        let armed = !busy && matches!(&preview, Some(Ok(preview)) if !preview.report.is_empty());
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 8.0))
                    .items_center()
                    .child(
                        // Spaces around the label: the fork's `MoonButton` `pad_x=0` bug, as
                        // the other tool buttons of the tab work around it.
                        MoonButton::new("trades-cleanup")
                            .outline()
                            .label(format!("  {}  ", t!("storage.trades_cleanup")))
                            .disabled(!armed)
                            .on_click(cx.listener(|this, _, _, cx| this.trades_cleanup_press(cx)))
                            .render(),
                    )
                    // MIXED NODE: the count combines a localized label with figures in one
                    // text node — stays mono, like the tab's other readouts.
                    .child(
                        div()
                            .text_color(muted)
                            .font_family(design::mono())
                            .child(line),
                    ),
            )
            .child(
                div()
                    .text_color(muted)
                    .child(t!("storage.trades_cleanup_hint").to_string()),
            )
    }
}

#[cfg(test)]
mod tests;
