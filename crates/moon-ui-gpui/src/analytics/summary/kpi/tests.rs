//! Column breakpoints of the KPI band.

use super::{KpiCols, kpi_cols};

/// Off-by-one or swapped thresholds stack the band a step early or late.
#[test]
fn kpi_columns_follow_the_breakpoints() {
    assert_eq!(kpi_cols(1400.0, 1.0), KpiCols::Four);
    assert_eq!(kpi_cols(1100.0, 1.0), KpiCols::Four);
    assert_eq!(kpi_cols(1099.0, 1.0), KpiCols::Two);
    assert_eq!(kpi_cols(900.0, 1.0), KpiCols::Two);
    assert_eq!(kpi_cols(700.0, 1.0), KpiCols::Two);
    assert_eq!(kpi_cols(699.0, 1.0), KpiCols::One);
    assert_eq!(kpi_cols(1100.0, 1.25), KpiCols::Two);
}

/// Trimming the ratio back to `fmt::compact` prints "2.9" beside two-decimal figures.
#[test]
fn ratios_keep_two_decimals() {
    assert_eq!(super::recovery_text(Some(2.9)), "2.90");
    assert_eq!(super::recovery_text(Some(3.0)), "3.00");
    assert_eq!(super::recovery_text(None), "—");
    assert_eq!(super::ratio_text(1.314), "1.31");
    assert_eq!(super::ratio_text(f64::INFINITY), "—");
}
