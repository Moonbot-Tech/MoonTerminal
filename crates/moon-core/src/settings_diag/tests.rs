use super::*;

/// The line has to carry the symbol verbatim — it is the answer the channel exists for — and the
/// remainder in both units, because MoonBot speaks hours while the wire carries days.
#[test]
fn the_line_carries_the_symbol_verbatim_and_both_units() {
    let shot = BlacklistShot {
        global_on: false,
        global_text: String::new(),
        temp: vec![("ADAUSDT".to_string(), 0.25)],
    };
    let line = shot.describe();
    assert!(line.contains("\"ADAUSDT\""), "{line}");
    assert!(line.contains("days=0.25"), "{line}");
    assert!(line.contains("hours=6.0000"), "{line}");
    assert!(line.contains("temp_rows=1"), "{line}");
}
