use super::{assets, detects, orders, problems};

/// Catches the snapshot going out unpriced: the panel then reads every total as unknown, whatever
/// the rows hold, which is the empty-looking window this module exists to avoid.
#[test]
fn the_balance_is_priced_and_adds_up_with_its_rows() {
    let snapshot = assets(1, "ACEUSDT", 0.2);
    assert!(snapshot.global.usd_rate_known);
    let held: f64 = snapshot.rows.iter().map(|row| row.value_usdt).sum();
    assert!((snapshot.global.total_usdt - snapshot.global.free_usdt - held).abs() < 1e-6);
}

/// The Order button needs the row's market in the catalog, and it reads that from the same snapshot:
/// a holding added without one would silently lose the button.
#[test]
fn every_held_market_is_in_the_catalog() {
    let snapshot = assets(1, "ACEUSDT", 0.2);
    for row in &snapshot.rows {
        assert!(snapshot.markets.contains(&row.market), "{}", row.market);
    }
}

/// An order line off the served price lands outside the chart's Y range, or drags the automatic fit
/// away from the candles: each line has to sit within a few percent of the price it was placed by.
#[test]
fn order_lines_stay_near_the_served_price() {
    let price = 0.2;
    for row in orders(1, "ACEUSDT", price, &(7, "S".to_string()), 0.0) {
        let levels = [
            Some(row.buy_price),
            row.stop_loss,
            row.take_profit,
            row.trailing,
        ];
        for level in levels.into_iter().flatten() {
            assert!(
                (level / price - 1.0).abs() < 0.06,
                "{level} on order {}",
                row.uid
            );
        }
    }
}

/// The panel drops a row with neither flag, and one routed to a chart tab unless the view asks for
/// those: a row that fails either rule never becomes the card it was sent to be.
#[test]
fn detects_are_rows_the_panel_keeps() {
    for row in detects("ACEUSDT", "S", 0.0) {
        assert!(row.sound_alert || row.is_alert);
        assert_eq!(row.add_to_chart, 0);
    }
}

/// The recorded cores are uids 1 and 3: if both had the same findings, one of the panel's two states
/// would never be photographed.
#[test]
fn only_one_of_the_recorded_cores_has_a_problem() {
    assert_ne!(problems(1, 0.0).items.len(), problems(3, 0.0).items.len());
    assert!(problems(1, 0.0).supported && problems(3, 0.0).supported);
}
