//! Moonbot's own appearance beyond its Interface page: the chart volumes, candles, overlays and
//! annotations its window paints, its order panel and alerts, and the layout of its other windows.
//!
//! Drawn under the Interface page on the same tab, in collapsible sections like the Special page,
//! because Moonbot keeps none of this in its Settings dialog — it sets these from the chart's own
//! menus and from each window — so there is no Moonbot page to reproduce row for row. None of it
//! changes this terminal; on a core without Moonbot's window this is the only place to set it.
//! `moon_core::feed::ViewSettings` carries the fields and their meanings.
//!
//! Rows are declared as data, one table per section, rather than written out as elements: the
//! sections hold over a hundred rows, and a table keeps each row's id, caption, reader and writer on
//! one line where a typo cannot pair one field's caption with another's setter. The slider and text
//! declarations the window needs are derived from the same tables.
//!
//! Choices store the core's own number. A core holding a value no item names shows `#` and the
//! list's length rather than that number — `choice_live` is given a POSITION, and a value outside
//! the list has none — and keeps its value until an item is picked.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use moon_core::feed::{CoreConfig, ViewSettings};

use crate::design;
use crate::shell::editors::EditorStore;

use super::super::CoreExpertView;
use super::super::widgets::{
    caption, choice_live, field, flag_live, slider, sound_cell, stepper_live,
};

/// One option label: a dictionary key, or a literal for a proper name or a unit-only label
/// ("5m", "OpenAI") that reads the same in every language.
#[derive(Clone, Copy)]
enum Label {
    Key(&'static str),
    Lit(&'static str),
}

use Label::{Key, Lit};

/// One row of a section.
#[derive(Clone, Copy)]
enum Row {
    /// A checkbox.
    Flag {
        id: &'static str,
        key: &'static str,
        get: fn(&ViewSettings) -> bool,
        set: fn(&mut CoreConfig, bool),
    },
    /// A selector over the listed wire values, in Moonbot's order.
    Choice {
        id: &'static str,
        key: &'static str,
        values: &'static [i32],
        labels: &'static [Label],
        get: fn(&ViewSettings) -> i32,
        set: fn(&mut CoreConfig, i32),
    },
    /// A `< n >` spinner for a count; `floor` bounds the arrows only, see `stepper_live`.
    Step {
        id: &'static str,
        key: &'static str,
        floor: i32,
        get: fn(&ViewSettings) -> i32,
        set: fn(&mut CoreConfig, i32),
    },
    /// A slider under a caption that formats the value into `%{v}`.
    Slide {
        id: &'static str,
        key: &'static str,
        bounds: (f32, f32, f32),
        get: fn(&ViewSettings) -> f32,
        set: fn(&mut CoreConfig, f32),
        shown: fn(&ViewSettings) -> String,
    },
    /// Moonbot's sound picker with its preview button.
    Sound {
        id: &'static str,
        key: &'static str,
        get: fn(&ViewSettings) -> i32,
        set: fn(&mut CoreConfig, i32),
    },
    /// A one-line text box.
    Text {
        id: &'static str,
        key: &'static str,
        get: fn(&ViewSettings) -> String,
        set: fn(&mut CoreConfig, &str),
    },
}

/// The sections, in the order they are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ViewSection {
    #[default]
    Volumes,
    Candles,
    Overlays,
    Annotations,
    Orders,
    Windows,
    News,
}

impl ViewSection {
    pub(crate) const ALL: [ViewSection; 7] = [
        Self::Volumes,
        Self::Candles,
        Self::Overlays,
        Self::Annotations,
        Self::Orders,
        Self::Windows,
        Self::News,
    ];

    /// Localized header.
    fn title(self) -> String {
        let key = match self {
            Self::Volumes => "core_expert.view_sec_volumes",
            Self::Candles => "core_expert.view_sec_candles",
            Self::Overlays => "core_expert.view_sec_overlays",
            Self::Annotations => "core_expert.view_sec_annotations",
            Self::Orders => "core_expert.view_sec_orders",
            Self::Windows => "core_expert.view_sec_windows",
            Self::News => "core_expert.view_sec_news",
        };
        t!(key).to_string()
    }

    /// The section's rows.
    fn rows(self) -> &'static [Row] {
        match self {
            Self::Volumes => VOLUMES,
            Self::Candles => CANDLES,
            Self::Overlays => OVERLAYS,
            Self::Annotations => ANNOTATIONS,
            Self::Orders => ORDERS,
            Self::Windows => WINDOWS,
            Self::News => NEWS,
        }
    }
}

/// A 0..=1 fraction as a whole per cent, for a caption.
fn pct(v: f32) -> String {
    format!("{}", (v * 100.0).round())
}

/// `v_tool_time` seconds for each of Moonbot's cursor-tool presets, by `v_tool_time_index`.
const V_TOOL_SECONDS: [f64; 11] = [
    0.0, 2.0, 5.0, 15.0, 30.0, 60.0, 180.0, 300.0, 900.0, 1800.0, 3600.0,
];

/// Write a cursor-tool preset: Moonbot's control writes the index AND the seconds it names, and the
/// renderer reads the seconds — writing only the index would change nothing on the chart.
fn set_v_tool_preset(d: &mut CoreConfig, index: i32) {
    d.view.v_tool_time_index = index;
    if let Some(&seconds) = usize::try_from(index)
        .ok()
        .and_then(|i| V_TOOL_SECONDS.get(i))
    {
        d.view.v_tool_time = seconds;
    }
}

/// Clamp a stepper value into a `u8` field's range rather than wrapping it.
fn to_u8(v: i32) -> u8 {
    u8::try_from(v.clamp(0, 255)).unwrap_or(u8::MAX)
}

// One row per line, on purpose: the row's id, caption, reader and writer stay side by side,
// which is what makes a mismatched pair visible. rustfmt would spread each over a dozen lines.
#[rustfmt::skip]
const OVERLAY_STACK: &[Label] = &[Key("core_expert.view_opt_overlay"), Key("core_expert.view_opt_stack")];

// One row per line, on purpose: the row's id, caption, reader and writer stay side by side,
// which is what makes a mismatched pair visible. rustfmt would spread each over a dozen lines.
#[rustfmt::skip]
const VOLUMES: &[Row] = &[
    Row::Flag { id: "exp-view-vv", key: "core_expert.view_vert_volume", get: |v| v.show_vert_volume, set: |d, on| d.view.show_vert_volume = on },
    Row::Choice { id: "exp-view-vv-tf", key: "core_expert.view_vert_vol_tf", values: &[0, 1, 2, 3, 4, 5, 6], labels: &[Key("core_expert.view_opt_auto"), Lit("5s"), Lit("15s"), Lit("30s"), Lit("1m"), Lit("3m"), Lit("5m")], get: |v| v.vert_vol_time_frame, set: |d, x| d.view.vert_vol_time_frame = x },
    Row::Choice { id: "exp-view-vv-kind", key: "core_expert.view_vert_vol_kind", values: &[0, 1], labels: OVERLAY_STACK, get: |v| v.vert_vol_kind, set: |d, x| d.view.vert_vol_kind = x },
    Row::Choice { id: "exp-view-vv-pos", key: "core_expert.view_vert_vol_pos", values: &[0, 1], labels: &[Key("core_expert.view_opt_left"), Key("core_expert.view_opt_right")], get: |v| v.vert_vol_ind_pos, set: |d, x| d.view.vert_vol_ind_pos = x },
    Row::Slide { id: "exp-view-vv-opacity", key: "core_expert.view_vert_vol_opacity", bounds: (0.0, 100.0, 1.0), get: |v| v.vert_vol_opacity * 100.0, set: |d, x| d.view.vert_vol_opacity = x.round() / 100.0, shown: |v| pct(v.vert_vol_opacity) },
    Row::Slide { id: "exp-view-vv-height", key: "core_expert.view_vert_vol_height", bounds: (0.0, 100.0, 1.0), get: |v| v.vert_vol_height * 100.0, set: |d, x| d.view.vert_vol_height = x.round() / 100.0, shown: |v| pct(v.vert_vol_height) },
    Row::Flag { id: "exp-view-vtool", key: "core_expert.view_v_tool", get: |v| v.v_tool_show, set: |d, on| d.view.v_tool_show = on },
    Row::Choice { id: "exp-view-vtool-time", key: "core_expert.view_v_tool_time", values: &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10], labels: &[Key("core_expert.view_opt_auto"), Lit("2s"), Lit("5s"), Lit("15s"), Lit("30s"), Lit("1m"), Lit("3m"), Lit("5m"), Lit("15m"), Lit("30m"), Lit("1h")], get: |v| v.v_tool_time_index, set: set_v_tool_preset },
    Row::Slide { id: "exp-view-vtool-opacity", key: "core_expert.view_v_tool_opacity", bounds: (0.0, 100.0, 1.0), get: |v| v.v_tool_opacity * 100.0, set: |d, x| d.view.v_tool_opacity = x.round() / 100.0, shown: |v| pct(v.v_tool_opacity) },
    Row::Flag { id: "exp-view-hv", key: "core_expert.view_hv", get: |v| v.hv_show, set: |d, on| d.view.hv_show = on },
    Row::Choice { id: "exp-view-hv-tf", key: "core_expert.view_hv_tf", values: &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10], labels: &[Key("core_expert.view_opt_auto"), Lit("1m"), Lit("5m"), Lit("15m"), Lit("30m"), Lit("1h"), Lit("2h"), Lit("6h"), Lit("12h"), Lit("24h"), Key("core_expert.view_opt_archive")], get: |v| v.hv_time_frame_ind, set: |d, x| d.view.hv_time_frame_ind = x },
    Row::Choice { id: "exp-view-hv-kind", key: "core_expert.view_hv_kind", values: &[0, 1], labels: OVERLAY_STACK, get: |v| v.hv_kind, set: |d, x| d.view.hv_kind = x },
    Row::Choice { id: "exp-view-hv-labels", key: "core_expert.view_hv_labels", values: &[0, 1, 2, 3], labels: &[Key("core_expert.view_opt_hv_label_0"), Key("core_expert.view_opt_hv_label_1"), Key("core_expert.view_opt_hv_label_2"), Key("core_expert.view_opt_hv_label_3")], get: |v| v.hv_disp_vol, set: |d, x| d.view.hv_disp_vol = x },
    Row::Slide { id: "exp-view-hv-width", key: "core_expert.view_hv_width", bounds: (0.0, 100.0, 1.0), get: |v| v.hv_width * 100.0, set: |d, x| d.view.hv_width = x.round() / 100.0, shown: |v| pct(v.hv_width) },
    Row::Slide { id: "exp-view-hv-opacity", key: "core_expert.view_hv_opacity", bounds: (0.0, 100.0, 1.0), get: |v| v.hv_opacity * 100.0, set: |d, x| d.view.hv_opacity = x.round() / 100.0, shown: |v| pct(v.hv_opacity) },
    Row::Slide { id: "exp-view-hv-price", key: "core_expert.view_hv_price_frame", bounds: (0.0, 1.0, 0.01), get: |v| v.hv_price_frame, set: |d, x| d.view.hv_price_frame = (x * 100.0).round() / 100.0, shown: |v| format!("{}", v.hv_price_frame) },
    Row::Choice { id: "exp-view-bv", key: "core_expert.view_base_vol", values: &[-2, -1, 0, 1, 3, 5, 15, 30], labels: &[Key("core_expert.view_opt_n_minutes"), Key("core_expert.view_opt_n_trades"), Key("core_expert.view_opt_prepump"), Lit("1m"), Lit("3m"), Lit("5m"), Lit("15m"), Lit("30m")], get: |v| v.base_vol_ind_type, set: |d, x| d.view.base_vol_ind_type = x },
    Row::Step { id: "exp-view-bv-n", key: "core_expert.view_base_vol_n", floor: 2, get: |v| v.base_vol_n, set: |d, x| d.view.base_vol_n = x },
];

// One row per line, on purpose: the row's id, caption, reader and writer stay side by side,
// which is what makes a mismatched pair visible. rustfmt would spread each over a dozen lines.
#[rustfmt::skip]
const CANDLES: &[Row] = &[
    Row::Choice { id: "exp-view-candle-style", key: "core_expert.view_candle_style", values: &[0, 1, 2], labels: &[Key("core_expert.view_opt_filled"), Key("core_expert.view_opt_outline"), Key("core_expert.view_opt_outline_ticks")], get: |v| i32::from(v.chart_candles_style), set: |d, x| d.view.chart_candles_style = to_u8(x) },
    Row::Choice { id: "exp-view-candle-outline", key: "core_expert.view_candle_outline", values: &[1, 2, 3], labels: &[Lit("1"), Lit("2"), Lit("3")], get: |v| i32::from(v.chart_candles_outline_width), set: |d, x| d.view.chart_candles_outline_width = to_u8(x) },
    Row::Slide { id: "exp-view-candle-opacity", key: "core_expert.view_candle_tick_opacity", bounds: (0.0, 100.0, 1.0), get: |v| f32::from(v.chart_candles_tick_opacity), set: |d, x| d.view.chart_candles_tick_opacity = to_u8(x.round() as i32), shown: |v| v.chart_candles_tick_opacity.to_string() },
    Row::Flag { id: "exp-view-candle-neutral", key: "core_expert.view_candle_neutral", get: |v| v.chart_candles_neutral_ticks, set: |d, on| d.view.chart_candles_neutral_ticks = on },
    Row::Flag { id: "exp-view-candle-wicks", key: "core_expert.view_candle_wicks", get: |v| v.chart_candles_tick_wicks, set: |d, on| d.view.chart_candles_tick_wicks = on },
    Row::Flag { id: "exp-view-trade-dots", key: "core_expert.view_trade_dots", get: |v| v.show_red_green_dots, set: |d, on| d.view.show_red_green_dots = on },
    Row::Flag { id: "exp-view-bidask", key: "core_expert.view_bid_ask_lines", get: |v| v.show_red_green_lines, set: |d, on| d.view.show_red_green_lines = on },
    Row::Choice { id: "exp-view-cur-price", key: "core_expert.view_cur_price", values: &[0, 1, 2], labels: &[Key("core_expert.view_opt_right_panel"), Key("core_expert.view_opt_in_spread"), Key("core_expert.view_opt_hidden")], get: |v| v.draw_cur_price, set: |d, x| d.view.draw_cur_price = x },
    Row::Choice { id: "exp-view-price-range", key: "core_expert.view_price_range", values: &[1, 2, 3, 4, 5, 6], labels: &[Key("core_expert.view_opt_auto"), Lit("2"), Lit("3"), Lit("4"), Lit("5"), Lit("6")], get: |v| v.price_range, set: |d, x| d.view.price_range = x },
    Row::Flag { id: "exp-view-x9", key: "core_expert.view_x9_mode", get: |v| v.x9_mode, set: |d, on| d.view.x9_mode = on },
    Row::Choice { id: "exp-view-scale-plus", key: "core_expert.view_scale_plus", values: &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9], labels: &[Key("core_expert.view_opt_normal"), Lit("100%"), Lit("200%"), Lit("500%"), Lit("1000%"), Lit("2000%"), Lit("5000%"), Lit("10000%"), Lit("20000%"), Lit("50000%")], get: |v| v.scale_plus_index, set: |d, x| d.view.scale_plus_index = x },
    Row::Flag { id: "exp-view-auto-shift", key: "core_expert.view_auto_shift", get: |v| v.charts_auto_shift, set: |d, on| d.view.charts_auto_shift = on },
    Row::Flag { id: "exp-view-cross", key: "core_expert.view_crosshair", get: |v| v.show_cross, set: |d, on| d.view.show_cross = on },
];

// One row per line, on purpose: the row's id, caption, reader and writer stay side by side,
// which is what makes a mismatched pair visible. rustfmt would spread each over a dozen lines.
#[rustfmt::skip]
const OVERLAYS: &[Row] = &[
    Row::Flag { id: "exp-view-spot-trades", key: "core_expert.view_spot_trades", get: |v| v.spot_show_trades, set: |d, on| d.view.spot_show_trades = on },
    Row::Flag { id: "exp-view-spot-book", key: "core_expert.view_spot_book", get: |v| v.spot_show_book, set: |d, on| d.view.spot_show_book = on },
    Row::Slide { id: "exp-view-spot-book-len", key: "core_expert.view_spot_book_len", bounds: (0.0, 200.0, 1.0), get: |v| v.spot_book_len, set: |d, x| d.view.spot_book_len = x.round(), shown: |v| format!("{}", v.spot_book_len) },
    Row::Flag { id: "exp-view-spot-btc", key: "core_expert.view_spot_btc", get: |v| v.spot_btc, set: |d, on| d.view.spot_btc = on },
    Row::Flag { id: "exp-view-spot-shift", key: "core_expert.view_spot_shift", get: |v| v.shift_spot, set: |d, on| d.view.shift_spot = on },
    Row::Flag { id: "exp-view-market-avg", key: "core_expert.view_market_avg", get: |v| v.show_market_avg, set: |d, on| d.view.show_market_avg = on },
    Row::Flag { id: "exp-view-min-max", key: "core_expert.view_min_max", get: |v| v.show_min_max, set: |d, on| d.view.show_min_max = on },
    Row::Flag { id: "exp-view-avg-price", key: "core_expert.view_avg_price", get: |v| v.show_avg_price, set: |d, on| d.view.show_avg_price = on },
    Row::Flag { id: "exp-view-avg-line", key: "core_expert.view_avg_price_line", get: |v| v.show_avg_price_line, set: |d, on| d.view.show_avg_price_line = on },
    Row::Flag { id: "exp-view-mark", key: "core_expert.view_mark_price", get: |v| v.show_mark_price, set: |d, on| d.view.show_mark_price = on },
    Row::Flag { id: "exp-view-liq", key: "core_expert.view_liquidations", get: |v| v.show_liq, set: |d, on| d.view.show_liq = on },
    Row::Flag { id: "exp-view-liq-big", key: "core_expert.view_liquidations_big", get: |v| v.huge_liq, set: |d, on| d.view.huge_liq = on },
    Row::Flag { id: "exp-view-oi", key: "core_expert.view_open_interest", get: |v| v.show_open_int, set: |d, on| d.view.show_open_int = on },
    Row::Flag { id: "exp-view-arb-lines", key: "core_expert.view_arb_lines", get: |v| v.arb_show_lines, set: |d, on| d.view.arb_show_lines = on },
    Row::Flag { id: "exp-view-arb-numbers", key: "core_expert.view_arb_numbers", get: |v| v.arb_show_numbers, set: |d, on| d.view.arb_show_numbers = on },
    Row::Flag { id: "exp-view-arb-percent", key: "core_expert.view_arb_percent", get: |v| v.arb_show_percent, set: |d, on| d.view.arb_show_percent = on },
    Row::Flag { id: "exp-view-arb-absolute", key: "core_expert.view_arb_absolute", get: |v| v.arb_show_absolute, set: |d, on| d.view.arb_show_absolute = on },
    Row::Flag { id: "exp-view-arb-right", key: "core_expert.view_arb_right", get: |v| v.arb_show_right, set: |d, on| d.view.arb_show_right = on },
];

// One row per line, on purpose: the row's id, caption, reader and writer stay side by side,
// which is what makes a mismatched pair visible. rustfmt would spread each over a dozen lines.
#[rustfmt::skip]
const ANNOTATIONS: &[Row] = &[
    Row::Flag { id: "exp-view-filters", key: "core_expert.view_filters_show", get: |v| v.filters_show, set: |d, on| d.view.filters_show = on },
    Row::Flag { id: "exp-view-filters-strats", key: "core_expert.view_filters_strategies", get: |v| v.filters_strategies, set: |d, on| d.view.filters_strategies = on },
    Row::Flag { id: "exp-view-filters-sessions", key: "core_expert.view_filters_sessions", get: |v| v.filters_sessions, set: |d, on| d.view.filters_sessions = on },
    Row::Flag { id: "exp-view-filters-zero", key: "core_expert.view_filters_hide_zero", get: |v| v.filters_hide_zero, set: |d, on| d.view.filters_hide_zero = on },
    Row::Flag { id: "exp-view-filters-mshot", key: "core_expert.view_filters_moonshot", get: |v| v.filters_moonshot, set: |d, on| d.view.filters_moonshot = on },
    Row::Flag { id: "exp-view-filters-scroll", key: "core_expert.view_filters_scroll", get: |v| v.filters_scroll, set: |d, on| d.view.filters_scroll = on },
    Row::Flag { id: "exp-view-filters-detects", key: "core_expert.view_filters_detects", get: |v| v.filters_detects, set: |d, on| d.view.filters_detects = on },
    Row::Flag { id: "exp-view-draw", key: "core_expert.view_draw_show", get: |v| v.draw_show, set: |d, on| d.view.draw_show = on },
    Row::Slide { id: "exp-view-draw-opacity", key: "core_expert.view_draw_opacity", bounds: (0.0, 100.0, 1.0), get: |v| v.draw_opacity * 100.0, set: |d, x| d.view.draw_opacity = x.round() / 100.0, shown: |v| pct(v.draw_opacity) },
    Row::Flag { id: "exp-view-heatmap", key: "core_expert.view_heatmap", get: |v| v.heatmap_show, set: |d, on| d.view.heatmap_show = on },
    Row::Flag { id: "exp-view-heatmap-q", key: "core_expert.view_heatmap_qty", get: |v| v.heatmap_use_q, set: |d, on| d.view.heatmap_use_q = on },
    Row::Slide { id: "exp-view-heatmap-height", key: "core_expert.view_heatmap_height", bounds: (5.0, 80.0, 1.0), get: |v| v.heatmap_height * 100.0, set: |d, x| d.view.heatmap_height = x.round() / 100.0, shown: |v| pct(v.heatmap_height) },
    Row::Flag { id: "exp-view-heatmap-cpu", key: "core_expert.view_heatmap_cpu", get: |v| v.heatmap_cpu, set: |d, on| d.view.heatmap_cpu = on },
    Row::Flag { id: "exp-view-heatmap-trades", key: "core_expert.view_heatmap_trades", get: |v| v.heatmap_trades, set: |d, on| d.view.heatmap_trades = on },
    Row::Flag { id: "exp-view-heatmap-app", key: "core_expert.view_heatmap_app", get: |v| v.heatmap_app_latency, set: |d, on| d.view.heatmap_app_latency = on },
    Row::Flag { id: "exp-view-heatmap-draw", key: "core_expert.view_heatmap_draw", get: |v| v.heatmap_draw_latency, set: |d, on| d.view.heatmap_draw_latency = on },
];

// One row per line, on purpose: the row's id, caption, reader and writer stay side by side,
// which is what makes a mismatched pair visible. rustfmt would spread each over a dozen lines.
#[rustfmt::skip]
const ORDERS: &[Row] = &[
    Row::Flag { id: "exp-view-all-buys", key: "core_expert.view_draw_all_buy", get: |v| v.draw_all_buy, set: |d, on| d.view.draw_all_buy = on },
    Row::Flag { id: "exp-view-order-num", key: "core_expert.view_order_numbers", get: |v| v.show_orders_num, set: |d, on| d.view.show_orders_num = on },
    Row::Slide { id: "exp-view-done-opacity", key: "core_expert.view_done_opacity", bounds: (0.0, 100.0, 1.0), get: |v| v.done_opacity * 100.0, set: |d, x| d.view.done_opacity = x.round() / 100.0, shown: |v| pct(v.done_opacity) },
    Row::Flag { id: "exp-view-order-comment", key: "core_expert.view_order_comment", get: |v| v.show_order_comment, set: |d, on| d.view.show_order_comment = on },
    Row::Choice { id: "exp-view-vo-sort", key: "core_expert.view_vo_sort", values: &[0, 1, 2], labels: &[Key("core_expert.view_opt_entries_first"), Key("core_expert.view_opt_exits_first"), Key("core_expert.view_opt_created")], get: |v| i32::from(v.vo_sort_kind), set: |d, x| d.view.vo_sort_kind = to_u8(x) },
    Row::Flag { id: "exp-view-vo-older", key: "core_expert.view_vo_older_first", get: |v| v.vo_older_first, set: |d, on| d.view.vo_older_first = on },
    Row::Flag { id: "exp-view-vo-market", key: "core_expert.view_vo_by_market", get: |v| v.vo_sort_by_market, set: |d, on| d.view.vo_sort_by_market = on },
    Row::Step { id: "exp-view-alert-keep", key: "core_expert.view_alert_keep", floor: 10, get: |v| v.alert_keep_time, set: |d, x| d.view.alert_keep_time = x },
    Row::Step { id: "exp-view-alert-repeat", key: "core_expert.view_alert_repeat", floor: 1, get: |v| v.alert_repeat, set: |d, x| d.view.alert_repeat = x },
    Row::Sound { id: "exp-view-alert-sound", key: "core_expert.view_alert_sound", get: |v| v.alert_sound, set: |d, x| d.view.alert_sound = x },
];

// One row per line, on purpose: the row's id, caption, reader and writer stay side by side,
// which is what makes a mismatched pair visible. rustfmt would spread each over a dozen lines.
#[rustfmt::skip]
const WINDOWS: &[Row] = &[
    Row::Flag { id: "exp-view-auto-close", key: "core_expert.view_auto_close_charts", get: |v| v.auto_close_charts, set: |d, on| d.view.auto_close_charts = on },
    Row::Flag { id: "exp-view-watch-multi", key: "core_expert.view_watch_multi", get: |v| v.chart_watch_multi_frames, set: |d, on| d.view.chart_watch_multi_frames = on },
    Row::Step { id: "exp-view-history-len", key: "core_expert.view_history_len", floor: 0, get: |v| v.chart_history_len, set: |d, x| d.view.chart_history_len = x },
    Row::Flag { id: "exp-view-mon-top", key: "core_expert.view_monitor_on_top", get: |v| v.charts_stay_on_top, set: |d, on| d.view.charts_stay_on_top = on },
    Row::Flag { id: "exp-view-mon-visible", key: "core_expert.view_monitor_visible", get: |v| v.monitor_visible, set: |d, on| d.view.monitor_visible = on },
    Row::Flag { id: "exp-view-mon-arrange", key: "core_expert.view_monitor_arrange", get: |v| v.monitor_auto_arrange, set: |d, on| d.view.monitor_auto_arrange = on },
    Row::Flag { id: "exp-view-mon-wide", key: "core_expert.view_monitor_wide", get: |v| v.monitor_wide, set: |d, on| d.view.monitor_wide = on },
    Row::Step { id: "exp-view-mon-max", key: "core_expert.view_monitor_max", floor: 0, get: |v| v.monitor_max_charts, set: |d, x| d.view.monitor_max_charts = x },
    Row::Choice { id: "exp-view-mon-refresh", key: "core_expert.view_monitor_refresh", values: &[0, 1, 2, 3, 4, 5], labels: &[Lit("5s"), Lit("15s"), Lit("30s"), Lit("1m"), Lit("5m"), Lit("15m")], get: |v| v.monitor_refresh, set: |d, x| d.view.monitor_refresh = x },
    Row::Choice { id: "exp-view-rep-type", key: "core_expert.view_report_type", values: &[0, 1, 2, 3], labels: &[Key("core_expert.view_opt_all"), Key("core_expert.view_opt_active"), Key("core_expert.view_opt_closed"), Key("core_expert.view_opt_with_deleted")], get: |v| i32::from(v.report_active_type), set: |d, x| d.view.report_active_type = to_u8(x) },
    Row::Choice { id: "exp-view-rep-dir", key: "core_expert.view_report_direction", values: &[0, 1, 2], labels: &[Key("core_expert.view_opt_both"), Lit("Long"), Lit("Short")], get: |v| i32::from(v.report_direction), set: |d, x| d.view.report_direction = to_u8(x) },
    Row::Step { id: "exp-view-rep-lines", key: "core_expert.view_report_lines", floor: 50, get: |v| i32::from(v.report_max_lines), set: |d, x| { d.view.report_max_lines = u16::try_from(x.clamp(0, i32::from(u16::MAX))).unwrap_or(u16::MAX) } },
    Row::Flag { id: "exp-view-rep-emu", key: "core_expert.view_report_emulator", get: |v| v.report_emulator, set: |d, on| d.view.report_emulator = on },
    Row::Flag { id: "exp-view-rep-close", key: "core_expert.view_report_by_close", get: |v| v.report_by_close_date, set: |d, on| d.view.report_by_close_date = on },
    Row::Flag { id: "exp-view-rep-stretch", key: "core_expert.view_report_stretch", get: |v| v.report_stretch, set: |d, on| d.view.report_stretch = on },
    Row::Flag { id: "exp-view-rep-two", key: "core_expert.view_report_two_orders", get: |v| v.report_only_2_orders, set: |d, on| d.view.report_only_2_orders = on },
    Row::Flag { id: "exp-view-rep-lev", key: "core_expert.view_report_leverage", get: |v| v.report_use_leverage, set: |d, on| d.view.report_use_leverage = on },
    Row::Flag { id: "exp-view-rep-liq", key: "core_expert.view_report_store_liq", get: |v| v.report_store_liq, set: |d, on| d.view.report_store_liq = on },
    Row::Flag { id: "exp-view-rep-fund", key: "core_expert.view_report_store_fund", get: |v| v.report_store_fund, set: |d, on| d.view.report_store_fund = on },
    Row::Choice { id: "exp-view-cb-period", key: "core_expert.view_cashback_period", values: &[0, 1, 2, 3], labels: &[Key("core_expert.view_opt_all"), Key("core_expert.view_opt_90d"), Key("core_expert.view_opt_30d"), Key("core_expert.view_opt_half_month")], get: |v| v.cashback_period, set: |d, x| d.view.cashback_period = x },
    Row::Flag { id: "exp-view-cb-sep", key: "core_expert.view_cashback_separate", get: |v| v.cashback_sep, set: |d, on| d.view.cashback_sep = on },
    Row::Flag { id: "exp-view-cb-bnb", key: "core_expert.view_cashback_bnb_then", get: |v| v.cashback_instant_bnb, set: |d, on| d.view.cashback_instant_bnb = on },
    Row::Flag { id: "exp-view-tr-total", key: "core_expert.view_transfer_total", get: |v| v.transfer_show_total, set: |d, on| d.view.transfer_show_total = on },
    Row::Flag { id: "exp-view-tr-zero", key: "core_expert.view_transfer_hide_zero", get: |v| v.transfer_hide_zero, set: |d, on| d.view.transfer_hide_zero = on },
    Row::Choice { id: "exp-view-main-button", key: "core_expert.view_main_button", values: &[1, 2, 3, 4, 6], labels: &[Key("core_expert.view_opt_help"), Key("core_expert.view_opt_reports"), Key("core_expert.view_opt_alerts"), Key("core_expert.view_opt_assets"), Lit("BackTest")], get: |v| i32::from(v.main_button), set: |d, x| d.view.main_button = to_u8(x) },
    Row::Step { id: "exp-view-sort-col", key: "core_expert.view_markets_sort_col", floor: -1, get: |v| v.markets_sort_col, set: |d, x| d.view.markets_sort_col = x },
    Row::Choice { id: "exp-view-sort-dir", key: "core_expert.view_markets_sort_dir", values: &[1, -1], labels: &[Key("core_expert.view_opt_ascending"), Key("core_expert.view_opt_descending")], get: |v| v.coins_sort_order, set: |d, x| d.view.coins_sort_order = x },
    Row::Flag { id: "exp-view-strat-num", key: "core_expert.view_strat_numbers", get: |v| v.show_strat_numbers, set: |d, on| d.view.show_strat_numbers = on },
    Row::Flag { id: "exp-view-spy", key: "core_expert.view_spy_mode", get: |v| v.spy_hide_mode, set: |d, on| d.view.spy_hide_mode = on },
    Row::Flag { id: "exp-view-ny", key: "core_expert.view_hide_new_year", get: |v| v.hide_ny_elka, set: |d, on| d.view.hide_ny_elka = on },
];

// One row per line, on purpose: the row's id, caption, reader and writer stay side by side,
// which is what makes a mismatched pair visible. rustfmt would spread each over a dozen lines.
#[rustfmt::skip]
const NEWS: &[Row] = &[
    Row::Choice { id: "exp-view-news-theme", key: "core_expert.view_news_theme", values: &[0, 1, 2, 3, 4, 5, 6, 7, 8], labels: &[Lit("Light Paper"), Lit("Light Default"), Lit("Light Classic"), Lit("Light Quartz"), Lit("Light Mist"), Lit("Light Sand"), Lit("Dark Carbon"), Lit("Dark Navy"), Lit("Dark Slate")], get: |v| v.news_theme, set: |d, x| d.view.news_theme = x },
    Row::Flag { id: "exp-view-news-top", key: "core_expert.view_news_on_top", get: |v| v.news_stay_on_top, set: |d, on| d.view.news_stay_on_top = on },
    Row::Flag { id: "exp-view-news-time", key: "core_expert.view_news_exact_time", get: |v| v.news_exact_time, set: |d, on| d.view.news_exact_time = on },
    Row::Flag { id: "exp-view-news-translate", key: "core_expert.view_news_translate", get: |v| v.news_translate, set: |d, on| d.view.news_translate = on },
    Row::Flag { id: "exp-view-news-tags", key: "core_expert.view_news_full_tags", get: |v| v.news_full_tags, set: |d, on| d.view.news_full_tags = on },
    Row::Flag { id: "exp-view-news-feed", key: "core_expert.view_news_feed_mode", get: |v| v.news_feed_mode, set: |d, on| d.view.news_feed_mode = on },
    Row::Step { id: "exp-view-news-font", key: "core_expert.view_news_font", floor: 0, get: |v| i32::from(v.news_font_size), set: |d, x| d.view.news_font_size = to_u8(x) },
    Row::Step { id: "exp-view-news-card-font", key: "core_expert.view_news_card_font", floor: 0, get: |v| v.news_coin_card_font, set: |d, x| d.view.news_coin_card_font = x },
    Row::Step { id: "exp-view-news-sound", key: "core_expert.view_news_sound", floor: 0, get: |v| i32::from(v.news_sound), set: |d, x| d.view.news_sound = to_u8(x) },
    Row::Choice { id: "exp-view-ai-provider", key: "core_expert.view_ai_provider", values: &[0, 1, 2, 3, 4, 5, 6], labels: &[Lit("OpenAI"), Lit("Anthropic"), Lit("Google Gemini"), Lit("xAI Grok"), Lit("DeepSeek"), Lit("Perplexity"), Lit("OpenRouter")], get: |v| i32::from(v.ai_card_provider), set: |d, x| d.view.ai_card_provider = to_u8(x) },
    Row::Text { id: "exp-view-ai-model", key: "core_expert.view_ai_model", get: |v| v.ai_card_model.clone(), set: |d, t| d.view.ai_card_model = t.trim().to_string() },
    Row::Text { id: "exp-view-ai-prompt", key: "core_expert.view_ai_prompt", get: |v| v.ai_card_prompt.clone(), set: |d, t| d.view.ai_card_prompt = t.to_string() },
];

/// Moonbot's main styles, by `theme.current_style`. Proper names in every language.
// One row per line, on purpose: the row's id, caption, reader and writer stay side by side,
// which is what makes a mismatched pair visible. rustfmt would spread each over a dozen lines.
#[rustfmt::skip]
const STYLE_LABELS: &[Label] = &[Lit("Native"), Lit("Light"), Lit("Diamond"), Lit("Dark"), Lit("Dark OldSchool")];

/// The Interface page's own style selector, live over `view.current_style`.
///
/// Built here rather than on that page so the style's labels sit with the area that owns the
/// field; the page keeps the row where Moonbot draws it, and passes its own control `id`.
pub(super) fn style_selector(
    id: &'static str,
    draft: &CoreConfig,
    view: &Entity<CoreExpertView>,
    cx: &App,
) -> AnyElement {
    let position = usize::try_from(draft.view.current_style)
        .ok()
        .filter(|&i| i < STYLE_LABELS.len())
        .unwrap_or(STYLE_LABELS.len());
    let items = options(id, STYLE_LABELS);
    choice_live(
        id,
        &items,
        u8::try_from(position).unwrap_or(u8::MAX),
        true,
        view,
        cx,
        |d, i| d.view.current_style = i32::from(i),
    )
    .into_any_element()
}

/// Every row of every section, for the declarations the window derives.
fn all_rows() -> impl Iterator<Item = &'static Row> {
    ViewSection::ALL.into_iter().flat_map(|s| s.rows().iter())
}

/// See [`super::field_specs`]: the text rows.
#[allow(clippy::type_complexity)]
pub(super) fn field_specs(
    draft: &CoreConfig,
) -> Vec<(&'static str, String, fn(&mut CoreConfig, &str))> {
    all_rows()
        .filter_map(|row| match *row {
            Row::Text { id, get, set, .. } => Some((id, get(&draft.view), set)),
            _ => None,
        })
        .collect()
}

/// See [`super::slider_specs`]: the slider rows.
#[allow(clippy::type_complexity)]
pub(super) fn slider_specs(
    draft: &CoreConfig,
) -> Vec<(
    &'static str,
    (f32, f32, f32),
    f32,
    fn(&mut CoreConfig, f32),
    Option<&'static str>,
)> {
    all_rows()
        .filter_map(|row| match *row {
            Row::Slide {
                id,
                bounds,
                get,
                set,
                ..
            } => Some((id, bounds, get(&draft.view), set, None)),
            _ => None,
        })
        .collect()
}

/// A selector's items: `(position, element id, label)`, the position standing for the wire value
/// at that index of the row's `values`.
fn options(id: &'static str, labels: &[Label]) -> Vec<(u8, SharedString, SharedString)> {
    labels
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let text = match *label {
                Key(key) => t!(key).to_string(),
                Lit(text) => text.to_string(),
            };
            (
                u8::try_from(i).unwrap_or(u8::MAX),
                SharedString::from(format!("{id}-{i}")),
                SharedString::from(text),
            )
        })
        .collect()
}

/// Build one row.
fn row(
    r: Row,
    view: &Entity<CoreExpertView>,
    store: &EditorStore,
    v: &ViewSettings,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let labeled = |key: &'static str, control: AnyElement| {
        h_flex()
            .w_full()
            .items_center()
            .gap(design::ui_px(cx, 8.0))
            .child(caption(t!(key).to_string(), true, p, cx))
            .child(div().flex_1())
            .child(control)
            .into_any_element()
    };
    match r {
        Row::Flag { id, key, get, set } => flag_live(id, key, get(v), view, set).into_any_element(),
        Row::Choice {
            id,
            key,
            values,
            labels,
            get,
            set,
        } => {
            let current = get(v);
            let position = values
                .iter()
                .position(|&value| value == current)
                .unwrap_or(values.len());
            let items = options(id, labels);
            let control = choice_live(
                id,
                &items,
                u8::try_from(position).unwrap_or(u8::MAX),
                true,
                view,
                cx,
                move |d, i| {
                    if let Some(&value) = values.get(usize::from(i)) {
                        set(d, value);
                    }
                },
            )
            .into_any_element();
            labeled(key, control)
        }
        Row::Step {
            id,
            key,
            floor,
            get,
            set,
        } => labeled(
            key,
            stepper_live(id, get(v), floor, view, set).into_any_element(),
        ),
        Row::Slide { id, key, shown, .. } => v_flex()
            .w_full()
            .gap(design::ui_px(cx, 2.0))
            .child(caption(t!(key, v = shown(v)).to_string(), true, p, cx))
            .children(slider(store, id, true))
            .into_any_element(),
        Row::Sound { id, key, get, set } => labeled(
            key,
            sound_cell(id, get(v), view, set, p, cx).into_any_element(),
        ),
        Row::Text { id, key, get, .. } => v_flex()
            .w_full()
            .gap(design::ui_px(cx, 2.0))
            .child(caption(t!(key).to_string(), true, p, cx))
            // Editable only while the value is one line: a single-line box neither shows a second
            // line nor could keep it, so an edit would replace a multi-line prompt with its first
            // line. Such a value round-trips untouched.
            .children(field(store, id, !get(v).contains(['\r', '\n'])))
            .into_any_element(),
    }
}

/// A section header that opens its body, as the Special page's does.
fn header(
    section: ViewSection,
    open: bool,
    view: &Entity<CoreExpertView>,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    let view = view.clone();
    h_flex()
        .id(SharedString::from(format!("exp-view-head-{section:?}")))
        .w_full()
        .items_center()
        .justify_center()
        .relative()
        .px(design::ui_px(cx, 8.0))
        .py(design::ui_px(cx, 4.0))
        .rounded(design::r_button(cx))
        .bg(rgb(p.shell_high))
        .border_1()
        .border_color(rgb(p.border))
        // Facet skin: the bar is a raised control standing off the page.
        .when(design::is_facet(cx), |this| {
            design::facet_surface(this, moon_ui::MoonFacetKind::Raised, cx)
        })
        .cursor_pointer()
        .on_click(move |_, _window, app| {
            view.update(app, |this, cx| this.set_view_section(section, cx));
        })
        .child(div().absolute().left(design::ui_px(cx, 8.0)).child(caption(
            if open { "˄" } else { "˅" }.to_string(),
            false,
            p,
            cx,
        )))
        .child(caption(section.title(), true, p, cx))
}

/// Build the sections. Only the open one is built, for the reason the Special page gives.
pub(super) fn body(
    view: &Entity<CoreExpertView>,
    store: &EditorStore,
    draft: &CoreConfig,
    open: ViewSection,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let gap = design::ui_px(cx, 6.0);
    let v = &draft.view;
    let rows = open.rows();
    // Two columns, the first taking the extra row of an odd count.
    let split = rows.len().div_ceil(2);
    let column = |part: &[Row]| {
        v_flex()
            .flex_1()
            .min_w_0()
            .gap(gap)
            .children(part.iter().map(|&r| row(r, view, store, v, p, cx)))
    };
    let mut open_body = Some(
        h_flex()
            .w_full()
            .items_start()
            .gap(design::ui_px(cx, 16.0))
            .child(column(&rows[..split]))
            .child(column(&rows[split..]))
            .into_any_element(),
    );
    v_flex()
        .w_full()
        .gap(gap)
        .child(caption(
            t!("core_expert.view_frame").to_string(),
            true,
            p,
            cx,
        ))
        .children(ViewSection::ALL.into_iter().map(|section| {
            v_flex()
                .w_full()
                .gap(gap)
                .child(header(section, section == open, view, p, cx))
                .children((section == open).then(|| open_body.take()).flatten())
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests;
