use super::super::schema::{default_ui_font_delta, default_ui_scale, SettingsFile};
use super::{load_or_default, load_or_default_status, ConfigLoad};
use std::path::{Path, PathBuf};

/// Bind the absent-file branch in [`load_or_default`] to `defaults_for_absent_file`.
///
/// The plausible breakage is treating `defaults_for_absent_file` as a redundant wrapper and
/// replacing it with `T::default()`. This compiles, but `#[serde(default = "...")]` applies
/// only during deserialization, so derived `Default` zeroes fields that have real serde
/// defaults; `ui_scale = 0.0` leaves visible UI without hit targets.
///
/// The oracle is independent of the loader: `schema::default_*` are the same functions used by
/// the fresh-config branch in `config::mod`, so the two paths cannot drift apart.
#[test]
fn an_absent_file_yields_the_schema_defaults_not_zeroes() {
    let missing = PathBuf::from("no-such-dir-4f21c8/no-such-settings.toml");
    assert!(
        !missing.exists(),
        "the fixture path must genuinely not exist"
    );

    let cfg: SettingsFile = load_or_default(&missing, "settings.toml", |_| {
        panic!("on_corrupt must not fire for a file that is merely absent")
    });

    assert_eq!(
        cfg.ui_scale,
        default_ui_scale(),
        "an absent settings.toml must load the schema default scale, not f32::default()"
    );
    assert_eq!(
        cfg.ui_font_delta,
        default_ui_font_delta(),
        "an absent settings.toml must load the schema default font delta, not f32::default()"
    );
}

/// An existing but UNREADABLE file must not be treated as absent.
///
/// A construction such as `let Ok(text) = read_to_string(..) else { default }` maps every
/// error, including access denial, a sharing violation, or an unhydrated cloud placeholder, to
/// "first launch." The resulting `version = 0` marks the config dirty and triggers write-back
/// in `AppConfig::load`, which would replace a valid `settings.toml` with defaults.
///
/// The plausible breakage is collapsing the `match` into a shorter `let Ok(..) else`. On a
/// machine where the file reads normally, both versions behave identically and hide the risk.
///
/// A directory is a portable way to obtain a read error other than `NotFound`: no platform
/// reports `NotFound` for an existing path that is not a file.
#[test]
fn an_unreadable_file_is_not_reported_as_absent() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    assert!(dir.is_dir(), "the fixture must be a directory that exists");

    let (_cfg, status) = load_or_default_status::<SettingsFile>(&dir, "settings.toml", |_| {
        panic!("on_corrupt must not fire for a file that could not be read at all")
    });

    assert_eq!(
        status,
        ConfigLoad::Unreadable,
        "a path that exists but does not read must report Unreadable, never Absent"
    );
}

/// A genuinely missing file reports `Absent` so first launch can save defaults.
#[test]
fn a_missing_file_reports_absent() {
    let missing = Path::new("no-such-dir-4f21c8/no-such-settings.toml");
    assert!(
        !missing.exists(),
        "the fixture path must genuinely not exist"
    );

    let (_cfg, status) = load_or_default_status::<SettingsFile>(missing, "settings.toml", |_| {
        panic!("on_corrupt must not fire for a file that is merely absent")
    });

    assert_eq!(status, ConfigLoad::Absent);
}
