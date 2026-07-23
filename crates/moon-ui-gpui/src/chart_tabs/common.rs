//! Shared per-tab chart-stack settings plumbing for TWO hosts: the tab strip (`ChartTabs`, active
//! tab) and a detached window (`DetachedChartHost`, its panel). This module contains the setting
//! value ([`StackSetting`]: apply to a stack and write to a spec), spec upsert, the
//! [`LayoutPopupHost`] trait with shared layout-popup logic (open/seed/read/commit/close plus
//! setting application) and shared overlay/dismiss rendering, and the [`CoinPopupHost`] trait with
//! coin-search input plumbing. Host-specific differences (the target tab/panel, Apply to all, and
//! `is_custom`) remain in the trait implementations ([`super::settings`] /
//! [`super::detached_host`]).

use gpui::*;
use moon_ui::{MoonInputState, MoonPalette};

use super::{layout_popup, stack};
use crate::Backend;
use crate::persistence::chart_persist::{
    self, ChartBtnPos, PriceAxisPos, StackLayoutMode, StackOrientation,
};
use moon_core::config::ChartBucket;
use moon_core::session::CoreId;

/// One per-tab chart-stack setting value. It can write itself to a spec (the shared persistence
/// half); [`set_stack_setting!`] applies it to panels using identically named and typed setters on
/// `MainChartStack` and `AddChartStack`.
#[derive(Clone, Copy)]
pub(super) enum StackSetting {
    /// Layout mode plus separate Fit and Scroll heights.
    Layout(Option<StackLayoutMode>, Option<u16>, Option<u16>),
    /// Stack orientation (vertical/horizontal).
    Orientation(StackOrientation),
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
    /// Candle/trade display settings (the ❚ popup).
    CandleView(moon_core::market::CandleViewCfg),
}

impl StackSetting {
    /// Write the value to a tab spec (the persistence half of `apply_*`, shared by both hosts).
    pub(super) fn write_spec(self, s: &mut chart_persist::ChartTabSpec) {
        match self {
            StackSetting::Layout(mode, hf, hs) => {
                s.layout_mode = mode;
                s.layout_height_fit = hf;
                s.layout_height_scroll = hs;
            }
            StackSetting::Orientation(o) => s.layout_orientation = Some(o),
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
            crate::chart_tabs::common::StackSetting::Orientation(o) => {
                $s.set_orientation(Some(o), $c)
            }
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
        }
    };
}
pub(super) use set_stack_setting;

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
}

/// Host of the ⚙ layout-settings popup. Required methods expose host state and the target
/// (`ChartTabs` targets its active tab, while `DetachedChartHost` targets the window panel);
/// default methods contain the SHARED popup and setting-application logic formerly duplicated.
pub(super) trait LayoutPopupHost: Sized + 'static {
    // --- Host popup state and inputs ---
    fn popup_open(&self) -> bool;
    fn set_popup_open(&mut self, open: bool);
    fn popup_hovered(&self) -> bool;
    fn set_popup_hovered(&mut self, hovered: bool);
    fn fit_input(&self) -> &Entity<MoonInputState>;
    fn scroll_input(&self) -> &Entity<MoonInputState>;
    fn rename_input(&self) -> &Entity<MoonInputState>;

    // --- Target tab ---
    fn backend(&self) -> &Entity<Backend>;
    fn spec_group(&self) -> &str;
    /// Target persistence key: `(num, bucket)`.
    fn spec_key(&self) -> (u32, ChartBucket);
    /// Current target layout: `(mode, height_fit, height_scroll)`.
    fn current_layout(&self, cx: &App) -> (Option<StackLayoutMode>, Option<u16>, Option<u16>);
    fn current_orientation(&self, cx: &App) -> Option<StackOrientation>;
    /// Target action-button positions as raw `Option`s for independently editing cancel/panic.
    fn action_btn_pos_opt(&self, cx: &App) -> (Option<ChartBtnPos>, Option<ChartBtnPos>);
    fn layout_popup_snapshot(&self, cx: &App) -> LayoutPopupSnapshot;
    /// Whether the target is a custom multi-coin tab, controlling rename-field visibility.
    fn popup_is_custom(&self, cx: &App) -> bool;
    /// Seed the custom-tab name field; a no-op when the target is not custom.
    fn seed_rename_input(&self, window: &mut Window, cx: &mut Context<Self>);
    /// Apply a value to the target stack(s), dispatching to Main/active stack or the window panel.
    fn set_on_stacks(&mut self, v: StackSetting, cx: &mut Context<Self>);
    /// Apply to all non-Main stacks. The strip also includes Main only when Main is the source
    /// (`include_main = true`); Add, Custom, and detached-window sources leave Main unchanged.
    /// Detached windows route the request through Backend because they cannot access group stacks.
    fn apply_all_from_popup(&mut self, cx: &mut Context<Self>);

    // --- Shared default logic ---

    /// Apply a setting to the target and persist it to the spec, rebuilding order-book demand for
    /// an Orderbook change.
    fn apply_tab_setting(&mut self, v: StackSetting, cx: &mut Context<Self>) {
        self.set_on_stacks(v, cx);
        let (num, bucket) = self.spec_key();
        let backend = self.backend().clone();
        upsert_spec(&backend, self.spec_group(), num, &bucket, cx, move |s| {
            v.write_spec(s)
        });
        if matches!(v, StackSetting::Orderbook(_)) {
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
        self.apply_tab_setting(StackSetting::Orientation(next), cx);
    }

    /// Open or close the target's in-scene layout popup.
    fn toggle_layout_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.popup_open() {
            self.close_layout_popup(true, cx);
        } else {
            self.seed_layout_popup_inputs(window, cx);
            self.set_popup_open(true);
            self.set_popup_hovered(false);
            cx.notify();
        }
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
        self.seed_rename_input(window, cx);
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
    }

    fn close_layout_popup(&mut self, commit: bool, cx: &mut Context<Self>) {
        if !self.popup_open() {
            return;
        }
        if commit {
            self.commit_layout_popup(cx);
        }
        self.set_popup_open(false);
        self.set_popup_hovered(false);
        cx.notify();
    }
}

/// ⚙ layout-popup overlay shared by both hosts: a positioned scene with hover-close plus
/// [`layout_popup::render_layout_popup`], with ALL callbacks routed through the trait.
/// `id_prefix` is "chart-layout" for the strip or "detached-chart-layout" for a window; `top` is
/// the anchor. Returns `None` while the popup is closed.
pub(super) fn layout_popup_overlay<T: LayoutPopupHost>(
    this: &T,
    id_prefix: &'static str,
    top: Pixels,
    apply_all_label: String,
    cx: &mut Context<T>,
) -> Option<Stateful<Div>> {
    if !this.popup_open() {
        return None;
    }
    let p = MoonPalette::active(cx);
    let snap = this.layout_popup_snapshot(cx);
    let is_custom = this.popup_is_custom(cx);
    let entity = cx.entity();
    let popup_w = layout_popup::content_width(cx, is_custom);
    let hover_entity = entity.clone();
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
    let cl_entity = entity;
    Some(
        div()
            .id(SharedString::from(format!("{id_prefix}-popup-scene")))
            .absolute()
            .right(px(6.0))
            .top(top)
            .w(popup_w)
            .on_mouse_down(MouseButton::Left, |_, _window, app| {
                app.stop_propagation();
            })
            .on_hover(move |hovered, _window, app| {
                hover_entity.update(app, |this, cx| {
                    if *hovered {
                        this.set_popup_hovered(true);
                    } else if this.popup_hovered() {
                        this.close_layout_popup(true, cx);
                    }
                });
            })
            .child(layout_popup::render_layout_popup(
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
                    all_entity.update(app, |this, cx| this.apply_all_from_popup(cx));
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
            )),
    )
}

/// Layer intercepting clicks outside the layout popup and closing it with a commit.
/// Returns `None` while the popup is closed.
pub(super) fn layout_popup_dismiss<T: LayoutPopupHost>(
    this: &T,
    id_prefix: &'static str,
    cx: &mut Context<T>,
) -> Option<Stateful<Div>> {
    if !this.popup_open() {
        return None;
    }
    let entity = cx.entity();
    Some(
        div()
            .id(SharedString::from(format!("{id_prefix}-popup-dismiss")))
            .absolute()
            .inset_0()
            .on_mouse_down(MouseButton::Left, move |_, _window, app| {
                entity.update(app, |this, cx| this.close_layout_popup(true, cx));
                app.stop_propagation();
            }),
    )
}

/// Host of a coin-search field (tab strip/detached-window header), defining where to open the
/// selected coin and how to clear the field/popup. [`coin_pick_handler`] and
/// [`coin_dismiss_handler`] provide shared plumbing; [`super::coin_search::render_popup`] renders
/// the list itself.
pub(super) trait CoinPopupHost: Sized + 'static {
    /// Clear the coin field and close the list after selection or an outside click.
    fn clear_coin_search(&mut self, cx: &mut Context<Self>);
    /// Open the selected coin on the host target (active tab/window stack).
    fn open_picked_coin(&mut self, core: CoreId, market: String, cx: &mut Context<Self>);
}

/// Handle a coin-list selection by opening it, clearing the field, and closing the popup.
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
        view.update(app, |this, cx| this.open_picked_coin(core, market, cx));
        input.update(app, |inp, c| {
            inp.set_value(SharedString::default(), window, c)
        });
        view.update(app, |this, cx| this.clear_coin_search(cx));
    }
}

/// Handle a click on the coin list's dismiss layer; the caller defines the layer geometry.
pub(super) fn coin_dismiss_handler<T: CoinPopupHost>(
    cx: &Context<T>,
) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
    let entity = cx.entity();
    move |_, _w, app| {
        entity.update(app, |this, cx| this.clear_coin_search(cx));
        app.stop_propagation();
    }
}
