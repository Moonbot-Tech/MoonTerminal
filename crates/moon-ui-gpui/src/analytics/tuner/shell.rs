//! The tuner shell: the toolbar (rounding / Make a copy / Save), shared by every axis and
//! dispatched by `TunerKind`, plus the suggestion row, which is NOT.
//!
//! The row is written per axis on purpose. "By filter" runs either a stoppable joint search or
//! field-set composition with live progress, a seed and a depth; "By time" runs one fixed sweep
//! with a single setting. Rendering both from one parametrized row is how the time sweep would
//! acquire a Stop button that stops nothing. On the filter row only the restart count stays
//! inline, because the card is laid out at a fixed narrow width and a row that cannot wrap has to
//! spend its space on the controls used every run — the rest sit behind the ⚙ settings popover.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::db::ReadFail;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonDropdown,
    MoonInput, MoonInputEvent, MoonInputState, MoonMenuSize, MoonPalette, MoonPopover,
    MoonPopoverPlacement, MoonTag, MoonTooltipView, h_flex, v_flex,
};
use rust_i18n::t;

use super::super::AnalyticsView;
use super::filter::state::{
    SuggestJob, SuggestState, TRAIN_OPTIONS, canonical_iters, edge_options, iters_of, persist_seed,
};
use super::shared::TunerKind;
use crate::design;
use crate::design::{moon, moon_alpha};

/// Content width of the suggestion settings popover, before the font scale and the component's
/// own padding, which [`MoonPopover::content_width_font`] adds.
const SETTINGS_POPUP_W: f32 = 250.0;

/// Which settings box of a suggestion row is being built.
#[derive(Clone, Copy, PartialEq)]
enum CfgInput {
    /// Restart count of the joint search ("By filter" only).
    Restarts,
    /// Minimum trades a suggestion must retain.
    MinTrades,
    /// Base seed of the random restarts ("By filter" only).
    Seed,
}

/// Cache key of one settings box, as one `&'static str` per axis-and-box pair.
///
/// Spelled out rather than composed with `format!` for two reasons: the ids are read on every
/// frame, so building them would allocate per box per frame; and the one place that used to
/// hand-write a composed id — the seed box, evicted by name when a seed is pinned — could drift
/// from the builder with no compiler help, turning that button into a silent no-op.
fn cfg_input_id(kind: TunerKind, which: CfgInput) -> &'static str {
    match (kind, which) {
        (TunerKind::Filter, CfgInput::Restarts) => "f-cfg-it",
        (TunerKind::Filter, CfgInput::MinTrades) => "f-cfg-mn",
        (TunerKind::Filter, CfgInput::Seed) => "f-cfg-seed",
        (TunerKind::Time, CfgInput::Restarts) => "t-cfg-it",
        (TunerKind::Time, CfgInput::MinTrades) => "t-cfg-mn",
        (TunerKind::Time, CfgInput::Seed) => "t-cfg-seed",
        (TunerKind::Coins, CfgInput::Restarts) => "c-cfg-it",
        (TunerKind::Coins, CfgInput::MinTrades) => "c-cfg-mn",
        (TunerKind::Coins, CfgInput::Seed) => "c-cfg-seed",
    }
}

/// A muted caption standing next to a settings control.
fn cfg_label(text: String, p: MoonPalette) -> Div {
    div().flex_none().text_color(moon(p.text_muted)).child(text)
}

/// A seed shortened to what the settings row can hold, keeping both ends.
///
/// A drawn seed comes from the clock and runs to twenty digits, several times the width of this
/// row. It also does not need to be read: the button beside it puts the WHOLE value into the box,
/// which is the only way it is ever meant to be reused. So the display keeps enough of each end to
/// tell one result from another, and the full number rides in the tooltip.
fn short_seed(seed: u64) -> String {
    let s = seed.to_string();
    // Digits are ASCII, so slicing by byte is slicing by character here.
    if s.len() <= 12 {
        s
    } else {
        format!("{}…{}", &s[..6], &s[s.len() - 4..])
    }
}

/// How a train share reads in the dropdown: a percentage, or the word for "no split".
///
/// 100% is spelled out as "off" rather than shown as a number, because a percentage that happens
/// to be the whole period reads as a setting in effect when it is the absence of one.
fn train_label(pct: usize) -> String {
    if pct >= 100 {
        t!("analytics.tuner.train_off").to_string()
    } else {
        format!("{pct}%")
    }
}

impl AnalyticsView {
    /// The shared toolbar of the tuner card: the title + (when a strategy is selected)
    /// "round results" + "Make a copy" + "Save". The actions are per-axis.
    pub(super) fn shell_toolbar(
        &self,
        kind: TunerKind,
        title: String,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let k = match kind {
            TunerKind::Filter => "f",
            TunerKind::Time => "t",
            TunerKind::Coins => "c",
        };
        // `None` — the axis has nothing to round. The coin list is text; a rounding control
        // beside it would be a switch that does nothing.
        let round = match kind {
            TunerKind::Filter => Some(self.tuner.round_results),
            TunerKind::Time => Some(self.time_tuner.round_results),
            TunerKind::Coins => None,
        };
        let mut header = h_flex()
            .w_full()
            // Pinned above the scrollable rows in both tuner panels — must not shrink.
            .flex_none()
            .px(design::ui_px(cx, 12.0))
            .py(design::ui_px(cx, 8.0))
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            // Let the title yield before the hosting card clips the trailing controls.
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(design::t_title(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            );
        // Rounding the result affects the SUGGESTION — always shown (the suggestion is
        // available without a selected strategy too, over the current scope).
        if let Some(round) = round {
            header = header
                .child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_muted))
                        .child(t!("analytics.tuner.round_lbl").to_string()),
                )
                .child(
                    div().flex_none().child(
                        MoonCheckbox::new(SharedString::from(format!("tun-round-{k}")))
                            .checked(round)
                            .size(MoonCheckboxSize::Compact)
                            .on_change({
                                let view = cx.entity();
                                move |ch: &bool, _w, app| {
                                    let on = *ch;
                                    view.update(app, |this, cx| {
                                        match kind {
                                            TunerKind::Filter => {
                                                this.tuner.round_results = on;
                                                this.tuner.invalidate_suggest();
                                            }
                                            TunerKind::Time => {
                                                this.time_tuner.round_results = on;
                                                this.time_tuner.invalidate_suggest();
                                            }
                                            // No rounding on this axis — the control is hidden.
                                            TunerKind::Coins => {}
                                        }
                                        cx.notify();
                                    });
                                }
                            }),
                    ),
                );
        }
        // Writing (Copy / Save) — ONLY into a selected strategy.
        if self.sel_strategy.is_some() {
            let workspace_target_visible = !self.selected_targets().is_empty();
            // Copy is single-target only — hidden in multi-select (many addressees, no
            // per-target preview); bulk Save is the multi path.
            if !self.is_multi() {
                header = header.child(
                    MoonButton::new(SharedString::from(format!("tun-copy-{k}")))
                        .variant(MoonButtonVariant::Soft)
                        .size(MoonButtonSize::Micro)
                        .label(t!("analytics.tuner.copy_btn").to_string())
                        .disabled(!workspace_target_visible)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            // One "copy" for all axes: axis changes → a NEW strategy.
                            match kind {
                                TunerKind::Filter => this.open_copy_dialog(window, cx),
                                TunerKind::Time => this.time_open_copy_dialog(window, cx),
                                TunerKind::Coins => this.coins_open_copy_dialog(window, cx),
                            }
                            cx.notify();
                        }))
                        .render(),
                );
            }
            header = header.child({
                // "Save" lights up amber when there is something to write (filter
                // thresholds OR a time schedule that differs from the current one).
                // Works for both axes; the action is dispatched by `kind`.
                let dirty = match kind {
                    TunerKind::Filter => self.save_dirty(),
                    TunerKind::Time => self.time_tuner.is_dirty(),
                    // The list differs from what the strategies hold — the same condition
                    // the coin table's "changed" badge and its Revert button read.
                    TunerKind::Coins => self.coins.has_changes(),
                };
                MoonButton::new(SharedString::from(format!("tun-save-{k}")))
                    .variant(if dirty {
                        MoonButtonVariant::Amber
                    } else {
                        MoonButtonVariant::Soft
                    })
                    .size(MoonButtonSize::Micro)
                    .label(t!("analytics.tuner.save_btn").to_string())
                    .disabled(!workspace_target_visible)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        match kind {
                            TunerKind::Filter => this.open_save_dialog(cx),
                            TunerKind::Time => this.time_open_save_dialog(cx),
                            TunerKind::Coins => this.coins_open_save_dialog(cx),
                        }
                        cx.notify();
                    }))
                    .render()
            });
        }
        header.into_any_element()
    }

    /// The suggestion row of the tuner card, branched per axis.
    ///
    /// Explicit branches rather than one parametrized row: the axes no longer hold the same
    /// controls, and a shared row is precisely how the time sweep would acquire a Stop button
    /// that stops nothing.
    pub(super) fn shell_config_row(
        &mut self,
        kind: TunerKind,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match kind {
            TunerKind::Filter => self.filter_config_row(p, window, cx),
            TunerKind::Time => self.time_config_row(p, window, cx),
            // The coin axis does not draw this row yet — its own controls arrive with the
            // selection metrics. Answering here keeps the match exhaustive rather than letting a
            // fourth axis compile into a silent default.
            TunerKind::Coins => div().into_any_element(),
        }
    }

    /// The frame every suggestion row shares: pinned above the scrollable rows, caption-sized.
    fn config_row_frame(&self, cx: &Context<Self>) -> Div {
        h_flex()
            .w_full()
            // Pinned above the scrollable rows — must not shrink.
            .flex_none()
            .px(design::ui_px(cx, 12.0))
            .pb(design::ui_px(cx, 6.0))
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .text_size(design::t_caption(cx))
    }

    /// The "By filter" suggestion row: restarts, the settings gear, live status, Stop, Search.
    ///
    /// Only the restart count stays inline. The rest sit behind the gear because this card is
    /// laid out at a fixed narrow width, and a row that cannot wrap has to spend its space on the
    /// controls that are used every run rather than on the ones set once.
    fn filter_config_row(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let running = self.tuner.sugg.is_running();
        // Only the joint search can be stopped; `Some(true)` means the stop is already in flight.
        let stopping = self
            .tuner
            .sugg
            .joint_run()
            .map(|(handle, _)| handle.is_cancelled());
        let (status, status_color) = self.filter_search_status(p);
        let it_input =
            self.shell_cfg_input(TunerKind::Filter, CfgInput::Restarts, "100", window, cx);
        // `MoonPopover` holds its content eagerly, so building it while the popover is shut costs
        // two menus, a dozen locale lookups and their closures on EVERY frame — and this panel
        // repaints several times a second for the whole of a running search. Closed, the gear is
        // all there is to draw.
        let settings = self.filter_search_settings(p, window, cx);
        self.config_row_frame(cx)
            .child(cfg_label(t!("analytics.tuner.iters").to_string(), p))
            .child(
                div().w(design::font_w_px(cx, 46.0)).flex_none().child(
                    MoonInput::new(SharedString::from("tun-cfg-it-f"))
                        .state(&it_input)
                        .small(),
                ),
            )
            .child(settings)
            // The status takes the free space and truncates, so a long failure message cannot
            // push the buttons off a narrow dock.
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(moon(status_color))
                    .child(status),
            )
            .when_some(stopping, |el, stopping| {
                el.child(
                    MoonButton::new(SharedString::from("tun-suggest-stop-f"))
                        .variant(MoonButtonVariant::Soft)
                        .size(MoonButtonSize::Micro)
                        .label(if stopping {
                            t!("analytics.tuner.stopping").to_string()
                        } else {
                            t!("analytics.tuner.stop").to_string()
                        })
                        .disabled(stopping)
                        .on_click(cx.listener(|this, _, _, cx| this.stop_suggest_into_v1(cx)))
                        .render(),
                )
            })
            // The suggestion buttons are ALWAYS visible — a suggestion can be run over the
            // current scope (no selected strategy = over everything shown). The result can only
            // be written into a selected strategy (that gate sits on Copy/Save in the toolbar).
            .child(
                MoonButton::new(SharedString::from("tun-suggest-one-f"))
                    .variant(MoonButtonVariant::Soft)
                    .size(MoonButtonSize::Micro)
                    .label(t!("analytics.tuner.suggest_one").to_string())
                    .disabled(running)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if !this.tuner.sugg.is_running() {
                            this.suggest_one_into_v1(cx);
                            cx.notify();
                        }
                    }))
                    .render(),
            )
            .child(
                MoonButton::new(SharedString::from("tun-suggest-run-f"))
                    .variant(MoonButtonVariant::Blue)
                    .size(MoonButtonSize::Micro)
                    // The label carries the mode, because the two searches answer different
                    // questions and take very different amounts of time. One button whose
                    // meaning is silently switched by a checkbox two clicks away is how a user
                    // ends up waiting minutes for what usually took half a second.
                    .label(if self.tuner.compose {
                        t!("analytics.tuner.compose_run").to_string()
                    } else {
                        t!("analytics.tuner.suggest_run").to_string()
                    })
                    .disabled(running)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if !this.tuner.sugg.is_running() {
                            this.suggest_into_v1(cx);
                            cx.notify();
                        }
                    }))
                    .render(),
            )
            .into_any_element()
    }

    /// One line describing what the "By filter" search is doing, with the colour to draw it in.
    ///
    /// An empty string is the idle state: the row simply shows nothing rather than a placeholder.
    ///
    /// Args:
    ///     p: Active MoonUI palette.
    ///
    /// Returns:
    ///     Localized status text and its palette color.
    fn filter_search_status(&self, p: MoonPalette) -> (String, u32) {
        match &self.tuner.sugg {
            SuggestState::Idle => (String::new(), p.text_muted),
            // The blocking overlay is this one's progress feedback, so the row stays quiet.
            SuggestState::Running(SuggestJob::SingleField) => (String::new(), p.text_muted),
            SuggestState::Running(SuggestJob::AllFields { handle, total }) => (
                t!(
                    "analytics.tuner.sugg_progress",
                    done = handle.completed(),
                    total = total
                )
                .to_string(),
                p.text_soft,
            ),
            // Composition reports where it is, not how far along: how many candidates it will
            // weigh is decided by what it finds, so there is no honest denominator to show. Until
            // the first step publishes one, it says only that it is working.
            SuggestState::Running(SuggestJob::Compose { handle }) => (
                match handle.stage() {
                    Some((step, done, total)) => t!(
                        "analytics.tuner.compose_progress",
                        step = step,
                        done = done + 1,
                        total = total
                    )
                    .to_string(),
                    None => t!("analytics.tuner.compose_started").to_string(),
                },
                p.text_soft,
            ),
            SuggestState::Done {
                rounds,
                stopped: true,
                ..
            } => (
                match rounds {
                    Some(n) => t!("analytics.tuner.sugg_stopped", rounds = n).to_string(),
                    None => t!("analytics.tuner.compose_stopped").to_string(),
                },
                p.orange,
            ),
            // A search that ran to the end with nothing to give did not fail: the scope simply
            // holds fewer trades than the minimum a suggestion must retain. Saying "done" here
            // would read as "these are your thresholds", which is the one thing it is not.
            SuggestState::Done { split: None, .. } => {
                (t!("analytics.tuner.sugg_small").to_string(), p.text_soft)
            }
            SuggestState::Done { rounds, .. } => (
                match rounds {
                    Some(n) => t!("analytics.tuner.sugg_done", rounds = n).to_string(),
                    None => t!("analytics.tuner.compose_done").to_string(),
                },
                p.text_muted,
            ),
            // A failed read must not read as "found nothing", so it is said in the danger colour
            // with the database's own words rather than left to the log alone.
            SuggestState::Failed(ReadFail::NotReady) => (
                t!("common.db_not_ready").to_string(),
                design::danger_color(p),
            ),
            SuggestState::Failed(ReadFail::IncomparableQuote) => {
                (t!("common.incomparable_quote").to_string(), p.orange)
            }
            SuggestState::Failed(e) => (
                format!("{}: {e}", t!("analytics.tuner.sugg_failed")),
                design::danger_color(p),
            ),
        }
    }

    /// The "By filter" search settings popover: the knobs that are set once and then left alone.
    ///
    /// It cannot start or stop a search — those stay in the row, reachable at any width.
    ///
    /// The contents are built ONLY while it is open. `MoonPopover` takes its content eagerly, so
    /// a shut popover would otherwise rebuild two menus, their closures and a dozen locale
    /// lookups on every frame — and this panel repaints several times a second for the whole of
    /// a running search, which is exactly when nobody is looking at these knobs.
    ///
    /// Args:
    ///     p: Active Moon palette.
    ///     window: Window that owns the popover layers.
    ///     cx: Analytics view context.
    ///
    /// Returns:
    ///     The controlled settings popover and its gear trigger.
    fn filter_search_settings(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.tuner.sugg_cfg_open;
        let content = open.then(|| self.filter_settings_content(p, window, cx));
        let entity = cx.entity();
        let mut popover = MoonPopover::new("tun-cfg-popover")
            .placement(MoonPopoverPlacement::BottomStart)
            .content_width_font(SETTINGS_POPUP_W)
            .close_on_content_click(false)
            // A `MoonDropdown` menu inside this popover paints in its OWN deferred layer, outside
            // this popover's box, and `on_mouse_down_out` is bounds-based and runs in the CAPTURE
            // phase — so the click that picks an item reads as "outside" and would shut the popover
            // before the pick landed. Until MoonUI suppresses that (see the Popover entry in
            // docs-internal/FORK_BUGS.md), outside-click dismissal has to be off here; the ✕ and the
            // gear are the dismissal paths.
            .overlay_closable(false)
            .open(open)
            .on_open_change(move |open, _window, app| {
                entity.update(app, |this, cx| {
                    this.tuner.sugg_cfg_open = open;
                    cx.notify();
                });
            })
            .trigger(
                MoonButton::new("tun-cfg-gear")
                    .label("⚙")
                    .size(MoonButtonSize::Micro)
                    .variant(MoonButtonVariant::Soft)
                    .tooltip(t!("analytics.tuner.cfg_title").to_string())
                    .render(),
            );
        if let Some(content) = content {
            popover = popover.content(content);
        }
        popover.into_any_element()
    }

    /// Build the scroll-bounded, grouped controls shown inside the settings popover.
    ///
    /// Args:
    ///     p: Active Moon palette.
    ///     window: Window supplying input state and the viewport height bound.
    ///     cx: Analytics view context.
    ///
    /// Returns:
    ///     Popup content that remains reachable in the narrowest supported window.
    fn filter_settings_content(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mn_input = self.shell_cfg_input(
            TunerKind::Filter,
            CfgInput::MinTrades,
            &t!("analytics.tuner.auto_ph"),
            window,
            cx,
        );
        let seed_input = self.shell_cfg_input(
            TunerKind::Filter,
            CfgInput::Seed,
            &t!("analytics.tuner.seed_ph"),
            window,
            cx,
        );
        let train_pct = self.tuner.train_pct;
        let tr_view = cx.entity();
        let tr_items = crate::panels::radio_items(
            TRAIN_OPTIONS.map(|n| {
                (
                    n,
                    SharedString::from(format!("tun-tr-{n}")),
                    SharedString::from(train_label(n)),
                )
            }),
            train_pct,
            crate::panels::RadioMark::Highlight,
            move |app, n| {
                tr_view.update(app, |this, cx| {
                    this.tuner.train_pct = n;
                    this.tuner.invalidate_suggest();
                    this.persist_tuner_train(cx);
                    cx.notify();
                });
            },
        );
        let tr_combo = MoonDropdown::new(SharedString::from("tun-cfg-tr-f"))
            .label(train_label(train_pct))
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Micro)
            .menu_width_scaled(96.0)
            .menu_size(MoonMenuSize::Compact)
            .items(tr_items);
        let edges = self.tuner.edges;
        let ed_view = cx.entity();
        let ed_items = crate::panels::radio_items(
            edge_options().iter().map(|n| {
                (
                    *n,
                    SharedString::from(format!("tun-ed-{n}")),
                    SharedString::from(n.to_string()),
                )
            }),
            edges,
            crate::panels::RadioMark::Highlight,
            move |app, n| {
                ed_view.update(app, |this, cx| {
                    this.tuner.edges = n;
                    this.tuner.invalidate_suggest();
                    this.persist_tuner_edges(cx);
                    cx.notify();
                });
            },
        );
        let ed_combo = MoonDropdown::new(SharedString::from("tun-cfg-ed-f"))
            .label(edges.to_string())
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Micro)
            .menu_width_scaled(64.0)
            .menu_size(MoonMenuSize::Compact)
            .items(ed_items);
        let gap = design::ui_px(cx, 6.0);
        let section_gap = design::ui_px(cx, 8.0);
        let label_w = design::font_w_px(cx, 90.0);
        let popup_max_h = px((f32::from(window.viewport_size().height)
            - f32::from(design::ui_px(cx, 24.0)))
        .max(f32::from(design::ui_px(cx, 180.0))));
        let head = h_flex()
            .w_full()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(moon(p.text))
                    .child(t!("analytics.tuner.cfg_title").to_string()),
            )
            .child(
                MoonButton::new("tun-cfg-close")
                    .label("✕")
                    .size(MoonButtonSize::Micro)
                    .variant(MoonButtonVariant::Ghost)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.tuner.sugg_cfg_open = false;
                        cx.notify();
                    }))
                    .render(),
            );
        let row = |caption: String, body: AnyElement| {
            h_flex()
                .w_full()
                .items_center()
                .gap(gap)
                .child(
                    div()
                        .w(label_w)
                        .flex_none()
                        .text_color(moon(p.text_muted))
                        .child(caption),
                )
                .child(body)
        };
        let section_title = |title: String| {
            div()
                .w_full()
                .pt(design::ui_px(cx, 3.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(moon(p.text_soft))
                .child(title)
        };
        // No padding, background, border or corners here: `MoonPopover` supplies all four, and
        // `content_width_font` treats the number it is given as the width of the CONTENT, adding
        // its own padding and border on top. Drawing a second set inside would both double the
        // chrome and make the real content box narrower than the width declared for it.
        let mut content = v_flex()
            .id("tun-cfg-popup")
            .w_full()
            .max_h(popup_max_h)
            .overflow_y_scroll()
            .gap(gap)
            .text_size(design::t_caption(cx))
            .child(head)
            .child(section_title(
                t!("analytics.tuner.cfg_search_section").to_string(),
            ))
            .child(row(
                t!("analytics.tuner.min_trades").to_string(),
                div()
                    .w(design::font_w_px(cx, 76.0))
                    .flex_none()
                    .child(
                        MoonInput::new(SharedString::from("tun-cfg-mn-f"))
                            .state(&mn_input)
                            .small(),
                    )
                    .into_any_element(),
            ))
            .child(row(
                t!("analytics.tuner.edges").to_string(),
                div().flex_none().child(ed_combo).into_any_element(),
            ))
            .child(div().mt(section_gap - gap).child(section_title(
                t!("analytics.tuner.cfg_validation_section").to_string(),
            )))
            // Holding trades back is what turns a suggestion from "these ranges fit the past" into
            // a claim that can be checked, so the control sits with the settings and its verdict
            // is printed under the grid rather than hidden in a log line.
            .child(row(
                t!("analytics.tuner.train_share").to_string(),
                div().flex_none().child(tr_combo).into_any_element(),
            ))
            // The composition switch lives here rather than as a second button in the run row:
            // the row is fixed-narrow and spends its width on per-run controls, and two adjacent
            // blue buttons would leave the user guessing which one they want. The mode shows
            // itself on the run button's LABEL instead.
            //
            // No budget means the machine is below the heavy-search bar, and the row is not
            // rendered at all — the feature does not exist there, so there is nothing to explain
            // and nothing greyed out to wonder about.
            .when_some(
                moon_core::db::tuner::threshold_search::composition_budget(),
                |el, budget| {
                    // Interpolation keeps the help aligned with the search budget.
                    let help = SharedString::from(
                        t!("analytics.tuner.compose_help", seeds = budget.seed_groups).to_string(),
                    );
                    let metrics = compose_budget_labels(&budget);
                    el.child(
                        // One contained block makes the mode, its short explanation, and its real
                        // work budget read as one setting instead of an unrelated checkbox plus a
                        // wrapped diagnostic sentence.
                        v_flex()
                            .id("tun-cfg-compose-help")
                            .w_full()
                            .gap(gap)
                            .p(design::ui_px(cx, 8.0))
                            .rounded(design::ui_px(cx, 6.0))
                            .border_1()
                            .border_color(moon(p.border))
                            .bg(moon_alpha(p.panel_high, 0.32))
                            .tooltip(move |_w, cx| {
                                cx.new(|_| MoonTooltipView::new(help.clone())).into()
                            })
                            .child(
                                MoonCheckbox::new(SharedString::from("tun-cfg-compose-f"))
                                    .label(t!("analytics.tuner.compose_toggle").to_string())
                                    .checked(self.tuner.compose)
                                    .size(MoonCheckboxSize::Compact)
                                    // `on_change` hands the callback an `&mut App`, not a
                                    // `Context`, so this is one of the call sites where a
                                    // `cx.listener` does not fit.
                                    .on_change({
                                        let view = cx.entity();
                                        move |checked: &bool, _w, app| {
                                            let on = *checked;
                                            view.update(app, |this, cx| {
                                                this.tuner.compose = on;
                                                // The two searches answer different questions,
                                                // so a suggestion made by one is not a
                                                // suggestion by the other.
                                                this.tuner.invalidate_suggest();
                                                this.persist_tuner_compose(cx);
                                                cx.notify();
                                            });
                                        }
                                    }),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .text_color(moon(p.text_muted))
                                    .child(t!("analytics.tuner.compose_short").to_string()),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .flex_wrap()
                                    .gap(design::ui_px(cx, 4.0))
                                    .children(
                                        metrics
                                            .map(|label| MoonTag::new().mono(false).child(label)),
                                    ),
                            ),
                    )
                },
            )
            .child(div().mt(section_gap - gap).child(section_title(
                t!("analytics.tuner.cfg_repeat_section").to_string(),
            )))
            .child(row(
                t!("analytics.tuner.seed").to_string(),
                div()
                    .w(design::font_w_px(cx, 126.0))
                    .flex_none()
                    .child(
                        MoonInput::new(SharedString::from("tun-cfg-seed-f"))
                            .state(&seed_input)
                            .small(),
                    )
                    .into_any_element(),
            ));
        // The seed a finished search actually used, so an interesting result can be pinned and
        // repeated. Without it a search left on "random" is not reproducible even once.
        if let Some(last) = self.tuner.last_seed {
            let full = SharedString::from(last.to_string());
            content = content.child(row(
                t!("analytics.tuner.seed_last").to_string(),
                h_flex()
                    .items_center()
                    .gap(gap)
                    .child(
                        div()
                            .id("tun-cfg-seed-last")
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(moon(p.text_soft))
                            .tooltip(move |_w, cx| {
                                cx.new(|_| MoonTooltipView::new(full.clone())).into()
                            })
                            .child(short_seed(last)),
                    )
                    .child(
                        MoonButton::new("tun-cfg-seed-pin")
                            .label(t!("analytics.tuner.seed_pin").to_string())
                            .size(MoonButtonSize::Micro)
                            .variant(MoonButtonVariant::Soft)
                            .on_click(cx.listener(|this, _, _, cx| this.pin_last_seed(cx)))
                            .render(),
                    )
                    .into_any_element(),
            ));
        }
        content.into_any_element()
    }

    /// The "By time" suggestion row: the sweep has one setting and one action.
    fn time_config_row(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mn_input = self.shell_cfg_input(
            TunerKind::Time,
            CfgInput::MinTrades,
            &t!("analytics.tuner.auto_ph"),
            window,
            cx,
        );
        let busy = self.time_tuner.sugg_busy;
        self.config_row_frame(cx)
            .child(cfg_label(t!("analytics.tuner.min_trades").to_string(), p))
            .child(
                div().w(design::font_w_px(cx, 52.0)).flex_none().child(
                    MoonInput::new(SharedString::from("tun-cfg-mn-t"))
                        .state(&mn_input)
                        .small(),
                ),
            )
            .child(div().flex_1())
            .child(
                MoonButton::new(SharedString::from("tun-suggest-run-t"))
                    .variant(MoonButtonVariant::Blue)
                    .size(MoonButtonSize::Micro)
                    .label(if busy {
                        "…".to_string()
                    } else {
                        t!("analytics.tuner.suggest_run").to_string()
                    })
                    // `time_suggest` gates on its own `sugg_busy` itself.
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.time_suggest(cx);
                        cx.notify();
                    }))
                    .render(),
            )
            .into_any_element()
    }

    /// A suggestion settings box for the `kind` axis, with a lazy cache in that axis's state.
    fn shell_cfg_input(
        &mut self,
        kind: TunerKind,
        which: CfgInput,
        placeholder: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        let id = cfg_input_id(kind, which);
        let cached = match kind {
            TunerKind::Filter => self.tuner.inputs.get(id),
            TunerKind::Time => self.time_tuner.inputs.get(id),
            TunerKind::Coins => None,
        };
        if let Some(state) = cached {
            return state.clone();
        }
        let value = match (kind, which) {
            (TunerKind::Filter, CfgInput::MinTrades) => self.tuner.min_trades.clone(),
            (TunerKind::Filter, CfgInput::Seed) => self.tuner.seed.clone(),
            (TunerKind::Filter, CfgInput::Restarts) => self.tuner.iters.clone(),
            (TunerKind::Time, CfgInput::MinTrades) => self.time_tuner.min_trades.clone(),
            // The time row draws only the minimum-trades box, and the coin axis draws no row at
            // all, so neither has a value for the rest.
            (TunerKind::Time, _) | (TunerKind::Coins, _) => String::new(),
        };
        let ph = placeholder.to_string();
        let state = cx.new(|cx| {
            MoonInputState::new(window, cx)
                .default_value(value)
                .placeholder(ph)
        });
        cx.subscribe_in(
            &state,
            window,
            move |this, state, ev: &MoonInputEvent, window, cx| {
                // Change is committed too: the value takes effect right away on a "Search" click.
                if matches!(
                    ev,
                    MoonInputEvent::Change
                        | MoonInputEvent::Blur
                        | MoonInputEvent::PressEnter { .. }
                ) {
                    let raw = state.read(cx).value().to_string();
                    let value = if matches!(
                        (kind, which, ev),
                        (
                            TunerKind::Filter,
                            CfgInput::Restarts,
                            MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }
                        )
                    ) {
                        canonical_iters(&raw)
                    } else {
                        raw.clone()
                    };
                    if value != raw {
                        state.update(cx, |input, cx| input.set_value(value.clone(), window, cx));
                    }
                    match (kind, which) {
                        (TunerKind::Filter, CfgInput::MinTrades) => {
                            this.tuner.min_trades = value;
                            this.tuner.invalidate_suggest();
                        }
                        (TunerKind::Filter, CfgInput::Seed) => {
                            this.tuner.seed = value;
                            this.tuner.invalidate_suggest();
                            this.persist_tuner_seed(cx);
                        }
                        (TunerKind::Filter, CfgInput::Restarts) => {
                            this.tuner.iters = value;
                            this.tuner.invalidate_suggest();
                            this.persist_tuner_iters(cx);
                        }
                        (TunerKind::Time, CfgInput::MinTrades) => {
                            this.time_tuner.min_trades = value;
                            this.time_tuner.invalidate_suggest();
                        }
                        (TunerKind::Time, _) | (TunerKind::Coins, _) => {}
                    }
                    if !matches!(ev, MoonInputEvent::Change) {
                        cx.notify();
                    }
                }
            },
        )
        .detach();
        match kind {
            TunerKind::Filter => self.tuner.inputs.insert(id.to_string(), state.clone()),
            TunerKind::Time => self.time_tuner.inputs.insert(id.to_string(), state.clone()),
            TunerKind::Coins => None,
        };
        state
    }

    /// Copy the seed the last completed search ran with into the seed box, pinning it.
    fn pin_last_seed(&mut self, cx: &mut Context<Self>) {
        let Some(seed) = self.tuner.last_seed else {
            return;
        };
        self.tuner.seed = seed.to_string();
        // Recreate the box so it renders from the START of the value, as `apply_bounds` does.
        self.tuner
            .inputs
            .remove(cfg_input_id(TunerKind::Filter, CfgInput::Seed));
        self.tuner.invalidate_suggest();
        self.persist_tuner_seed(cx);
        cx.notify();
    }

    /// Write one search setting into the saved layout through its dirty-flag drain.
    ///
    /// `pick` names the field. Every setting persists the same way — compare, assign, mark dirty
    /// — so keeping the sequence here prevents later settings from drifting into a subtly
    /// different write path.
    ///
    /// Args:
    ///     cx: GPUI context used to update the backend layout.
    ///     pick: The layout field this setting lives in.
    ///     value: Its normalized value, or `None` where the setting has no usable value.
    fn persist_setting<T: PartialEq>(
        &self,
        cx: &mut Context<Self>,
        pick: impl Fn(&mut moon_core::config::WindowLayout) -> &mut T,
        value: T,
    ) {
        self.backend.update(cx, |b, _| {
            let slot = pick(&mut b.layout);
            if *slot != value {
                *slot = value;
                b.layout_dirty = true;
            }
        });
    }

    /// Persist a changed quantile depth.
    fn persist_tuner_edges(&self, cx: &mut Context<Self>) {
        let value = Some(self.tuner.edges as u32);
        self.persist_setting(cx, |l| &mut l.analytics_tuner_edges, value);
    }

    /// Persist the normalized restart count.
    ///
    /// `Change` events must persist because closing the window does not guarantee a blur.
    fn persist_tuner_iters(&self, cx: &mut Context<Self>) {
        let value = Some(iters_of(&self.tuner.iters) as u32);
        self.persist_setting(cx, |l| &mut l.analytics_tuner_iters, value);
    }

    /// Persist the chosen train share.
    fn persist_tuner_train(&self, cx: &mut Context<Self>) {
        let value = Some(self.tuner.train_pct as u32);
        self.persist_setting(cx, |l| &mut l.analytics_tuner_train, value);
    }

    /// Persist the chosen seed.
    ///
    /// Only a usable seed is stored: box text that is not a plain number means "no seed", and
    /// writing it back would reopen the tuner showing a seed no search would honour.
    fn persist_tuner_seed(&self, cx: &mut Context<Self>) {
        let value = persist_seed(&self.tuner.seed);
        self.persist_setting(cx, |l| &mut l.analytics_tuner_seed, value);
    }

    /// Persist which fields take part in the automatic search — the grid checkboxes.
    ///
    /// Always `Some`, never `None`: an empty selection is the user having unchecked everything,
    /// and `None` would restore the default as though no usable preference had been loaded.
    ///
    /// Reachable from `filter`, which is a sibling module rather than a descendant of this one,
    /// hence the wider visibility than the four wrappers above.
    pub(in crate::analytics::tuner) fn persist_tuner_fields(&self, cx: &mut Context<Self>) {
        let value = Some(self.tuner.enabled_cols());
        self.persist_setting(cx, |l| &mut l.analytics_tuner_fields, value);
    }

    /// Persist whether the search composes the field set as well as the thresholds.
    fn persist_tuner_compose(&self, cx: &mut Context<Self>) {
        let value = self.tuner.compose;
        self.persist_setting(cx, |l| &mut l.analytics_tuner_compose, value);
    }
}

/// Compact labels for the composition work budget shown inside its setting block.
///
/// Args:
///     budget: The same machine budget that allowed the composition block to render.
///
/// Returns:
///     Localized field-count, adaptive-beam, seed-group, and fold-count labels.
fn compose_budget_labels(
    budget: &moon_core::db::tuner::threshold_search::ComposeBudget,
) -> [String; 4] {
    [
        t!(
            "analytics.tuner.compose_budget_fields",
            n = budget.max_fields
        )
        .to_string(),
        t!(
            "analytics.tuner.compose_budget_beam",
            from = budget.beam_width_min,
            to = budget.beam_width_max
        )
        .to_string(),
        t!(
            "analytics.tuner.compose_budget_seeds",
            n = budget.seed_groups
        )
        .to_string(),
        t!("analytics.tuner.compose_budget_folds", n = budget.folds).to_string(),
    ]
}
