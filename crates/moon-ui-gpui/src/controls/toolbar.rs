//! Compose the trading toolbar: size, leverage with an exchange max-order readout, stop loss, TP/S
//! slots, and Live controls.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::feed::{CoreConfigArea, CoreConfigRejection};
use moon_core::session::CoreId;
use rust_i18n::t;

use moon_ui::{
    MoonButton, MoonButtonIconSlot, MoonButtonSegment, MoonButtonVariant,
    MoonContextMenuWindowExt as _, MoonInputState, MoonMenuItem, MoonPalette, MoonText, MoonTheme,
    MoonToggle, MoonToggleSize, MoonWindowExt as _, h_flex,
};

use super::DASH;
use super::metric::{metric_button, sl_toggle};
use super::strips::{self, sell_strip, size_strip};
use super::{MaxOrderReadout, STRATEGIES_ICON, TradeMetric, fmt_field2, fmt_field2_signed};
use crate::backend::{ManualSource, ManualStop};
use crate::panels::common::text_tooltip;
use crate::shell::Shell;
use crate::{Backend, design};
use moon_core::util::fmt;

mod compose;
mod cores;
mod fit;
mod launch;
mod strip;

pub use compose::*;
pub(crate) use cores::*;
use fit::*;
use launch::*;
use strip::*;

#[cfg(test)]
mod tests;

/// Base gap between the Sell caption and its first percentage cell.
const SELL_CAPTION_GAP: f32 = 8.0;

/// Button widths for this row — the ONE home of these numbers.
///
/// The text-bearing ones are passed through `design::font_w` at the point of use: their consumer
/// (`MoonButton::width`) puts the value into `px(..)` verbatim, so a raw width would squeeze a
/// label that grows with the legacy font-delta channel — the same ailment the preset cells had.
///
/// Icon-only launchers use [`design::glyph_btn_w`], which follows the control tier. The
/// [`ICON_BTN_W`] constant is retained only because a contract test pins its name.
const LEV_W: f32 = 61.6;
/// Base width of the stop-loss metric button.
const SL_W: f32 = 58.0;
/// Base width of the take-profit metric button.
const TP_W: f32 = 74.6;
/// Base width of the Live/Pause button.
const LIVE_W: f32 = 62.0;
/// Unused width constant kept because a theme-contract test pins this name. Live icon-only
/// buttons use [`design::glyph_btn_w`].
#[allow(dead_code)]
pub(crate) const ICON_BTN_W: f32 = 30.0;
/// Font weight used by the toolbar launchers' localized text segments.
const TOOLBAR_LAUNCHER_TEXT_WEIGHT: f32 = 500.0;
/// Two raw one-pixel borders enclosing a labeled Soft button's horizontal content.
const TOOLBAR_LAUNCHER_BORDER_W: f32 = 2.0;
/// Horizontal inset on each side of a labeled launcher. Action/Sm ship with
/// `pad_x = 0` so icon-only targets stay square; labeled buttons must opt into the same
/// 7-unit inset used by other Action labels (`core_settings_popup`, connections tab).
const TOOLBAR_LAUNCHER_PAD_X: f32 = 7.0;
/// Caption of the sell group — unlike `Size` it carries no unit, the cells already show percents.
const SELL_CAPTION: &str = "Sell";
/// Stable unit for group-local manual order-size equivalents.
const SIZE_UNIT: &str = "USDT eq.";

/// The localized labels of the three singleton-window launchers at the row's trailing edge.
///
/// Grouped because they are ONE fact — the trailing cluster's text — and the budget reads all
/// three or none of them. Passing them separately also pushed [`row_fit`] past the argument count
/// where a reader stops tracking which string is which.
struct LauncherLabels<'a> {
    analytics: &'a str,
    strategies: &'a str,
    settings: &'a str,
}

/// Incremental widths of the optional labels above the icon-only row.
#[derive(Clone, Copy, Debug)]
struct LabelWidths {
    /// Complete row width with every optional label removed.
    icon_only: f32,
    /// Width of the compact size unit caption.
    size_unit: f32,
    /// Extra width that expands the unit caption to `Size, USDT eq.`.
    size_noun: f32,
    /// Extra width of the Settings launcher label.
    settings: f32,
    /// Extra width of the Strategies launcher label.
    strategies: f32,
    /// Extra width of the Analytics launcher label.
    analytics: f32,
    /// Width of the Sell caption.
    sell: f32,
    /// Extra width of the caption naming the exchange max-order value.
    ///
    /// The VALUE itself is not on this ladder: it is permanently visible and therefore part of the
    /// unsheddable `controls` budget instead — only the word naming it may go.
    max_order_caption: f32,
}

/// Visibility of every optional label at one available row width.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct LabelLadder {
    size_unit: bool,
    size_noun: bool,
    settings: bool,
    strategies: bool,
    analytics: bool,
    sell: bool,
    max_order_caption: bool,
}

/// One of the trailing window launchers, named in the order they FOLD into the overflow menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Launcher {
    Settings,
    Analytics,
    Strategies,
    Screener,
    ProfitMonitor,
}

/// Fold order: the rightmost launcher goes first, because it is the one that clipped first
/// before the overflow button existed. The index of a launcher here is its fold rung.
const LAUNCHER_FOLD_ORDER: [Launcher; 5] = [
    Launcher::Settings,
    Launcher::Analytics,
    Launcher::Strategies,
    Launcher::Screener,
    Launcher::ProfitMonitor,
];

/// Budget inputs of the overflow fold, on top of the label ladder's icon-only row.
#[derive(Clone, Copy, Debug)]
struct LauncherFoldWidths {
    /// Complete row width with every optional label removed and every launcher visible — the
    /// same figure as [`LabelWidths::icon_only`].
    icon_only: f32,
    /// What one icon-only launcher costs the row: its glyph button plus the gap in front of it.
    launcher: f32,
    /// What the overflow button costs the row: its glyph button plus the gap in front of it.
    overflow: f32,
}

/// How many trailing launchers the row folds into its overflow menu, and the row width it keeps.
#[derive(Clone, Copy, Debug, PartialEq)]
struct LauncherFold {
    /// Count of launchers folded, taken from the head of [`LAUNCHER_FOLD_ORDER`].
    folded: usize,
    /// Budgeted row width after the fold, overflow button included when `folded > 0`.
    width: f32,
    /// Even the fully folded row is wider than the window: the trading controls alone outgrow it.
    /// The overflow button then pins itself to the row's right edge, over the clipped tail of
    /// the trading controls, because at the end of the flow it would sit past the window's edge.
    pinned: bool,
}

impl LauncherFold {
    /// Whether `launcher` is drawn on the row rather than listed in the overflow menu.
    fn shows(self, launcher: Launcher) -> bool {
        LAUNCHER_FOLD_ORDER
            .iter()
            .position(|&l| l == launcher)
            .is_some_and(|rung| rung >= self.folded)
    }
}

/// The optional labels the row renders at the current window width, already resolved to the values
/// it renders — see [`row_fit`]. `None` means that label does not fit and is not drawn.
struct RowFit {
    /// Which trailing launchers are drawn and which sit in the overflow menu.
    launchers: LauncherFold,
    size_caption: Option<SharedString>,
    sell_caption: Option<SharedString>,
    /// Complete Analytics-button width when its label fits; `None` renders it icon-only.
    analytics_width: Option<f32>,
    /// Complete Strategies-button width when its label fits; `None` renders it icon-only.
    strategies_width: Option<f32>,
    /// Complete Settings-button width when its label fits; `None` renders it icon-only.
    settings_width: Option<f32>,
    /// The caption naming the permanent max-order figure; `None` leaves the value identified by its tooltip.
    max_order_caption: Option<SharedString>,
}

/// Singleton-window entry point shared by every trailing launcher.
type OpenWindow = fn(Entity<Backend>, Option<AnyWindowHandle>, Option<DisplayId>, &mut App);

/// Everything one trailing launcher needs, whether it is drawn on the row or listed in the
/// overflow menu — one description, so the two places cannot open different things.
struct LaunchTarget {
    /// Stable button element identity; doubles as the overflow menu row key.
    id: &'static str,
    /// Localized launcher name: visible label, icon tooltip or menu row text.
    label: String,
    /// MoonUI asset path for the launcher glyph.
    icon: &'static str,
    /// Fixed labeled width, or `None` for an icon-only button.
    labeled_width: Option<f32>,
    /// Group to record before opening a workspace-scoped singleton.
    workspace_owner: Option<String>,
    /// Singleton-window entry point.
    open: OpenWindow,
}
