//! Localized wording for typed MoonBot import-preview facts, without GPUI dependencies.

use moon_core::config::moonbot_import::preview::{
    ImportReason, ImportWarning, PreviewCaption, PreviewValue,
};
use moon_core::config::moonbot_import::schema_v7::ShortcutAction;
use rust_i18n::t;

#[cfg(test)]
mod tests;

/// Resolve a caption in the active locale; exported identifiers remain verbatim data.
pub(super) fn caption(value: &PreviewCaption) -> String {
    match value {
        PreviewCaption::ConfigField(name) => name.clone(),
        PreviewCaption::Action(action) => action_caption(*action),
        PreviewCaption::OrderSizeSlot(n) => {
            t!("import.preview.caption.order_size_slot", n = n).to_string()
        }
        PreviewCaption::FixedSellSlot(n) => {
            t!("import.preview.caption.fixed_sell_slot", n = n).to_string()
        }
        PreviewCaption::ColorField { key, light } => format!("{key} ({})", theme_name(*light)),
        PreviewCaption::IniEntry {
            section,
            key,
            value,
        } => format!("[{section}] {key} = {value}"),
        PreviewCaption::UiTheme => t!("import.preview.caption.ui_theme").to_string(),
        PreviewCaption::SplitParts => t!("import.preview.caption.split_parts").to_string(),
        PreviewCaption::ChartBackground => {
            t!("import.preview.caption.chart_background").to_string()
        }
        PreviewCaption::Grid => t!("import.preview.caption.grid").to_string(),
        PreviewCaption::Crosshair => t!("import.preview.caption.crosshair").to_string(),
        PreviewCaption::NeutralLabels => t!("import.preview.caption.neutral_labels").to_string(),
        PreviewCaption::CandleUp => t!("import.preview.caption.candle_up").to_string(),
        PreviewCaption::CandleDown => t!("import.preview.caption.candle_down").to_string(),
        PreviewCaption::CandleNeutral => t!("import.preview.caption.candle_neutral").to_string(),
        PreviewCaption::BookBid => t!("import.preview.caption.book_bid").to_string(),
        PreviewCaption::BookAsk => t!("import.preview.caption.book_ask").to_string(),
        PreviewCaption::BuyLine => t!("import.preview.caption.buy_line").to_string(),
        PreviewCaption::BuyPendingLine => t!("import.preview.caption.buy_pending_line").to_string(),
        PreviewCaption::SellLine => t!("import.preview.caption.sell_line").to_string(),
        PreviewCaption::BuyShortLine => t!("import.preview.caption.buy_short_line").to_string(),
        PreviewCaption::SellShortLine => t!("import.preview.caption.sell_short_line").to_string(),
        PreviewCaption::TrailingLine => t!("import.preview.caption.trailing_line").to_string(),
        PreviewCaption::LiquidationLine => {
            t!("import.preview.caption.liquidation_line").to_string()
        }
        PreviewCaption::OrderSizes => t!("import.preview.caption.order_sizes").to_string(),
        PreviewCaption::OrderSizeSelection => {
            t!("import.preview.caption.order_size_selection").to_string()
        }
        PreviewCaption::FixedSellPrices => {
            t!("import.preview.caption.fixed_sell_prices").to_string()
        }
        PreviewCaption::FixedSellSelection => {
            t!("import.preview.caption.fixed_sell_selection").to_string()
        }
        PreviewCaption::MarketsTable => t!("import.preview.caption.markets_table").to_string(),
        PreviewCaption::MouseGestures => t!("import.preview.caption.mouse_gestures").to_string(),
    }
}

/// Render a before/after value without changing its numeric or shortcut representation.
pub(super) fn preview_value(value: &PreviewValue) -> String {
    match value {
        PreviewValue::Data(data) => data.clone(),
        PreviewValue::ThemeLight(light) => theme_name(*light),
        PreviewValue::SelectedGroup => t!("import.preview.value.selected_group").to_string(),
    }
}

/// Name the color set in the current UI locale.
fn theme_name(light: bool) -> String {
    t!(if light {
        "import.preview.value.light"
    } else {
        "import.preview.value.dark"
    })
    .to_string()
}

/// Render a refusal and its original parameters in the active locale.
pub(super) fn reason(value: &ImportReason) -> String {
    match value {
        ImportReason::UnknownKey { vk } => t!(
            "import.preview.reason.unknown_key",
            vk = format!("{vk:02X}")
        )
        .to_string(),
        ImportReason::InvalidColor { value } => {
            t!("import.preview.reason.invalid_color", value = value).to_string()
        }
        ImportReason::ColorAlpha { alpha } => t!(
            "import.preview.reason.color_alpha",
            alpha = format!("{alpha:02X}")
        )
        .to_string(),
        ImportReason::NoAction => t!("import.preview.reason.no_action").to_string(),
        ImportReason::NoBookLevelColor => {
            t!("import.preview.reason.no_book_level_color").to_string()
        }
        ImportReason::ClosedOrderOpacity => {
            t!("import.preview.reason.closed_order_opacity").to_string()
        }
        ImportReason::NoStyle => t!("import.preview.reason.no_style").to_string(),
        ImportReason::UnmappedColor => t!("import.preview.reason.unmapped_color").to_string(),
        ImportReason::UnmappedSection => t!("import.preview.reason.unmapped_section").to_string(),
        ImportReason::NoSetting => t!("import.preview.reason.no_setting").to_string(),
        ImportReason::UnmappedColumns => t!("import.preview.reason.unmapped_columns").to_string(),
        ImportReason::GesturesUnavailable => {
            t!("import.preview.reason.gestures_unavailable").to_string()
        }
    }
}

/// Render a warning without changing any of the planner's decisions or numeric precision.
pub(super) fn warning(value: &ImportWarning) -> String {
    match value {
        ImportWarning::SplitPartsClamped { parts, max, value } => t!(
            "import.preview.warning.split_parts_clamped",
            parts = parts,
            max = max,
            value = value
        )
        .to_string(),
        ImportWarning::HotkeysUnfilled => t!("import.preview.warning.hotkeys_unfilled").to_string(),
        ImportWarning::InvalidOrderSizes { values } => t!(
            "import.preview.warning.invalid_order_sizes",
            values = numbers(values)
        )
        .to_string(),
        ImportWarning::OrderSizeSelection { value } => {
            t!("import.preview.warning.order_size_selection", value = value).to_string()
        }
        ImportWarning::InvalidFixedSellPrices { values } => t!(
            "import.preview.warning.invalid_fixed_sell_prices",
            values = numbers(values)
        )
        .to_string(),
        ImportWarning::FixedSellSelection { value } => {
            t!("import.preview.warning.fixed_sell_selection", value = value).to_string()
        }
    }
}

/// Keep each numeric warning parameter in its native precision, including invalid values.
fn numbers<T: std::fmt::Display>(values: &[T]) -> String {
    values
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Resolve MoonBot's action captions through the same locale tree as other preview labels.
fn action_caption(action: ShortcutAction) -> String {
    match action {
        ShortcutAction::CancelBuy => t!("import.preview.action.cancel_buy"),
        ShortcutAction::PanicSell => t!("import.preview.action.panic_sell"),
        ShortcutAction::JoinSells => t!("import.preview.action.join_sells"),
        ShortcutAction::SwitchCharts => t!("import.preview.action.switch_charts"),
        ShortcutAction::ReloadBook => t!("import.preview.action.reload_book"),
        ShortcutAction::NewLong => t!("import.preview.action.new_long"),
        ShortcutAction::NewShort => t!("import.preview.action.new_short"),
        ShortcutAction::SplitOrder => t!("import.preview.action.split_order"),
        ShortcutAction::ShiftBuyUp => t!("import.preview.action.shift_buy_up"),
        ShortcutAction::ShiftBuyDown => t!("import.preview.action.shift_buy_down"),
        ShortcutAction::ShiftSellUp => t!("import.preview.action.shift_sell_up"),
        ShortcutAction::ShiftSellDown => t!("import.preview.action.shift_sell_down"),
        ShortcutAction::MakeShot => t!("import.preview.action.make_shot"),
        ShortcutAction::MakeShotBot => t!("import.preview.action.make_shot_bot"),
        ShortcutAction::ReloadChart => t!("import.preview.action.reload_chart"),
        ShortcutAction::ScalePlus => t!("import.preview.action.scale_plus"),
        ShortcutAction::ScaleMinus => t!("import.preview.action.scale_minus"),
        ShortcutAction::SellPlus => t!("import.preview.action.sell_plus"),
        ShortcutAction::SellMinus => t!("import.preview.action.sell_minus"),
        ShortcutAction::SpyMode => t!("import.preview.action.spy_mode"),
        ShortcutAction::ShowCharts => t!("import.preview.action.show_charts"),
        ShortcutAction::SplitOrderX => t!("import.preview.action.split_order_x"),
        ShortcutAction::SwitchFigure => t!("import.preview.action.switch_figure"),
        ShortcutAction::FitSells => t!("import.preview.action.fit_sells"),
        ShortcutAction::PanicSellOne => t!("import.preview.action.panic_sell_one"),
        ShortcutAction::CancelAllBuys => t!("import.preview.action.cancel_all_buys"),
        ShortcutAction::Broadcast => t!("import.preview.action.broadcast"),
    }
    .to_string()
}
