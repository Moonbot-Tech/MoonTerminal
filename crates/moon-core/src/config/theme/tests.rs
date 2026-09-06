use super::*;
use crate::config::{HotkeysConfig, OrdersStyleSet};

/// Builds a complete pre-marker file whose migrating colours are all retired defaults.
fn legacy_retired_set() -> ChartThemeSet {
    let mut set = ChartThemeSet::default();
    let dark = RETIRED_DARK
        .first()
        .expect("the dark migration has a frozen generation");
    let light = RETIRED_LIGHT
        .first()
        .expect("the light migration has a frozen generation");
    set.palette_rev = absent_palette_rev();
    set.dark.candle_up = dark.candle_up;
    set.dark.candle_down = dark.candle_down;
    set.dark.book_bid = dark.book_bid;
    set.dark.book_ask = dark.book_ask;
    set.light.candle_up = light.candle_up;
    set.light.candle_down = light.candle_down;
    set.light.book_bid = light.book_bid;
    set.light.book_ask = light.book_ask;
    set
}

/// Round trip: copied tab text pastes back exactly.
#[test]
fn share_roundtrip() {
    let mut set = ChartThemeSet::default();
    set.dark.bg = [1, 2, 3];
    set.light.grid = [7, 8, 9];
    let text = set.to_share_string().unwrap();
    let parsed = ChartThemeSet::parse_share(&text, &ChartThemeSet::default()).unwrap();
    assert_eq!(parsed, set);
}

/// Old flat theme.toml becomes dark while the caller's light theme remains current.
#[test]
fn share_flat_legacy_goes_dark() {
    let mut flat = ChartTheme::default();
    flat.bg = [10, 20, 30];
    let text = toml::to_string_pretty(&flat).unwrap();
    let mut current = ChartThemeSet::default();
    current.light.bg = [200, 200, 200];
    let parsed = ChartThemeSet::parse_share(&text, &current).unwrap();
    assert_eq!(parsed.dark, flat);
    assert_eq!(parsed.light, current.light);
}

/// Foreign files do not parse as a theme, and vice versa.
#[test]
fn share_rejects_foreign_files() {
    let orders = OrdersStyleSet::default().to_share_string().unwrap();
    let hotkeys = HotkeysConfig::default().to_share_string().unwrap();
    let theme = ChartThemeSet::default().to_share_string().unwrap();
    let cur_t = ChartThemeSet::default();
    let cur_o = OrdersStyleSet::default();

    assert!(ChartThemeSet::parse_share(&orders, &cur_t).is_none());
    assert!(ChartThemeSet::parse_share(&hotkeys, &cur_t).is_none());
    assert!(OrdersStyleSet::parse_share(&theme, &cur_o).is_none());
    assert!(OrdersStyleSet::parse_share(&hotkeys, &cur_o).is_none());
    assert!(HotkeysConfig::parse_share(&theme).is_none());
    assert!(HotkeysConfig::parse_share(&orders).is_none());
    assert!(ChartThemeSet::parse_share("not toml at all {", &cur_t).is_none());

    assert!(OrdersStyleSet::parse_share(&orders, &cur_o).is_some());
    assert!(HotkeysConfig::parse_share(&hotkeys).is_some());
}

/// `theme.rs:ChartThemeSet::retire_old_defaults` must leave current defaults alone; changing its
/// marker guard would make a fresh palette look retired and trigger an unnecessary save.
#[test]
fn current_defaults_are_a_retirement_fixed_point() {
    let mut set = ChartThemeSet::default();
    let before = set.clone();

    assert!(
        !set.retire_old_defaults(),
        "a fresh palette must not be treated as a retired generation"
    );
    assert_eq!(set, before);
}

/// `theme.rs:ChartThemeSet::retire_old_defaults` must move a revision-zero file holding every
/// retired value to current defaults, or existing charts retain the superseded palette.
#[test]
fn revision_zero_retired_defaults_become_current_and_are_stamped() {
    let mut set = legacy_retired_set();

    assert!(
        set.retire_old_defaults(),
        "a retired palette generation must request a one-time save"
    );
    assert_eq!(set, ChartThemeSet::default());
    assert_eq!(set.palette_rev, CURRENT_PALETTE_REV);
}

/// `theme.rs:retire_theme` must preserve a custom byte while carrying other revision-zero values;
/// widening its equality check would overwrite a colour a user deliberately chose.
#[test]
fn revision_zero_custom_colour_survives_the_carry_over() {
    let mut set = legacy_retired_set();
    set.light.candle_up = [0, 129, 0];

    assert!(set.retire_old_defaults());
    assert_eq!(set.light.candle_up, [0, 129, 0]);
    assert_eq!(set.palette_rev, CURRENT_PALETTE_REV);
}

/// `theme.rs:ChartThemeSet::retire_old_defaults` must respect a current revision marker even
/// when a value is retired, or Settings and Moonbot imports cannot keep a selected old colour.
#[test]
fn current_revision_keeps_a_deliberately_reselected_retired_value() {
    let mut set = ChartThemeSet::default();
    set.light.candle_up = RETIRED_LIGHT
        .first()
        .expect("the light migration has a frozen generation")
        .candle_up;
    let before = set.clone();

    assert!(
        !set.retire_old_defaults(),
        "a current revision must not reconsider a user-selected retired colour"
    );
    assert_eq!(set, before);
}

/// `theme.rs:ChartThemeSet::retire_old_defaults` must stamp a revision-zero custom file once;
/// otherwise every launch re-examines and rewrites a palette with no old colours left to move.
#[test]
fn revision_zero_customised_palette_is_stamped_once() {
    let mut set = ChartThemeSet::default();
    set.palette_rev = absent_palette_rev();
    set.dark.candle_up = [1, 2, 3];
    set.dark.candle_down = [4, 5, 6];
    set.dark.book_bid = [7, 8, 9];
    set.dark.book_ask = [10, 11, 12];
    set.light.candle_up = [13, 14, 15];
    set.light.candle_down = [16, 17, 18];
    set.light.book_bid = [19, 20, 21];
    set.light.book_ask = [22, 23, 24];

    assert!(set.retire_old_defaults());
    let once = set.clone();
    assert!(
        !set.retire_old_defaults(),
        "the stamped palette must not be reconsidered on the next launch"
    );
    assert_eq!(set, once);
}

/// `theme.rs:ChartTheme::default` and `default_light` must keep the reviewed palette values;
/// changing one without migration leaves a fresh user and an upgraded user on different colours.
#[test]
fn migrating_defaults_stay_pinned_to_the_reviewed_palette() {
    let dark = ChartTheme::default();
    assert_eq!(dark.candle_up, [26, 158, 92]);
    assert_eq!(dark.candle_down, [230, 59, 59]);
    assert_eq!(dark.book_bid, [28, 100, 64]);
    assert_eq!(dark.book_ask, [140, 46, 46]);

    let light = ChartTheme::default_light();
    assert_eq!(light.candle_up, [26, 158, 92]);
    assert_eq!(light.candle_down, [230, 59, 59]);
    assert_eq!(light.book_bid, [118, 197, 157]);
    assert_eq!(light.book_ask, [240, 137, 137]);
}

/// `theme.rs:retire_theme` must leave untargeted fields byte-for-byte intact while migrating
/// retired palette colours; widening the migration would unexpectedly restyle a user's chart.
#[test]
fn palette_migration_leaves_untargeted_theme_fields_unchanged() {
    let mut set = legacy_retired_set();
    let untouched_light_label = set.light.label_positive;
    assert_eq!(
        untouched_light_label,
        RETIRED_LIGHT
            .first()
            .expect("the light migration has a frozen generation")
            .candle_up,
        "the collision guard requires label_positive to share bytes with retired candle_up"
    );

    assert!(set.retire_old_defaults());

    assert_eq!(set.light.candle_up, ChartTheme::default_light().candle_up);
    assert_eq!(set.light.label_positive, untouched_light_label);
    assert_eq!(set.dark.book_bg, [30, 30, 30]);
    assert_eq!(set.dark.price_line, [209, 153, 92]);
    assert_eq!(set.light.book_bg, [255, 255, 255]);
    assert_eq!(set.light.price_line, [166, 110, 46]);
}

/// `theme.rs:ChartThemeSet::parse_share` must restore an unsent light table from the live set;
/// dropping that restoration replaces a user's light chart colours with shipped defaults.
#[test]
fn sharing_dark_table_keeps_the_distinctive_live_light_theme() {
    let mut current = ChartThemeSet::default();
    current.light.bg = [3, 17, 251];
    let live_light = current.light.clone();
    let text = "[dark]\nbg = [11, 22, 33]\n";

    let parsed = ChartThemeSet::parse_share(text, &current).expect("a dark-only theme parses");

    assert_eq!(parsed.dark.bg, [11, 22, 33]);
    assert_eq!(parsed.light, live_light);
    assert_ne!(parsed.light, ChartTheme::default_light());
}

/// `theme.rs:ChartThemeSet::parse_share` must restore an unsent dark table from the live set;
/// dropping that restoration replaces a user's dark chart colours with shipped defaults.
#[test]
fn sharing_light_table_keeps_the_distinctive_live_dark_theme() {
    let mut current = ChartThemeSet::default();
    current.dark.bg = [251, 17, 3];
    let live_dark = current.dark.clone();
    let text = "[light]\nbg = [220, 221, 222]\n";

    let parsed = ChartThemeSet::parse_share(text, &current).expect("a light-only theme parses");

    assert_eq!(parsed.light.bg, [220, 221, 222]);
    assert_eq!(parsed.dark, live_dark);
    assert_ne!(parsed.dark, ChartTheme::default());
}

/// `theme.rs:ChartThemeSet::parse_share` must migrate only a markerless pasted dark table;
/// migrating the restored light side would rewrite an unsent user-selected retired colour.
#[test]
fn sharing_markerless_dark_table_migrates_only_the_pasted_side() {
    let retired = RETIRED_DARK
        .first()
        .expect("the dark migration has a frozen generation");
    let mut current = ChartThemeSet::default();
    current.light.candle_up = RETIRED_LIGHT
        .first()
        .expect("the light migration has a frozen generation")
        .candle_up;
    let live_light = current.light.clone();
    let text = format!(
        "[dark]\nbg = [11, 22, 33]\ncandle_up = {:?}\ncandle_down = {:?}\nbook_bid = {:?}\nbook_ask = {:?}\n",
        retired.candle_up, retired.candle_down, retired.book_bid, retired.book_ask,
    );

    let parsed =
        ChartThemeSet::parse_share(&text, &current).expect("a markerless dark-only theme parses");

    assert_eq!(parsed.dark.candle_up, ChartTheme::default().candle_up);
    assert_eq!(parsed.dark.candle_down, ChartTheme::default().candle_down);
    assert_eq!(parsed.dark.book_bid, ChartTheme::default().book_bid);
    assert_eq!(parsed.dark.book_ask, ChartTheme::default().book_ask);
    assert_eq!(parsed.light, live_light);
}

/// `theme.rs:ChartThemeSet::parse_share` must stamp a mixed future/current set at the current
/// generation; stamping it at the pasted future generation would strand stale live colours.
#[test]
fn sharing_future_dark_table_with_current_light_uses_the_lower_generation() {
    let current = ChartThemeSet::default();
    let text = format!(
        "palette_rev = {}\n\n[dark]\nbg = [11, 22, 33]\n",
        CURRENT_PALETTE_REV + 2
    );

    let parsed =
        ChartThemeSet::parse_share(&text, &current).expect("a future dark-only theme parses");

    assert_eq!(parsed.palette_rev, current.palette_rev);
}

/// `theme.rs:ChartThemeSet::parse_share` must stamp a mixed set at the lower live/pasted value;
/// taking the higher live generation would prevent a later build from migrating the pasted side.
#[test]
fn sharing_older_dark_table_with_newer_light_uses_the_lower_generation() {
    let mut current = ChartThemeSet::default();
    current.palette_rev = CURRENT_PALETTE_REV + 2;
    let text = format!(
        "palette_rev = {}\n\n[dark]\nbg = [11, 22, 33]\n",
        CURRENT_PALETTE_REV + 1
    );

    let parsed =
        ChartThemeSet::parse_share(&text, &current).expect("a future dark-only theme parses");

    assert_eq!(parsed.palette_rev, CURRENT_PALETTE_REV + 1);
}

/// `theme.rs:ChartThemeSet::parse_share` must retain a paired paste's own newer generation;
/// treating a complete paste as mixed would make a newer portable palette look stale.
#[test]
fn sharing_paired_tables_keeps_the_pasted_generation() {
    let pasted_rev = CURRENT_PALETTE_REV + 2;
    let text = format!(
        "palette_rev = {pasted_rev}\n\n[dark]\nbg = [11, 22, 33]\n\n[light]\nbg = [220, 221, 222]\n"
    );

    let parsed = ChartThemeSet::parse_share(&text, &ChartThemeSet::default())
        .expect("a paired theme parses");

    assert_eq!(parsed.palette_rev, pasted_rev);
    assert_eq!(parsed.dark.bg, [11, 22, 33]);
    assert_eq!(parsed.light.bg, [220, 221, 222]);
}

/// `theme.rs:ChartThemeSet::parse_share` must migrate a revision-zero paired paste while keeping
/// the caller's flat light side, or pasting a dark file can silently recolour unsent light data.
#[test]
fn sharing_retires_both_tables_but_never_the_live_flat_light_set() {
    let paired = legacy_retired_set();
    let paired_text = paired.to_share_string().expect("theme pairs serialize");
    let parsed = ChartThemeSet::parse_share(&paired_text, &ChartThemeSet::default())
        .expect("a paired theme paste parses");
    assert_eq!(parsed, ChartThemeSet::default());

    let flat_text = toml::to_string_pretty(&paired.dark).expect("flat themes serialize");
    let mut current = ChartThemeSet::default();
    current.light = paired.light.clone();
    let live_light = current.light.clone();
    let parsed = ChartThemeSet::parse_share(&flat_text, &current).expect("a flat theme parses");

    assert_eq!(parsed.dark, ChartTheme::default());
    assert_eq!(parsed.light, live_light);
}

/// `theme.rs:ChartThemeSet::resolve` must save a missing file as the current default; otherwise
/// a new installation has no durable palette file for Settings and sharing.
#[test]
fn resolve_missing_text_returns_default_and_requests_a_save() {
    let (set, write_back) = ChartThemeSet::resolve(None);

    assert_eq!(set, ChartThemeSet::default());
    assert!(write_back);
}

/// `theme.rs:ChartThemeSet::resolve` must save only marker-shaped files whose generation moved;
/// rewriting a current file on every launch makes a stable user palette churn needlessly.
#[test]
fn resolve_marker_shaped_text_saves_only_after_a_generation_move() {
    let legacy_text = legacy_retired_set()
        .to_share_string()
        .expect("legacy-shaped pair serializes");
    let (migrated, migrated_write_back) = ChartThemeSet::resolve(Some(&legacy_text));
    assert_eq!(migrated, ChartThemeSet::default());
    assert!(migrated_write_back);

    let current = ChartThemeSet::default();
    let current_text = current.to_share_string().expect("current pair serializes");
    let (unchanged, unchanged_write_back) = ChartThemeSet::resolve(Some(&current_text));
    assert_eq!(unchanged, current);
    assert!(!unchanged_write_back);
}

/// `theme.rs:ChartThemeSet::palette_rev` must keep its field-level serde default; removing it
/// would treat a pre-marker `theme.toml` as current and silently skip its retired-colour carry-over.
#[test]
fn resolve_marker_shaped_text_without_palette_rev_is_migrated_and_stamped() {
    let legacy = legacy_retired_set();
    let text = format!(
        "[dark]\n{}\n[light]\n{}",
        toml::to_string_pretty(&legacy.dark).expect("dark theme serializes"),
        toml::to_string_pretty(&legacy.light).expect("light theme serializes"),
    );

    let (migrated, write_back) = ChartThemeSet::resolve(Some(&text));

    assert_eq!(migrated, ChartThemeSet::default());
    assert_eq!(migrated.palette_rev, CURRENT_PALETTE_REV);
    assert!(write_back);
}

/// `theme.rs:ChartThemeSet::resolve` must not overwrite a corrupt marker-shaped file; otherwise
/// a parse error destroys the user's only inspectable copy of their palette.
#[test]
fn resolve_corrupt_marker_shaped_text_keeps_the_file() {
    let (set, write_back) = ChartThemeSet::resolve(Some("[dark"));

    assert_eq!(set, ChartThemeSet::default());
    assert!(!write_back);
}

/// `theme.rs:ChartThemeSet::resolve` must rewrite both legacy flat and markerless malformed text;
/// collapsing either branch loses automatic old-file migration or leaves an unusable format.
#[test]
fn resolve_markerless_text_is_migrated_and_saved() {
    let legacy = legacy_retired_set();
    let legacy_text = toml::to_string_pretty(&legacy.dark).expect("flat theme serializes");
    let (migrated, migrated_write_back) = ChartThemeSet::resolve(Some(&legacy_text));
    assert_eq!(migrated, ChartThemeSet::default());
    assert!(migrated_write_back);

    let (fallback, fallback_write_back) = ChartThemeSet::resolve(Some("[broken"));
    assert_eq!(fallback, ChartThemeSet::default());
    assert!(fallback_write_back);
}
