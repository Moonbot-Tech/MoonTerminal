//! Schema for a decompressed `MBSP` v7 container: header plus `(kind u8, size u32 LE)` blocks.
//! Reads UI v3 (kind 6), Theme v1 (kind 4), and Ini v1 (kind 5). Signals/Trading/Visual
//! (kinds 1/2/3) must be present, but their contents are skipped entirely (spec section 10).
//! An unread tail in a KNOWN block is allowed as an append-only extension of the same version;
//! an unknown kind is skipped by size, while a repeated known block is an error.

use super::reader::{IniSection, Reader};
use super::ImportError;

/// Actions of the UI block's 27 positional shortcut slots, ordered exactly as in spec section 6.
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

/// All 27 actions in wire order.
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

/// Raw `TShortCut` values for 27 slots; each index matches its position in [`SHORTCUT_ACTIONS`].
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

/// `HotkeysPublic` from the UI block: order-size presets, fixed-sell, and shortcut slots.
#[derive(Debug, Clone, PartialEq)]
pub struct HotkeysPublic {
    pub filled: bool,
    pub ver: u8,
    /// Six manual-order sizes (`OSize`).
    pub order_sizes: [f64; 6],
    /// Selected size slot (`bNum`); the plan validates the 0..=5 range.
    pub order_size_sel: i32,
    /// Raw `TShortCut` hotkeys for size slots (`OKeys`).
    pub order_size_keys: [u16; 6],
    pub split_parts: u8,
    /// Selected fixed-sell slot (`sbNum`); the plan validates its range.
    pub fixed_sell_sel: u8,
    /// Raw `TShortCut` hotkeys for fixed-sell slots (`SKeys`).
    pub fixed_sell_keys: [u16; 6],
    /// Fixed-sell percentages (`SPrice`).
    pub fixed_sell_prices: [f32; 6],
    pub shortcuts: Shortcuts,
}

/// `MarketsTablePublic`: sorting and layout of the market table's 41 columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketsTable {
    pub sort_col: i32,
    pub col_visible: [bool; 41],
    pub col_pos: [u8; 41],
}

/// UI v3 block (kind 6), the primary block for the importer's first version.
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

/// Theme v1 block (kind 4): current style plus INI color sections for both themes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeBlock {
    pub current_style: i32,
    pub sections: Vec<IniSection>,
}

impl ThemeBlock {
    /// `CurrentStyle` values 3 and 4 are dark; all other known values are light (spec section 8).
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

/// Ini v1 block (kind 5): `Charts` and `ArbColors` sections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IniBlock {
    pub sections: Vec<IniSection>,
}

impl IniBlock {
    pub fn section(&self, name: &str) -> Option<&IniSection> {
        self.sections.iter().find(|s| s.name == name)
    }
}

/// Parsed MoonBot payload model without interpretation, which is performed by the plan.
#[derive(Debug, Clone, PartialEq)]
pub struct MoonBotConfig {
    /// MoonBot `ConfigVersion` from the header, used only for diagnostics and NOT as a Terminal schema version.
    pub config_version: u16,
    pub ui: UiBlock,
    pub theme: ThemeBlock,
    pub ini: IniBlock,
}

/// Container version understood by this reader.
const SUPPORTED_CONTAINER: u8 = 7;
/// Known blocks: each kind must appear exactly once.
const KNOWN_KINDS: [u8; 6] = [1, 2, 3, 4, 5, 6];

/// Parses a decompressed payload containing the `MBSP` header and all blocks.
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
            // Signals/Trading/Visual must be present, but their contents are skipped.
            1 | 2 | 3 => {}
            4 => theme = Some(parse_theme(sub)?),
            5 => ini = Some(parse_ini(sub)?),
            6 => ui = Some(parse_ui(sub)?),
            // Skip an unknown kind, such as a future Interop block before support is added.
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
    // seen guarantees that 4/5/6 appeared exactly once. Unwraps would be safe, but avoid panic.
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

/// Parses the strictly positional UI v3 block (spec section 6), ignoring its append-only tail.
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
    //! Binary payload test builder that mirrors the Delphi writer. Used by schema and transport
    //! unit tests, and later by golden fixtures.

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

    /// Builds a UI v3 body with specified hotkey values and fixed values for everything else.
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
            // 27 shortcut slots with deterministic values; slot 0 is empty.
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

    /// Builds a complete valid payload with six blocks and optional extra bytes in the UI tail.
    pub fn full_payload(ui_tail_extra: &[u8]) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend_from_slice(b"MBSP");
        p.push(7u8);
        p.extend_from_slice(&1234u16.to_le_bytes()); // ConfigVersion
        block(&mut p, 1, &[0xAA; 3]); // Signals contents are not read.
        block(&mut p, 2, &[0xBB; 5]); // Trading contents are skipped.
        block(&mut p, 3, &[]); // An empty Visual block is valid because its contents are not read.
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
mod tests;
