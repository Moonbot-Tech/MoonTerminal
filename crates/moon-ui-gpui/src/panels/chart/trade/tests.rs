// НЕ `use super::*`: он затащит `gpui::test` (attr-макрос из `use gpui::*` родителя),
// и `#[test]` перестанет быть std-тестом.
use super::hover_probe_due;

/// Норматив горячего пути MouseMove (docs-internal/INPUT_HOTPATH_NORMS.md):
/// hit-test линий запускается только при реальном сдвиге курсора ≥1px X или ≥0.5px Y.
#[test]
fn hover_probe_threshold_matches_delphi() {
    // Первый заход — всегда пересчёт.
    assert!(hover_probe_due(None, (10.0, 10.0)));
    // Суб-пиксельный дрожащий MouseMove — НЕ пересчитываем.
    assert!(!hover_probe_due(Some((10.0, 10.0)), (10.0, 10.0)));
    assert!(!hover_probe_due(Some((10.0, 10.0)), (10.9, 10.4)));
    // Сдвиг ≥1px по X ИЛИ ≥0.5px по Y — пересчёт.
    assert!(hover_probe_due(Some((10.0, 10.0)), (11.0, 10.0)));
    assert!(hover_probe_due(Some((10.0, 10.0)), (10.0, 10.5)));
    assert!(hover_probe_due(Some((10.0, 10.0)), (9.0, 10.0)));
    assert!(hover_probe_due(Some((10.0, 10.0)), (10.0, 9.5)));
}
