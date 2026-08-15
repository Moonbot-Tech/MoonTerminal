//! Regression coverage for fixed-width Fact-versus-variant KPI values.

use super::{CellFormat, KpiCellText, format_kpi_cell};
use crate::analytics::set_pnl_unit;
use moon_core::db::ProfitUnit;

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
        format_kpi_cell(CellFormat::Ratio, 392_964_096.0),
        KpiCellText {
            display: "+393M".to_string(),
            tooltip: Some("+392964096".to_string()),
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
        format_kpi_cell(CellFormat::Ratio, 2.34),
        KpiCellText {
            display: "+2.34".to_string(),
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
