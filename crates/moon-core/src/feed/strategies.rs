//! Стратегии на стороне feed: декаплинг схемы moonproto, алерт-параметры,
//! формат/парсинг значений полей, имена видов.

use moonproto::{
    FieldValue, StrategyFieldType, StrategyFieldUiKind, StrategySchema, StrategySnapshot,
};

use super::{SchemaField, SchemaFieldUi, SchemaKind, SchemaSection, StrategySchemaModel};

/// Параметры стратегии-источника, влияющие на UI детекта.
/// Дефолт — (false, 60): кнопку-детект показываем только при SoundAlert=Yes,
/// держим KeepAlert секунд.
#[derive(Default)]
pub(super) struct AlertParams {
    pub sound_alert: bool,
    pub keep_alert_secs: u32,
    /// Номер чарта-вкладки (0 = не добавлять).
    pub add_to_chart: u32,
    pub keep_in_chart_secs: u32,
}

/// Целочисленное значение поля стратегии (AddToChart/KeepInChart/KeepAlert) —
/// принимаем ЛЮБОЙ числовой/булев тип moonproto, иначе `default`.
fn field_secs_or(s: &StrategySnapshot, name: &str, default: u32) -> u32 {
    match s.fields.get(name) {
        Some(FieldValue::Int32(v)) => (*v).max(0) as u32,
        Some(FieldValue::Int64(v)) => (*v).max(0) as u32,
        Some(FieldValue::UInt32(v)) => *v,
        Some(FieldValue::UInt64(v)) => *v as u32,
        Some(FieldValue::Byte(v)) => *v as u32,
        Some(FieldValue::Word(v)) => *v as u32,
        Some(FieldValue::Bool(b)) => *b as u32,
        Some(FieldValue::Double(v)) => v.max(0.0) as u32,
        Some(FieldValue::Single(v)) => v.max(0.0) as u32,
        _ => default,
    }
}

pub(super) fn alert_params(s: &StrategySnapshot) -> AlertParams {
    AlertParams {
        sound_alert: s.field_bool_or_false("SoundAlert"),
        keep_alert_secs: field_secs_or(s, "KeepAlert", 60),
        add_to_chart: field_secs_or(s, "AddToChart", 0),
        keep_in_chart_secs: field_secs_or(s, "KeepInChart", 60),
    }
}

/// Форматирует значение поля стратегии в строку (read-only показ в плашках).
pub(super) fn fmt_field(v: &FieldValue) -> String {
    match v {
        FieldValue::Bool(b) => if *b { "Yes" } else { "No" }.to_string(),
        FieldValue::Int32(n) => n.to_string(),
        FieldValue::Int64(n) => n.to_string(),
        FieldValue::UInt32(n) => n.to_string(),
        FieldValue::UInt64(n) => n.to_string(),
        FieldValue::Byte(n) => n.to_string(),
        FieldValue::Word(n) => n.to_string(),
        FieldValue::Double(d) => crate::util::fmt::compact(*d, 6),
        FieldValue::Single(f) => crate::util::fmt::compact(*f as f64, 6),
        FieldValue::String(s) => s.clone(),
    }
}

/// Собирает `FieldValue` из строки UI по ТИПУ поля: приоритет — тип существующего
/// значения снимка, иначе тип из схемы, иначе строка. Кривое число → 0.
pub(super) fn fv_from_str(
    existing: Option<&FieldValue>,
    stype: Option<StrategyFieldType>,
    s: &str,
) -> FieldValue {
    let b = || {
        matches!(
            s.trim().to_ascii_lowercase().as_str(),
            "yes" | "true" | "1" | "on"
        )
    };
    let i = |def: i64| s.trim().parse::<i64>().unwrap_or(def);
    let u = || s.trim().parse::<u64>().unwrap_or(0);
    let f = || s.trim().parse::<f64>().unwrap_or(0.0);
    // По существующему значению.
    if let Some(ev) = existing {
        return match ev {
            FieldValue::Bool(_) => FieldValue::Bool(b()),
            FieldValue::Int32(_) => FieldValue::Int32(i(0) as i32),
            FieldValue::Int64(_) => FieldValue::Int64(i(0)),
            FieldValue::UInt32(_) => FieldValue::UInt32(u() as u32),
            FieldValue::UInt64(_) => FieldValue::UInt64(u()),
            FieldValue::Byte(_) => FieldValue::Byte(u() as u8),
            FieldValue::Word(_) => FieldValue::Word(u() as u16),
            FieldValue::Double(_) => FieldValue::Double(f()),
            FieldValue::Single(_) => FieldValue::Single(f() as f32),
            FieldValue::String(_) => FieldValue::String(s.to_string()),
        };
    }
    // По типу схемы.
    match stype {
        Some(StrategyFieldType::Bool) => FieldValue::Bool(b()),
        Some(StrategyFieldType::Int32) => FieldValue::Int32(i(0) as i32),
        Some(StrategyFieldType::Int64) => FieldValue::Int64(i(0)),
        Some(StrategyFieldType::UInt32) => FieldValue::UInt32(u() as u32),
        Some(StrategyFieldType::UInt64) => FieldValue::UInt64(u()),
        Some(StrategyFieldType::Byte) => FieldValue::Byte(u() as u8),
        Some(StrategyFieldType::Word) => FieldValue::Word(u() as u16),
        Some(StrategyFieldType::Double) => FieldValue::Double(f()),
        Some(StrategyFieldType::Single) => FieldValue::Single(f() as f32),
        _ => FieldValue::String(s.to_string()),
    }
}

/// Декаплированная модель схемы из moonproto `StrategySchema`: по каждому виду —
/// его секции (editor sections) с полями (имя/тип/вид виджета/пиклист/дефолт).
pub(super) fn build_schema_model(schema: &StrategySchema) -> StrategySchemaModel {
    let kinds = schema
        .kinds
        .iter()
        .map(|k| {
            let kind = k.kind();
            let sections = schema
                .editor_sections_for_strategy_kind(kind)
                .into_iter()
                .map(|sec| SchemaSection {
                    title: sec.title,
                    fields: sec
                        .fields
                        .iter()
                        .map(|f| SchemaField {
                            name: f.name.clone(),
                            type_name: f.type_id.name().to_string(),
                            ui: map_ui(f.ui_kind),
                            picklist: f.static_picklist.clone(),
                            default: f.default_value.as_ref().map(fmt_field),
                        })
                        .collect(),
                })
                .collect();
            SchemaKind {
                ordinal: k.ordinal(),
                name: k.name.clone(),
                sections,
            }
        })
        .collect();
    StrategySchemaModel { kinds }
}

fn map_ui(u: StrategyFieldUiKind) -> SchemaFieldUi {
    match u {
        StrategyFieldUiKind::Checkbox => SchemaFieldUi::Checkbox,
        StrategyFieldUiKind::Combo => SchemaFieldUi::Combo,
        StrategyFieldUiKind::Color => SchemaFieldUi::Color,
        _ => SchemaFieldUi::Edit, // Edit + Unknown
    }
}

/// Тип (вид) стратегии MoonBot по ordinal `StrategyKind`.
pub(super) fn strat_kind_name(ordinal: u8) -> &'static str {
    match ordinal {
        0 => "Unknown",
        1 => "Telegram",
        2 => "Drops",
        3 => "Walls",
        4 => "Volumes",
        5 => "PumpDetection",
        6 => "MoonShot",
        7 => "V Lite",
        8 => "Delta",
        9 => "Waves",
        10 => "Combo",
        11 => "UDP",
        12 => "Manual",
        13 => "MoonStrike",
        14 => "New Listing",
        15 => "Liquidations",
        16 => "TopMarket",
        17 => "EMA",
        18 => "Spread",
        19 => "Chart Wall",
        20 => "Moon Hook",
        21 => "Activity",
        22 => "Alerts",
        23 => "Watcher",
        _ => "?",
    }
}
