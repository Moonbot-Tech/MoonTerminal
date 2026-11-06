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
    let flat = ChartTheme {
        bg: [10, 20, 30],
        ..ChartTheme::default()
    };
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
// Each palette channel is named on its own line so a round-trip mismatch points at that channel.
#[allow(clippy::field_reassign_with_default)]
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

/// `theme.rs:ChartTheme::default`, `default_light` and `default_obsidian` must ship `hvol_bg`
/// equal to `book_bg`: that is the field the profile's backdrop read before it had its own, so a
/// fresh install must draw exactly what the previous version drew — and the Obsidian set, which
/// moves both, must move them together.
#[test]
fn fresh_hvol_backdrop_matches_the_book_background_it_used_to_borrow() {
    for theme in [
        ChartTheme::default(),
        ChartTheme::default_light(),
        ChartTheme::default_obsidian(),
    ] {
        assert_eq!(theme.hvol_bg, theme.book_bg);
    }
}

/// `theme.rs:retire_theme` must copy a customised `book_bg` into `hvol_bg` on a file older than
/// the split; leaving the shipped default there would recolour the profile's backdrop for every
/// user who had tuned their spread background.
#[test]
fn pre_split_file_inherits_the_hvol_backdrop_from_the_book_background() {
    let text =
        "palette_rev = 2\n\n[dark]\nbook_bg = [7, 8, 9]\n\n[light]\nbook_bg = [21, 22, 23]\n";

    let (set, write_back) = ChartThemeSet::resolve(Some(text)).unwrap();

    assert!(write_back, "the split must be written back once");
    assert_eq!(set.palette_rev, CURRENT_PALETTE_REV);
    assert_eq!(set.dark.hvol_bg, [7, 8, 9]);
    assert_eq!(set.light.hvol_bg, [21, 22, 23]);
}

/// `theme.rs:ChartThemeSet::retire_old_defaults` must leave `hvol_bg` of a file at the current
/// generation alone even when it differs from `book_bg`; re-running the inheritance would erase
/// the very choice the split exists to allow.
#[test]
fn post_split_hvol_backdrop_is_never_reinherited() {
    let mut set = ChartThemeSet::default();
    set.dark.hvol_bg = [93, 94, 95];
    let before = set.clone();

    assert!(!set.retire_old_defaults());
    assert_eq!(set, before);

    let text = before.to_share_string().expect("current pair serializes");
    let (resolved, write_back) = ChartThemeSet::resolve(Some(&text)).unwrap();
    assert!(!write_back);
    assert_eq!(resolved, before);
}

/// `theme.rs:ChartThemeSet::parse_share` must apply the split to a pasted pre-split table on its
/// own generation, and only to the sides actually sent.
#[test]
fn sharing_pre_split_dark_table_inherits_the_hvol_backdrop_on_the_pasted_side_only() {
    let mut current = ChartThemeSet::default();
    current.light.hvol_bg = [50, 51, 52];
    let live_light = current.light.clone();
    let text = "palette_rev = 2\n\n[dark]\nbg = [11, 22, 33]\nbook_bg = [7, 8, 9]\n";

    let parsed =
        ChartThemeSet::parse_share(text, &current).expect("a pre-split dark-only theme parses");

    assert_eq!(parsed.dark.hvol_bg, [7, 8, 9]);
    assert_eq!(parsed.light, live_light);
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
    let current = ChartThemeSet {
        palette_rev: CURRENT_PALETTE_REV + 2,
        ..ChartThemeSet::default()
    };
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
    let current = ChartThemeSet {
        light: paired.light.clone(),
        ..ChartThemeSet::default()
    };
    let live_light = current.light.clone();
    let parsed = ChartThemeSet::parse_share(&flat_text, &current).expect("a flat theme parses");

    assert_eq!(parsed.dark, ChartTheme::default());
    assert_eq!(parsed.light, live_light);
}

/// `theme.rs:ChartThemeSet::resolve` must save a missing file as the current default; otherwise
/// a new installation has no durable palette file for Settings and sharing.
#[test]
fn resolve_missing_text_returns_default_and_requests_a_save() {
    let (set, write_back) = ChartThemeSet::resolve(None).unwrap();

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
    let (migrated, migrated_write_back) = ChartThemeSet::resolve(Some(&legacy_text)).unwrap();
    assert_eq!(migrated, ChartThemeSet::default());
    assert!(migrated_write_back);

    let current = ChartThemeSet::default();
    let current_text = current.to_share_string().expect("current pair serializes");
    let (unchanged, unchanged_write_back) = ChartThemeSet::resolve(Some(&current_text)).unwrap();
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

    let (migrated, write_back) = ChartThemeSet::resolve(Some(&text)).unwrap();

    assert_eq!(migrated, ChartThemeSet::default());
    assert_eq!(migrated.palette_rev, CURRENT_PALETTE_REV);
    assert!(write_back);
}

/// `theme.rs:ChartThemeSet::resolve` must not overwrite a corrupt marker-shaped file; otherwise
/// a parse error destroys the user's only inspectable copy of their palette.
#[test]
fn resolve_corrupt_marker_shaped_text_keeps_the_file() {
    assert!(ChartThemeSet::resolve(Some("[dark")).is_err());
}

/// Valid legacy themes still migrate; malformed markerless text must not be rewritten as defaults.
#[test]
fn resolve_markerless_text_is_migrated_and_saved() {
    let legacy = legacy_retired_set();
    let legacy_text = toml::to_string_pretty(&legacy.dark).expect("flat theme serializes");
    let (migrated, migrated_write_back) = ChartThemeSet::resolve(Some(&legacy_text)).unwrap();
    assert_eq!(migrated, ChartThemeSet::default());
    assert!(migrated_write_back);

    assert!(ChartThemeSet::resolve(Some("[broken")).is_err());
}

/// Making level defaults explicit would detach old flat/paired themes from their wall colours.
#[test]
fn old_theme_files_keep_automatic_book_levels() {
    for text in [
        "book_bid = [17, 91, 231]
book_ask = [240, 139, 7]
",
        "[dark]
book_bid = [17, 91, 231]
[light]
book_ask = [240, 139, 7]
",
    ] {
        let (set, _) = ChartThemeSet::resolve(Some(text)).unwrap();
        for theme in [&set.dark, &set.light] {
            assert_eq!(theme.book_level_bid, None);
            assert_eq!(theme.book_level_ask, None);
        }
        assert_eq!(set.dark.book_bid, [17, 91, 231]);
    }
}

/// Removing brightening/clamping or rounding to bytes changes untouched level pixels.
/// Exhaust all input bytes against the old shader expression, including fractional channels.
#[test]
fn automatic_book_levels_match_the_old_shader_for_every_byte() {
    let mut theme = ChartTheme::default();
    for value in 0..=255u8 {
        theme.book_bid = [value, 255 - value, 0];
        theme.book_ask = [255 - value, value, 255];
        for (bid, wall) in [(true, theme.book_bid), (false, theme.book_ask)] {
            let actual = theme.resolved_book_level(bid);
            for channel in 0..3 {
                let old_shader = (f32::from(wall[channel]) / 255.0 * 1.25).min(1.0);
                assert_eq!(actual[channel].to_bits(), old_shader.to_bits());
            }
            assert_eq!(actual[3], 1.0);
        }
    }
    theme.book_bid = [1, 102, 254];
    let resolved = theme.resolved_book_level(true);
    assert_eq!(resolved[1], 0.5);
    assert_eq!(resolved[2], 1.0);
    assert_ne!(
        resolved[0],
        1.0 / 255.0,
        "auto must not be rounded to a byte"
    );
}

/// Ignoring an override or brightening it would recolour an explicitly selected line.
#[test]
fn explicit_book_levels_survive_reload_and_ignore_wall_changes() {
    let text = "palette_rev = 2
[dark]
book_level_bid = [0, 102, 255]
book_level_ask = [255, 51, 0]
[light]
book_level_bid = [51, 0, 102]
book_level_ask = [102, 255, 51]
";
    let (set, _) = ChartThemeSet::resolve(Some(text)).unwrap();
    let saved = set
        .to_share_string()
        .expect("explicit level colours serialize");
    let (mut reloaded, write_back) = ChartThemeSet::resolve(Some(&saved)).unwrap();
    assert!(!write_back);
    assert_eq!(reloaded.dark.book_level_bid, Some([0, 102, 255]));
    assert_eq!(reloaded.dark.book_level_ask, Some([255, 51, 0]));
    assert_eq!(reloaded.light.book_level_bid, Some([51, 0, 102]));
    assert_eq!(reloaded.light.book_level_ask, Some([102, 255, 51]));
    for theme in [&mut reloaded.dark, &mut reloaded.light] {
        theme.book_bid = [255, 255, 255];
        theme.book_ask = [0, 0, 0];
    }
    assert_eq!(
        reloaded.dark.resolved_book_level(true),
        [0.0, 0.4, 1.0, 1.0]
    );
    assert_eq!(
        reloaded.dark.resolved_book_level(false),
        [1.0, 0.2, 0.0, 1.0]
    );
    assert_eq!(
        reloaded.light.resolved_book_level(true),
        [0.2, 0.0, 0.4, 1.0]
    );
    assert_eq!(
        reloaded.light.resolved_book_level(false),
        [0.4, 1.0, 0.2, 1.0]
    );
    reloaded.dark.book_level_bid = None;
    let saved = reloaded.to_share_string().expect("reset serializes");
    let (reset, _) = ChartThemeSet::resolve(Some(&saved)).unwrap();
    assert_eq!(reset.dark.book_level_bid, None);
    assert_eq!(reset.dark.resolved_book_level(true), [1.0; 4]);
}

// --- The Obsidian chart table ------------------------------------------------------------------

/// The seven surfaces the Obsidian set moves off the dark default, as reviewed.
const OBSIDIAN_OVERRIDES: [(&str, [u8; 3]); 7] = [
    ("bg", [8, 9, 11]),
    ("book_bg", [8, 9, 11]),
    ("book_bg_ask", [28, 13, 12]),
    ("book_bg_bid", [10, 24, 16]),
    ("grid", [22, 24, 28]),
    ("hvol_bg", [8, 9, 11]),
    ("panel_bg", [13, 15, 18]),
];

/// One theme as its stored TOML table, so two themes can be compared key by key without the test
/// having to name every field — and so a field added later is compared without anyone remembering.
fn theme_table(theme: &ChartTheme) -> toml::Table {
    toml::from_str(&toml::to_string(theme).expect("a chart theme serializes"))
        .expect("a serialized chart theme parses back")
}

/// A complete `theme.toml` exactly as a build from before the `[obsidian]` table wrote it: the
/// generation stamp and the two tables, nothing else.
fn two_table_file(dark: &ChartTheme, light: &ChartTheme) -> String {
    format!(
        "palette_rev = {CURRENT_PALETTE_REV}\n\n[dark]\n{}\n[light]\n{}",
        toml::to_string_pretty(dark).expect("dark theme serializes"),
        toml::to_string_pretty(light).expect("light theme serializes"),
    )
}

/// `theme.rs:ChartTheme::default_obsidian` must differ from the dark default in the seven
/// reviewed surfaces and in nothing else. An eighth override — a candle or a label drifting along
/// with the ground — would restyle the Obsidian chart beyond what was reviewed, and a dropped one
/// would leave a grey zone inside the near-black panels.
#[test]
fn obsidian_defaults_are_the_dark_defaults_with_only_the_reviewed_surfaces_moved() {
    let dark = theme_table(&ChartTheme::default());
    let obsidian = theme_table(&ChartTheme::default_obsidian());
    assert_eq!(
        dark.keys().collect::<Vec<_>>(),
        obsidian.keys().collect::<Vec<_>>(),
        "the two sets must store the same keys"
    );

    // Keyed maps on both sides, so the comparison does not depend on the order either was built in.
    let moved: std::collections::BTreeMap<String, toml::Value> = obsidian
        .iter()
        .filter(|(key, value)| dark.get(key.as_str()) != Some(*value))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();

    let expected: std::collections::BTreeMap<String, toml::Value> = OBSIDIAN_OVERRIDES
        .iter()
        .map(|(key, rgb)| {
            let channels = rgb
                .iter()
                .map(|channel| toml::Value::Integer(i64::from(*channel)))
                .collect();
            ((*key).to_string(), toml::Value::Array(channels))
        })
        .collect();
    assert_eq!(moved, expected);
}

/// `theme.rs:ChartColorSet::for_mode` and the `for_mode` accessors must hand each interface mode
/// its own table: Obsidian the third one and ONLY Obsidian, the two light modes the light one,
/// everything else dark. Routing Obsidian through `is_light()` again draws the dark set's grey
/// ground inside its near-black panels; routing Graphite to the new table restyles a theme nobody
/// asked to change.
#[test]
fn every_interface_mode_draws_its_own_chart_table() {
    let set = ChartThemeSet {
        dark: ChartTheme {
            bg: [1, 1, 1],
            ..ChartTheme::default()
        },
        light: ChartTheme {
            bg: [2, 2, 2],
            ..ChartTheme::default_light()
        },
        obsidian: ChartTheme {
            bg: [3, 3, 3],
            ..ChartTheme::default_obsidian()
        },
        ..ChartThemeSet::default()
    };
    for (mode, color_set, bg) in [
        (UiThemeMode::Dark, ChartColorSet::Dark, [1, 1, 1]),
        (UiThemeMode::Graphite, ChartColorSet::Dark, [1, 1, 1]),
        (
            UiThemeMode::DarkExperimental,
            ChartColorSet::Dark,
            [1, 1, 1],
        ),
        (UiThemeMode::Light, ChartColorSet::Light, [2, 2, 2]),
        (
            UiThemeMode::LightExperimental,
            ChartColorSet::Light,
            [2, 2, 2],
        ),
        (UiThemeMode::Obsidian, ChartColorSet::Obsidian, [3, 3, 3]),
    ] {
        assert_eq!(ChartColorSet::for_mode(mode), color_set, "{mode:?}");
        assert_eq!(ChartColorSet::from(mode), color_set, "{mode:?}");
        assert_eq!(set.for_mode(mode).bg, bg, "{mode:?}");
        assert_eq!(set.for_set(color_set).bg, bg, "{mode:?}");
    }
    // The bool accessor is kept for callers that know only light/dark, and still means that.
    assert_eq!(set.get(false).bg, [1, 1, 1]);
    assert_eq!(set.get(true).bg, [2, 2, 2]);
}

/// `theme.rs:ChartThemeSet::for_mode_mut` must write the table `for_mode` reads and no other. A
/// mutable accessor that resolved Obsidian to the dark table would have every Appearance picker
/// show the Obsidian colour and write it into `[dark]`.
#[test]
fn editing_through_a_mode_writes_only_that_modes_table() {
    for (mode, changed) in [
        (UiThemeMode::Dark, [true, false, false]),
        (UiThemeMode::Graphite, [true, false, false]),
        (UiThemeMode::DarkExperimental, [true, false, false]),
        (UiThemeMode::Light, [false, true, false]),
        (UiThemeMode::LightExperimental, [false, true, false]),
        (UiThemeMode::Obsidian, [false, false, true]),
    ] {
        let before = ChartThemeSet::default();
        let mut set = before.clone();

        set.for_mode_mut(mode).bg = [9, 9, 9];

        assert_eq!(set.for_mode(mode).bg, [9, 9, 9], "{mode:?}");
        assert_eq!(
            [
                set.dark != before.dark,
                set.light != before.light,
                set.obsidian != before.obsidian,
            ],
            changed,
            "{mode:?} must edit exactly its own table"
        );
    }
}

/// `theme.rs:ChartThemeSet::resolve` must load a file written before the `[obsidian]` table
/// existed to the very dark and light values it held, hand it the default Obsidian table, and NOT
/// ask for a write-back. Making the absent table a reason to save would rewrite every existing
/// user's `theme.toml` on the first launch of this build.
#[test]
fn a_two_table_file_loads_its_dark_and_light_unchanged_and_is_not_rewritten() {
    let dark = ChartTheme {
        bg: [1, 2, 3],
        grid_alpha: 0.37,
        book_level_bid: Some([4, 5, 6]),
        ..ChartTheme::default()
    };
    let light = ChartTheme {
        grid: [7, 8, 9],
        price_line_px: 2.3,
        ..ChartTheme::default_light()
    };
    let text = two_table_file(&dark, &light);
    assert!(
        !text.contains("obsidian"),
        "the fixture must be a file from before the table existed"
    );

    let (set, write_back) = ChartThemeSet::resolve(Some(&text)).unwrap();

    assert_eq!(set.dark, dark);
    assert_eq!(set.light, light);
    assert_eq!(set.obsidian, ChartTheme::default_obsidian());
    assert_eq!(set.palette_rev, CURRENT_PALETTE_REV);
    assert!(
        !write_back,
        "an absent [obsidian] table must not rewrite the user's file"
    );
}

/// `theme.rs:ChartThemeSet` must keep filling the keys a hand-trimmed two-table file leaves out
/// exactly as it always has — from the DARK default, on the light side too. The new table's own
/// default must not leak into either, and the sparse file must still cost no write.
#[test]
fn a_sparse_two_table_file_still_fills_missing_keys_from_the_dark_default() {
    let text = format!(
        "palette_rev = {CURRENT_PALETTE_REV}\n\n[dark]\nbg = [1, 2, 3]\n\n[light]\ngrid = [4, 5, 6]\n"
    );

    let (set, write_back) = ChartThemeSet::resolve(Some(&text)).unwrap();

    assert_eq!(
        set.dark,
        ChartTheme {
            bg: [1, 2, 3],
            ..ChartTheme::default()
        }
    );
    assert_eq!(
        set.light,
        ChartTheme {
            grid: [4, 5, 6],
            ..ChartTheme::default()
        }
    );
    assert_eq!(set.obsidian, ChartTheme::default_obsidian());
    assert!(!write_back);
}

/// `theme.rs:ChartThemeSet::resolve` must read a file that carries only an `[obsidian]` table as
/// a set. Left out of the shape check, such a file is taken for a legacy flat theme and rewritten
/// at once with the table gone.
#[test]
fn an_obsidian_only_file_is_read_as_a_set_and_keeps_its_table() {
    let (set, _) = ChartThemeSet::resolve(Some("[obsidian]\nbg = [40, 41, 42]\n")).unwrap();

    assert_eq!(set.obsidian.bg, [40, 41, 42]);
    assert_eq!(set.dark, ChartTheme::default());
    assert_eq!(set.light, ChartTheme::default_light());
}

/// `theme.rs:ChartThemeSet::to_share_string` is the text `save` writes, and it must carry all
/// three tables back through both readers: the load path without asking for another write, and
/// the paste path over an unrelated live set.
#[test]
fn a_saved_set_round_trips_all_three_tables() {
    let set = ChartThemeSet {
        dark: ChartTheme {
            bg: [1, 2, 3],
            ..ChartTheme::default()
        },
        light: ChartTheme {
            grid: [7, 8, 9],
            ..ChartTheme::default_light()
        },
        obsidian: ChartTheme {
            bg: [4, 5, 6],
            cross_alpha: 0.25,
            book_level_ask: Some([10, 11, 12]),
            ..ChartTheme::default_obsidian()
        },
        ..ChartThemeSet::default()
    };
    let saved = set.to_share_string().expect("a three-table set serializes");

    let (loaded, write_back) = ChartThemeSet::resolve(Some(&saved)).unwrap();
    assert_eq!(loaded, set);
    assert!(!write_back);

    let pasted = ChartThemeSet::parse_share(&saved, &ChartThemeSet::default())
        .expect("a three-table theme parses");
    assert_eq!(pasted, set);
}

/// `theme.rs:ChartThemeSet::parse_share` must keep the live Obsidian table when the pasted text
/// has none — every theme copied by a build older than the table. Without the restoration serde
/// fills it from the shipped default and a paste of someone's dark/light theme silently resets the
/// user's own Obsidian colours.
#[test]
fn sharing_a_two_table_text_keeps_the_live_obsidian_table() {
    let mut current = ChartThemeSet::default();
    current.obsidian.bg = [3, 17, 251];
    let live_obsidian = current.obsidian.clone();
    let text = two_table_file(
        &ChartTheme {
            bg: [11, 22, 33],
            ..ChartTheme::default()
        },
        &ChartTheme {
            bg: [220, 221, 222],
            ..ChartTheme::default_light()
        },
    );

    let parsed = ChartThemeSet::parse_share(&text, &current).expect("a two-table theme parses");

    assert_eq!(parsed.dark.bg, [11, 22, 33]);
    assert_eq!(parsed.light.bg, [220, 221, 222]);
    assert_eq!(parsed.obsidian, live_obsidian);
    assert_ne!(parsed.obsidian, ChartTheme::default_obsidian());
    assert_eq!(parsed.palette_rev, CURRENT_PALETTE_REV);
}

/// `theme.rs:ChartThemeSet::parse_share` must take the Obsidian table from a text that carries
/// one; restoring the live table unconditionally would make the third table impossible to share.
#[test]
fn sharing_a_three_table_text_replaces_the_live_obsidian_table() {
    let mut current = ChartThemeSet::default();
    current.obsidian.bg = [3, 17, 251];
    let sent = ChartThemeSet {
        obsidian: ChartTheme {
            bg: [40, 41, 42],
            ..ChartTheme::default_obsidian()
        },
        ..ChartThemeSet::default()
    };
    let text = sent
        .to_share_string()
        .expect("a three-table set serializes");

    let parsed = ChartThemeSet::parse_share(&text, &current).expect("a three-table theme parses");

    assert_eq!(parsed.obsidian, sent.obsidian);
}

/// `theme.rs:ChartThemeSet::parse_share` must accept a text carrying only `[obsidian]`, keep both
/// live sides, and keep THEIR generation. Folding this case into the mixed-set rule would stamp a
/// newer live pair down to the current generation although neither side came from the paste.
#[test]
fn sharing_an_obsidian_only_table_keeps_both_live_sides_and_their_generation() {
    let mut current = ChartThemeSet {
        palette_rev: CURRENT_PALETTE_REV + 2,
        ..ChartThemeSet::default()
    };
    current.dark.bg = [251, 17, 3];
    current.light.bg = [3, 17, 251];
    let text = "[obsidian]\nbg = [40, 41, 42]\n";

    let parsed = ChartThemeSet::parse_share(text, &current).expect("an obsidian-only theme parses");

    assert_eq!(parsed.obsidian.bg, [40, 41, 42]);
    assert_eq!(parsed.dark, current.dark);
    assert_eq!(parsed.light, current.light);
    assert_eq!(parsed.palette_rev, current.palette_rev);
}

/// `theme.rs:ChartThemeSet::parse_share` must keep the live Obsidian table when a legacy FLAT
/// theme is pasted: a flat theme is one dark table, and the user's Obsidian colours were never
/// part of it.
#[test]
fn sharing_a_flat_legacy_theme_keeps_the_live_obsidian_table() {
    let flat = ChartTheme {
        bg: [10, 20, 30],
        ..ChartTheme::default()
    };
    let text = toml::to_string_pretty(&flat).expect("flat themes serialize");
    let mut current = ChartThemeSet::default();
    current.obsidian.bg = [3, 17, 251];

    let parsed = ChartThemeSet::parse_share(&text, &current).expect("a flat theme parses");

    assert_eq!(parsed.dark, flat);
    assert_eq!(parsed.obsidian, current.obsidian);
}

/// The palette carry-over belongs to `[dark]` and `[light]` alone. The Obsidian table never
/// shipped a colour that was later retired and was born with its own `hvol_bg`, so on an older
/// generation neither the load path nor the paste path may touch it — running it through
/// `retire_theme` would swap a colour the user picked that happens to equal a retired dark
/// default, and overwrite the profile backdrop they chose with `book_bg`.
#[test]
fn an_older_generation_never_carries_the_obsidian_table_across() {
    let retired = RETIRED_DARK
        .first()
        .expect("the dark migration has a frozen generation");
    let text = format!(
        "palette_rev = 2\n\n[dark]\nbg = [11, 22, 33]\nbook_bg = [7, 8, 9]\n\n[obsidian]\nbg = [40, 41, 42]\nbook_bg = [7, 8, 9]\nhvol_bg = [1, 2, 3]\ncandle_up = {:?}\n",
        retired.candle_up
    );

    let (loaded, write_back) = ChartThemeSet::resolve(Some(&text)).unwrap();
    assert!(write_back, "the dark side did move a generation");
    assert_eq!(
        loaded.dark.hvol_bg,
        [7, 8, 9],
        "the fixture must be old enough for the split to run on the dark side"
    );
    assert_eq!(loaded.obsidian.hvol_bg, [1, 2, 3]);
    assert_eq!(loaded.obsidian.candle_up, retired.candle_up);

    let pasted = ChartThemeSet::parse_share(&text, &ChartThemeSet::default())
        .expect("an older dark-and-obsidian theme parses");
    assert_eq!(pasted.dark.hvol_bg, [7, 8, 9]);
    assert_eq!(pasted.obsidian.hvol_bg, [1, 2, 3]);
    assert_eq!(pasted.obsidian.candle_up, retired.candle_up);
}
