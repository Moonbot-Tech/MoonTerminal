//! The "Auto-pick" card: its settings, the running pick, and the verdict on the fresh trades
//! the pick never saw.
//!
//! The card is a FREE render function over a plain model plus handler closures, so a layout
//! test can draw every state in a probe view (see `fields`); the view supplies the handlers.

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonVariant, MoonCheckbox, MoonInput, MoonInputState, MoonPalette, h_flex,
    v_flex,
};
use rust_i18n::t;

use super::super::super::AnalyticsView;
use super::fields::Act;
use super::plural_key;
use super::state::{CoinsState, HOLDOUT_PCT_RANGE, PickCfg, PickStaleCause, PickState};
use crate::design;
use crate::design::{moon, moon_alpha};
use crate::load_state::{LoadState, Note};
use moon_core::db::tuner::coin_pick::{CoinPick, CoinSuggestion, NoiseReason, PickVerdict};

/// A signed money figure as the card prints it.
fn money(v: f64) -> String {
    moon_core::util::fmt::signed_amount(v, 2).0
}

/// The verdict on the fresh trades, in words.
///
/// Args:
///     pick: The finished pick.
///     holdout_pct: The share of the period the pick held back.
///
/// Returns:
///     One sentence per verdict; the useful one names the pick's figure, the as-traded figure
///     and the gain over the manual lists.
pub(in crate::analytics::tuner) fn verdict_line(pick: &CoinPick, holdout_pct: u32) -> String {
    let n = pick.black.len() + pick.white.len();
    let gain = money(pick.pick_holdout.profit - pick.manual_holdout.profit);
    match &pick.verdict {
        PickVerdict::Useful => t!(
            "analytics.coins.v_useful",
            pct = holdout_pct,
            pick = money(pick.pick_holdout.profit),
            fact = money(pick.fact_holdout.profit),
            gain = gain
        )
        .to_string(),
        PickVerdict::Noise(NoiseReason::SmallGain) => {
            let key = plural_key("analytics.coins.v_small", n);
            t!(key.as_str(), n = n, gain = gain).to_string()
        }
        PickVerdict::Noise(NoiseReason::HoldoutWorse) => {
            let key = plural_key("analytics.coins.v_worse", n);
            t!(key.as_str(), n = n, gain = gain).to_string()
        }
        PickVerdict::Noise(NoiseReason::FewHoldout) => {
            let key = plural_key("analytics.coins.v_few", n);
            t!(key.as_str(), n = n).to_string()
        }
        PickVerdict::NothingFound => t!("analytics.coins.v_nothing").to_string(),
        PickVerdict::TooFewTrades => t!("analytics.coins.v_too_few").to_string(),
    }
}

/// One suggested coin's tag: its full-period profit and trade count.
pub(in crate::analytics::tuner) fn tag_line(c: &CoinSuggestion) -> String {
    t!(
        "analytics.coins.tag_line",
        coin = c.token.clone(),
        profit = money(c.profit),
        n = c.n
    )
    .to_string()
}

/// A finished pick, as the card draws it.
pub(in crate::analytics::tuner) struct ResultModel {
    pub(in crate::analytics::tuner) line: String,
    /// The verdict says the pick helps.
    pub(in crate::analytics::tuner) useful: bool,
    pub(in crate::analytics::tuner) black: Vec<SuggestionChip>,
    pub(in crate::analytics::tuner) white: Vec<SuggestionChip>,
    pub(in crate::analytics::tuner) adopted: bool,
}

/// A suggestion's identity and caption are separate so exclusions never parse display text.
pub(in crate::analytics::tuner) struct SuggestionChip {
    pub(in crate::analytics::tuner) token: String,
    pub(in crate::analytics::tuner) text: String,
    pub(in crate::analytics::tuner) excluded: bool,
}

/// Which face the card shows.
pub(in crate::analytics::tuner) enum AutoView {
    Settings,
    Running { line: String },
    Result(ResultModel),
    Failed(moon_core::db::ReadFail),
}

/// Everything the card draws besides its two input entities.
pub(in crate::analytics::tuner) struct AutoCardModel {
    pub(in crate::analytics::tuner) view: AutoView,
    pub(in crate::analytics::tuner) cfg: PickCfg,
    /// A strategy is selected, so a pick has something to score.
    pub(in crate::analytics::tuner) enabled: bool,
    /// A sentence above the body: no strategy, or several scored as one.
    pub(in crate::analytics::tuner) hint: Option<String>,
    /// Why the finished pick no longer matches its inputs. `None` while the pick is current.
    /// Lists wins when both the lists and the report moved.
    pub(in crate::analytics::tuner) stale: Option<PickStaleCause>,
    /// The coin statistics and the saved lists have landed: a pick launched before would
    /// score against lists that are about to change.
    pub(in crate::analytics::tuner) loaded: bool,
    pub(in crate::analytics::tuner) pending: Option<String>,
    pub(in crate::analytics::tuner) summary: String,
}

impl AutoCardModel {
    /// Build the card's model from the coin state.
    ///
    /// Args:
    ///     s: Coin state (pick state, settings, lists).
    ///     selected: A strategy is selected.
    ///     multi: Several strategies are selected.
    ///     loaded: The coin statistics and saved lists are read and no reload is running.
    ///
    /// Returns:
    ///     The model for the current pick state.
    pub(in crate::analytics::tuner) fn new(
        s: &CoinsState,
        selected: bool,
        multi: bool,
        loaded: bool,
    ) -> Self {
        let view = match &s.pick {
            PickState::Idle => AutoView::Settings,
            PickState::Running { .. } => AutoView::Running {
                line: t!("analytics.coins.running", pct = s.pick_cfg.holdout_pct).to_string(),
            },
            PickState::Done {
                pick,
                holdout_pct,
                adopted,
                ..
            } => AutoView::Result(ResultModel {
                line: verdict_line(pick, *holdout_pct),
                useful: pick.verdict == PickVerdict::Useful,
                black: pick
                    .black
                    .iter()
                    .map(|c| SuggestionChip {
                        token: c.token.clone(),
                        text: tag_line(c),
                        excluded: s.excluded(true, &c.token),
                    })
                    .collect(),
                white: pick
                    .white
                    .iter()
                    .map(|c| SuggestionChip {
                        token: c.token.clone(),
                        text: tag_line(c),
                        excluded: s.excluded(false, &c.token),
                    })
                    .collect(),
                adopted: *adopted,
            }),
            PickState::Failed(e) => AutoView::Failed(e.clone()),
        };
        let hint = if !selected {
            Some(t!("analytics.coins.pick_needs_strategy").to_string())
        } else if multi {
            Some(t!("analytics.coins.multi_note").to_string())
        } else {
            None
        };
        Self {
            view,
            cfg: s.pick_cfg,
            enabled: selected,
            hint,
            stale: s.pick_stale_cause(),
            loaded,
            pending: super::fields::pending_text(&s.pending_delta()),
            summary: settings_summary(s.pick_cfg),
        }
    }

    /// Pick is offered only with a strategy, settled data and at least one list to look for.
    pub(in crate::analytics::tuner) fn can_pick(&self) -> bool {
        self.enabled && self.loaded && (self.cfg.black || self.cfg.white)
    }
}

/// The card's handlers.
pub(in crate::analytics::tuner) struct AutoCardActs {
    pub(in crate::analytics::tuner) on_pick: Act,
    /// Stops a running pick, or drops a finished one.
    pub(in crate::analytics::tuner) on_reset: Act,
    pub(in crate::analytics::tuner) on_one_by_one: Act,
    /// Adds the suggestion for one list (`true` = blacklist).
    pub(in crate::analytics::tuner) on_add: Act,
    pub(in crate::analytics::tuner) on_save: Act,
    pub(in crate::analytics::tuner) on_revert: Act,
    pub(in crate::analytics::tuner) on_exclude: Rc<dyn Fn(bool, String, &mut Window, &mut App)>,
    pub(in crate::analytics::tuner) on_black: Rc<dyn Fn(bool, &mut Window, &mut App)>,
    pub(in crate::analytics::tuner) on_white: Rc<dyn Fn(bool, &mut Window, &mut App)>,
}

/// Keep adoption and the existing dialog request in one ordered action, including in probes.
pub(in crate::analytics::tuner) fn add_and_save(add: Act, save: Act) -> Act {
    Rc::new(move |window, app| {
        add(window, app);
        save(window, app);
    })
}

/// Compact live settings with the locale's trade-count plural form.
fn settings_summary(cfg: PickCfg) -> String {
    let unit = plural_key("analytics.coins.summary_unit", cfg.min_trades as usize);
    let sides = [
        cfg.black.then(|| t!("analytics.coins.black").to_string()),
        cfg.white.then(|| t!("analytics.coins.white").to_string()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(", ");
    t!(
        "analytics.coins.summary",
        n = cfg.min_trades,
        unit = t!(unit.as_str()),
        pct = cfg.holdout_pct,
        sides = sides
    )
    .to_string()
}

/// A small text button with a tooltip.
fn btn(
    id: &'static str,
    label: String,
    tip: String,
    variant: MoonButtonVariant,
    enabled: bool,
    act: Act,
) -> AnyElement {
    let button = MoonButton::new(id)
        .variant(variant)
        .xsmall()
        .label(label)
        .tooltip(tip)
        .disabled(!enabled)
        .on_click(move |_, window, app| act(window, app))
        .render();
    let button = div()
        .flex_none()
        .when(matches!(variant, MoonButtonVariant::Blue), |el| {
            el.debug_selector(move || format!("an-primary-{id}"))
        })
        .child(button);
    super::layout::tagged(id, button).into_any_element()
}

/// The card, bounded by its host while prose wraps and the collapsed summary truncates.
///
/// Args:
///     m: The card's model.
///     inputs: The minimum-trades and holdout inputs; `None` in a probe without a window.
///     acts: The card's handlers.
///     p: Theme palette.
///     cx: App, for scale.
///
/// Returns:
///     The rendered card.
pub(in crate::analytics::tuner) fn auto_card_el(
    m: &AutoCardModel,
    inputs: Option<(&Entity<MoonInputState>, &Entity<MoonInputState>)>,
    acts: &AutoCardActs,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let caption = |id: &'static str, text: String, color: u32| {
        div()
            .debug_selector(move || id.into())
            .w_full()
            .min_w_0()
            .flex_none()
            .whitespace_normal()
            .text_size(design::t_caption(cx))
            .text_color(moon(color))
            .child(text)
    };
    let mut body = v_flex()
        .debug_selector(|| "an-pick-card".into())
        .w_full()
        .min_w_0()
        .flex_none()
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 10.0))
        .gap(design::ui_px(cx, 10.0))
        .font_family(design::ui_font())
        .text_size(design::t_body(cx))
        .when_some(m.hint.clone(), |el, hint| {
            el.child(caption("an-pick-hint", hint, p.text_muted))
        });
    if matches!(m.view, AutoView::Result(_)) {
        body = body.child(
            h_flex()
                .debug_selector(|| "an-pick-summary-row".into())
                .w_full()
                .min_w_0()
                .items_center()
                .gap(design::ui_px(cx, 8.0))
                .child(
                    div()
                        .id("an-pick-summary")
                        .debug_selector(|| "an-pick-summary".into())
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .tooltip(crate::panels::common::text_tooltip(m.summary.clone()))
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_muted))
                        .child(m.summary.clone()),
                )
                .child(btn(
                    "an-pick-settings",
                    t!("analytics.coins.settings").to_string(),
                    t!("analytics.coins.tip_settings").to_string(),
                    MoonButtonVariant::Bare,
                    true,
                    acts.on_reset.clone(),
                )),
        );
    } else {
        body = body.child(settings_el(m, inputs, acts, cx));
    }
    body = match &m.view {
        AutoView::Settings => body,
        AutoView::Running { line } => body
            .child(caption("an-pick-running", line.clone(), p.text_soft))
            .child(
                div()
                    .debug_selector(|| "an-pick-progress".into())
                    .w_full()
                    .h(design::ui_px(cx, 4.0))
                    .rounded(design::ui_px(cx, 2.0))
                    .bg(moon(p.border))
                    .overflow_hidden()
                    .child(div().h_full().w(relative(0.4)).bg(moon(p.blue))),
            )
            .child(h_flex().justify_end().child(btn(
                "an-pick-stop",
                t!("analytics.coins.stop_btn").to_string(),
                t!("analytics.coins.tip_stop").to_string(),
                if m.pending.is_some() {
                    MoonButtonVariant::Soft
                } else {
                    MoonButtonVariant::Blue
                },
                true,
                acts.on_reset.clone(),
            ))),
        AutoView::Result(r) => body.child(result_el(r, m, acts, p, cx)),
        AutoView::Failed(note) => body.child(crate::load_state::note_el(
            "an-pick-note",
            note_of(note),
            4.0,
            p,
            cx,
        )),
    };
    if let Some(text) = &m.pending {
        body = body.child(super::fields::pending_band_el(
            text.clone(),
            acts.on_revert.clone(),
            acts.on_save.clone(),
            !matches!(&m.view, AutoView::Result(r) if !r.adopted && m.stale.is_some()),
            p,
            cx,
        ));
    }
    body.into_any_element()
}

/// The placeholder a failed pick read shows.
fn note_of(e: &moon_core::db::ReadFail) -> Note {
    match e {
        moon_core::db::ReadFail::NotReady => Note::NotReady,
        e => crate::load_state::failed_note(e),
    }
}

/// The settings face: whole controls wrap within the column, above the Pick action.
fn settings_el(
    m: &AutoCardModel,
    inputs: Option<(&Entity<MoonInputState>, &Entity<MoonInputState>)>,
    acts: &AutoCardActs,
    cx: &App,
) -> AnyElement {
    let enabled = m.enabled && !matches!(m.view, AutoView::Running { .. });
    let field = |id: &'static str,
                 lead: String,
                 unit: String,
                 tip: String,
                 input: Option<&Entity<MoonInputState>>,
                 value: u32| {
        h_flex()
            .id(id)
            .debug_selector(move || id.to_string())
            .flex_none()
            .max_w_full()
            .min_w_0()
            .flex_wrap()
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .text_size(design::t_caption(cx))
            .tooltip(crate::panels::common::text_tooltip(tip))
            .child(
                div()
                    .w(px(design::ui_caption_text_width(cx, &lead, 400.0).ceil()))
                    .max_w_full()
                    .flex_none()
                    .whitespace_normal()
                    .child(lead),
            )
            .child(div().w(design::font_w_px(cx, 46.0)).flex_none().map(|el| {
                match input {
                    Some(input) => el.child(
                        MoonInput::new(SharedString::from(format!("{id}-in")))
                            .state(input)
                            .size(design::INPUT_SIZE)
                            .disabled(!enabled),
                    ),
                    None => el.font_family(design::mono()).child(value.to_string()),
                }
            }))
            .child(unit)
    };
    let check = |id: &'static str,
                 label: String,
                 tip: String,
                 on: bool,
                 act: Rc<dyn Fn(bool, &mut Window, &mut App)>| {
        div()
            .id(SharedString::from(format!("{id}-ctl")))
            .debug_selector(move || format!("{id}-ctl"))
            .flex_none()
            .tooltip(crate::panels::common::text_tooltip(tip))
            .child(
                MoonCheckbox::new(id)
                    .label(label)
                    .checked(on)
                    .disabled(!enabled)
                    .on_change(move |v: &bool, window, app| act(*v, window, app)),
            )
    };
    let (min_in, hold_in) = match inputs {
        Some((a, b)) => (Some(a), Some(b)),
        None => (None, None),
    };
    let unit_key = plural_key("analytics.coins.pick_min_unit", m.cfg.min_trades as usize);
    let unit_text = t!(unit_key.as_str()).to_string();
    v_flex()
        .debug_selector(|| "an-pick-settings-body".into())
        .w_full()
        .min_w_0()
        .flex_none()
        .gap(design::ui_px(cx, 8.0))
        .child(
            h_flex()
                .debug_selector(|| "an-pick-settings-fields".into())
                .w_full()
                .flex_wrap()
                .gap(design::ui_px(cx, 12.0))
                .child(field(
                    "an-pick-min",
                    t!("analytics.coins.pick_min_lbl").to_string(),
                    unit_text,
                    t!("analytics.coins.tip_pick_min").to_string(),
                    min_in,
                    m.cfg.min_trades,
                ))
                .child(field(
                    "an-pick-hold",
                    t!("analytics.coins.pick_hold_lbl").to_string(),
                    t!("analytics.coins.pick_hold_unit").to_string(),
                    t!("analytics.coins.tip_pick_hold").to_string(),
                    hold_in,
                    m.cfg.holdout_pct,
                ))
                .child(div().flex_1())
                .child(check(
                    "an-pick-bl",
                    t!("analytics.coins.pick_bl").to_string(),
                    t!("analytics.coins.tip_pick_bl").to_string(),
                    m.cfg.black,
                    acts.on_black.clone(),
                ))
                .child(check(
                    "an-pick-wl",
                    t!("analytics.coins.pick_wl").to_string(),
                    t!("analytics.coins.tip_pick_wl").to_string(),
                    m.cfg.white,
                    acts.on_white.clone(),
                )),
        )
        .child(
            h_flex()
                .w_full()
                .flex_wrap()
                .justify_end()
                .debug_selector(|| "tuner-run-c".into())
                .gap(design::ui_px(cx, 6.0))
                .when(!matches!(m.view, AutoView::Running { .. }), |el| {
                    el.child(btn(
                        "an-pick-run",
                        t!("analytics.coins.pick_btn").to_string(),
                        t!("analytics.coins.tip_pick_btn").to_string(),
                        if m.pending.is_some() {
                            MoonButtonVariant::Soft
                        } else {
                            MoonButtonVariant::Blue
                        },
                        m.can_pick() && enabled,
                        acts.on_pick.clone(),
                    ))
                }),
        )
        .into_any_element()
}

/// The amber sentence under a finished pick whose inputs moved.
///
/// Args:
///     cause: Which input moved. Lists is also used when the report moved too.
///
/// Returns:
///     The sentence for the active locale.
fn stale_note(cause: PickStaleCause) -> String {
    match cause {
        PickStaleCause::Lists => t!("analytics.coins.stale_note").to_string(),
        PickStaleCause::Report => t!("analytics.coins.stale_note_report").to_string(),
    }
}

/// Name each adopted list with the approved sentence and a singular or plural coin subject.
fn adopted_hint(r: &ResultModel) -> String {
    [(true, &r.black), (false, &r.white)]
        .into_iter()
        .filter_map(|(black, chips)| {
            let tokens = chips
                .iter()
                .filter(|c| !c.excluded)
                .map(|c| c.token.as_str())
                .collect::<Vec<_>>();
            (!tokens.is_empty()).then(|| {
                let key = match (black, tokens.len() == 1) {
                    (true, true) => "analytics.coins.added_below_bl_one",
                    (true, false) => "analytics.coins.added_below_bl_many",
                    (false, true) => "analytics.coins.added_below_wl_one",
                    (false, false) => "analytics.coins.added_below_wl_many",
                };
                t!(key, coins = tokens.join(", ")).to_string()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The result face: wrapping prose, measured single-line suggestions, and wrapping actions.
fn result_el(
    r: &ResultModel,
    m: &AutoCardModel,
    acts: &AutoCardActs,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let has_pick = !r.black.is_empty() || !r.white.is_empty();
    let enabled = r.black.iter().chain(&r.white).any(|c| !c.excluded);
    let primary = if m.pending.is_some() && m.stale.is_none() {
        MoonButtonVariant::Soft
    } else {
        MoonButtonVariant::Blue
    };
    let mut body = v_flex()
        .debug_selector(|| "an-pick-result".into())
        .w_full()
        .min_w_0()
        .flex_none()
        .gap(design::ui_px(cx, 10.0))
        .child(
            div()
                .debug_selector(|| "an-pick-verdict".into())
                .w_full()
                .min_w_0()
                .whitespace_normal()
                .px(design::ui_px(cx, 10.0))
                .py(design::ui_px(cx, 8.0))
                .rounded(design::ui_px(cx, 5.0))
                .bg(moon_alpha(
                    if r.useful {
                        p.green
                    } else if has_pick {
                        p.red
                    } else {
                        p.border
                    },
                    0.12,
                ))
                .text_size(design::t_caption(cx))
                .child(r.line.clone()),
        );
    if let Some(cause) = m.stale {
        body = body.child(
            div()
                .debug_selector(|| "an-pick-stale".into())
                .w_full()
                .min_w_0()
                .whitespace_normal()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.amber))
                .child(stale_note(cause)),
        );
    }
    if r.adopted {
        return body
            .child(
                div()
                    .debug_selector(|| "an-pick-adopted".into())
                    .w_full()
                    .min_w_0()
                    .whitespace_normal()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .child(adopted_hint(r)),
            )
            .into_any_element();
    }
    if !has_pick {
        return body
            .child(h_flex().w_full().justify_end().child(btn(
                "an-pick-change",
                t!("analytics.coins.change_settings").to_string(),
                t!("analytics.coins.tip_settings").to_string(),
                primary,
                true,
                acts.on_reset.clone(),
            )))
            .into_any_element();
    }
    let mut chips = Vec::new();
    for (black, items) in [(true, &r.black), (false, &r.white)] {
        for c in items {
            let token = c.token.clone();
            let action = acts.on_exclude.clone();
            let id = format!("an-suggestion-{black}-{}", c.token);
            let (bg, fg) = if black {
                (p.red, p.red_text)
            } else {
                (p.green, p.green_text)
            };
            chips.push(
                h_flex()
                    .debug_selector({
                        let id = id.clone();
                        move || format!("{id}-chip")
                    })
                    .flex_none()
                    .max_w_full()
                    .whitespace_nowrap()
                    .items_center()
                    .pl(design::ui_px(cx, 6.0))
                    .rounded(design::ui_px(cx, 3.0))
                    .bg(moon_alpha(bg, if c.excluded { 0.06 } else { 0.16 }))
                    .text_color(moon_alpha(fg, if c.excluded { 0.45 } else { 1.0 }))
                    .font_family(design::mono())
                    .text_size(design::t_caption(cx))
                    .child(
                        div()
                            .id(SharedString::from(format!("{id}-caption")))
                            // A measured basis survives GPUI's intrinsic-width pass even for
                            // one chip. Only this viewport shrinks for an oversized caption.
                            .w(px(
                                design::mono_caption_text_width(cx, &c.text, 400.0).ceil()
                            ))
                            .min_w_0()
                            .truncate()
                            .tooltip(crate::panels::common::text_tooltip_value(c.text.clone()))
                            .when(c.excluded, |el| el.line_through())
                            .child(
                                div()
                                    .debug_selector({
                                        let id = id.clone();
                                        move || format!("{id}-text")
                                    })
                                    .w_full()
                                    .flex_none()
                                    .truncate()
                                    .child(c.text.clone()),
                            ),
                    )
                    .child(super::layout::tagged(
                        format!("{id}-exclude"),
                        MoonButton::new(SharedString::from(id))
                            .variant(MoonButtonVariant::Bare)
                            .xsmall()
                            .label("\u{00d7}")
                            .tooltip(t!("analytics.coins.tip_exclude").to_string())
                            .on_click(move |_, window, app| {
                                action(black, token.clone(), window, app)
                            })
                            .render(),
                    ))
                    .into_any_element(),
            );
        }
    }
    body = body.child(
        h_flex()
            .debug_selector(|| "an-pick-suggestions".into())
            .w_full()
            .min_w_0()
            .flex_none()
            .flex_wrap()
            .gap(design::ui_px(cx, 6.0))
            .children(chips),
    );
    let label = match (r.black.is_empty(), r.white.is_empty()) {
        (false, true) => "analytics.coins.add_save_bl",
        (true, false) => "analytics.coins.add_save_wl",
        _ => "analytics.coins.add_save",
    };
    let save = add_and_save(acts.on_add.clone(), acts.on_save.clone());
    let mut actions = h_flex()
        .debug_selector(|| "an-pick-actions".into())
        .w_full()
        .min_w_0()
        .flex_none()
        .flex_wrap()
        .items_center()
        .gap(design::ui_px(cx, 8.0));
    if m.stale.is_some() {
        actions = actions
            .child(btn(
                "an-pick-rerun",
                t!("analytics.coins.rerun").to_string(),
                t!("analytics.coins.tip_pick_btn").to_string(),
                primary,
                m.can_pick(),
                acts.on_pick.clone(),
            ))
            .child(div().flex_1())
            .child(btn(
                "an-pick-add-save",
                t!(label).to_string(),
                t!("analytics.coins.tip_add_save").to_string(),
                MoonButtonVariant::Bare,
                enabled,
                save,
            ));
    } else {
        actions = actions
            .child(btn(
                "an-pick-add-save",
                t!(label).to_string(),
                t!("analytics.coins.tip_add_save").to_string(),
                primary,
                enabled,
                save,
            ))
            .child(btn(
                "an-pick-add-only",
                t!("analytics.coins.add_only").to_string(),
                t!("analytics.coins.tip_add_only").to_string(),
                MoonButtonVariant::Soft,
                enabled,
                acts.on_add.clone(),
            ))
            .child(div().flex_1());
        if r.useful {
            actions = actions.child(btn(
                "an-pick-one",
                t!("analytics.coins.one_by_one").to_string(),
                t!("analytics.coins.tip_one_by_one").to_string(),
                MoonButtonVariant::Bare,
                true,
                acts.on_one_by_one.clone(),
            ));
        }
        actions = actions.child(btn(
            "an-pick-rerun",
            t!("analytics.coins.rerun").to_string(),
            t!("analytics.coins.tip_pick_btn").to_string(),
            MoonButtonVariant::Bare,
            m.can_pick(),
            acts.on_pick.clone(),
        ));
    }
    body.child(actions).into_any_element()
}

impl AnalyticsView {
    /// Lazily-created number input of the pick card. A valid number commits on every change
    /// and is persisted; leaving the field writes back the value actually in force.
    fn pick_input(
        &mut self,
        holdout: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        let slot = if holdout {
            &self.coins.pick_holdout_input
        } else {
            &self.coins.pick_min_input
        };
        if let Some(s) = slot {
            return s.clone();
        }
        let state = if holdout {
            self.numeric_setting_input(
                "",
                |s| s.pick_cfg.holdout_pct,
                |s, v| s.pick_cfg.holdout_pct = v,
                |raw| {
                    raw.parse::<u32>()
                        .ok()
                        .filter(|v| HOLDOUT_PCT_RANGE.contains(v))
                },
                window,
                cx,
            )
        } else {
            self.numeric_setting_input(
                "",
                |s| s.pick_cfg.min_trades,
                |s, v| s.pick_cfg.min_trades = v,
                |raw| raw.parse::<u32>().ok().filter(|v| *v >= 1),
                window,
                cx,
            )
        };
        if holdout {
            self.coins.pick_holdout_input = Some(state.clone());
        } else {
            self.coins.pick_min_input = Some(state.clone());
        }
        state
    }

    /// The Auto-pick card, fed from this view's state.
    pub(in crate::analytics::tuner) fn coins_auto_card(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let min_in = self.pick_input(false, window, cx);
        let hold_in = self.pick_input(true, window, cx);
        fn settled<T>(s: &LoadState<T>) -> bool {
            s.data().is_some() && !matches!(s, LoadState::Loading { .. })
        }
        let loaded = settled(&self.coins.stats) && settled(&self.coin_lists.rows);
        if self.coins.retire_saved_pick() {
            self.arm_coin_kpi(cx);
        }
        // The batch already fitted this strategy: show its verdict without a second scan.
        self.adopt_auto_pick(loaded, cx);
        let model = AutoCardModel::new(
            &self.coins,
            self.sel_strategy.is_some(),
            self.is_multi(),
            loaded,
        );
        let view = cx.entity();
        let act = |f: fn(&mut AnalyticsView, &mut Context<AnalyticsView>)| -> Act {
            let view = view.clone();
            Rc::new(move |_: &mut Window, app: &mut App| view.update(app, f))
        };
        let flag = |f: fn(&mut AnalyticsView, bool, &mut Context<AnalyticsView>)| {
            let view = view.clone();
            Rc::new(move |on: bool, _: &mut Window, app: &mut App| {
                view.update(app, |this, cx| f(this, on, cx))
            }) as Rc<dyn Fn(bool, &mut Window, &mut App)>
        };
        let acts = AutoCardActs {
            on_pick: act(|this, cx| this.run_coin_pick(cx)),
            on_reset: act(|this, cx| this.reset_coin_pick(cx)),
            on_one_by_one: act(|this, cx| {
                this.coins.filter = super::state::CoinFilter::Suggested;
                this.coins.settle_filter();
                cx.notify();
            }),
            on_add: act(|this, cx| {
                this.coins.adopt_pick();
                this.arm_coin_kpi(cx);
                cx.notify();
            }),
            on_save: act(|this, cx| this.coins_open_save_dialog(cx)),
            on_revert: act(|this, cx| {
                this.coins.revert();
                this.arm_coin_kpi(cx);
                cx.notify();
            }),
            on_exclude: Rc::new({
                let view = view.clone();
                move |black, token, _, app| {
                    view.update(app, |this, cx| {
                        this.coins.toggle_exclude(black, token);
                        cx.notify();
                    })
                }
            }),
            on_black: flag(|this, on, cx| {
                this.coins.pick_cfg.black = on;
                this.persist_coins_settings(cx);
                cx.notify();
            }),
            on_white: flag(|this, on, cx| {
                this.coins.pick_cfg.white = on;
                this.persist_coins_settings(cx);
                cx.notify();
            }),
        };
        auto_card_el(&model, Some((&min_in, &hold_in)), &acts, p, cx)
    }
}

// Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
// shadows the built-in attribute and makes `#[test]` expand recursively.
#[cfg(test)]
mod tests;
