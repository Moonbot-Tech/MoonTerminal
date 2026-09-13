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
    assert!(!path.show, "the saved show flag must survive");
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
    assert!(set.dark.path.show && set.light.path.show);
}
