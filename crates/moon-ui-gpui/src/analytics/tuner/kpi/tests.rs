//! Regression coverage for fixed-width Fact-versus-variant KPI values.

use super::{
    CellFormat, FigureTone, KpiCellText, MetricKind, figure_of, format_kpi_cell, plain_factor,
};
use crate::analytics::set_pnl_unit;
use moon_core::db::ProfitUnit;
use moon_core::db::tuner::VarStats;

/// Changing `kpi.rs:format_kpi_cell` to return the exact formatter for every finite magnitude must
/// fail the large-value assertions; otherwise All-period Fact/v1/v2 values wrap or overpaint
/// adjacent cells and rows. Removing the active percent suffix from its compact profit branch must
/// fail the percent assertion, preventing abbreviated PnL from being displayed in the wrong unit.
#[test]
fn all_period_large_values_use_compact_text_and_keep_exact_tooltips() {
    set_pnl_unit(None);

    assert_eq!(
        format_kpi_cell(CellFormat::Integer, 454_520_257_399.0),
        KpiCellText {
            display: "455B".to_string(),
            tooltip: Some("454520257399".to_string()),
        }
    );
    assert_eq!(
        format_kpi_cell(CellFormat::Profit, 4_175_275_332_081.0),
        KpiCellText {
            display: "+4.18T".to_string(),
            tooltip: Some("+4175275332081".to_string()),
        }
    );
    assert_eq!(
        plain_factor(392_964_096.0),
        KpiCellText {
            display: "393M".to_string(),
            tooltip: Some("392964096.00".to_string()),
        }
    );

    assert_eq!(
        format_kpi_cell(CellFormat::Integer, 365.0),
        KpiCellText {
            display: "365".to_string(),
            tooltip: None,
        }
    );
    assert_eq!(
        format_kpi_cell(CellFormat::Profit, 71.41),
        KpiCellText {
            display: "+71.41".to_string(),
            tooltip: None,
        }
    );
    assert_eq!(
        plain_factor(2.34),
        KpiCellText {
            display: "2.34".to_string(),
            tooltip: None,
        }
    );

    set_pnl_unit(Some(ProfitUnit::Percent));
    assert_eq!(
        format_kpi_cell(CellFormat::Profit, 1_234_567.0),
        KpiCellText {
            display: "+1.23M%".to_string(),
            tooltip: Some("+1234567%".to_string()),
        }
    );
    set_pnl_unit(None);
}

fn column(
    n: i64,
    wins: i64,
    profit: f64,
    pf: f64,
    avg_win: f64,
    avg_loss: f64,
    max_dd: f64,
) -> VarStats {
    VarStats {
        n,
        wins,
        profit,
        pf,
        avg: if n > 0 { profit / n as f64 } else { 0.0 },
        avg_win,
        avg_loss,
        max_dd,
        ..VarStats::default()
    }
}

/// A loss magnitude must not paint as a green plus, in either column: the same function paints
/// every column. Zero winning deals leave win rate at 0% (it has a unit), profit factor at
/// 0.00, and average win undefined.
#[test]
fn loss_metrics_are_negative_and_undefined_ratios_are_a_dash() {
    set_pnl_unit(None);
    let losing = column(2, 0, -138.32, 0.0, 0.0, 69.16, 138.32);
    let loss = figure_of(MetricKind::AvgLoss, &losing);
    assert_eq!(loss.text.display, "-69.16");
    assert_eq!(loss.tone, FigureTone::Signed);
    assert!(loss.signed < 0.0);
    let dd = figure_of(MetricKind::MaxDrawdown, &losing);
    assert_eq!(dd.text.display, "-138.32");
    assert_eq!(dd.tone, FigureTone::Signed);
    assert_eq!(figure_of(MetricKind::Winrate, &losing).text.display, "0.0%");
    assert_eq!(
        figure_of(MetricKind::ProfitFactor, &losing).text.display,
        "0.00"
    );
    assert_eq!(figure_of(MetricKind::AvgWin, &losing).text.display, "—");
    assert_eq!(
        figure_of(MetricKind::Winrate, &losing).tone,
        FigureTone::Neutral
    );
    let empty = column(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0);
    assert_eq!(figure_of(MetricKind::Winrate, &empty).text.display, "—");
    assert_eq!(
        figure_of(MetricKind::ProfitFactor, &empty).tone,
        FigureTone::Muted
    );
    assert_eq!(figure_of(MetricKind::AvgLoss, &empty).text.display, "—");
    assert_eq!(figure_of(MetricKind::MaxDrawdown, &empty).text.display, "—");
    // The same inputs paint the same text for every column. A second call is the other column.
    assert_eq!(
        figure_of(MetricKind::Profit, &losing).text.display,
        figure_of(MetricKind::Profit, &losing).text.display
    );
    assert_eq!(
        figure_of(MetricKind::Profit, &losing).tone,
        FigureTone::Signed
    );
    set_pnl_unit(None);
}

/// Only break-even trades have a finite profit factor of 0, because both sums are zero.
/// Painting that `0.00` says the ratio is defined. All-winners stay at `99.00`, which is
/// the figure the matrix already shows when the loss sum is zero and the win sum is not.
/// All-losers stay at `0.00`: their loss average is positive.
///
/// Dropping the both-sums-zero guard from `figure_of` turns the break-even assertion red
/// and the cell reads `0.00`. Treating 99 as undefined turns the all-winners assertion red.
#[test]
fn break_even_profit_factor_is_a_dash_and_all_winners_stay_99() {
    set_pnl_unit(None);
    let even = column(4, 0, 0.0, 0.0, 0.0, 0.0, 0.0);
    let pf = figure_of(MetricKind::ProfitFactor, &even);
    assert_eq!(pf.text.display, "—");
    assert_eq!(pf.tone, FigureTone::Muted);
    let winners = column(3, 3, 30.0, 99.0, 10.0, 0.0, 0.0);
    let shown = figure_of(MetricKind::ProfitFactor, &winners);
    assert_eq!(shown.text.display, "99.00");
    assert_eq!(shown.tone, FigureTone::Neutral);
    let losers = column(2, 0, -10.0, 0.0, 0.0, 5.0, 10.0);
    assert_eq!(
        figure_of(MetricKind::ProfitFactor, &losers).text.display,
        "0.00"
    );
    set_pnl_unit(None);
}
