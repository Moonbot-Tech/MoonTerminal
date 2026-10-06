//! Charts of the "Summary" tab: daily bars, horizontal per-core rankings, and the
//! bucket-popup chrome these charts share with the cumulative chart. The cumulative
//! chart itself lives in `cumulative`.

/// Summary chart axis helpers.
mod axis;
/// Summary chart bucket helpers.
mod bucket;
/// Summary chart daily helpers.
mod daily;
/// Summary chart hover helpers.
mod hover;
/// Summary chart popup helpers.
mod popup;
/// Summary chart ranking helpers.
mod ranking;

pub(super) use axis::thinned_labels;
pub(super) use axis::{
    bucket_label, core_color_by_uid, distinct_core_colors, dm, fallback_core_color,
};
pub(super) use bucket::{bucket_popup, kind_bars};
pub(super) use daily::daily_bars;
pub(in crate::analytics) use hover::PopupHover;
pub(super) use hover::{
    CHART_H, LABEL_WEIGHT, PLOT_W_NOMINAL, PopupKey, chart_hover, widest_label_w,
};
use popup::profit_trades;
pub(super) use popup::{PopupMode, PopupRow, core_color, popup_card};
#[cfg(test)]
use popup::{popup_limits, popup_outer_width};
pub(super) use ranking::{CoreRankStats, core_rank_stats, core_totals_rank};
#[cfg(test)]
use ranking::{core_rank_rows, overview_ranges};

use super::{fmt_signed, sign_color};

use std::ops::Range;
use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonPalette, MoonProgress, MoonScrollAxis, MoonScrollbarVisibility, MoonVirtualList, h_flex,
    moon_scrollbar_overlay_with_palette, v_flex,
};
use rust_i18n::t;

use super::super::AnalyticsView;
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::analytics::{CoreSeries, DayPoint, KindStat};

#[cfg(test)]
mod tests;

/// X-axis labels of the daily-bars chart: the first and last date.
fn axis_row(p: MoonPalette, left: String, right: String) -> AnyElement {
    h_flex()
        .w_full()
        .justify_between()
        .child(muted_caption(p, left))
        .child(muted_caption(p, right))
        .into_any_element()
}

pub(super) fn muted_caption(p: MoonPalette, text: String) -> Div {
    div().text_color(moon(p.text_muted)).child(text)
}
