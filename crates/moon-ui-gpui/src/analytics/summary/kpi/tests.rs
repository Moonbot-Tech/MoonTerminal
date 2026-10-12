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
