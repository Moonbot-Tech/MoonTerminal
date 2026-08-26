//! Shared per-tab chart-stack settings plumbing for TWO hosts: the tab strip (`ChartTabs`, active
//! tab) and a detached window (`DetachedChartHost`, its panel). This module contains the setting
//! value ([`StackSetting`]: apply to a stack and write to a spec), spec upsert, the
//! [`LayoutPopupHost`] trait with shared layout-popup logic (open/seed/read/commit/close plus
//! setting application) and shared overlay/dismiss rendering, and the [`CoinPopupHost`] trait with
//! coin-search input plumbing. Every overlay a host shows — the four settings popups, the drawing
//! tool's defaults panel and the market list — is opened and closed through `LayoutPopupHost`'s one
//! [`super::popup_slot::PopupSlot`], which is what makes them mutually exclusive. Host-specific
//! differences (the target tab/panel, Apply to all, and `is_custom`) remain in the trait
//! implementations ([`super::settings`] / [`super::detached_host`]).

use gpui::*;
use moon_ui::{
    MoonAccent, MoonInputEvent, MoonInputState, MoonPalette, MoonPopover, MoonPopoverPlacement,
    MoonSegmentItem, MoonSegmentedControl, v_flex,
};

use super::add_stack::detect_cap::{resolved_max_charts, resolved_max_charts_evict};
use super::popup_slot::{ChartPopup, PopupSlot};
use super::{layout_popup, stack};
use crate::Backend;
use crate::design;
use crate::persistence::chart_persist::{
    self, ChartBtnPos, PriceAxisPos, StackLayoutMode, StackOrientation,
};
use moon_core::config::ChartBucket;
use moon_core::session::CoreId;

/// Build one popup setting as a caption with a segmented control below it.
///
/// Shared by the candle and graphics popups, which had a byte-identical copy each. It takes only
/// primitives, so it is coupled to neither host trait and neither settings type.
///
/// Args:
///     id: Element identity for the segmented control.
///     caption: Localized label drawn above it.
///     labels: One `(text, selected)` pair per segment, in display order.
///     seg_w: Width of one segment, in design units.
///     p: Active palette, for the caption colour.
///     cx: App context, for the caption text size.
///     on_pick: Receives the picked segment index.
///
/// Returns:
///     The caption and its segmented control as one column.
pub(crate) fn seg_row(
    id: String,
    caption: String,
    labels: Vec<(String, bool)>,
    seg_w: f32,
    p: MoonPalette,
    cx: &App,
    on_pick: impl Fn(usize, &mut App) + 'static,
) -> impl IntoElement {
    let items: Vec<MoonSegmentItem> = labels
        .into_iter()
        .map(|(label, selected)| {
            let mut it = MoonSegmentItem::new("", label).width(seg_w);
            if selected {
                it = it.selected(true);
            }
            it
        })
        .collect();
    let seg = MoonSegmentedControl::new(id)
        .accent(MoonAccent::Blue)
        .items(items)
        .on_click(move |ix, _, _, cx| on_pick(ix, cx))
        .render();
    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 2.0))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text))
                .child(caption),
        )
        .child(seg)
}

/// One per-tab chart-stack setting value. It can write itself to a spec (the shared persistence
/// half); [`set_stack_setting!`] applies it to panels using identically named and typed setters on
/// `MainChartStack` and `AddChartStack`.
///
/// `pub(crate)` rather than `pub(super)` because a detached window's ⧉ press travels through
/// Backend as a list of these; see [`super::apply_all`].
/// Not `Copy`: the caption configuration it carries owns a name string per row, and a value that
/// silently copies a heap-owning payload through a walk that touches every tab is a cost nobody
/// sees until it is large.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StackSetting {
    /// Layout mode plus separate Fit and Scroll heights.
    Layout(Option<StackLayoutMode>, Option<u16>, Option<u16>),
    /// Stack orientation (vertical/horizontal), where `None` means the Vertical default.
    ///
    /// Optional rather than resolved so that ⧉ copies an unset orientation AS unset, the way it did
    /// before the walk was shared: a spec that never named one keeps naming none.
    Orientation(Option<StackOrientation>),
    /// Order book enabled/disabled.
    Orderbook(bool),
    /// Liquidation trades enabled/disabled.
    Liquidations(bool),
    /// Management-zone fill enabled/disabled.
    ShowZone(bool),
    /// Automatic pinning on order enabled/disabled.
    AutoPin(bool),
    /// Cancel Buy / Panic Sell button positions.
    ActionPos(Option<ChartBtnPos>, Option<ChartBtnPos>),
    /// Price-axis position.
    PriceAxis(PriceAxisPos),
    /// Time-axis visibility.
    TimeAxis(bool),
    /// Order-line labels.
    LineLabels(bool),
    /// Crosshair labels.
    CursorLabels(bool),
    /// Candle/trade display settings (the candlestick popup).
    CandleView(moon_core::market::CandleViewCfg),
    /// Chart-drawing settings: trade-arrow size, connector thickness, which closed trades are drawn,
    /// whether a closed order keeps its sell line, the trade-mark size and the bottom volume band
    /// (the palette popup).
    Graphics(moon_core::config::ChartGraphicsCfg),
    /// Which captions the chart prints beside its plot, where, and in which style (the labels popup).
    Labels(moon_core::config::ChartLabelsCfg),
    /// Price scale, where `None` means Auto. Copied by the ⚙ popup's ⧉ along with the layout.
    Scale(Option<f32>),
    /// Whether an arriving chart flashes its accent border.
    ArrivalFlash(bool),
    /// Screen divider: `(columns, exact, minimum slot)`. The three travel together because none of
    /// them lays anything out alone — a divider with no policy has no layout, and a minimum with no
    /// divider has nothing to divide.
    Grid(Option<u8>, Option<bool>, Option<u16>),
    /// Detect cap and what a detect does at it: `(cap, replace the stalest)`. `None` copies "follow
    /// the built-in default"; `Some(0)` copies "uncapped". The two travel together because a press
    /// that moved only one of them would leave the other saying something the reader never chose.
    MaxCharts(Option<u16>, bool),
}

/// A setting that ALSO has a default in `layout.toml`, inherited by every tab without an override
/// of its own — one per KIND of tab, see `moon_core::config::ChartTabKind`.
///
/// A ⧉ press stores one of those defaults, which is what makes new tabs — and every tab still
/// following it — adopt the pressed values. It names the slot only: the VALUE always travels as the
/// [`StackSetting`] itself, so the two cannot disagree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GlobalSlot {
    /// The candle defaults, from the "Candles and Trades" popup.
    CandleView,
    /// The chart-graphics defaults, from the "Chart graphics" popup.
    Graphics,
    /// The caption defaults, from the "Chart labels" popup.
    Labels,
}

impl GlobalSlot {
    /// Store a pressed value as one KIND's default, reporting whether it actually moved.
    ///
    /// The caller uses that answer to skip marking the file dirty for a press that stored what was
    /// already there.
    pub(crate) fn write_default(
        self,
        layout: &mut moon_core::config::WindowLayout,
        kind: moon_core::config::ChartTabKind,
        value: StackSetting,
    ) -> bool {
        match (self, value) {
            (GlobalSlot::CandleView, StackSetting::CandleView(v)) => {
                layout.set_candle_view_default(kind, v)
            }
            // NORMALIZED and SANITIZED on the way in, for the reason the panel does it on the way
            // out: this value is COMPARED, and `layout.toml` is hand-editable, so a stored
            // impossibility would look like a change on every notification for the rest of time.
            (GlobalSlot::Graphics, StackSetting::Graphics(v)) => {
                layout.set_chart_graphics_default(kind, moon_chart::normalize_chart_graphics(v))
            }
            (GlobalSlot::Labels, StackSetting::Labels(mut v)) => {
                v.sanitize();
                layout.set_chart_labels_default(kind, v)
            }
            // Unreachable through `StackSetting::global_slot`, which pairs slot and value by
            // construction. Storing a mismatched value would be worse than storing none.
            _ => false,
        }
    }

    /// The Main stack's own value for this setting, or `None` when it follows its kind's default.
    pub(crate) fn main_value(self, main: &super::MainChartStack) -> Option<StackSetting> {
        match self {
            GlobalSlot::CandleView => main.candle_view().map(StackSetting::CandleView),
            GlobalSlot::Graphics => main.chart_graphics().map(StackSetting::Graphics),
            GlobalSlot::Labels => main.chart_labels().map(StackSetting::Labels),
        }
    }

    /// One stack's own value for this setting, or `None` when it follows its kind's default.
    pub(crate) fn stack_value(self, stack: &super::AddChartStack) -> Option<StackSetting> {
        match self {
            GlobalSlot::CandleView => stack.candle_view().map(StackSetting::CandleView),
            GlobalSlot::Graphics => stack.chart_graphics().map(StackSetting::Graphics),
            GlobalSlot::Labels => stack.chart_labels().map(StackSetting::Labels),
        }
    }

    /// Drop the Main stack's own value for this setting, so it follows its kind's default again.
    pub(crate) fn clear_on_main(
        self,
        main: &mut super::MainChartStack,
        cx: &mut Context<super::MainChartStack>,
    ) {
        match self {
            GlobalSlot::CandleView => main.set_candle_view(None, cx),
            GlobalSlot::Graphics => main.set_chart_graphics(None, cx),
            GlobalSlot::Labels => main.set_chart_labels(None, cx),
        }
    }

    /// Drop one stack's own value for this setting, so it follows its kind's default again.
    pub(crate) fn clear_on_stack(
        self,
        stack: &mut super::AddChartStack,
        cx: &mut Context<super::AddChartStack>,
    ) {
        match self {
            GlobalSlot::CandleView => stack.set_candle_view(None, cx),
            GlobalSlot::Graphics => stack.set_chart_graphics(None, cx),
            GlobalSlot::Labels => stack.set_chart_labels(None, cx),
        }
    }
}

impl StackSetting {
    /// The global default this setting inherits from, when it has one.
    pub(crate) fn global_slot(&self) -> Option<GlobalSlot> {
        match self {
            StackSetting::CandleView(_) => Some(GlobalSlot::CandleView),
            StackSetting::Graphics(_) => Some(GlobalSlot::Graphics),
            StackSetting::Labels(_) => Some(GlobalSlot::Labels),
            _ => None,
        }
    }

    /// Whether this setting means anything on a tab described by these two facts.
    ///
    /// Applicability lives on the SETTING, beside `global_slot` and `rebuilds_orderbook_demand`,
    /// rather than in the popup that draws it — the popup is only one of the places that decides.
    /// The ⧉ walk is the other, and it addresses tabs by KIND: a custom tab that is not detached is
    /// kind `AddTo` like any other, so a press that skipped the control on screen would still have
    /// written it into that tab's spec — a cap nobody could see and nobody could clear.
    pub(crate) fn applies_to(&self, is_main: bool, is_custom: bool) -> bool {
        match self {
            // Main draws no arrival flash at all.
            StackSetting::ArrivalFlash(_) => !is_main,
            // Ingest routes detects to numbered AddToChart tabs only: Main is opened by hand, and a
            // custom tab holds the markets its owner picked.
            StackSetting::MaxCharts(..) => !is_main && !is_custom,
            _ => true,
        }
    }

    /// Whether applying this setting can change which markets need an order book.
    ///
    /// One definition for both callers: the single-setting path below and the ⧉ walk, which rebuilds
    /// demand once for a whole press.
    pub(crate) fn rebuilds_orderbook_demand(&self) -> bool {
        matches!(self, StackSetting::Orderbook(_))
    }

    /// Write the value to a tab spec (the persistence half of `apply_*`, shared by both hosts).
    pub(super) fn write_spec(self, s: &mut chart_persist::ChartTabSpec) {
        match self {
            StackSetting::Layout(mode, hf, hs) => {
                s.layout_mode = mode;
                s.layout_height_fit = hf;
                s.layout_height_scroll = hs;
            }
            StackSetting::Orientation(o) => s.layout_orientation = o,
            StackSetting::Orderbook(v) => s.orderbook_enabled = Some(v),
            StackSetting::Liquidations(v) => s.liquidations_enabled = Some(v),
            StackSetting::ShowZone(v) => s.show_zone = Some(v),
            StackSetting::AutoPin(v) => s.auto_pin = Some(v),
            StackSetting::ActionPos(cancel, panic) => {
                s.cancel_buy_pos = cancel;
                s.panic_sell_pos = panic;
            }
            StackSetting::PriceAxis(p) => s.price_axis_pos = Some(p),
            StackSetting::TimeAxis(v) => s.time_axis_visible = Some(v),
            StackSetting::LineLabels(v) => s.line_labels = Some(v),
            StackSetting::CursorLabels(v) => s.cursor_labels = Some(v),
            StackSetting::CandleView(v) => s.candle_view = Some(v),
            StackSetting::Graphics(v) => s.chart_graphics = Some(v),
            StackSetting::Labels(v) => s.chart_labels = Some(v),
            StackSetting::Scale(v) => s.scale = v,
            StackSetting::ArrivalFlash(v) => s.arrival_flash = Some(v),
            StackSetting::Grid(columns, exact, min_slot) => {
                s.layout_columns = columns;
                s.layout_columns_exact = exact;
                s.layout_min_slot = min_slot;
            }
            StackSetting::MaxCharts(max, evict) => {
                s.max_charts = max;
                s.max_charts_evict = Some(evict);
            }
        }
    }
}

/// Apply [`StackSetting`] to stack `$s` inside `entity.update`, dispatching every setter in one
/// place. This is a macro rather than a function so it works with both `MainChartStack` and
/// `AddChartStack`: the types differ, but their setter names and signatures match.
macro_rules! set_stack_setting {
    ($s:expr, $c:expr, $v:expr) => {
        match $v {
            crate::chart_tabs::common::StackSetting::Layout(mode, hf, hs) => {
                $s.set_layout(mode, hf, hs, $c)
            }
            crate::chart_tabs::common::StackSetting::Orientation(o) => $s.set_orientation(o, $c),
            crate::chart_tabs::common::StackSetting::Orderbook(v) => {
                $s.set_orderbook_enabled(Some(v), $c)
            }
            crate::chart_tabs::common::StackSetting::Liquidations(v) => {
                $s.set_liquidations_enabled(Some(v), $c)
            }
            crate::chart_tabs::common::StackSetting::ShowZone(v) => $s.set_show_zone(Some(v), $c),
            crate::chart_tabs::common::StackSetting::AutoPin(v) => $s.set_auto_pin(Some(v), $c),
            crate::chart_tabs::common::StackSetting::ActionPos(cancel, panic) => {
                $s.set_action_btn_pos(cancel, panic, $c)
            }
            crate::chart_tabs::common::StackSetting::PriceAxis(p) => {
                $s.set_price_axis_pos(Some(p), $c)
            }
            crate::chart_tabs::common::StackSetting::TimeAxis(v) => {
                $s.set_time_axis_visible(Some(v), $c)
            }
            crate::chart_tabs::common::StackSetting::LineLabels(v) => {
                $s.set_line_labels(Some(v), $c)
            }
            crate::chart_tabs::common::StackSetting::CursorLabels(v) => {
                $s.set_cursor_labels(Some(v), $c)
            }
            crate::chart_tabs::common::StackSetting::CandleView(v) => {
                $s.set_candle_view(Some(v), $c)
            }
            crate::chart_tabs::common::StackSetting::Graphics(v) => {
                $s.set_chart_graphics(Some(v), $c)
            }
            crate::chart_tabs::common::StackSetting::Labels(v) => $s.set_chart_labels(Some(v), $c),
            crate::chart_tabs::common::StackSetting::Scale(v) => $s.set_scale(v, $c),
            crate::chart_tabs::common::StackSetting::ArrivalFlash(v) => {
                $s.set_arrival_flash(Some(v), $c)
            }
            crate::chart_tabs::common::StackSetting::Grid(columns, exact, min_slot) => {
                $s.set_layout_columns(columns, exact, min_slot, $c)
            }
            crate::chart_tabs::common::StackSetting::MaxCharts(max, evict) => {
                $s.set_max_charts(max, Some(evict), $c)
            }
        }
    };
}
pub(crate) use set_stack_setting;

/// Find or create a tab spec by group/number/bucket, apply the mutator, and mark it dirty.
/// This upsert is shared by the tab strip and detached windows.
pub(super) fn upsert_spec(
    backend: &Entity<Backend>,
    group: &str,
    num: u32,
    bucket: &ChartBucket,
    cx: &mut App,
    f: impl FnOnce(&mut chart_persist::ChartTabSpec),
) {
    let group = group.to_string();
    backend.update(cx, |b, _| {
        chart_persist::upsert(&mut b.chart_specs, &group, num, bucket, f);
        b.chart_specs_dirty = true;
    });
}

/// Snapshot of the target tab's current settings for rendering the ⚙ popup, with defaults already
/// resolved into effective values.
pub(super) struct LayoutPopupSnapshot {
    pub mode: StackLayoutMode,
    pub orientation: StackOrientation,
    pub orderbook: bool,
    pub liquidations: bool,
    pub show_zone: bool,
    pub auto_pin: bool,
    pub cancel_pos: ChartBtnPos,
    pub panic_pos: ChartBtnPos,
    pub price_axis_pos: PriceAxisPos,
    pub time_axis: bool,
    pub line_labels: bool,
    pub cursor_labels: bool,
    pub arrival_flash: bool,
    /// Whether a detect at the cap replaces the stalest chart instead of going unshown. The cap
    /// ITSELF is not here: it lives in the popup's field like the two heights, and is read from
    /// there so a number typed but not yet committed is the one that travels.
    pub max_charts_evict: bool,
}

/// Host of the ⚙ layout-settings popup. Required methods expose host state and the target
/// (`ChartTabs` targets its active tab, while `DetachedChartHost` targets the window panel);
/// default methods contain the SHARED popup and setting-application logic formerly duplicated.
pub(super) trait LayoutPopupHost: super::apply_row::ApplyRowHost + Sized + 'static {
    // --- Host popup state and inputs ---
    /// The host's ONE overlay slot, shared by every popup on its toolbar row.
    ///
    /// Every popup on a host goes through this, which is what makes them mutually exclusive; see
    /// [`super::popup_slot`] for why a slot rather than a flag each.
    fn popup_slot(&self) -> PopupSlot;
    fn popup_slot_mut(&mut self) -> &mut PopupSlot;
    fn fit_input(&self) -> &Entity<MoonInputState>;
    fn scroll_input(&self) -> &Entity<MoonInputState>;
    fn rename_input(&self) -> &Entity<MoonInputState>;
    /// The detect-cap field, committed on the way out like the two size fields.
    fn max_charts_input(&self) -> &Entity<MoonInputState>;
    /// The minimum-slot field of the screen divider, committed the same way.
    fn min_slot_input(&self) -> &Entity<MoonInputState>;

    // --- Target tab ---
    /// Which of the three kinds the tab this popup edits IS.
    ///
    /// A press starts by addressing its own kind, which is what makes the common case one click:
    /// the reader adjusting a torn-off window means the torn-off windows.
    fn source_kind(&self, cx: &App) -> moon_core::config::ChartTabKind;
    /// This window's X time scale, which only the candle popup's press carries.
    fn source_x_ppm(&self, cx: &App) -> Option<f32>;
    fn backend(&self) -> &Entity<Backend>;
    fn spec_group(&self) -> &str;
    /// Target persistence key: `(num, bucket)`.
    fn spec_key(&self) -> (u32, ChartBucket);
    /// Current target layout: `(mode, height_fit, height_scroll)`.
    fn current_layout(&self, cx: &App) -> (Option<StackLayoutMode>, Option<u16>, Option<u16>);
    fn current_orientation(&self, cx: &App) -> Option<StackOrientation>;
    /// Target detect cap as raw `Option`s: `(cap, replace the stalest)`.
    fn current_max_charts(&self, cx: &App) -> (Option<u16>, Option<bool>);
    /// Target screen divider as raw `Option`s: `(columns, exact, minimum slot)`.
    fn current_grid(&self, cx: &App) -> (Option<u8>, Option<bool>, Option<u16>);
    /// Whether this popup edits the MAIN tab. A detached window is never Main.
    ///
    /// A fact about the target rather than a per-setting predicate: which settings that fact rules
    /// out is [`StackSetting::applies_to`]'s to say, in one place, for both the popup and the ⧉
    /// walk. `source_kind` cannot stand in for it — Main with a comparison anchor reports `Compare`.
    fn target_is_main(&self, cx: &App) -> bool;
    /// Target action-button positions as raw `Option`s for independently editing cancel/panic.
    fn action_btn_pos_opt(&self, cx: &App) -> (Option<ChartBtnPos>, Option<ChartBtnPos>);
    fn layout_popup_snapshot(&self, cx: &App) -> LayoutPopupSnapshot;
    /// Whether the target is a custom multi-coin tab, controlling rename-field visibility.
    fn popup_is_custom(&self, cx: &App) -> bool;
    /// Seed the custom-tab name field; a no-op when the target is not custom.
    fn seed_rename_input(&self, window: &mut Window, cx: &mut Context<Self>);
    /// Apply a value to the target stack(s), dispatching to Main/active stack or the window panel.
    fn set_on_stacks(&mut self, v: StackSetting, cx: &mut Context<Self>);
    /// This popup's layout values as they stand right now, for the ⧉ row to perform. The two
    /// detect-flow values join them only on a target whose popup actually shows them.
    fn layout_press_values(&self, cx: &App) -> Vec<StackSetting>;

    // --- The one open overlay ---

    /// Whether `popup` is the overlay this host is showing.
    fn popup_shows(&self, popup: ChartPopup) -> bool {
        self.popup_slot().shows(popup)
    }

    /// Show `popup`, closing whichever overlay was up.
    ///
    /// The single entry point for OPENING anything on a chart host: pressing a toolbar button,
    /// focusing the market field, coming back from a modal editor. Routing them all through here is
    /// what keeps two popups from sharing the screen — nothing else on the host closes a sibling.
    ///
    /// Notifies even when `popup` was ALREADY the one showing, deliberately: the market list calls
    /// this on every keystroke, and the repaint it needs is for the query behind it rather than for
    /// the open flag. An early return here would leave the list showing matches for older text.
    ///
    /// Args:
    ///     popup: The overlay to show.
    ///     cx: Host context.
    fn open_chart_popup(&mut self, popup: ChartPopup, cx: &mut Context<Self>) {
        if let Some(displaced) = self.popup_slot_mut().show(popup) {
            self.settle_closed_popup(displaced, cx);
        }
        // The armed ⧉ row belongs to the popup that opened it: one press is shared by all four, so
        // leaving it up would show it over a popup that never armed it.
        self.apply_press_mut().open = false;
        cx.notify();
    }

    /// Hide `popup`, if it is the one showing.
    ///
    /// A no-op otherwise — a close report can arrive for a popup something else already replaced;
    /// see [`PopupSlot::hide`].
    ///
    /// Args:
    ///     popup: The overlay asking to be hidden.
    ///     cx: Host context.
    fn close_chart_popup(&mut self, popup: ChartPopup, cx: &mut Context<Self>) {
        if !self.popup_slot_mut().hide(popup) {
            return;
        }
        self.settle_closed_popup(popup, cx);
        self.apply_press_mut().open = false;
        cx.notify();
    }

    /// Route a popover's `on_open_change` report to the slot.
    ///
    /// The three settings popovers each report open and close the same way; this is that call.
    ///
    /// Args:
    ///     popup: The overlay the report is about.
    ///     open: What the popover now says its state is.
    ///     cx: Host context.
    fn report_chart_popup(&mut self, popup: ChartPopup, open: bool, cx: &mut Context<Self>) {
        match open {
            true => self.open_chart_popup(popup, cx),
            false => self.close_chart_popup(popup, cx),
        }
    }

    /// Show `popup` if it is not up, hide it if it is — for a button that is its own dismissal.
    fn toggle_chart_popup(&mut self, popup: ChartPopup, cx: &mut Context<Self>) {
        match self.popup_shows(popup) {
            true => self.close_chart_popup(popup, cx),
            false => self.open_chart_popup(popup, cx),
        }
    }

    /// Settle what a popup owes as it leaves the screen.
    ///
    /// Only ⚙ owes anything: its layout fields are committed on the way out rather than per
    /// keystroke, so a popup displaced by a press on a neighbouring button has to commit here —
    /// exactly as it does when dismissed by a click on the chart. The Custom-tab NAME field is not
    /// part of that: it commits from its own `Blur`/Enter subscription, which is gated on the ⚙
    /// popup still being the one showing, so a name typed and never confirmed is dropped on every
    /// close path. That predates the slot and is left as it was.
    ///
    /// Args:
    ///     popup: The overlay that just stopped showing.
    ///     cx: Host context.
    fn settle_closed_popup(&mut self, popup: ChartPopup, cx: &mut Context<Self>) {
        if popup == ChartPopup::Layout {
            self.commit_layout_popup(cx);
        }
    }

    /// Hand the keyboard back when the ⚙ popup closes with the focus inside one of ITS OWN fields.
    ///
    /// `MoonPopover` restores the holder it remembered when it opened, and otherwise blurs only if
    /// it finds its own handle focused — a field INSIDE it answers to neither test. So when nothing
    /// was remembered, the focus is left on an input that stops being rendered that same frame, and
    /// a focus id with no node in the frame collapses the dispatch path: every hotkey dies silently
    /// until something focusable is clicked. [`crate::hotkeys::restore_root_focus`] cannot repair
    /// that one — these fields are permanent members of the host, so the handle still resolves and
    /// the window reads as focused. Blurring here is what lets the root re-take it next frame.
    ///
    /// Reachable because ending a market search releases the keyboard rather than parking it
    /// somewhere (see [`coin_toolbar_press_handler`]), which is what leaves a popover opened by
    /// that same press with nothing to restore. The ⚙ popup is the only one of the four that has
    /// text fields at all.
    ///
    /// Args:
    ///     window: The window whose focus is being released.
    ///     cx: Application context used to read the fields' handles.
    fn release_layout_field_focus(&self, window: &mut Window, cx: &mut App) {
        for field in self.layout_fields() {
            crate::hotkeys::release_field_focus(field, window, cx);
        }
    }

    /// Close the ⚙ popup, settling what it owes the KEYBOARD on the way out.
    ///
    /// The one funnel for closing it, and it has to be: `MoonPopover` reports a close through
    /// `on_open_change` only when IT decided one — an outside click, Escape, the trigger. The ✕
    /// inside the popup is none of those; it flips the controlled flag, which `render` later turns
    /// into `set_open` + `sync_open_focus` with no report at all. A release hung off the report
    /// alone would leave the ✕ as the one exit that still strands the focus. What the popup owes
    /// the DATA — committing the size fields — travels the other way, through
    /// [`Self::settle_closed_popup`], because that one needs no window and every displacement path
    /// reaches it.
    ///
    /// Args:
    ///     window: The window whose focus may need releasing.
    ///     cx: Host context.
    fn close_layout_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_chart_popup(ChartPopup::Layout, cx);
        self.release_layout_field_focus(window, cx);
    }

    /// Every input this popup renders, as one list.
    ///
    /// Two things have to be done to ALL of them and to nothing else — releasing the keyboard on
    /// the way out, and the count the contract test holds against the trait's own getters — so the
    /// membership question is answered once, here, rather than re-enumerated at each. Seeding them
    /// stays per-field because each takes a different value.
    fn layout_fields(&self) -> [&Entity<MoonInputState>; 5] {
        [
            self.fit_input(),
            self.scroll_input(),
            self.rename_input(),
            self.max_charts_input(),
            self.min_slot_input(),
        ]
    }

    /// ARM the ⧉ press: the row that opens names which kinds of tab it reaches, and performs it
    /// with the values this popup holds AT THAT MOMENT — never a snapshot taken here.
    fn arm_apply_press(&mut self, cx: &mut Context<Self>) {
        let source = self.source_kind(cx);
        self.apply_press_mut().arm(source);
        cx.notify();
    }

    // --- Shared default logic ---

    /// Apply a setting to the target and persist it to the spec, rebuilding order-book demand for
    /// an Orderbook change.
    fn apply_tab_setting(&mut self, v: StackSetting, cx: &mut Context<Self>) {
        let rebuild = v.rebuilds_orderbook_demand();
        self.set_on_stacks(v.clone(), cx);
        let (num, bucket) = self.spec_key();
        let backend = self.backend().clone();
        upsert_spec(&backend, self.spec_group(), num, &bucket, cx, move |s| {
            v.write_spec(s)
        });
        if rebuild {
            // Rebuild the set of markets requiring an order book because demand may have changed.
            backend.update(cx, |b, _| b.rebuild_orderbook_wanted());
        }
        cx.notify();
    }

    /// Set and persist the Cancel Buy button position without changing Panic Sell.
    fn apply_cancel_pos(&mut self, pos: ChartBtnPos, cx: &mut Context<Self>) {
        let (_, panic) = self.action_btn_pos_opt(cx);
        self.apply_tab_setting(StackSetting::ActionPos(Some(pos), panic), cx);
    }

    /// Set and persist the Panic Sell button position without changing Cancel Buy.
    fn apply_panic_pos(&mut self, pos: ChartBtnPos, cx: &mut Context<Self>) {
        let (cancel, _) = self.action_btn_pos_opt(cx);
        self.apply_tab_setting(StackSetting::ActionPos(cancel, Some(pos)), cx);
    }

    /// Toggle from the current orientation to its opposite.
    fn toggle_orientation_setting(&mut self, cx: &mut Context<Self>) {
        use StackOrientation as O;
        let next = match self.current_orientation(cx).unwrap_or(O::Vertical) {
            O::Vertical => O::Horizontal,
            O::Horizontal => O::Vertical,
        };
        self.apply_tab_setting(StackSetting::Orientation(Some(next)), cx);
    }

    /// Seed height fields with EFFECTIVE values (Fit → 0, Scroll → default); otherwise unset
    /// heights would appear as blank fields after a restart.
    fn seed_layout_popup_inputs(&self, window: &mut Window, cx: &mut Context<Self>) {
        let (_, hf, hs) = self.current_layout(cx);
        let fit = hf.unwrap_or(0).to_string();
        let scroll = hs.unwrap_or(stack::DEFAULT_SCROLL_HEIGHT).to_string();
        self.fit_input()
            .clone()
            .update(cx, |input, c| input.set_value(fit, window, c));
        self.scroll_input()
            .clone()
            .update(cx, |input, c| input.set_value(scroll, window, c));
        // The RESOLVED cap, not the raw one: an unconfigured tab is capped at the built-in default,
        // and a field reading "0" beside a cap of eight in force would be the popup lying about
        // what the feed is doing. Zero appears here only for a tab that asked to be uncapped, which
        // is the one case the "0 — no limit" hint describes.
        let cap = resolved_max_charts(self.current_max_charts(cx).0).to_string();
        self.max_charts_input()
            .clone()
            .update(cx, |input, c| input.set_value(cap, window, c));
        let min_slot = self
            .current_grid(cx)
            .2
            .unwrap_or(stack::grid::DEFAULT_MIN_SLOT)
            .to_string();
        self.min_slot_input()
            .clone()
            .update(cx, |input, c| input.set_value(min_slot, window, c));
        self.seed_rename_input(window, cx);
    }

    /// Whether the screen divider belongs on this target's popup.
    ///
    /// Comparison's broom mode overrides the divider with one column — anchor plus order books is
    /// one row by construction — so showing the control there would print a number the layout does
    /// not follow. Hosts that cannot be in broom mode leave the default.
    fn divider_applies(&self, _cx: &App) -> bool {
        true
    }

    /// Whether the arrival-flash switch belongs on this target's popup.
    fn arrival_flash_applies(&self, cx: &App) -> bool {
        StackSetting::ArrivalFlash(true)
            .applies_to(self.target_is_main(cx), self.popup_is_custom(cx))
    }

    /// Whether the detect-cap controls belong on this target's popup.
    fn detect_cap_applies(&self, cx: &App) -> bool {
        StackSetting::MaxCharts(None, false)
            .applies_to(self.target_is_main(cx), self.popup_is_custom(cx))
    }

    /// Keep only the values that mean something on THIS target, for a ⧉ press it is the source of.
    ///
    /// A press carries what its popup showed: without this, a press from Main would hand every
    /// addressed AddToChart tab the resolved defaults of two controls Main never drew — clearing
    /// their caps and switching their flashes back on as a side effect of copying a height.
    fn applicable_here(&self, values: Vec<StackSetting>, cx: &App) -> Vec<StackSetting> {
        let (is_main, is_custom) = (self.target_is_main(cx), self.popup_is_custom(cx));
        // The divider is filtered by a fact `applies_to` cannot express: broom mode is dynamic
        // state, not a KIND of tab, and a press from a popup that hid the control would otherwise
        // write this tab's unset divider over every addressed tab's own.
        let divider = self.divider_applies(cx);
        values
            .into_iter()
            .filter(|v| v.applies_to(is_main, is_custom))
            .filter(|v| divider || !matches!(v, StackSetting::Grid(..)))
            .collect()
    }

    /// Read the detect cap from its field: ZERO is the STORED "no cap" sentinel, a number is
    /// clamped to `MAX_CHARTS_MAX`, and anything unreadable — including a momentarily EMPTY field —
    /// keeps the target's current value.
    ///
    /// Zero returns `Some(0)` rather than `None`, because the two no longer mean the same thing:
    /// `None` is "never configured", which resolves to the built-in cap, so collapsing a typed zero
    /// to it would make the field the hint calls "no limit" turn the cap back ON.
    ///
    /// Blank deliberately does NOT mean uncapped, unlike the height fields: removing a cap is what
    /// the zero the hint names is for, while a blank field is what the user sees mid-edit, and a
    /// checkbox pressed at that moment would otherwise carry the emptiness along and drop the cap.
    fn read_max_charts(&self, cx: &App) -> Option<u16> {
        let fallback = self.current_max_charts(cx).0;
        let value = self.max_charts_input().read(cx).value().to_string();
        // Parsed as u32, not u16: a number past 65535 is an over-large CAP, not an unreadable
        // field, and clamping it is closer to what was asked for than silently keeping the old
        // value. Like the size fields, an out-of-range number stays on screen until the popup is
        // reopened, at which point it reads back as the clamped value that took effect.
        match value.trim().parse::<u32>() {
            Ok(0) => Some(0),
            Ok(raw) => Some(raw.min(u32::from(layout_popup::MAX_CHARTS_MAX)) as u16),
            Err(_) => fallback,
        }
    }

    /// The cap value to STORE, which is not always the one the field shows.
    ///
    /// The field is seeded with the RESOLVED cap, so an unconfigured tab displays the built-in
    /// default as an ordinary number. Writing that number back would pin the tab to today's value
    /// forever and dirty `charts.json` for a popup the reader only opened — and a ⧉ press about
    /// heights would do the same to every tab it addresses. So while the typed number resolves to
    /// the SAME effective cap the target already has, the target's own raw value travels instead;
    /// a genuinely different number, zero included, travels as typed.
    ///
    /// Args:
    ///     cx: App context used to read the target and popup field.
    ///
    /// Returns:
    ///     The raw cap value to persist without materializing an unchanged default.
    fn cap_to_persist(&self, cx: &App) -> Option<u16> {
        let current = self.current_max_charts(cx).0;
        let typed = self.read_max_charts(cx);
        if resolved_max_charts(typed) == resolved_max_charts(current) {
            current
        } else {
            typed
        }
    }

    /// Set whether a detect at the cap replaces the stalest chart, keeping the cap itself as typed.
    fn apply_max_charts_evict(&mut self, evict: bool, cx: &mut Context<Self>) {
        let cap = self.cap_to_persist(cx);
        self.apply_tab_setting(StackSetting::MaxCharts(cap, evict), cx);
    }

    /// Read the minimum-slot field the same way the cap is read: blank or unreadable keeps the
    /// target's current value, and the number is held to the size fields' own range.
    fn read_min_slot(&self, cx: &App) -> Option<u16> {
        let fallback = self.current_grid(cx).2;
        // Only FIT-stretch draws the field. Everywhere else there is nothing on screen to read, and
        // reading the seeded default anyway would stamp a number the reader never chose into that
        // tab's spec — on the first blur of ANY field in the popup.
        if !self.min_slot_applies(cx) {
            return fallback;
        }
        let value = self.min_slot_input().read(cx).value().to_string();
        match value.trim().parse::<u32>() {
            Ok(0) => None,
            // The field is SEEDED with the effective default, so a tab that never set a minimum
            // shows the same number one would type to mean "the default". Committing that as an
            // override would write a value the reader never chose into the spec — on the first blur
            // of any field in the popup — so an untouched default keeps meaning "unset".
            Ok(raw) if fallback.is_none() && raw == u32::from(stack::grid::DEFAULT_MIN_SLOT) => {
                None
            }
            Ok(raw) => {
                Some((raw.min(u32::from(layout_popup::MAX_H)) as u16).max(layout_popup::MIN_H))
            }
            Err(_) => fallback,
        }
    }

    /// Whether the minimum-slot field belongs on this target's popup.
    ///
    /// FIT-stretch alone: the other two modes state a slot size outright, and that size is what
    /// says when the charts have stopped fitting.
    fn min_slot_applies(&self, cx: &App) -> bool {
        let (mode, height_fit, _) = self.current_layout(cx);
        // Gated on the divider too: it is the divider's own minimum, so where the divider is not
        // drawn there is nothing for it to qualify — and nothing on screen to read it from.
        self.divider_applies(cx)
            && mode.unwrap_or(StackLayoutMode::Fit) == StackLayoutMode::Fit
            && height_fit.unwrap_or(0) == 0
    }

    /// Set the screen divider, keeping the other two parts of it as they STAND — the stored
    /// minimum, not the field's current text.
    ///
    /// Pressing a segment or a checkbox does not blur `MoonInput`, so the field can hold "3" on its
    /// way to "300"; committing that here would clamp it to `MIN_H` and store a number the reader
    /// was in the middle of typing. The minimum has its own commit, on blur and on close.
    fn apply_divider(&mut self, columns: u8, cx: &mut Context<Self>) {
        let (_, exact, min_slot) = self.current_grid(cx);
        self.apply_tab_setting(StackSetting::Grid(Some(columns), exact, min_slot), cx);
    }

    /// Set whether the divider is exact, keeping the number and the minimum as they stand.
    fn apply_divider_exact(&mut self, exact: bool, cx: &mut Context<Self>) {
        let (columns, _, min_slot) = self.current_grid(cx);
        self.apply_tab_setting(StackSetting::Grid(columns, Some(exact), min_slot), cx);
    }

    /// Read a mode height from its field: blank → `None`, invalid → the target's current value.
    fn read_layout_height(&self, mode: StackLayoutMode, cx: &App) -> Option<u16> {
        let (_, fit_fallback, scroll_fallback) = self.current_layout(cx);
        let (input, fallback) = match mode {
            StackLayoutMode::Fit => (self.fit_input(), fit_fallback),
            StackLayoutMode::Scroll => (self.scroll_input(), scroll_fallback),
        };
        let value = input.read(cx).value().to_string();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }
        trimmed
            .parse::<u16>()
            .ok()
            .map(|raw| layout_popup::clamp_height(mode, raw))
            .or(fallback)
    }

    /// Commit the popup field contents to the target layout.
    fn commit_layout_popup(&mut self, cx: &mut Context<Self>) {
        let (mode, _, _) = self.current_layout(cx);
        let hf = self.read_layout_height(StackLayoutMode::Fit, cx);
        let hs = self.read_layout_height(StackLayoutMode::Scroll, cx);
        self.apply_tab_setting(
            StackSetting::Layout(Some(mode.unwrap_or(StackLayoutMode::Fit)), hf, hs),
            cx,
        );
        // The cap field commits on the way out for the same reason the size fields do: committing
        // per keystroke would apply "1" on the way to "12".
        //
        // The minimum-slot field commits alongside the sizes, for the same reason — and BEFORE the
        // cap's own gate below, because the two are shown on different sets of tabs: gating this on
        // the cap's rule would drop a typed minimum on every Main and custom tab, which draw the
        // field but no cap. `read_min_slot` carries its own "was it shown" check.
        let (columns, exact, current_min) = self.current_grid(cx);
        let min_slot = self.read_min_slot(cx);
        if min_slot != current_min {
            self.apply_tab_setting(StackSetting::Grid(columns, exact, min_slot), cx);
        }
        // Only where the field was actually SHOWN, and only when it moved. A tab whose popup hides
        // the cap has no value to commit, and writing one anyway would put a number the user never
        // chose into its spec — and mark `charts.json` dirty on every blur of an unrelated field.
        if !self.detect_cap_applies(cx) {
            return;
        }
        let (current_cap, current_evict) = self.current_max_charts(cx);
        let cap = self.cap_to_persist(cx);
        if cap == current_cap {
            // The checkbox writes its own half the moment it is clicked, so an unchanged number
            // leaves nothing for this to do. An unconfigured tab whose field still shows the
            // resolved default lands here too, which is what keeps its spec unwritten.
            return;
        }
        // The evict half is RESOLVED rather than `unwrap_or(false)`: on a tab that never configured
        // it, writing false here would switch eviction off the moment the reader names a cap.
        self.apply_tab_setting(
            StackSetting::MaxCharts(cap, resolved_max_charts_evict(current_evict)),
            cx,
        );
    }
}

/// Commit the ⚙ popup's fields when one of them loses focus or takes Enter.
///
/// One subscription for every numeric field of the popup, on both hosts: each had a byte-identical
/// copy per field, and the copies are what let a field be added without being committed.
pub(super) fn subscribe_layout_commit<T: LayoutPopupHost>(
    input: &Entity<MoonInputState>,
    cx: &mut Context<T>,
) {
    cx.subscribe(input, |this, _input, ev: &MoonInputEvent, cx| {
        if this.popup_shows(ChartPopup::Layout)
            && matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. })
        {
            this.commit_layout_popup(cx);
        }
    })
    .detach();
}

/// ⚙ layout popup shared by both hosts: a `MoonPopover` anchored to the gear that opens it, with
/// ALL callbacks routed through the trait.
///
/// `id_prefix` is "chart-layout" for the strip or "detached-chart-layout" for a window. The content
/// is built ONLY while open — `MoonPopover` takes it eagerly, and this sits in a chart host that
/// repaints constantly.
///
/// Args:
///     this: The popup's host.
///     id_prefix: Per-host element identity prefix.
///     trigger: The gear button the popover anchors to.
///     apply_all_label: Tooltip for the apply-to-all icon, which differs per host.
///     cx: Host context.
///
/// Returns:
///     The trigger with its anchored popover.
pub(super) fn layout_popup_host<T: LayoutPopupHost>(
    this: &T,
    id_prefix: &'static str,
    trigger: impl IntoElement,
    apply_all_label: String,
    cx: &mut Context<T>,
) -> MoonPopover {
    let open_entity = cx.entity();
    let mut popover = MoonPopover::new(SharedString::from(format!("{id_prefix}-popover")))
        // Anchored bottom-right of the gear, which sits at the right edge of the tab strip: growing
        // left keeps the popup inside the window instead of off its right side.
        .placement(MoonPopoverPlacement::BottomEnd)
        .content_width(f32::from(layout_popup::content_width(cx)))
        .close_on_content_click(false)
        // Outside-click dismissal stays ON: nothing in here opens a deferred overlay of its own
        // (the segmented controls render inline), so there is no menu whose click could be mistaken
        // for an outside click. Dismissal is the ✕, a click outside, Escape, or the gear.
        .open(this.popup_shows(ChartPopup::Layout))
        .on_open_change(move |open, window, app| {
            open_entity.update(app, |this, cx| {
                if open {
                    this.seed_layout_popup_inputs(window, cx);
                }
                // Closing COMMITS the size field; `settle_closed_popup` owns that, so every way out
                // of this popup — ✕, outside click, Escape, the gear, or a press on a neighbouring
                // button that displaces it — commits exactly once. The keyboard travels through
                // `close_layout_popup` instead, and only AFTER the popover has had its say about
                // the focus: it may have restored a remembered holder, and only what is left on one
                // of the popup's own fields afterwards is stranded.
                match open {
                    true => this.open_chart_popup(ChartPopup::Layout, cx),
                    false => this.close_layout_popup(window, cx),
                }
            });
        })
        .trigger(trigger);
    if !this.popup_shows(ChartPopup::Layout) {
        return popover;
    }
    let p = MoonPalette::active(cx);
    let snap = this.layout_popup_snapshot(cx);
    let is_custom = this.popup_is_custom(cx);
    let entity = cx.entity();
    let close_entity = entity.clone();
    let pick_entity = entity.clone();
    let all_entity = entity.clone();
    let ob_entity = entity.clone();
    let liq_entity = entity.clone();
    let sz_entity = entity.clone();
    let ap_entity = entity.clone();
    let or_entity = entity.clone();
    let cbp_entity = entity.clone();
    let psp_entity = entity.clone();
    let pap_entity = entity.clone();
    let tav_entity = entity.clone();
    let ll_entity = entity.clone();
    let ev_entity = entity.clone();
    let dv_entity = entity.clone();
    let ex_entity = entity.clone();
    let fl_entity = entity.clone();
    let cl_entity = entity;
    let row = super::apply_row::render_apply_row(
        this,
        id_prefix,
        this.layout_press_values(cx),
        None,
        p,
        cx,
    );
    let content = layout_popup::render_layout_popup(
        id_prefix,
        snap.mode,
        snap.orientation,
        is_custom.then_some(this.rename_input()),
        this.fit_input(),
        this.scroll_input(),
        snap.orderbook,
        snap.liquidations,
        snap.show_zone,
        snap.auto_pin,
        snap.cancel_pos,
        snap.panic_pos,
        snap.price_axis_pos,
        snap.time_axis,
        snap.line_labels,
        snap.cursor_labels,
        {
            let (columns, exact, _) = this.current_grid(cx);
            layout_popup::GridControls {
                shown: this.divider_applies(cx),
                columns: columns.unwrap_or(1),
                exact: exact.unwrap_or(false),
                // One definition of "the field is shown", shared with the code that READS it: two
                // would be free to drift, and a field read where it is not drawn is exactly the
                // sin `read_min_slot` guards against.
                min_slot_input: this.min_slot_applies(cx).then(|| this.min_slot_input()),
                on_pick_columns: Box::new(move |columns, app| {
                    dv_entity.update(app, |this, cx| this.apply_divider(columns, cx));
                }),
                on_toggle_exact: Box::new(move |checked, app| {
                    ex_entity.update(app, |this, cx| this.apply_divider_exact(checked, cx));
                }),
            }
        },
        this.arrival_flash_applies(cx)
            .then(|| layout_popup::DetectFlow {
                cap: this
                    .detect_cap_applies(cx)
                    .then(|| layout_popup::DetectCap {
                        max_input: this.max_charts_input(),
                        evict: snap.max_charts_evict,
                        on_toggle_evict: Box::new(move |checked, app| {
                            ev_entity
                                .update(app, |this, cx| this.apply_max_charts_evict(checked, cx));
                        }),
                    }),
                flash: snap.arrival_flash,
                on_toggle_flash: Box::new(move |on, app| {
                    fl_entity.update(app, |this, cx| {
                        this.apply_tab_setting(StackSetting::ArrivalFlash(on), cx)
                    });
                }),
            }),
        p,
        cx,
        move |mode, app| {
            pick_entity.update(app, |this, cx| {
                let hf = this.read_layout_height(StackLayoutMode::Fit, cx);
                let hs = this.read_layout_height(StackLayoutMode::Scroll, cx);
                this.apply_tab_setting(StackSetting::Layout(Some(mode), hf, hs), cx);
            });
        },
        apply_all_label,
        move |app| {
            all_entity.update(app, |this, cx| this.arm_apply_press(cx));
        },
        move |checked, app| {
            ob_entity.update(app, |this, cx| {
                this.apply_tab_setting(StackSetting::Orderbook(checked), cx)
            });
        },
        move |checked, app| {
            liq_entity.update(app, |this, cx| {
                this.apply_tab_setting(StackSetting::Liquidations(checked), cx)
            });
        },
        move |checked, app| {
            sz_entity.update(app, |this, cx| {
                this.apply_tab_setting(StackSetting::ShowZone(checked), cx)
            });
        },
        move |checked, app| {
            ap_entity.update(app, |this, cx| {
                this.apply_tab_setting(StackSetting::AutoPin(checked), cx)
            });
        },
        move |app| {
            or_entity.update(app, |this, cx| this.toggle_orientation_setting(cx));
        },
        move |pos, app| {
            cbp_entity.update(app, |this, cx| this.apply_cancel_pos(pos, cx));
        },
        move |pos, app| {
            psp_entity.update(app, |this, cx| this.apply_panic_pos(pos, cx));
        },
        move |pos, app| {
            pap_entity.update(app, |this, cx| {
                this.apply_tab_setting(StackSetting::PriceAxis(pos), cx)
            });
        },
        move |checked, app| {
            tav_entity.update(app, |this, cx| {
                this.apply_tab_setting(StackSetting::TimeAxis(checked), cx)
            });
        },
        move |checked, app| {
            ll_entity.update(app, |this, cx| {
                this.apply_tab_setting(StackSetting::LineLabels(checked), cx)
            });
        },
        move |checked, app| {
            cl_entity.update(app, |this, cx| {
                this.apply_tab_setting(StackSetting::CursorLabels(checked), cx)
            });
        },
        move |_, window, app| {
            close_entity.update(app, |this, cx| this.close_layout_popup(window, cx));
        },
    );
    // The ⧉ row rides ABOVE the popup's own content, inline: see `apply_row`.
    popover = popover.content(v_flex().gap_2().children(row).child(content));
    popover
}

/// Host of a coin-search field (tab strip/detached-window header), defining where to open the
/// selected coin and how to clear the field/popup. [`coin_pick_handler`], [`coin_dismiss_handler`]
/// and [`coin_toolbar_press_handler`] provide shared plumbing;
/// [`super::coin_search::render_popup`] renders the list itself.
pub(super) trait CoinPopupHost: Sized + 'static {
    /// Clear the coin field and close the list after selection or an outside click.
    fn clear_coin_search(&mut self, cx: &mut Context<Self>);
    /// Open the selected coin on the host target (active tab/window stack).
    fn open_picked_coin(&mut self, core: CoreId, market: String, cx: &mut Context<Self>);
    /// The search field itself, for the plumbing that has to release its keyboard.
    fn coin_field(&self) -> &Entity<MoonInputState>;
    /// The backend this host reads and writes, so shared plumbing can reach persisted state.
    fn coin_backend(&self) -> Entity<crate::Backend>;
}

/// Handle a coin-list selection by opening it, clearing the field, and closing the popup.
///
/// Recording the market as recently opened happens HERE rather than in each host's
/// `open_picked_coin`: this is the one funnel every single pick passes through, so a future opening
/// path cannot quietly stop feeding the suggestion list. The bulk "open in new tab" path does not
/// pass through here and records its own.
pub(super) fn coin_pick_handler<T: CoinPopupHost>(
    cx: &Context<T>,
    input: Entity<MoonInputState>,
) -> impl Fn(CoreId, String, &mut Window, &mut App) + Clone + 'static {
    // IMPORTANT: do NOT read `cx.entity().read(cx)` here. This helper runs DURING host rendering
    // (`ChartTabs`/`DetachedChartHost`), while the entity is already borrowed as `&mut self`, so a
    // read panics with "cannot read … while it is already being updated" and crashes when coin
    // search opens. The caller passes `coin_input` while it still has `&self`.
    let view = cx.entity();
    move |core, market, window, app| {
        view.update(app, |this, cx| {
            this.coin_backend()
                .update(cx, |b, _| b.push_recent_coin(core, &market));
            this.open_picked_coin(core, market, cx);
        });
        input.update(app, |inp, c| {
            inp.set_value(SharedString::default(), window, c)
        });
        view.update(app, |this, cx| this.clear_coin_search(cx));
        crate::controls::coin_search::release_focus(&input, window, app);
    }
}

/// End the market search when a press lands on a NEIGHBOURING toolbar control, in the CAPTURE
/// phase.
///
/// A `MoonDropdown` is the one neighbour that leaves the list STANDING, and there are two of them:
/// the price scale and the drawing-tool picker. Its trigger stops the press in the bubble phase
/// (`Popover`), so the dismiss layer painted under the toolbar row never sees it, and unlike the
/// four settings popovers it holds no seat in [`super::popup_slot`] — opening it displaces nothing.
/// The list simply stayed up under the menu that opened over it. The plain `MoonButton`s beside
/// them stop nothing, so wherever the dismiss layer reaches them they already closed it; this
/// handler only arrives first. In the detached header that layer starts BELOW the row, so there
/// they needed it for the list half too.
///
/// The FOCUS half belongs to every control here, dropdowns and buttons alike, and capture is the
/// only phase early enough for it: `MoonPopover` remembers the window's focus holder inside that
/// same bubble handler and hands it back when it closes, so a blur any later is undone a moment
/// after. The field takes the keyboard back — where Ctrl+Z is Undo and Ctrl+X is Cut rather than
/// the hotkeys they are bound to — and its `Focus` reopens the list the user just left.
///
/// Deliberately does NOT stop propagation, unlike [`coin_dismiss_handler`]: the control that was
/// pressed still has to do its own job. Left button only, matching that layer, and scoped to the
/// sections FLANKING the field rather than to the toolbar row as a whole — a press on the field
/// itself must keep focusing it.
pub(super) fn coin_toolbar_press_handler<T: CoinPopupHost + LayoutPopupHost>(
    cx: &Context<T>,
) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + Clone + 'static {
    let entity = cx.entity();
    move |ev: &MouseDownEvent, window: &mut Window, app: &mut App| {
        if ev.button != MouseButton::Left {
            return;
        }
        let field = entity.update(app, |this, cx| {
            // Gated because this body serves both exits: the dismiss layer runs it too, and a
            // plain button's press reaches BOTH for the one press. `close_chart_popup` would
            // already decline the second, but `clear_coin_search` notifies either way.
            if this.popup_shows(ChartPopup::Coin) {
                this.clear_coin_search(cx);
            }
            this.coin_field().clone()
        });
        // Outside that gate on purpose: the field can hold the keyboard with the list already
        // closed, and a press on a neighbour ends its claim either way. It blurs only the field.
        crate::controls::coin_search::release_focus(&field, window, app);
    }
}

/// Handle a click on the coin list's dismiss layer; the caller defines the layer geometry.
///
/// The same end-of-search body as [`coin_toolbar_press_handler`] — one funnel, so a rule added to
/// either exit reaches both — plus the one thing that is this layer's alone: swallowing the press,
/// which is why a click that dismisses the list does not also land on the chart underneath.
pub(super) fn coin_dismiss_handler<T: CoinPopupHost + LayoutPopupHost>(
    cx: &Context<T>,
) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
    let end_search = coin_toolbar_press_handler(cx);
    move |ev, window, app| {
        end_search(ev, window, app);
        app.stop_propagation();
    }
}
