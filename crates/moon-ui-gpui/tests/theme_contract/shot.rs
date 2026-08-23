//! Static contracts for the chart shot's privacy, rendering, and clipboard boundaries.

use super::support::*;

/// `shot/header.rs` must not reach for an account/core name; otherwise a shared chart image
/// reintroduces the private label that caption substitution deliberately removed.
#[test]
fn the_burnt_in_header_never_reaches_for_the_core_name() {
    let source = code_only(&read_src("panels/chart/shot/header.rs"));
    for banned in ["core_name", "core_label"] {
        assert!(
            !source.contains(banned),
            "shot/header.rs must not mention {banned:?}: a shared screenshot must not expose the account name"
        );
    }
}

/// `ChartPanel::shot_inputs` must select the chart ticker and venue rather than an account label;
/// otherwise a harmless-looking metadata substitution leaks the user's core name into every shared
/// screenshot while leaving the header formatter unchanged.
#[test]
fn the_shot_input_snapshot_selects_the_ticker_and_venue_not_a_core_name() {
    let source = read_src("panels/chart/mod.rs");
    let inputs = code_only(braced_body(&source, "pub(crate) fn shot_inputs("));

    assert!(inputs.contains("pane_ticker"));
    assert!(inputs.contains("venue_section_label"));
    assert!(!inputs.contains("core_name"));
    assert!(!inputs.contains("core_label"));
    assert!(!inputs.contains(".name("));
}

/// `shot/header.rs:window_field` must render range magnitudes without a sign; otherwise the
/// screenshot asserts a market direction that the supplied data does not contain.
#[test]
fn the_header_prints_window_moves_unsigned() {
    let source = code_only(&read_src("panels/chart/shot/header.rs"));
    assert!(source.contains("fmt::pct("));
    assert!(!source.contains("signed_pct"));
}

/// `shot/header.rs`, `shot/ink.rs`, and `shot/paint_win.rs` must not carry `DeltaSign`; otherwise
/// an unsigned market range can acquire a directional colour in a shared chart screenshot.
#[test]
fn the_header_pipeline_has_no_directional_delta_sign() {
    for path in [
        "panels/chart/shot/header.rs",
        "panels/chart/shot/ink.rs",
        "panels/chart/shot/paint_win.rs",
    ] {
        let source = code_only(&read_src(path));
        assert!(
            !source.contains("DeltaSign"),
            "{path} must not carry a directional sign into unsigned screenshot ranges"
        );
    }
}

/// `shot/paint_win.rs:draw_strips` must take all strip geometry from `resize`; otherwise a second
/// formula drifts from the reserved height and the messenger recompresses the final screenshot.
#[test]
fn the_windows_painter_uses_only_resize_owned_strip_geometry() {
    let source = read_src("panels/chart/shot/paint_win.rs");
    let draw_strips = code_only(braced_body(&source, "pub(super) fn draw_strips("));

    for required in [
        "super::resize::lead_px(base_px)",
        "super::resize::strip_pad(base_px)",
        "super::resize::strip_height(base_px)",
        "super::resize::HAIRLINE_PX",
    ] {
        assert!(
            draw_strips.contains(required),
            "draw_strips must use {required}"
        );
    }
    for forbidden in ["FONT_DIVISOR", "LEAD_NUM", "LEAD_DEN", "STRIP_PADDING"] {
        assert!(
            !draw_strips.contains(forbidden),
            "draw_strips must not recompute resize geometry through {forbidden}"
        );
    }
}

/// `shot/paint_win.rs:compose` must select baseline text placement and discard the old top-centred
/// formula; otherwise mixed-size runs sit at different vertical positions in the header strip.
#[test]
fn the_windows_painter_uses_one_shared_text_baseline() {
    let source = code_only(&read_src("panels/chart/shot/paint_win.rs"));
    let compose = braced_body(&source, "fn compose(");

    assert!(compose.contains("TA_BASELINE"));
    assert!(!source.contains("(layout.strip_h - layout.font_px) / 2"));
}

/// `shot/header.rs:scale_field` must keep the chart badge's exact `<1%` spelling; otherwise the
/// screenshot contradicts the scale badge already visible inside the chart it shares.
#[test]
fn the_header_and_chart_badge_share_the_sub_percent_spelling() {
    let header = read_src("panels/chart/shot/header.rs");
    let labels = read_src("chartdx/text/labels.rs");
    let header_scale = code_only(braced_body(&header, "fn scale_field("));
    let label_scale = code_only(chain_between(
        &labels,
        "ChartLabelField::ScaleBadge =>",
        "ChartLabelField::",
        "scale badge",
    ));
    let header_line = header_scale
        .lines()
        .find(|line| line.contains("<1%"))
        .expect("header scale spelling");
    let label_line = label_scale
        .lines()
        .find(|line| line.contains("<1%"))
        .expect("chart badge spelling");

    assert_eq!(header_line.trim(), label_line.trim());
}

/// `shot/caption.rs:finish` must restore the caption before notifying the user; otherwise the
/// substituted venue can remain visibly armed after a chart-shot path completes.
#[test]
fn the_caption_is_restored_before_the_user_is_told() {
    let source = code_only(&read_src("panels/chart/shot/caption.rs"));
    let finish = braced_body(&source, "fn finish(");
    let restore = finish
        .find("arm_shot_caption(None")
        .expect("finish restores the caption");
    let notification = finish
        .find("push_notification")
        .expect("finish tells the user");

    assert!(restore < notification);
}

/// `shot/win.rs:capture_client_rect` must remain a desktop read; otherwise strip composition can
/// blur the audit boundary around the exact pixels and privacy substitution being captured.
#[test]
fn the_desktop_capture_still_only_reads_and_never_draws() {
    let source = code_only(&read_src("panels/chart/shot/win.rs"));
    let capture = braced_body(&source, "fn capture_client_rect(");

    assert!(capture.contains("SRCCOPY | CAPTUREBLT"));
    for drawing in ["TextOutW", "DrawTextW", "CreateFontIndirectW", "FillRect"] {
        assert!(
            !capture.contains(drawing),
            "capture must not call {drawing}"
        );
    }
}

/// `shot` production sources must never write a file; otherwise a future disk-save path revives
/// private local artifacts instead of keeping a completed chart shot solely on the clipboard.
#[test]
fn the_shot_never_writes_a_file() {
    let shot_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/panels/chart/shot");
    let mut sources = Vec::new();
    rust_sources(&shot_dir, &mut sources);
    for path in sources {
        if path.components().any(|part| part.as_os_str() == "tests")
            || path.file_name().is_some_and(|name| name == "tests.rs")
        {
            continue;
        }
        let source = code_only(
            &fs::read_to_string(&path)
                .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display())),
        );
        for banned in [
            "shots_dir",
            "save::",
            "OpenOptions",
            "create_dir_all",
            "write_all",
            "background_executor",
        ] {
            assert!(
                !source.contains(banned),
                "{} must not mention {banned:?}: a chart shot belongs only on the clipboard",
                path.display()
            );
        }
    }

    let paths = code_only(&read_src("../../moon-core/src/config/paths.rs"));
    assert!(!paths.contains("fn shots_dir("));
}

/// `shot/paint_win.rs:write_strip` must use the extracted centring calculation; otherwise a later
/// simplification starts the header at the left inset and shared charts visibly lose centred text.
#[test]
fn the_header_run_is_centred_rather_than_left_inset() {
    let source = read_src("panels/chart/shot/paint_win.rs");
    let write_strip = code_only(braced_body(&source, "fn write_strip("));

    assert!(write_strip.contains("centred_start_x("));
    assert!(!write_strip.contains("let mut x = inset"));
}

/// `shot/paint_win.rs` must compose one header strip; otherwise restoring the second strip makes
/// the final chart too tall for the messenger and causes another destructive resample.
#[test]
fn the_composition_burns_in_one_strip() {
    let source = read_src("panels/chart/shot/paint_win.rs");
    let draw_strips = code_only(braced_body(&source, "pub(super) fn draw_strips("));
    let compose = code_only(braced_body(&source, "fn compose("));

    assert_eq!(draw_strips.matches(".checked_add(strip_h)").count(), 1);
    assert_eq!(compose.matches("write_strip(").count(), 1);
}

/// `shot/resize.rs:normalize` must not enlarge a small image; otherwise the only path intended to
/// preserve a lossless small capture creates a larger, softer PNG instead.
#[test]
fn the_size_rule_only_ever_shrinks() {
    let source = code_only(&read_src("panels/chart/shot/resize.rs"));
    let normalize = braced_body(&source, "pub(super) fn normalize(");

    assert!(normalize.contains("fitted(frame.width, frame.height)"));
}
