//! Synthetic Screener rows for the minimum-width rendering test.

use std::{rc::Rc, sync::Arc};

use gpui::{
    Bounds, Context, IntoElement, ParentElement as _, Pixels, Render, Styled as _, TestAppContext,
    VisualTestContext, WeakEntity, Window, div, px, size,
};
use moon_chart::auto_figures::scan::ScanAnswer;
use moon_core::{
    config::UiThemeMode,
    market::{ScreenerRow, listing::Listing},
    venue::MarketKind,
};
use moon_ui::{MoonDataTable, MoonDataTableColumn, MoonPalette, MoonTheme};

use crate::screener::{
    auto_figures::{MarketScan, ScanBook},
    prepared::{Prepared, SelectedRow},
    table::{
        COLS, ColDef, Entry, EntryVenue, RowStyle, column_label, screener_row, short_kind_captions,
        short_listed_captions,
    },
    view::{CoinQuery, ScreenerView},
};

pub(super) const LONG_COIN: &str = "XALPHAEXCEPTIONALLYLONGSYMBOL";
pub(super) const LONG_VENUE: &str = "XALPHA International Quarterly Futures Exchange";

/// Create a grouped market with mixed leverage and a singleton with a long core name.
fn entry(core: u64) -> Entry {
    let coin = if core < 33 { LONG_COIN } else { "XBETA" };
    let market = format!("{coin}USDT");
    Entry {
        row: ScreenerRow {
            core,
            provider: core,
            market: market.clone(),
            coin: coin.into(),
            vol_24h: 1_234_567.0,
            vol_1h: 98_765.0,
            vol_5m: 12_345.0,
            ask: 0.000_000_029_72,
            price_step: 0.000_000_000_01,
            d_24h: 25.27,
            d_1h: 14.26,
            d_15m: 2.29,
            funding_pct: -0.08,
            leverage_x: if core == 0 { 5 } else { 20 },
            max_leverage: 20,
            isolated: Some(core == 0),
            core_pnl: -19.35,
            pos_size: 987_654.32,
            orders: 9,
            listing: Listing::FuturesOnly,
            ..Default::default()
        }
        .into(),
        core_name: format!("CORE_XALPHA_LONG_LABEL_{core}").into(),
        market_uc: market.into(),
        coin_uc: coin.into(),
        venue: EntryVenue {
            label: LONG_VENUE.into(),
            brand: None,
            kind: Some(MarketKind::Quarterly),
        },
    }
}

/// Draw production cells with the column schema and width fitting used by the Screener.
struct LayoutTable {
    prepared: Arc<Prepared>,
    rows: Rc<Vec<SelectedRow>>,
    cols: Rc<Vec<&'static ColDef>>,
    style: Rc<RowStyle>,
}

impl Render for LayoutTable {
    /// Render the rows without connecting a navigation target or a backend.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = MoonPalette::active(cx);
        let prepared = self.prepared.clone();
        let rows = self.rows.clone();
        let cols = self.cols.clone();
        let style = self.style.clone();
        let view = WeakEntity::new_invalid();
        let columns = self.cols.iter().map(|&&(key, title, width, right)| {
            let col = MoonDataTableColumn::new(key, column_label(key, title), width).sortable(true);
            let col = if right { col.right() } else { col };
            if key == "market" {
                col.fixed_left()
            } else {
                col
            }
        });
        div().size_full().child(
            MoonDataTable::new("screener-layout", rows.len(), move |ix, _, _| {
                screener_row(&rows[ix], &prepared, &view, palette, &cols, &style)
            })
            .columns(columns)
            .style(crate::design::table_style(palette)),
        )
    }
}

/// Open a 900 by 480 fixture window in the light presentation theme.
pub(super) fn open(cx: &mut TestAppContext, keys: &[&str]) -> VisualTestContext {
    cx.update(|cx| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Light, 1.0),
            cx,
        );
    });
    let cols = Rc::new(COLS.iter().filter(|col| keys.contains(&col.0)).collect());
    let prepared = Arc::new(Prepared::new((0..34).map(entry).collect(), "market", false));
    let ranks: Vec<_> = (0..34).map(Some).collect();
    let selection = prepared.select(&ranks, &CoinQuery::new(""), 0.0, "market", false, true);
    let mut scans = ScanBook::default();
    let run = scans.begin(0, 1);
    assert!(scans.apply(
        run,
        vec![(
            0,
            prepared.entries[selection.rows[0].lead].row.market.clone(),
            MarketScan::Scanned(ScanAnswer {
                cache_lag_ms: Some(987_654_321),
                ..Default::default()
            }),
        )]
    ));
    let [both, spot_only, futures_only] = short_listed_captions();
    let window = cx.open_window(size(px(900.0), px(480.0)), move |_, _| LayoutTable {
        prepared,
        rows: Rc::new(selection.rows),
        cols,
        style: Rc::new(RowStyle {
            logos_ready: false,
            logo_px: px(13.0),
            logo_gap: px(5.0),
            kinds: short_kind_captions(),
            both,
            spot_only,
            futures_only,
            scan_book: Rc::new(scans),
            scan_labels: ScreenerView::scan_labels(),
            scan_running: false,
        }),
    });
    VisualTestContext::from_window(window.into(), cx)
}

/// Flush the layout feedback from the table's viewport into its fitted columns.
pub(super) fn draw(visual: &mut VisualTestContext) {
    for _ in 0..4 {
        visual.update(|window, _| window.refresh());
        visual.run_until_parked();
    }
}

/// Look up a MoonUI selector generated from a row and column index.
pub(super) fn bounds(visual: &mut VisualTestContext, selector: String) -> Bounds<Pixels> {
    let selector = Box::leak(selector.into_boxed_str());
    visual
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("missing bounds for {selector}"))
}

/// Allow half a pixel of layout rounding on each edge.
pub(super) fn inside(inner: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
    let slop = px(0.5);
    inner.left() + slop >= outer.left()
        && inner.right() <= outer.right() + slop
        && inner.top() + slop >= outer.top()
        && inner.bottom() <= outer.bottom() + slop
}
