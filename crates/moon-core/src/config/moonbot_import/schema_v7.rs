//! Схема распакованного контейнера `MBSP` v7: заголовок + блоки `(kind u8, size u32 LE)`.
//! Читаем UI v3 (kind 6), Theme v1 (kind 4), Ini v1 (kind 5); Signals/Trading/Visual
//! (kind 1/2/3) обязаны присутствовать, но их содержимое пропускается целиком (ТЗ §10).
//! Непрочитанный хвост ИЗВЕСТНОГО блока разрешён (append-only той же версии);
//! неизвестный kind пропускается по size; повтор известного блока — ошибка.

use super::reader::{IniSection, Reader};
use super::ImportError;

/// Действия 27 позиционных shortcut-слотов UI-блока (порядок — строго как в ТЗ §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutAction {
    CancelBuy,
    PanicSell,
    JoinSells,
    SwitchCharts,
    ReloadBook,
    NewLong,
    NewShort,
    SplitOrder,
    ShiftBuyUp,
    ShiftBuyDown,
    ShiftSellUp,
    ShiftSellDown,
    MakeShot,
    MakeShotBot,
    ReloadChart,
    ScalePlus,
    ScaleMinus,
    SellPlus,
    SellMinus,
    SpyMode,
    ShowCharts,
    SplitOrderX,
    SwitchFigure,
    FitSells,
    PanicSellOne,
    CancelAllBuys,
    Broadcast,
}

/// Все 27 действий в wire-порядке.
pub const SHORTCUT_ACTIONS: [ShortcutAction; 27] = [
    ShortcutAction::CancelBuy,
    ShortcutAction::PanicSell,
    ShortcutAction::JoinSells,
    ShortcutAction::SwitchCharts,
    ShortcutAction::ReloadBook,
    ShortcutAction::NewLong,
    ShortcutAction::NewShort,
    ShortcutAction::SplitOrder,
    ShortcutAction::ShiftBuyUp,
    ShortcutAction::ShiftBuyDown,
    ShortcutAction::ShiftSellUp,
    ShortcutAction::ShiftSellDown,
    ShortcutAction::MakeShot,
    ShortcutAction::MakeShotBot,
    ShortcutAction::ReloadChart,
    ShortcutAction::ScalePlus,
    ShortcutAction::ScaleMinus,
    ShortcutAction::SellPlus,
    ShortcutAction::SellMinus,
    ShortcutAction::SpyMode,
    ShortcutAction::ShowCharts,
    ShortcutAction::SplitOrderX,
    ShortcutAction::SwitchFigure,
    ShortcutAction::FitSells,
    ShortcutAction::PanicSellOne,
    ShortcutAction::CancelAllBuys,
    ShortcutAction::Broadcast,
];

/// Сырые `TShortCut`-значения 27 слотов (индекс = позиция в [`SHORTCUT_ACTIONS`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcuts(pub [u16; 27]);

impl Shortcuts {
    pub fn get(&self, action: ShortcutAction) -> u16 {
        let idx = SHORTCUT_ACTIONS
            .iter()
            .position(|a| *a == action)
            .expect("action присутствует в SHORTCUT_ACTIONS");
        self.0[idx]
    }
}

/// `HotkeysPublic` из UI-блока: пресеты размера ордера, fixed-sell и shortcut-слоты.
#[derive(Debug, Clone, PartialEq)]
pub struct HotkeysPublic {
    pub filled: bool,
    pub ver: u8,
    /// Шесть размеров ручного ордера (`OSize`).
    pub order_sizes: [f64; 6],
    /// Выбранный слот размера (`bNum`); валидный диапазон 0..=5 проверяет план.
    pub order_size_sel: i32,
    /// Хоткеи слотов размера (`OKeys`), сырые `TShortCut`.
    pub order_size_keys: [u16; 6],
    pub split_parts: u8,
    /// Выбранный fixed-sell слот (`sbNum`); диапазон проверяет план.
    pub fixed_sell_sel: u8,
    /// Хоткеи fixed-sell слотов (`SKeys`), сырые `TShortCut`.
    pub fixed_sell_keys: [u16; 6],
    /// Fixed-sell проценты (`SPrice`).
    pub fixed_sell_prices: [f32; 6],
    pub shortcuts: Shortcuts,
}

/// `MarketsTablePublic`: сортировка и раскладка 41 колонки таблицы рынков.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketsTable {
    pub sort_col: i32,
    pub col_visible: [bool; 41],
    pub col_pos: [u8; 41],
}

/// Блок UI v3 (kind 6) — основной для первой версии импортера.
#[derive(Debug, Clone, PartialEq)]
pub struct UiBlock {
    pub hide_demo_button: bool,
    pub confirm_close: bool,
    pub new_markets_on_top: bool,
    pub coins_sort_order: i32,
    pub hotkeys: HotkeysPublic,
    pub strat_editor_chapters: String,
    pub markets_table: MarketsTable,
    pub main_buttons_index: u8,
    pub strat_expanded: [bool; 11],
}

/// Блок Theme v1 (kind 4): текущий стиль + INI-секции цветов обеих тем.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeBlock {
    pub current_style: i32,
    pub sections: Vec<IniSection>,
}

impl ThemeBlock {
    /// `CurrentStyle` 3 и 4 — тёмная тема, остальные известные — светлая (ТЗ §8).
    pub fn is_dark(&self) -> bool {
        matches!(self.current_style, 3 | 4)
    }

    pub fn section(&self, name: &str) -> Option<&IniSection> {
        self.sections.iter().find(|s| s.name == name)
    }

    pub fn colors_light(&self) -> Option<&IniSection> {
        self.section("ColorsLight")
    }

    pub fn colors_dark(&self) -> Option<&IniSection> {
        self.section("ColorsDark")
    }
}

/// Блок Ini v1 (kind 5): секции `Charts`, `ArbColors`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IniBlock {
    pub sections: Vec<IniSection>,
}

impl IniBlock {
    pub fn section(&self, name: &str) -> Option<&IniSection> {
        self.sections.iter().find(|s| s.name == name)
    }
}

/// Прочитанная модель payload MoonBot (без интерпретации — её делает план).
#[derive(Debug, Clone, PartialEq)]
pub struct MoonBotConfig {
    /// `ConfigVersion` MoonBot из заголовка — только диагностика, НЕ версия схемы Terminal.
    pub config_version: u16,
    pub ui: UiBlock,
    pub theme: ThemeBlock,
    pub ini: IniBlock,
}

/// Версия контейнера, которую понимает этот reader.
const SUPPORTED_CONTAINER: u8 = 7;
/// Известные блоки: kind → обязан присутствовать ровно один раз.
const KNOWN_KINDS: [u8; 6] = [1, 2, 3, 4, 5, 6];

/// Разбор распакованного payload: заголовок `MBSP` + все блоки.
pub fn parse_payload(payload: &[u8]) -> Result<MoonBotConfig, ImportError> {
    let mut r = Reader::new(payload);

    let magic = [
        r.u8("magic")?,
        r.u8("magic")?,
        r.u8("magic")?,
        r.u8("magic")?,
    ];
    if magic != *b"MBSP" {
        return Err(ImportError::BadHeader("нет сигнатуры MBSP".into()));
    }
    let container_ver = r.u8("версия контейнера")?;
    if container_ver > SUPPORTED_CONTAINER {
        return Err(ImportError::NewerFormat {
            found: container_ver as u32,
        });
    }
    if container_ver != SUPPORTED_CONTAINER {
        return Err(ImportError::BadHeader(format!(
            "версия контейнера {container_ver}, поддерживается {SUPPORTED_CONTAINER}"
        )));
    }
    let config_version = r.u16_le("ConfigVersion")?;

    let mut seen = [false; 256];
    let mut ui: Option<UiBlock> = None;
    let mut theme: Option<ThemeBlock> = None;
    let mut ini: Option<IniBlock> = None;

    while !r.is_empty() {
        let kind = r.u8("kind блока")?;
        let size = r.u32_le("size блока")? as usize;
        let sub = r.sub_reader(size, &format!("тело блока kind={kind}"))?;
        if KNOWN_KINDS.contains(&kind) {
            if seen[kind as usize] {
                return Err(ImportError::BadValue(format!(
                    "блок kind={kind} встречается дважды"
                )));
            }
            seen[kind as usize] = true;
        }
        match kind {
            // Signals/Trading/Visual: присутствие обязательно, содержимое пропускаем.
            1 | 2 | 3 => {}
            4 => theme = Some(parse_theme(sub)?),
            5 => ini = Some(parse_ini(sub)?),
            6 => ui = Some(parse_ui(sub)?),
            // Неизвестный kind (например, будущий Interop до его поддержки) — пропуск.
            _ => {}
        }
    }

    for kind in KNOWN_KINDS {
        if !seen[kind as usize] {
            return Err(ImportError::Truncated(format!(
                "отсутствует обязательный блок kind={kind}"
            )));
        }
    }
    // seen гарантирует, что 4/5/6 были ровно по разу — unwrap'ы безопасны, но без паники:
    match (ui, theme, ini) {
        (Some(ui), Some(theme), Some(ini)) => Ok(MoonBotConfig {
            config_version,
            ui,
            theme,
            ini,
        }),
        _ => Err(ImportError::Truncated(
            "внутренняя ошибка: блок отмечен, но не разобран".into(),
        )),
    }
}

/// Блок UI v3 — строго позиционный (ТЗ §6). Хвост блока (append-only) не читаем.
fn parse_ui(mut r: Reader) -> Result<UiBlock, ImportError> {
    let ver = r.u8("UI.version")?;
    if ver != 3 {
        return Err(ImportError::BadValue(format!(
            "UI.version = {ver}, поддерживается 3"
        )));
    }
    let hide_demo_button = r.bool("UI.HideDemoButton")?;
    let confirm_close = r.bool("UI.ConfirmClose")?;
    let new_markets_on_top = r.bool("UI.NewMarketsOnTop")?;
    let coins_sort_order = r.i32_le("UI.CoinsSortOrder")?;
    let hotkeys = parse_hotkeys(&mut r)?;
    let strat_editor_chapters = r.string_x("UI.StratEditorChapters")?;
    let markets_table = parse_markets_table(&mut r)?;
    let main_buttons_index = r.u8("UI.MainButtonsIndex")?;
    let mut strat_expanded = [false; 11];
    for (i, slot) in strat_expanded.iter_mut().enumerate() {
        *slot = r.bool(&format!("UI.StratExpandedState[{i}]"))?;
    }
    Ok(UiBlock {
        hide_demo_button,
        confirm_close,
        new_markets_on_top,
        coins_sort_order,
        hotkeys,
        strat_editor_chapters,
        markets_table,
        main_buttons_index,
        strat_expanded,
    })
}

fn parse_hotkeys(r: &mut Reader) -> Result<HotkeysPublic, ImportError> {
    let filled = r.bool("Hotkeys.Filled")?;
    let ver = r.u8("Hotkeys.ver")?;
    let mut order_sizes = [0f64; 6];
    for (i, s) in order_sizes.iter_mut().enumerate() {
        *s = r.f64_finite(&format!("Hotkeys.OSize[{i}]"))?;
    }
    let order_size_sel = r.i32_le("Hotkeys.bNum")?;
    let mut order_size_keys = [0u16; 6];
    for (i, k) in order_size_keys.iter_mut().enumerate() {
        *k = r.u16_le(&format!("Hotkeys.OKeys[{i}]"))?;
    }
    let split_parts = r.u8("Hotkeys.SplitParts")?;
    let fixed_sell_sel = r.u8("Hotkeys.sbNum")?;
    let mut fixed_sell_keys = [0u16; 6];
    for (i, k) in fixed_sell_keys.iter_mut().enumerate() {
        *k = r.u16_le(&format!("Hotkeys.SKeys[{i}]"))?;
    }
    let mut fixed_sell_prices = [0f32; 6];
    for (i, p) in fixed_sell_prices.iter_mut().enumerate() {
        *p = r.f32_finite(&format!("Hotkeys.SPrice[{i}]"))?;
    }
    let mut shortcuts = [0u16; 27];
    for (i, s) in shortcuts.iter_mut().enumerate() {
        *s = r.u16_le(&format!("Hotkeys.shortcut[{i}]"))?;
    }
    Ok(HotkeysPublic {
        filled,
        ver,
        order_sizes,
        order_size_sel,
        order_size_keys,
        split_parts,
        fixed_sell_sel,
        fixed_sell_keys,
        fixed_sell_prices,
        shortcuts: Shortcuts(shortcuts),
    })
}

fn parse_markets_table(r: &mut Reader) -> Result<MarketsTable, ImportError> {
    let sort_col = r.i32_le("MarketsTable.SortCol")?;
    let mut col_visible = [false; 41];
    for (i, v) in col_visible.iter_mut().enumerate() {
        *v = r.bool(&format!("MarketsTable.ColVis[{i}]"))?;
    }
    let mut col_pos = [0u8; 41];
    for (i, p) in col_pos.iter_mut().enumerate() {
        *p = r.u8(&format!("MarketsTable.ColPos[{i}]"))?;
    }
    Ok(MarketsTable {
        sort_col,
        col_visible,
        col_pos,
    })
}

fn parse_theme(mut r: Reader) -> Result<ThemeBlock, ImportError> {
    let ver = r.u8("Theme.version")?;
    if ver != 1 {
        return Err(ImportError::BadValue(format!(
            "Theme.version = {ver}, поддерживается 1"
        )));
    }
    let current_style = r.i32_le("Theme.CurrentStyle")?;
    let sections = r.ini_list("Theme")?;
    Ok(ThemeBlock {
        current_style,
        sections,
    })
}

fn parse_ini(mut r: Reader) -> Result<IniBlock, ImportError> {
    let ver = r.u8("Ini.version")?;
    if ver != 1 {
        return Err(ImportError::BadValue(format!(
            "Ini.version = {ver}, поддерживается 1"
        )));
    }
    let sections = r.ini_list("Ini")?;
    Ok(IniBlock { sections })
}

// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
pub(super) mod build {
    //! Тест-билдер бинарного payload (двойник Delphi-writer'а). Используется юнитами
    //! схемы и транспорта; позже — золотыми фикстурами.

    pub fn push_string(out: &mut Vec<u8>, s: &str) {
        let units: Vec<u16> = s.encode_utf16().collect();
        out.extend_from_slice(&(units.len() as i32).to_le_bytes());
        for u in units {
            out.extend_from_slice(&u.to_le_bytes());
        }
    }

    pub fn block(out: &mut Vec<u8>, kind: u8, body: &[u8]) {
        out.push(kind);
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
    }

    /// Тело UI v3 с заданными hotkeys-значениями (остальное — фиксированные значения).
    pub fn ui_body(
        order_sizes: [f64; 6],
        order_size_keys: [u16; 6],
        fixed_sell_keys: [u16; 6],
        fixed_sell_prices: [f32; 6],
    ) -> Vec<u8> {
        let mut b = Vec::new();
        b.push(3u8); // version
        b.push(0u8); // HideDemoButton
        b.push(1u8); // ConfirmClose
        b.push(0u8); // NewMarketsOnTop
        b.extend_from_slice(&5i32.to_le_bytes()); // CoinsSortOrder
                                                  // HotkeysPublic
        b.push(1u8); // Filled
        b.push(2u8); // ver
        for s in order_sizes {
            b.extend_from_slice(&s.to_le_bytes());
        }
        b.extend_from_slice(&2i32.to_le_bytes()); // bNum
        for k in order_size_keys {
            b.extend_from_slice(&k.to_le_bytes());
        }
        b.push(2u8); // SplitParts
        b.push(1u8); // sbNum
        for k in fixed_sell_keys {
            b.extend_from_slice(&k.to_le_bytes());
        }
        for p in fixed_sell_prices {
            b.extend_from_slice(&p.to_le_bytes());
        }
        for i in 0..27u16 {
            // 27 shortcut-слотов: детерминированные значения (слот 0 пустой).
            let v = if i == 0 {
                0
            } else {
                0x2000 | (0x70 + (i % 12))
            };
            b.extend_from_slice(&v.to_le_bytes());
        }
        push_string(&mut b, "chapters");
        // MarketsTablePublic
        b.extend_from_slice(&3i32.to_le_bytes()); // SortCol
        for i in 0..41 {
            b.push((i % 2 == 0) as u8); // ColVis
        }
        for i in 0..41u8 {
            b.push(i); // ColPos
        }
        b.push(4u8); // MainButtonsIndex
        for i in 0..11 {
            b.push((i % 2 == 1) as u8); // StratExpandedState
        }
        b
    }

    pub fn theme_body() -> Vec<u8> {
        let mut b = Vec::new();
        b.push(1u8); // version
        b.extend_from_slice(&3i32.to_le_bytes()); // CurrentStyle (dark)
        b.extend_from_slice(&2i32.to_le_bytes()); // section_count
        push_string(&mut b, "ColorsLight");
        b.extend_from_slice(&1i32.to_le_bytes());
        push_string(&mut b, "graphBK");
        push_string(&mut b, "16777215");
        push_string(&mut b, "ColorsDark");
        b.extend_from_slice(&1i32.to_le_bytes());
        push_string(&mut b, "graphBK");
        push_string(&mut b, "1973790");
        b
    }

    pub fn ini_body() -> Vec<u8> {
        let mut b = Vec::new();
        b.push(1u8); // version
        b.extend_from_slice(&2i32.to_le_bytes());
        push_string(&mut b, "Charts");
        b.extend_from_slice(&1i32.to_le_bytes());
        push_string(&mut b, "CandleGreen");
        push_string(&mut b, "65280");
        push_string(&mut b, "ArbColors");
        b.extend_from_slice(&0i32.to_le_bytes());
        b
    }

    /// Полный корректный payload с шестью блоками (+опционально лишние байты в хвосте UI).
    pub fn full_payload(ui_tail_extra: &[u8]) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend_from_slice(b"MBSP");
        p.push(7u8);
        p.extend_from_slice(&1234u16.to_le_bytes()); // ConfigVersion
        block(&mut p, 1, &[0xAA; 3]); // Signals — содержимое не читаем
        block(&mut p, 2, &[0xBB; 5]); // Trading — пропуск
        block(&mut p, 3, &[]); // Visual — пустой допустим (не читаем)
        block(&mut p, 4, &theme_body());
        block(&mut p, 5, &ini_body());
        let mut ui = ui_body(
            [100.0, 500.0, 1000.0, 5000.0, 10000.0, 50000.0],
            [0x70, 0x71, 0x72, 0x73, 0x74, 0x75], // F1..F6
            [
                0x2000 | 0x76,
                0x2000 | 0x77,
                0x2000 | 0x78,
                0x2000 | 0x79,
                0x2000 | 0x7A,
                0x2000 | 0x7B,
            ], // Shift+F7..F12
            [1.0, 5.0, 10.0, 25.0, 50.0, 100.0],
        );
        ui.extend_from_slice(ui_tail_extra);
        block(&mut p, 6, &ui);
        p
    }
}

#[cfg(test)]
mod tests {
    use super::build::*;
    use super::*;

    #[test]
    fn parses_full_payload() {
        let cfg = parse_payload(&full_payload(&[])).unwrap();
        assert_eq!(cfg.config_version, 1234);
        let h = &cfg.ui.hotkeys;
        assert_eq!(
            h.order_sizes,
            [100.0, 500.0, 1000.0, 5000.0, 10000.0, 50000.0]
        );
        assert_eq!(h.order_size_sel, 2);
        assert_eq!(h.order_size_keys, [0x70, 0x71, 0x72, 0x73, 0x74, 0x75]);
        assert_eq!(h.fixed_sell_sel, 1);
        assert_eq!(h.fixed_sell_prices, [1.0, 5.0, 10.0, 25.0, 50.0, 100.0]);
        assert_eq!(h.shortcuts.get(ShortcutAction::CancelBuy), 0);
        assert_ne!(h.shortcuts.get(ShortcutAction::PanicSell), 0);
        assert!(cfg.theme.is_dark());
        assert_eq!(
            cfg.theme.colors_light().unwrap().entries[0],
            ("graphBK".to_string(), "16777215".to_string())
        );
        assert_eq!(
            cfg.theme.colors_dark().unwrap().entries[0],
            ("graphBK".to_string(), "1973790".to_string())
        );
        assert_eq!(
            cfg.ini.section("Charts").unwrap().entries[0],
            ("CandleGreen".to_string(), "65280".to_string())
        );
        assert!(cfg.ini.section("ArbColors").unwrap().entries.is_empty());
        assert_eq!(cfg.ui.strat_editor_chapters, "chapters");
        assert_eq!(cfg.ui.markets_table.sort_col, 3);
        assert!(cfg.ui.markets_table.col_visible[0]);
        assert!(!cfg.ui.markets_table.col_visible[1]);
    }

    #[test]
    fn ui_appended_tail_is_allowed() {
        // Append-only расширение той же версии: лишние байты в хвосте UI-блока ок.
        let cfg = parse_payload(&full_payload(&[1, 2, 3, 4, 5])).unwrap();
        assert_eq!(cfg.ui.main_buttons_index, 4);
    }

    #[test]
    fn unknown_block_is_skipped() {
        let mut p = full_payload(&[]);
        block(&mut p, 42, &[0xCC; 10]); // неизвестный kind в конце
        assert!(parse_payload(&p).is_ok());
    }

    #[test]
    fn duplicate_known_block_rejected() {
        let mut p = full_payload(&[]);
        block(&mut p, 4, &theme_body());
        assert!(matches!(
            parse_payload(&p),
            Err(ImportError::BadValue(ref s)) if s.contains("дважды")
        ));
    }

    #[test]
    fn missing_block_rejected() {
        // Собираем payload без блока 5 (Ini).
        let mut p = Vec::new();
        p.extend_from_slice(b"MBSP");
        p.push(7u8);
        p.extend_from_slice(&1u16.to_le_bytes());
        block(&mut p, 1, &[]);
        block(&mut p, 2, &[]);
        block(&mut p, 3, &[]);
        block(&mut p, 4, &theme_body());
        block(&mut p, 6, &ui_body([0.0; 6], [0; 6], [0; 6], [0.0; 6]));
        assert!(matches!(
            parse_payload(&p),
            Err(ImportError::Truncated(ref s)) if s.contains("kind=5")
        ));
    }

    #[test]
    fn block_overrun_rejected() {
        // size блока больше остатка payload.
        let mut p = Vec::new();
        p.extend_from_slice(b"MBSP");
        p.push(7u8);
        p.extend_from_slice(&1u16.to_le_bytes());
        p.push(1u8);
        p.extend_from_slice(&100u32.to_le_bytes()); // заявлено 100, есть 2
        p.extend_from_slice(&[0, 0]);
        assert!(matches!(parse_payload(&p), Err(ImportError::Truncated(_))));
    }

    #[test]
    fn bad_magic_and_newer_version() {
        assert!(matches!(
            parse_payload(b"XXXX\x07\x00\x00"),
            Err(ImportError::BadHeader(_))
        ));
        let mut p = Vec::new();
        p.extend_from_slice(b"MBSP");
        p.push(8u8); // контейнер новее
        p.extend_from_slice(&1u16.to_le_bytes());
        assert_eq!(
            parse_payload(&p),
            Err(ImportError::NewerFormat { found: 8 })
        );
    }

    #[test]
    fn ui_version_mismatch_rejected() {
        let mut p = Vec::new();
        p.extend_from_slice(b"MBSP");
        p.push(7u8);
        p.extend_from_slice(&1u16.to_le_bytes());
        block(&mut p, 1, &[]);
        block(&mut p, 2, &[]);
        block(&mut p, 3, &[]);
        block(&mut p, 4, &theme_body());
        block(&mut p, 5, &ini_body());
        let mut ui = ui_body([0.0; 6], [0; 6], [0; 6], [0.0; 6]);
        ui[0] = 4; // version 4 вместо 3
        block(&mut p, 6, &ui);
        assert!(matches!(
            parse_payload(&p),
            Err(ImportError::BadValue(ref s)) if s.contains("UI.version")
        ));
    }

    #[test]
    fn nan_order_size_rejected() {
        let mut sizes = [1.0f64; 6];
        sizes[3] = f64::NAN;
        let mut p = Vec::new();
        p.extend_from_slice(b"MBSP");
        p.push(7u8);
        p.extend_from_slice(&1u16.to_le_bytes());
        block(&mut p, 1, &[]);
        block(&mut p, 2, &[]);
        block(&mut p, 3, &[]);
        block(&mut p, 4, &theme_body());
        block(&mut p, 5, &ini_body());
        block(&mut p, 6, &ui_body(sizes, [0; 6], [0; 6], [0.0; 6]));
        assert!(matches!(
            parse_payload(&p),
            Err(ImportError::BadValue(ref s)) if s.contains("OSize")
        ));
    }

    #[test]
    fn full_clipboard_roundtrip_through_transport() {
        // Интеграция транспорта и схемы: encode → parse_clipboard.
        let text = super::super::transport::encode_mbsc7(&full_payload(&[]));
        let cfg = super::super::parse_clipboard(&text).unwrap();
        assert_eq!(cfg.ui.hotkeys.order_sizes[5], 50000.0);
    }
}
