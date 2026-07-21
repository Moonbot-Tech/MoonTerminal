use super::super::schema::{default_ui_font_delta, default_ui_scale, ServersFile, SettingsFile};
use super::{merge, Merged};

/// Merge a settings file carrying nothing but the two scaling knobs.
fn merged_with(ui_scale: f32, ui_font_delta: f32) -> Merged {
    merge(
        ServersFile::default(),
        SettingsFile {
            ui_scale,
            ui_font_delta,
            ..Default::default()
        },
        None,
    )
}

/// Pins the `repair_ui_scale` CALL inside [`merge`], not the repair itself — a pure repair
/// function nobody invokes is exactly how this regresses. The plausible edit: someone reads
/// `ui_scale` back as a plain passthrough, matching its unrepaired neighbours on either side.
///
/// A stored `ui_scale = 0.0` is not hypothetical — it is what every `settings.toml` written
/// before the loader applied schema defaults contains. `MoonThemeTokens::ui` floors the factor
/// at `0.25`, so honouring the zero renders the whole interface at a quarter size: text still
/// paints, every hit rectangle shrinks past the point where clicks land, and the Settings
/// screen the user would repair it from is itself unusable. Loading has to fix it.
#[test]
fn a_degenerate_stored_ui_scale_is_repaired_on_load() {
    for broken in [0.0_f32, -1.0, f32::NAN, f32::INFINITY] {
        assert_eq!(
            merged_with(broken, 0.0).ui_scale,
            default_ui_scale(),
            "a scale of {broken} cannot mean anything; loading must repair it, not pass it on"
        );
    }
}

/// The other half of the contract, and the half that is easy to break while "hardening" the
/// first: repair must not become a range clamp.
///
/// `ui_scale` has no settings-UI control, so hand-editing `settings.toml` is the only way to
/// set it — and the loaded value is written straight back by the next `save()`. A clamp would
/// therefore not just ignore an unusual choice, it would DESTROY it on disk, with nothing in
/// the UI to restore it from. `0.25` is MoonUI's own floor in `MoonThemeTokens::ui` and `6.0`
/// is far past any preset; both are usable, so both must survive untouched.
#[test]
fn an_unusual_but_usable_scale_survives_the_load() {
    for kept in [0.25_f32, 0.4, 6.0, 10.0] {
        assert_eq!(
            merged_with(kept, 0.0).ui_scale,
            kept,
            "a usable scale of {kept} must load verbatim; repair is not a clamp"
        );
    }
}

/// `ui_font_delta` splits the other way from `ui_scale`: `0.0` means "no adjustment" and is a
/// real choice, while a non-finite value is not — TOML parses `inf`/`nan`, and MoonUI adds
/// this delta directly into text metrics, where an infinity spreads into layout dimensions.
#[test]
fn a_non_finite_font_delta_is_repaired_while_zero_is_kept() {
    for broken in [f32::INFINITY, f32::NEG_INFINITY, f32::NAN] {
        assert_eq!(
            merged_with(1.0, broken).ui_font_delta,
            default_ui_font_delta(),
            "a font delta of {broken} reaches MoonUI text metrics; it must be repaired"
        );
    }
    assert_eq!(
        merged_with(1.0, 0.0).ui_font_delta,
        0.0,
        "zero font delta is 'no adjustment', a legitimate choice — it must NOT be repaired"
    );
}
