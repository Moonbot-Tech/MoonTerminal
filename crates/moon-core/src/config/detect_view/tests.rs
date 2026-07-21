use super::*;

/// КАЖДОЕ поле конструктора переживает toml-roundtrip (то, чем пишется/читается
/// detects_view.toml и Копировать/Вставить): активный размер, per-размер
/// w/h/chart/rail и все флаги каждого слота.
#[test]
fn detect_view_roundtrip_preserves_every_field() {
    let mut cfg = DetectViewCfg::default();
    cfg.size = DETECT_SIZE_LARGE;
    cfg.delta_decimals = 0;
    cfg.mini.w = 77;
    cfg.mini.h = 33;
    cfg.mini.chart = DetectChart::Line;
    cfg.mini.rail_w = 5;
    cfg.mini.rail_grad = 61;
    cfg.medium.chart = DetectChart::None;
    for (i, slot) in cfg.large.slots.iter_mut().enumerate() {
        slot.field = DetectField::ALL[i % DetectField::ALL.len()];
        slot.over = i % 2 == 0;
        slot.right = i % 3 == 0;
        slot.below = i % 4 == 0;
    }

    // Общий формат (Копировать/Вставить = одна группа).
    let text = cfg.to_share_string().expect("serialize");
    let back = DetectViewCfg::parse_share(&text).expect("parse");
    assert_eq!(cfg, back);

    // Файл целиком (в т.ч. пустое имя группы — дефолтная группа окна).
    let mut file = DetectViewFile::default();
    file.set_group("", cfg);
    file.set_group("Группа 2", DetectViewCfg::default());
    let text = toml::to_string_pretty(&file).expect("serialize file");
    let back: DetectViewFile = toml::from_str(&text).expect("parse file");
    assert_eq!(back.group(""), cfg);
    assert_eq!(back.group("Группа 2"), DetectViewCfg::default());
    // Незнакомая группа → дефолт.
    assert_eq!(back.group("нет такой"), DetectViewCfg::default());
}

/// Частичная запись (старый/чужой файл без новых полей) добивается дефолтами,
/// а мусор не валит парс всего файла в load_or_default-цепочке.
#[test]
fn detect_view_partial_toml_fills_defaults() {
    let cfg: DetectViewCfg =
        toml::from_str("size = 2\n[medium]\nw = 150\n").expect("partial parse");
    assert_eq!(cfg.size, DETECT_SIZE_LARGE);
    assert_eq!(cfg.medium.w, 150);
    // Остальное — из дефолтов.
    assert_eq!(cfg.mini, DetectViewCfg::default().mini);
    assert_eq!(cfg.medium.h, DetectViewCfg::default().medium.h);
}
