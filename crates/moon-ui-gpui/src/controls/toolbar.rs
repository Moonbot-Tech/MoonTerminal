//! Compose the trading toolbar: labelled pods for the manual strategy, order size, leverage with
//! the exchange's limits, stop, take and fixed sell, then the chart tools and Live.
//!
//! Built in three steps every frame: [`ToolbarModel::read`] resolves the state, [`toolbar_widths`]
//! measures every element group, and [`render_toolbar`] draws the row at the collapse level those
//! widths pick (`design::ToolbarLevel`). `Shell` holds the level between the steps, so the popups
//! whose triggers a level hides close in the same frame.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::feed::{CoreConfigArea, CoreConfigRejection};
use moon_core::session::CoreId;
use rust_i18n::t;

use moon_ui::{
    MoonButton, MoonButtonIconSlot, MoonButtonSegment, MoonButtonVariant, MoonInputState,
    MoonMenuItem, MoonPalette, MoonText, h_flex,
};

use super::DASH;
use super::manual_strat::StratPod;
use super::metric::{
    MetricOpen, SlWrite, metric_button, metric_popup_anchor, metric_value_width,
    open_metric_action, sl_switch_width, sl_toggle, sl_toggle_action,
};
use super::strips::{self, StripForm, sell_strip, size_strip};
use super::{MaxOrderReadout, TradeMetric, fmt_field2, fmt_field2_signed};
use crate::backend::{ManualSource, ManualStop};
use crate::design::{ToolbarLevel, ToolbarWidths};
use crate::panels::common::text_tooltip;
use crate::shell::Shell;
use crate::{Backend, design};
use moon_core::util::fmt;

mod compose;
mod cores;
mod fit;
mod strip;

use crate::controls::launch::{LaunchTarget, open_window_button, overflow_button, overflow_row};
pub(crate) use compose::*;
pub(crate) use cores::*;
pub(crate) use fit::*;
use strip::*;

#[cfg(test)]
mod tests;

/// The fixed unit beside the Size caption: every size preset is one USDT equivalent for the group.
const SIZE_POD_UNIT: &str = "USDT";
