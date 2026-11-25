use super::*;

/// `orders.rs:PathStyle` gained `use_line_color` after users had already saved `orders.toml`
/// files without it. Such a file must still load, keep every path field it does carry, and take
/// the shipped default for the new one — a `[dark.path]` table written before #508 is exactly what
/// most installs have on disk.
#[test]
fn an_orders_toml_written_before_use_line_color_still_loads_its_path() {
    let text = "\
[dark]
trace_alpha = 0.4

[dark.path]
show = false
color = [211, 211, 211]
thickness = 2.5
dashed = false
";
    let set: OrdersStyleSet = toml::from_str(text).expect("a pre-#508 orders.toml must decode");
    let path = &set.dark.path;
    assert!(
        !path.show,
        "the retired show flag must still decode: the startup migration reads it"
    );
    assert_eq!(path.color, [211, 211, 211], "the saved colour must survive");
    assert!(
        (path.thickness - 2.5).abs() < 1e-6,
        "the saved thickness must survive"
    );
    assert!(!path.dashed, "the saved dash flag must survive");
    assert!(
        path.use_line_color,
        "a file without the field takes the shipped default, the line's own colour"
    );
    assert!(
        (set.dark.trace_alpha - 0.4).abs() < 1e-6,
        "trace_alpha stays a top-level field, so an old value is not lost"
    );
}

/// `orders.rs:PathStyle::default` ships `use_line_color` ON, and both theme sets inherit it, so a
/// fresh install draws the repricing trail as the server trace always was drawn.
#[test]
fn both_theme_sets_default_to_the_line_colour_for_the_path() {
    let set = OrdersStyleSet::default();
    assert!(set.dark.path.use_line_color);
    assert!(set.light.path.use_line_color);
}

/// `OrdersStyleSet::draw_take_profit` arrived after users had saved `orders.toml` without it (#1409).
/// Such a file must load with the line still drawn — an upgrade must not hide what a chart showed —
/// and keep everything else it carries.
#[test]
fn an_orders_toml_written_before_draw_take_profit_still_draws_the_line() {
    let text = "\
[dark]
active_alpha = 0.8

[dark.take_profit]
color = [1, 2, 3]
";
    let set: OrdersStyleSet = toml::from_str(text).expect("a pre-#1409 orders.toml must decode");
    assert!(set.draw_take_profit, "an absent switch resolves to drawn");
    assert_eq!(set.dark.take_profit.color, [1, 2, 3]);
    assert!((set.dark.active_alpha - 0.8).abs() < 1e-6);
}

/// A stored OFF is the user's choice and must survive a save and a load — the value sits at the
/// top of the file, ahead of the two theme tables, so it must also serialize there at all.
#[test]
fn a_stored_draw_take_profit_off_survives_a_round_trip() {
    let set = OrdersStyleSet {
        draw_take_profit: false,
        ..OrdersStyleSet::default()
    };
    let text = toml::to_string_pretty(&set).expect("the set serializes");
    let (back, write_back) = OrdersStyleSet::resolve(&text).expect("its own output decodes");
    assert!(!back.draw_take_profit, "a stored off must not be rewritten");
    assert!(!write_back, "nothing needed migrating");
    assert_eq!(back, set);
}

/// Pasting a set copied before the switch existed changes the colours it carries, not the user's
/// answer to a question that file never asked; a pasted file that does carry it wins.
#[test]
fn pasting_a_set_without_the_switch_keeps_the_current_choice() {
    let current = OrdersStyleSet {
        draw_take_profit: false,
        ..OrdersStyleSet::default()
    };
    let old = "[dark.take_profit]\ncolor = [9, 9, 9]\n";
    let pasted = OrdersStyleSet::parse_share(old, &current).expect("an orders set");
    assert!(!pasted.draw_take_profit);
    assert_eq!(pasted.dark.take_profit.color, [9, 9, 9]);

    let flat = "[take_profit]\ncolor = [9, 9, 9]\n";
    let pasted = OrdersStyleSet::parse_share(flat, &current).expect("a flat set");
    assert!(!pasted.draw_take_profit);

    let new = "draw_take_profit = true\n\n[dark.take_profit]\ncolor = [9, 9, 9]\n";
    let pasted = OrdersStyleSet::parse_share(new, &current).expect("an orders set");
    assert!(pasted.draw_take_profit);
}

/// A file with no theme table takes the legacy flat branch of `resolve`, and that branch must not
/// reset a stored switch: a stored off stays off even in a file trimmed down to it.
#[test]
fn a_file_without_theme_tables_keeps_a_stored_draw_take_profit() {
    let (set, _) = OrdersStyleSet::resolve("draw_take_profit = false\n").expect("decodes");
    assert!(!set.draw_take_profit);
    let (set, _) = OrdersStyleSet::resolve("active_alpha = 0.5\n").expect("decodes");
    assert!(set.draw_take_profit, "absent still resolves to drawn");
}

/// `OrdersStyleSet::show_position_age` arrived after users had saved `orders.toml` without it
/// (#1402). A new display feature ships ON, so such a file shows the age — in both the themed and
/// the legacy flat shape — and a stored off is the user's and survives a save and a load.
#[test]
fn position_age_reads_on_when_absent_and_keeps_a_stored_off() {
    let themed = "draw_take_profit = false\n\n[dark]\nactive_alpha = 0.8\n";
    let (set, _) = OrdersStyleSet::resolve(themed).expect("a pre-#1402 orders.toml decodes");
    assert!(set.show_position_age, "absent resolves to shown");
    assert!(
        !set.draw_take_profit,
        "the neighbouring switch keeps its own answer"
    );
    let (flat, _) = OrdersStyleSet::resolve("active_alpha = 0.5\n").expect("decodes");
    assert!(flat.show_position_age, "the flat branch defaults it on too");

    let (flat_off, _) = OrdersStyleSet::resolve("show_position_age = false\n").expect("decodes");
    assert!(
        !flat_off.show_position_age,
        "the flat branch keeps a stored off"
    );
    let off = OrdersStyleSet {
        show_position_age: false,
        ..OrdersStyleSet::default()
    };
    let text = toml::to_string_pretty(&off).expect("the set serializes");
    let (back, write_back) = OrdersStyleSet::resolve(&text).expect("its own output decodes");
    assert!(
        !back.show_position_age,
        "a stored off must not be rewritten"
    );
    assert!(!write_back);
}

/// Pasting a set copied before the switch existed keeps the user's current answer for it, in the
/// themed and the flat shape alike; a pasted file that carries the switch wins.
#[test]
fn pasting_a_set_without_position_age_keeps_the_current_choice() {
    let current = OrdersStyleSet {
        show_position_age: false,
        ..OrdersStyleSet::default()
    };
    let old = "[dark.take_profit]\ncolor = [9, 9, 9]\n";
    let pasted = OrdersStyleSet::parse_share(old, &current).expect("an orders set");
    assert!(!pasted.show_position_age);
    let flat = "[take_profit]\ncolor = [9, 9, 9]\n";
    let pasted = OrdersStyleSet::parse_share(flat, &current).expect("a flat set");
    assert!(!pasted.show_position_age);
    let new = "show_position_age = true\n\n[dark.take_profit]\ncolor = [9, 9, 9]\n";
    let pasted = OrdersStyleSet::parse_share(new, &current).expect("an orders set");
    assert!(pasted.show_position_age);
}
