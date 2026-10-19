//! Independent reconciliation checks for the shared tape window.

use super::adopt_larger;
use crate::station_api::TapeWindow;

/// Every writer must reject out-of-picker values in the same way as reconciliation.
#[test]
fn normalize_bounds_a_direct_window_edit() {
    assert_eq!(
        super::normalize(TapeWindow {
            margin_s: 601,
            long_position_min: 500
        }),
        TapeWindow {
            margin_s: 600,
            long_position_min: 120
        }
    );
    assert_eq!(
        super::normalize(TapeWindow {
            margin_s: 0,
            long_position_min: 0
        }),
        TapeWindow {
            margin_s: 30,
            long_position_min: 1
        }
    );
}

/// Taking a whole station pair would discard the larger local long-position threshold.
#[test]
fn reconcile_takes_each_larger_field_and_normalizes() {
    let local = TapeWindow {
        margin_s: 180,
        long_position_min: 5,
    };
    let station = TapeWindow {
        margin_s: 600,
        long_position_min: 3,
    };
    assert_eq!(
        adopt_larger(local, station),
        TapeWindow {
            margin_s: 600,
            long_position_min: 5
        }
    );
    assert_eq!(
        adopt_larger(station, local),
        TapeWindow {
            margin_s: 600,
            long_position_min: 5
        }
    );
    assert_eq!(adopt_larger(local, local), local);
    assert_eq!(
        adopt_larger(
            local,
            TapeWindow {
                margin_s: 601,
                long_position_min: 500
            }
        ),
        TapeWindow {
            margin_s: 600,
            long_position_min: 120
        }
    );
}
