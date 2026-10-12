//! The toolbar Strategy pod: the toggle, picker and quick-select buttons for Moonbot manual
//! strategies.
//!
//! The mode is TERMINAL state, stored per core in `ServerConfig::manual_strategy` and not mirrored
//! to the core: an order names its strategy explicitly, so the core's own `use_manual_strategy`
//! switch stays where its user left it and two terminals on one core can work on different
//! strategies. The one exception is an order placed with the mode OFF — the exit barrier switches
//! the core's own mode off first, or the core would attach the strategy that switch names to a
//! zero `StratID`. A core that has never been set here is seeded once from its own snapshot
//! (`Backend::tick_manual_strat_seed`), which is what carries an upgrade over.
//!
//! With a strategy selected, the core derives the sell from that strategy unless its "ignore the
//! strategy's sell price" checkbox is on, so the toolbar's TP and S slots do not apply to new
//! orders and are disabled. The stop follows the core's own "Moonbot logic" switch
//! (`ManualStratState::mb_logic`, on by default): with it on the strategy owns the stop and the
//! toolbar only reports it; with it off the visible stop is written to the order after placement.

use gpui::prelude::FluentBuilder;
use gpui::*;
use rust_i18n::t;

use moon_ui::{
    MoonButton, MoonButtonSegment, MoonButtonVariant, MoonMenuItem, MoonPalette, MoonPopover,
    MoonPopoverPlacement, MoonPopupMenu, MoonSelectorPill, MoonSelectorSegment, MoonSize,
    MoonTheme, MoonToggle, MoonToggleLabelSide, MoonToggleSize, h_flex,
};

use moon_core::config::MANUAL_STRAT_SLOTS;
use moon_core::feed::{StrategyRow, StrategySchemaModel};
use moon_core::session::CoreId;

use crate::backend::{MANUAL_STRATEGY_KIND, manual_strategy_id};
use crate::shell::Shell;
use crate::{Backend, design};

mod fit;
mod settings;
use fit::{LabelMode, SlotWidths, StratFit, resolve_strat_fit};

/// Gap between two adjacent quick-strategy buttons in the Strategy pod.
const BTN_GAP: f32 = 4.0;
/// Estimated non-text chrome (padding, border) of one `Sm`-tier button carrying one
/// segment, at design-reference scale — run through `design::ui_value` before use, like every
/// other estimate below. MoonUI computes the real value from its own metrics; this is a
/// conservative estimate for the fit ladder, pending an on-screen check.
///
const BTN_CHROME_W: f32 = 20.0;
/// Padding between a quick-select button and the dashed hook frame around it, at design-reference
/// scale. Non-zero so the frame reads as a mark ON the button rather than as a second border
/// welded to its own.
///
/// Deliberately NOT folded into [`BTN_CHROME_W`]: that one is the BUTTON's own chrome and is also
/// what `.width()` hands the button, so widening it would fatten every button instead of the frame
/// around it. The frame is the wrapper's, and only the fit ladder adds it.
///
/// ONE, not two, and the height is what fixes it: a `Sm`-tier button is 26 + font delta tall, so
/// the frame has exactly 6px to spend on both sides together — `2 * (1 pad + 2 border)`. At two
/// the buttons would overhang the row they sit in.
const HOOK_FRAME_PAD: f32 = 1.0;
/// Border width of that frame, in raw pixels — `border_2()` is not font-scaled, so the ladder must
/// not scale it either.
///
/// TWO, not one: the dash pattern is a fixed multiple of the border width, so a 1px dashed border
/// is a 2px dash against a 1px gap and reads as solid at a glance — which would leave the mark
/// saying the same thing as an ordinary border.
const HOOK_FRAME_BORDER: f32 = 2.0;
/// Opacity of that frame's amber.
///
/// At full alpha a 2px dashed amber outline around a small toolbar button is the loudest thing on
/// the row — louder than the strategy it marks. The frame is a MARK, not an alarm: it
/// says "this slot's exits come from a hook", a standing fact rather than a state anybody has to
/// act on. At this alpha it still reads as amber and still reads as dashed, without pulling the
/// eye off the row.
///
/// Owner's choice at 0.3, one step below the 0.45 a second-hand status carries
/// ([`design::STALE_ALPHA`]): that value was still too loud on this surface. Not shared with the
/// stale-status ladder on purpose — the two say different things, and tying them would move both.
const HOOK_FRAME_ALPHA: f32 = 0.3;
/// Content width of the quick-select settings popup, in font-scaled pixels.
///
/// Sized for the widest row it holds: the show box, the strategy picker, the hook picker, the two
/// reveal buttons and the hotkey column — about 484 of it at design-reference scale, leaving the
/// same headroom the row had before those columns joined it.
const SLOT_SETTINGS_W: f32 = 540.0;
const BTN_TEXT_WEIGHT: f32 = 500.0;
/// Estimated non-text chrome of the picker pill at design-reference scale: leading dot, padding,
/// and border.
const PILL_CHROME_W: f32 = 40.0;
/// Reduced picker-pill label cap used once the button row has already dropped to zero buttons.
const REDUCED_PILL_MAX_W: f32 = 120.0;
/// Width of the "MS" toggle (track, gap and label) at the control tier.
///
/// The label is measured in the mono family because the MS toggle sets no `.mono()` and
/// `MoonToggle` defaults `mono: true`.
fn ms_toggle_width(cx: &App) -> f32 {
    let m = MoonToggleSize::density_default(&MoonTheme::active_tokens(cx)).reference_metrics();
    // The label renders in the inherited UI face (`MoonToggle` is not `.mono`), in a whole-pixel
    // text box.
    design::ui_value(cx, m.track_width + m.gap)
        + design::ui_text_width_zoomed(cx, "MS", m.font_size, m.label_weight, false).ceil()
}

/// One quick-select button slot as the pod draws it.
///
/// No hotkey segment: a button shows its CAPTION alone — the trader's own, or the strategy's name
/// when they set none. The binding belongs to the slot rather than to the label, and is stated in
/// the settings popup, where there is room for it.
struct StratButtonSlot {
    /// Zero-based quick-select slot.
    slot: usize,
    /// The strategy name the slot is assigned, trimmed; empty when unassigned.
    strategy: String,
    /// The strategy the name resolves to in this snapshot.
    sid: Option<u64>,
    /// The button's caption: the strategy name, or the slot number when unassigned.
    caption: String,
    /// The slot's 1-based number, the caption at the number-only clip level.
    numeric: String,
    /// The hook this slot's strategy defers its exits to; empty when none.
    hook: String,
}

/// The selected strategy's parameter summary, as the pod's read-outs show it.
#[derive(Clone, Debug, PartialEq)]
enum StratSummary {
    /// The strategy hands its exits to a MoonHook, named here: its own Buy/Sell/SL apply to nothing.
    Hook(String),
    /// Moonbot's own four read-outs.
    Params {
        buy: String,
        sell: String,
        sl_on: bool,
        ts_on: bool,
    },
}

/// One summary read-out: an optional caption (UI face) and its value (mono), struck when off.
struct SummaryPart {
    caption: Option<&'static str>,
    value: String,
    struck: bool,
}

impl StratSummary {
    /// The read-outs in the order the pod draws them.
    fn parts(&self) -> Vec<SummaryPart> {
        match self {
            StratSummary::Hook(name) => vec![SummaryPart {
                caption: None,
                value: t!("header.ms_summary_hook", name = name.as_str()).to_string(),
                struck: false,
            }],
            StratSummary::Params {
                buy,
                sell,
                sl_on,
                ts_on,
            } => vec![
                SummaryPart {
                    caption: Some("Buy"),
                    value: buy.clone(),
                    struck: false,
                },
                SummaryPart {
                    caption: Some("Sell"),
                    value: sell.clone(),
                    struck: false,
                },
                SummaryPart {
                    caption: Some("SL"),
                    value: on_off(*sl_on).to_string(),
                    struck: !*sl_on,
                },
                SummaryPart {
                    caption: Some("TS"),
                    value: on_off(*ts_on).to_string(),
                    struck: false,
                },
            ],
        }
    }
}

/// Gap between a summary read-out's caption and its value, before the UI scale.
const SUMMARY_CAPTION_GAP: f32 = 3.0;
/// Gap between two summary read-outs, before the UI scale.
const SUMMARY_PART_GAP: f32 = 6.0;
/// Weight of a summary read-out's value.
const SUMMARY_VALUE_WEIGHT: f32 = 500.0;

/// The toolbar Strategy pod's contents: the MS toggle, its slots gear, the picker pill or the
/// quick-select buttons, and the selected strategy's parameter summary.
///
/// Built in two phases so the toolbar can count the pod before it picks a collapse level: [`read`]
/// resolves and measures everything, [`render`] draws it at the level the toolbar chose. Both read
/// the one resolved state, so the counted and the drawn pod cannot disagree.
///
/// The buttons are drawn only while manual-strategy mode is ENABLED: with MS off they select
/// nothing a new order would use.
///
/// Each button carries two gestures, matching Moonbot: left click fires the slot's strategy and
/// right click assigns which strategy it fires. Everything else lives in the gear popup beside them
/// (`settings::slot_settings_content`): per-slot visibility, which strategy each slot fires, that
/// strategy's hook, the two exit rules, and a link into the Strategies window.
///
/// [`read`]: StratPod::read
/// [`render`]: StratPod::render
pub(crate) struct StratPod {
    core: CoreId,
    on: bool,
    id: u64,
    manuals: Vec<(u64, String)>,
    summary: Option<StratSummary>,
    full_pill_text: String,
    dot_color: u32,
    /// The slots on screen while the mode is on; `None` with the mode off.
    buttons: Option<Vec<StratButtonSlot>>,
    /// Measured width of each slot in `buttons`, in render order.
    slot_widths: Vec<SlotWidths>,
    /// Which drawn slot fires the selected strategy.
    selected_slot: Option<usize>,
    /// Everything but the buttons: toggle, gear, pill and their gaps.
    base: f32,
    /// What the pill and its gap add to `base` where the full ladder's buttons replace the pill
    /// (the pod then does not draw it); `0` where the full ladder draws the pill.
    pill_replaced_w: f32,
    /// Width of the summary read-outs, the gap in front of them included; `0` without one.
    summary_w: f32,
    /// Whether the selection names a strategy that does not resolve, which keeps the picker up.
    unresolved: bool,
}

impl StratPod {
    /// Resolve and measure the pod for `group`'s active trade core.
    ///
    /// The caller owns the rest of the gate (MANUAL workspace mode, not Overview); this answers
    /// `None` when the group has no active trade core or that core has no Manual-kind strategies.
    ///
    /// Args:
    ///     group: Window group whose active trade core the pod governs.
    ///     backend: Shared terminal state.
    ///     p: Active palette, for the pill's status dot.
    ///     cx: Application context used for state reads and measurement.
    ///
    /// Returns:
    ///     The resolved pod, or `None` when there is nothing to choose.
    pub(crate) fn read(
        group: &str,
        backend: &Entity<Backend>,
        p: MoonPalette,
        cx: &App,
    ) -> Option<Self> {
        let b = backend.read(cx);
        let core = b.active_trade_core(group)?;
        let core_data = b.session.store().core(core)?;
        let manuals = manual_strategy_options(&core_data.strategies)?;
        let (on, id) = b.manual_strat_state(core);
        let sel_row = core_data.strategies.iter().find(|s| s.id == id && id != 0);
        let schema = core_data.schema.as_ref();
        // Show the Moonbot-style Buy/Sell/SL/TS summary only while the mode is enabled.
        let summary = (on && sel_row.is_some())
            .then(|| sel_row.map(|r| strat_summary(r, schema)))
            .flatten();

        // The pill shows the selected strategy, `?` when one is selected but does not resolve, and
        // the localized none marker when nothing is selected at all. A name that resolves to
        // nothing is NOT an empty selection — the strategy was renamed, deleted, or has not
        // arrived, and an order in that state is refused rather than placed, so the two must not
        // look alike.
        let named = b.selected_manual_strategy_name(core).is_some();
        let full_pill_text: String = match sel_row {
            Some(r) => r.name.clone(),
            None if named => "?".to_string(),
            None => t!("header.ms_none").to_string(),
        };
        let dot_color = if on && sel_row.is_some() {
            design::positive_color(p)
        } else if on && named {
            // A selection that NAMES a strategy but does not resolve: the danger colour, because
            // every order on this core is being refused. The mode being on with nothing selected
            // is not that — it is an ordinary manual order — and painting it red would cry wolf.
            design::danger_color(p)
        } else {
            p.text_muted
        };

        // Quick-select buttons. The SLOTS come from this terminal (`Backend::strat_slots`), which
        // falls back to the core's own `manual_strats_names` until a button is assigned here; the
        // core's config still decides which slots are ON SCREEN (`use_buttons` / `show_button[i]`),
        // plus any slot this terminal has assigned — that one must be reachable even if the core
        // never showed it, or an assignment could not be undone.
        let slots = b.strat_slots(core);
        let buttons: Option<Vec<StratButtonSlot>> = (on && slots.is_some()).then(|| {
            let slots = slots.as_deref().unwrap_or_default();
            (0..MANUAL_STRAT_SLOTS)
                .filter(|&i| slots.get(i).is_some_and(|slot| slot.show))
                .map(|i| {
                    let slot = slots.get(i).cloned().unwrap_or_default();
                    let strategy = slot.strategy.trim().to_string();
                    // Match the slot's STRATEGY NAME against this snapshot's Manual-kind strategies
                    // rather than its ordinal position: the slot is the core's slot `i` while an
                    // ordinal match would address the ix-th Manual-kind strategy in snapshot order
                    // — the two can disagree, and a button that fires the wrong strategy places a
                    // real order silently. Through the shared resolver, so the button and the
                    // hotkey for the same slot cannot reach different strategies.
                    let sid = manual_strategy_id(&core_data.strategies, &strategy);
                    let numeric = (i + 1).to_string();
                    // The button IS its strategy; an unassigned slot falls back to its own number
                    // so the empty slot stays clickable and identifiable.
                    let caption = if strategy.is_empty() {
                        numeric.clone()
                    } else {
                        strategy.clone()
                    };
                    // What this slot's strategy defers its exits to: it changes what the button
                    // MEANS — both exits then come from that hook — so a drawn button states it.
                    let hook = sid
                        .and_then(|sid| core_data.strategies.iter().find(|row| row.id == sid))
                        .map(|row| crate::backend::hook_of(row, schema))
                        .unwrap_or_default();
                    StratButtonSlot {
                        slot: i,
                        strategy,
                        sid,
                        caption,
                        numeric,
                        hook,
                    }
                })
                .collect::<Vec<_>>()
        });
        let unresolved = b.manual_strat_unresolved(core).is_some();
        Some(Self::measure(
            core,
            (on, id),
            manuals,
            summary,
            full_pill_text,
            dot_color,
            unresolved,
            buttons,
            cx,
        ))
    }

    /// Measure a resolved pod: slot ladder, pill, base chrome and summary widths.
    #[allow(clippy::too_many_arguments)]
    fn measure(
        core: CoreId,
        (on, id): (bool, u64),
        manuals: Vec<(u64, String)>,
        summary: Option<StratSummary>,
        full_pill_text: String,
        dot_color: u32,
        unresolved: bool,
        buttons: Option<Vec<StratButtonSlot>>,
        cx: &App,
    ) -> Self {
        let btn_chrome_w = design::ui_value(cx, BTN_CHROME_W);
        // The dashed hook frame is real width every slot spends, hooked or not — the row draws it
        // on all of them so its geometry cannot depend on which strategies carry a hook.
        let frame_w = (design::ui_value(cx, HOOK_FRAME_PAD) + HOOK_FRAME_BORDER) * 2.0;
        let label_w = |text: &str| {
            design::ui_text_width_zoomed(cx, text, design::BODY_TEXT, BTN_TEXT_WEIGHT, true)
        };
        let slot_widths: Vec<SlotWidths> = buttons
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|slot| SlotWidths {
                name_only: (btn_chrome_w + label_w(&slot.caption)).ceil() + frame_w,
                number_only: (btn_chrome_w + label_w(&slot.numeric)).ceil() + frame_w,
            })
            .collect();
        let pill_text_w = |cap: f32| {
            design::fit_text(&full_pill_text, design::font_w(cx, cap), |s| {
                design::ui_text_width_zoomed(cx, s, design::BODY_TEXT, 500.0, true)
            })
            .1
        };
        let pill_full_w =
            design::ui_value(cx, PILL_CHROME_W) + pill_text_w(design::HEADER_LABEL_MAX_W);
        let gap = design::ui_value(cx, ROW_GAP);
        // The toggle, and with the mode on the gear and the pill: the gear is permanent chrome
        // while the mode is on, and the pill is budgeted even where the buttons end up replacing
        // it — whether it is drawn depends on what the slots resolve to, and a budget that
        // under-reserves overflows the row instead of merely showing one button fewer.
        let base = ms_toggle_width(cx)
            + if on {
                gap * 2.0 + design::glyph_btn_w(cx) + pill_full_w
            } else {
                0.0
            };
        // At the full ladder every slot is drawn, so the buttons replace the pill exactly when one
        // assigned slot fires the selection and it resolves (`render`'s `buttons_shown`).
        let pill_replaced = on
            && !unresolved
            && buttons
                .as_deref()
                .unwrap_or_default()
                .iter()
                .any(|slot| slot.sid.is_some() && slot.sid == Some(id));
        let pill_replaced_w = if pill_replaced {
            gap + pill_full_w
        } else {
            0.0
        };
        let summary_w = summary
            .as_ref()
            .map(|summary| design::ui_value(cx, ROW_GAP) + summary_width(summary, cx))
            .unwrap_or(0.0);
        // Which rendered slot is the active one, so the ladder measures it the way it draws it.
        let selected_slot = on
            .then(|| {
                buttons.as_deref().and_then(|slots| {
                    slots
                        .iter()
                        .position(|slot| slot.sid.is_some() && slot.sid == Some(id))
                })
            })
            .flatten();
        Self {
            core,
            on,
            id,
            manuals,
            summary,
            full_pill_text,
            dot_color,
            unresolved,
            buttons,
            slot_widths,
            selected_slot,
            base,
            pill_replaced_w,
            summary_w,
        }
    }

    /// A synthetic pod with the mode on, three named slots and the parameter summary — the
    /// widest shape the toolbar draws, for layout tests.
    #[cfg(test)]
    pub(crate) fn synthetic(cx: &App) -> Self {
        let names = ["Scalp fast", "Breakout long", "Dip catcher"];
        let buttons = names
            .iter()
            .enumerate()
            .map(|(i, name)| StratButtonSlot {
                slot: i,
                strategy: name.to_string(),
                sid: Some(i as u64 + 1),
                caption: name.to_string(),
                numeric: (i + 1).to_string(),
                hook: String::new(),
            })
            .collect();
        Self::measure(
            1,
            (true, 1),
            names
                .iter()
                .enumerate()
                .map(|(i, name)| (i as u64 + 1, name.to_string()))
                .collect(),
            Some(StratSummary::Params {
                buy: "-1.25%".into(),
                sell: "+2.50%".into(),
                sl_on: true,
                ts_on: false,
            }),
            names[0].to_string(),
            0x22aa66,
            false,
            Some(buttons),
            cx,
        )
    }

    /// The pod's controls at their FULL inner ladder — every slot at its name — without the
    /// summary: what the toolbar counts at every level (`ToolbarWidths::strat_controls`). Exactly
    /// what that ladder draws: no pill where its buttons replace it. The narrower rungs keep
    /// reserving the pill (`fit`), since shedding the selected slot brings it back.
    pub(crate) fn controls_width(&self, cx: &App) -> f32 {
        let row = self.row_width(cx);
        self.base - self.pill_replaced_w
            + if row > 0.0 {
                design::ui_value(cx, ROW_GAP) + row
            } else {
                0.0
            }
    }

    /// Width of the parameter summary, the gap in front of it included; `0` when there is none.
    pub(crate) fn summary_width(&self) -> f32 {
        self.summary_w
    }

    /// Full-name button row width, `0` with no buttons.
    fn row_width(&self, cx: &App) -> f32 {
        if self.slot_widths.is_empty() {
            return 0.0;
        }
        self.slot_widths.iter().map(|w| w.name_only).sum::<f32>()
            + design::ui_value(cx, BTN_GAP) * (self.slot_widths.len() - 1) as f32
    }

    /// Resolve the inner ladder against `budget`, the width the pod's controls may spend.
    fn fit(&self, budget: f32, cx: &App) -> Option<StratFit> {
        self.buttons.as_ref().map(|_| {
            // The row stands after the base with one gap in front of it.
            resolve_strat_fit(
                budget,
                design::ui_value(cx, BTN_GAP),
                &self.slot_widths,
                self.selected_slot,
                self.base + design::ui_value(cx, ROW_GAP),
            )
        })
    }

    /// Whether the slots gear is drawn: it stands with the switch while the mode is on, at every
    /// inner ladder rung.
    pub(crate) fn gear_shown(&self) -> bool {
        self.on
    }

    /// Whether quick-select slot `slot` is drawn at `budget` — a slot menu whose button the ladder
    /// sheds must close with it.
    pub(crate) fn slot_shown(&self, slot: usize, budget: f32, cx: &App) -> bool {
        let (Some(buttons), Some(fit)) = (self.buttons.as_deref(), self.fit(budget, cx)) else {
            return false;
        };
        buttons
            .iter()
            .take(fit.visible_count)
            .any(|button| button.slot == slot)
    }

    /// The core the pod governs.
    pub(crate) fn core(&self) -> CoreId {
        self.core
    }

    /// Draw the pod's contents at the toolbar's level.
    ///
    /// Args:
    ///     group: Window group whose active trade core the pod governs.
    ///     io: Shared terminal state receiving the pod's edits and the owning shell holding the
    ///         slot menu and settings popup state; `None` renders the pod inert, same geometry.
    ///     slot_menu: The open right-click assign menu, when any.
    ///     slots_open: Whether the slots settings popup is open.
    ///     show_summary: Whether the toolbar level keeps the parameter summary.
    ///     budget: Width the controls may spend; `f32::INFINITY` keeps the full inner ladder.
    ///     p: Active palette.
    ///     cx: Application context used to render.
    ///
    /// Returns:
    ///     The pod's contents, without the pod frame or caption.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render(
        self,
        group: &str,
        io: Option<(&Entity<Backend>, &Entity<Shell>)>,
        slot_menu: Option<(CoreId, usize)>,
        slots_open: bool,
        show_summary: bool,
        budget: f32,
        p: MoonPalette,
        cx: &App,
    ) -> AnyElement {
        let fit = self.fit(budget, cx);
        let backend: Option<Entity<Backend>> = io.map(|(backend, _)| backend.clone());
        let shell: Option<Entity<Shell>> = io.map(|(_, shell)| shell.clone());
        let Self {
            core,
            on,
            id,
            manuals,
            summary,
            full_pill_text,
            dot_color,
            buttons,
            unresolved,
            ..
        } = self;

        // Only ASSIGNED slots replace the picker. A row of empty slots chooses nothing — clicking
        // one is a no-op — so hiding the dropdown behind it would leave no way to pick a strategy
        // at all. The picker also stays whenever the selection resolves to nothing, because its `?`
        // and its danger dot are the only thing on screen saying why every order is being refused.
        let assigned_shown = match (buttons.as_deref(), fit) {
            (Some(slots), Some(fit)) => slots
                .iter()
                .take(fit.visible_count)
                .any(|slot| slot.sid.is_some()),
            _ => false,
        };
        // The selected strategy has to be visible SOMEWHERE. A button carries it only if one of
        // the rendered slots fires it; otherwise the picker is the only thing naming the selection.
        let selection_on_a_button = match (buttons.as_deref(), fit) {
            (Some(slots), Some(fit)) => slots
                .iter()
                .take(fit.visible_count)
                .any(|slot| slot.sid.is_some() && slot.sid == Some(id)),
            _ => false,
        };
        let buttons_shown = assigned_shown && selection_on_a_button && !unresolved;
        let pill_cap = if fit.is_some_and(|f| f.pill_reduced) {
            design::font_w(cx, REDUCED_PILL_MAX_W)
        } else {
            design::font_w(cx, design::HEADER_LABEL_MAX_W)
        };
        let display = design::fit_text(&full_pill_text, pill_cap, |s| {
            design::ui_text_width_zoomed(cx, s, design::BODY_TEXT, 500.0, true)
        })
        .0;

        let mut items = Vec::with_capacity(manuals.len());
        for (sid, name) in &manuals {
            let sid = *sid;
            let backend = backend.clone();
            items.push(
                MoonMenuItem::with_key(format!("ms-{sid}"), name.clone())
                    .selected(id == sid)
                    .checked(id == sid)
                    // Selecting a strategy also enables the mode, matching the Moonbot menu.
                    .on_click(move |_, _, cx| {
                        let Some(backend) = backend.as_ref() else {
                            return;
                        };
                        backend.update(cx, |b, bcx| {
                            set_manual(b, core, true, sid);
                            bcx.notify();
                        });
                    }),
            );
        }

        let toggle_backend = backend.clone();
        let mut row = h_flex()
            .flex_none()
            .gap(design::ui_px(cx, ROW_GAP))
            .items_center()
            .child(
                MoonToggle::new("header-ms-toggle")
                    .label("MS")
                    .label_side(MoonToggleLabelSide::Left)
                    .checked(on)
                    // Never disabled, including with nothing selected: the picker and the gear
                    // appear only while the mode is ON, so a switch that refused to turn on without
                    // a strategy would hide the one control able to choose one. The mode with no
                    // selection places ordinary manual orders — `manual_strat_active` answers
                    // `None` — which is exactly what it did before this switch existed.
                    .on_change(move |ch: &bool, _w, app| {
                        let v = *ch;
                        let Some(toggle_backend) = toggle_backend.as_ref() else {
                            return;
                        };
                        toggle_backend.update(app, |b, bcx| {
                            let (_, cur_id) = b.manual_strat_state(core);
                            // Disabling preserves the id so the next toggle restores the same
                            // strategy.
                            set_manual(b, core, v, cur_id);
                            bcx.notify();
                        });
                    }),
            )
            // The gear sits with the switch it belongs to and exists only while MS is on: with the
            // mode off there are no buttons on the row, so settings for them would configure
            // nothing visible.
            .children(on.then(|| {
                let gear_shell = shell.clone();
                let content = io.filter(|_| slots_open).map(|(backend, shell)| {
                    settings::slot_settings_content(core, group, backend, shell, p, cx)
                });
                crate::chrome::terminal_chrome::header_gear_popover(
                    "ms-slots",
                    MoonPopoverPlacement::BottomStart,
                    SLOT_SETTINGS_W,
                    slots_open,
                    content,
                    MoonButton::new("ms-slots-gear")
                        // An explicit square, the width the fit ladder counts.
                        .width(design::glyph_btn_w(cx))
                        .variant(MoonButtonVariant::Ghost)
                        .icon("icons/settings.svg")
                        .tooltip(t!("header.ms_slots_gear").to_string())
                        .render(),
                    move |open, _, app| {
                        let Some(gear_shell) = gear_shell.as_ref() else {
                            return;
                        };
                        gear_shell.update(app, |shell, cx| {
                            shell.set_strat_slots_open(open);
                            cx.notify();
                        });
                    },
                )
            }))
            // The picker exists to choose a strategy, and only the enabled mode uses one: with MS
            // off nothing it selects reaches an order, so it leaves the row entirely. With the mode
            // on it yields to the quick-select buttons — the same choice, one click away — and
            // returns the moment none are shown.
            .children((on && !buttons_shown).then(|| {
                MoonPopover::new("header-ms-selector")
                    .placement(MoonPopoverPlacement::BottomStart)
                    .fit_content()
                    .close_on_content_click(true)
                    .trigger({
                        // MoonSelectorPill.height/radius take design units and scale them with
                        // `tokens.ui()`; `action_control_h_value` is already scaled, so undo that
                        // once (same conversion as the header core pill).
                        let pill_h_units =
                            design::action_control_h_value(cx) / design::ui_value(cx, 1.0);
                        MoonSelectorPill::new("header-ms-pill")
                            .height(pill_h_units)
                            .radius(pill_h_units / 2.0)
                            .leading_dot(dot_color)
                            .segment(
                                MoonSelectorSegment::new(display)
                                    .color(if on { p.text } else { p.text_soft })
                                    .weight(500.0)
                                    .font_size(design::body_font_base(cx, 0.0)),
                            )
                            .render()
                    })
                    .content(
                        MoonPopupMenu::new("header-ms-menu")
                            .fit_width(200.0, 560.0)
                            .size(MoonSize::Xs)
                            .items(items)
                            .render(),
                    )
            }));
        // The quick-select buttons: the fit ladder resolved above decides the label mode and how
        // many slots to render; `0` renders none, leaving the toggle and the pill.
        if let (Some(slots), Some(fit)) = (buttons, fit)
            && fit.visible_count > 0
        {
            row = row.child(slot_buttons(
                slots, fit, core, id, on, &manuals, &backend, &shell, slot_menu, p, cx,
            ));
        }
        if let Some(summary) = summary.filter(|_| show_summary) {
            row = row.child(summary_readouts(&summary, p, cx));
        }
        row.into_any_element()
    }
}

/// Gap between the pod's toggle, gear, pill and button row, before the UI scale.
const ROW_GAP: f32 = 8.0;

/// Rendered width of the summary read-outs.
fn summary_width(summary: &StratSummary, cx: &App) -> f32 {
    let parts = summary.parts();
    let caption_gap = design::ui_value(cx, SUMMARY_CAPTION_GAP);
    let part_gap = design::ui_value(cx, SUMMARY_PART_GAP);
    parts
        .iter()
        .map(|part| {
            part.caption.map_or(0.0, |caption| {
                design::ui_caption_text_width(cx, caption, 400.0) + caption_gap
            }) + match part.caption {
                Some(_) => design::mono_caption_text_width(cx, &part.value, SUMMARY_VALUE_WEIGHT),
                None => design::ui_caption_text_width(cx, &part.value, 400.0),
            }
        })
        .sum::<f32>()
        + part_gap * parts.len().saturating_sub(1) as f32
}

/// The summary as compact read-outs: captions in the UI face, values mono, the stop struck and
/// muted while it is off. A hook summary is one UI-face sentence.
fn summary_readouts(summary: &StratSummary, p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .id("toolbar-ms-summary")
        .debug_selector(|| "toolbar-ms-summary".into())
        .flex_none()
        .items_center()
        .gap(design::ui_px(cx, SUMMARY_PART_GAP))
        .children(summary.parts().into_iter().map(|part| {
            let value = div()
                .flex_none()
                .whitespace_nowrap()
                .text_size(design::t_caption(cx))
                .when(part.caption.is_some(), |el| {
                    el.font_family(design::mono())
                        .font_weight(FontWeight(SUMMARY_VALUE_WEIGHT))
                })
                .when(part.caption.is_none(), |el| {
                    el.font_family(design::ui_font())
                })
                .text_color(rgb(if part.struck {
                    p.text_muted
                } else {
                    p.text_soft
                }))
                .when(part.struck, |el| el.line_through())
                .child(part.value);
            h_flex()
                .flex_none()
                .items_center()
                .gap(design::ui_px(cx, SUMMARY_CAPTION_GAP))
                .children(
                    part.caption
                        .map(|caption| design::pod_caption(caption, p, cx)),
                )
                .child(value)
        }))
}

/// The quick-select button row, `fit.visible_count` slots long.
///
/// Two gestures per button, matching Moonbot: left click fires the slot, right click assigns which
/// strategy it fires.
#[allow(clippy::too_many_arguments)]
fn slot_buttons(
    slots: Vec<StratButtonSlot>,
    fit: StratFit,
    core: CoreId,
    id: u64,
    on: bool,
    manuals: &[(u64, String)],
    backend: &Option<Entity<Backend>>,
    shell: &Option<Entity<Shell>>,
    slot_menu: Option<(CoreId, usize)>,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    // One list for every assign menu, built before the loop: it does not vary by slot, and
    // rebuilding it per button clones every strategy name ten times on every repaint.
    let slot_names = slot_strategy_names(manuals);
    let mut btn_row = h_flex().flex_none().gap(design::ui_px(cx, BTN_GAP));
    for StratButtonSlot {
        slot: i,
        strategy,
        sid,
        caption,
        numeric,
        hook,
    } in slots.into_iter().take(fit.visible_count)
    {
        let selected = on && sid.is_some() && sid == Some(id);
        let hooked = !hook.is_empty();
        // The SELECTED slot keeps its caption even at the number-only clip level: the row's job is
        // to say which strategy a new order will use, and a lone digit does not.
        let label = match fit.label_mode {
            LabelMode::NumberOnly if !selected => numeric,
            _ => caption,
        };
        // An explicit width, so the caption sits inside the button's own padding instead of
        // against its border. Measured through the same helper the fit ladder used, so the budget
        // and the rendered button cannot disagree.
        let label_w =
            design::ui_text_width_zoomed(cx, &label, design::BODY_TEXT, BTN_TEXT_WEIGHT, true);
        // Whole pixels, as the fit ladder counts it.
        let btn_w = (design::ui_value(cx, BTN_CHROME_W) + label_w).ceil();

        let mut btn = MoonButton::new(SharedString::from(format!("ms-btn-{i}")))
            // The active slot carries the accent variant rather than only `selected`: on a Panel
            // button the selected state is a few percent of background and reads as nothing on a
            // row of ten. Fill is therefore the SELECTION and nothing else — the hook is marked by
            // the dashed frame around the button instead, so a mark and a selection can never be
            // mistaken for each other.
            .variant(if selected {
                MoonButtonVariant::Blue
            } else {
                MoonButtonVariant::Panel
            })
            .width(btn_w)
            .selected(selected)
            .segment(MoonButtonSegment::new(label).weight(BTN_TEXT_WEIGHT));
        if sid.is_none() && !strategy.is_empty() {
            // Assigned to a name this core's snapshot does not have: say so rather than firing
            // something else. Still re-assignable by right click, hence not disabled.
            btn = btn
                .tooltip(t!("hotkeys.ms_button_unresolved", name = strategy.as_str()).to_string());
        } else if sid.is_none() {
            btn = btn.tooltip(t!("header.ms_slot_empty").to_string());
        } else if hooked {
            // The same sentence the parameter summary uses for the same fact, from the same key:
            // two wordings for "these exits come from that hook" would drift apart.
            btn = btn.tooltip(t!("header.ms_summary_hook", name = hook.as_str()).to_string());
        }
        let click_backend = backend.clone();
        let btn = btn.on_click(move |_, _, app| {
            let Some(sid) = sid else { return };
            let Some(click_backend) = click_backend.as_ref() else {
                return;
            };
            click_backend.update(app, |b, bcx| {
                set_manual(b, core, true, sid);
                bcx.notify();
            });
        });

        // Right click opens the assign menu for THIS slot. `MoonPopover` opens from a click on its
        // trigger, so the open state is driven from `Shell` instead and the wrapper only has to
        // catch the right button.
        let menu_shell = shell.clone();
        let close_shell = shell.clone();
        let clear_backend = backend.clone();
        let clear_shell = shell.clone();
        let mut menu_items = Vec::with_capacity(slot_names.len() + 1);
        menu_items.push(
            MoonMenuItem::with_key(
                format!("ms-slot-{i}-clear"),
                t!("header.ms_slot_clear").to_string(),
            )
            .selected(strategy.is_empty())
            .on_click(move |_, _, app| {
                let Some(clear_backend) = clear_backend.as_ref() else {
                    return;
                };
                clear_backend.update(app, |b, bcx| {
                    b.set_strat_slot_strategy(core, i, String::new());
                    bcx.notify();
                });
                let Some(clear_shell) = clear_shell.as_ref() else {
                    return;
                };
                clear_shell.update(app, |shell, cx| {
                    shell.close_strat_slot_menu();
                    cx.notify();
                });
            }),
        );
        for (ix, name) in slot_names.iter().enumerate() {
            let assigned = *name == strategy;
            let assign_name = name.clone();
            let assign_backend = backend.clone();
            let assign_shell = shell.clone();
            menu_items.push(
                // Keyed by POSITION: the key identifies the row to the menu, and a name is
                // arbitrary user text that two strategies can share.
                MoonMenuItem::with_key(format!("ms-slot-{i}-strat-{ix}"), name.clone())
                    .selected(assigned)
                    .checked(assigned)
                    .on_click(move |_, _, app| {
                        let assign_name = assign_name.clone();
                        let Some(assign_backend) = assign_backend.as_ref() else {
                            return;
                        };
                        assign_backend.update(app, |b, bcx| {
                            b.set_strat_slot_strategy(core, i, assign_name);
                            bcx.notify();
                        });
                        let Some(assign_shell) = assign_shell.as_ref() else {
                            return;
                        };
                        assign_shell.update(app, |shell, cx| {
                            shell.close_strat_slot_menu();
                            cx.notify();
                        });
                    }),
            );
        }
        // The menu hangs off a ZERO-SIZE anchor beside the button rather than wrapping it.
        // `MoonPopover` toggles itself on a left mouse-down on its trigger, so making the button
        // the trigger would open this menu on the very click that fires the strategy — and swallow
        // that click's mouse-down. An anchor nothing can click leaves the open state entirely to
        // the right-click handler below.
        btn_row = btn_row.child(
            div()
                .id(SharedString::from(format!("ms-btn-wrap-{i}")))
                .flex_none()
                .relative()
                // The hook mark: a dashed amber frame around the button, drawn on the wrapper the
                // right-click menu already needs. `MoonButton` offers no dashed border of its own —
                // variant and `.outline()` are its only border knobs — so this is the framework's
                // own `border_dashed`, in the palette's amber at [`HOOK_FRAME_ALPHA`] and the
                // shared button radius, rather than a bespoke widget or a hand-mixed colour.
                //
                // Drawn on EVERY button, transparent where there is no hook: a frame that only some
                // buttons carry would make the row's spacing depend on which strategies are hooked.
                .p(px(design::ui_value(cx, HOOK_FRAME_PAD)))
                // The button's own radius plus the padding between them, so the frame's corners
                // stay concentric with the ones they enclose instead of cutting inside them.
                .rounded(design::r_button(cx) + px(design::ui_value(cx, HOOK_FRAME_PAD)))
                .border_2()
                .border_dashed()
                .border_color(if hooked {
                    design::moon_alpha(p.amber, HOOK_FRAME_ALPHA)
                } else {
                    gpui::transparent_black()
                })
                .on_mouse_down(MouseButton::Right, move |_, _, app| {
                    let Some(menu_shell) = menu_shell.as_ref() else {
                        return;
                    };
                    menu_shell.update(app, |shell, cx| {
                        shell.open_strat_slot_menu(core, i);
                        cx.notify();
                    });
                })
                .child(btn.render())
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .bottom_0()
                        .w(px(0.0))
                        .h(px(0.0))
                        .child(
                            MoonPopover::new(SharedString::from(format!("ms-slot-menu-{i}")))
                                .placement(MoonPopoverPlacement::BottomStart)
                                .fit_content()
                                .close_on_content_click(true)
                                .open(slot_menu == Some((core, i)))
                                .on_open_change(move |open, _, app| {
                                    if !open {
                                        let Some(close_shell) = close_shell.as_ref() else {
                                            return;
                                        };
                                        close_shell.update(app, |shell, cx| {
                                            shell.close_strat_slot_menu();
                                            cx.notify();
                                        });
                                    }
                                })
                                .trigger(div().w(px(0.0)).h(px(0.0)))
                                .content(
                                    MoonPopupMenu::new(SharedString::from(format!(
                                        "ms-slot-list-{i}"
                                    )))
                                    .fit_width(160.0, 420.0)
                                    .size(MoonSize::Xs)
                                    .items(menu_items)
                                    .render(),
                                ),
                        ),
                ),
        );
    }
    btn_row
}

/// The distinct strategy NAMES a quick-select slot can be assigned.
///
/// A slot stores a name (`StratSlot::strategy`) and fires whatever that name resolves to, first
/// match wins. Two strategies sharing a name are therefore one choice however they are listed:
/// offering both would let the second be picked and the first be assigned, with both rows then
/// drawn as selected. The pod's own picker is unaffected — it selects an id and pins it.
fn slot_strategy_names(manuals: &[(u64, String)]) -> Vec<String> {
    let mut names: Vec<String> = Vec::with_capacity(manuals.len());
    for (_, name) in manuals {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    names
}

/// Collect Manual-kind strategy ids and names in their snapshot order.
///
/// Duplicates are kept: the pod's own picker selects by ID and pins it, so two strategies
/// sharing a name are two real, separately selectable choices there. A SLOT is different — it
/// stores the name — and its pickers go through [`slot_strategy_names`] instead.
///
/// Args:
///     strategies: Current strategy snapshot for the active trade core.
///
/// Returns:
///     Ordered picker options, or `None` when the snapshot has no Manual-kind strategies.
fn manual_strategy_options(strategies: &[StrategyRow]) -> Option<Vec<(u64, String)>> {
    let options: Vec<_> = strategies
        .iter()
        // The same two rules `manual_strategy_id` resolves by: a zero id is the sentinel for
        // "nothing selected" everywhere here, and the name is compared trimmed. An untrimmed option
        // is stored untrimmed and then never compares equal to the slot that fires it, so the
        // picker shows no selection for a button that works.
        .filter(|strategy| strategy.kind_ordinal == MANUAL_STRATEGY_KIND && strategy.id != 0)
        .map(|strategy| (strategy.id, strategy.name.trim().to_string()))
        .collect();
    (!options.is_empty()).then_some(options)
}

#[cfg(test)]
mod tests;

/// Fire quick-select SLOT `ix` and enable manual-strategy mode.
///
/// This performs the same update as clicking button `ix` in the Strategy pod, and deliberately addresses
/// the same thing that button does: the slot. Resolving `ix` as "the ix-th Manual-kind strategy in
/// snapshot order" instead — as this did before slots were assignable — makes the hotkey and the
/// button with the same number fire DIFFERENT strategies the moment a trader assigns a slot by
/// hand, which places a real order on the wrong strategy with no visible cause.
///
/// Args:
///     b: Backend used to read the slot, resolve the strategy, and send the settings edit.
///     core: Core whose manual strategy should be selected.
///     ix: Zero-based quick-select slot.
///
/// Returns:
///     `true` when that slot names a strategy this core actually has; `false` to let the hotkey
///     propagate otherwise.
pub(crate) fn select_manual_strategy(b: &mut Backend, core: CoreId, ix: usize) -> bool {
    let slot_strategy = b
        .strat_slots(core)
        .and_then(|slots| slots.get(ix).map(|slot| slot.strategy.trim().to_string()))
        .filter(|strategy| !strategy.is_empty());
    let sid = match slot_strategy {
        Some(strategy) => b
            .session
            .store()
            .core(core)
            .and_then(|cd| manual_strategy_id(&cd.strategies, &strategy)),
        // No slot to go by — the core has assigned none and this terminal owns none either, which
        // is every core that has not reported its config yet. Fall back to the ordinal reading
        // this used before slots existed, so the hotkeys keep working on an unconfigured core;
        // once slots exist they are authoritative, and an empty slot fires nothing on purpose.
        None if !b.core_owns_strat_trade_slots(core) => {
            b.session.store().core(core).and_then(|cd| {
                cd.strategies
                    .iter()
                    .filter(|s| s.kind_ordinal == MANUAL_STRATEGY_KIND)
                    .nth(ix)
                    .map(|s| s.id)
            })
        }
        None => None,
    };
    match sid {
        Some(sid) => {
            set_manual(b, core, true, sid);
            true
        }
        None => false,
    }
}

/// Store this core's manual-strategy mode. Nothing leaves the terminal.
///
/// Args:
///     b: Backend whose per-core configuration is updated.
///     core: Target core.
///     on: Whether manual-strategy mode should be enabled.
///     id: Selected strategy id, retained by name even when the mode is disabled.
fn set_manual(b: &mut Backend, core: CoreId, on: bool, id: u64) {
    // The id actually STORED, which is not always the one clicked: an id the snapshot cannot name
    // keeps the previous selection rather than clearing it, and the exits must be seeded from what
    // the order will really use.
    let id = b.set_manual_strat(core, on, id);
    // Selecting a strategy SEEDS the visible take profit and stop from it, once — the helper keeps
    // the per-strategy overlay it already has. From then on the screen is authoritative (the order
    // carries what the trader sees, not what the strategy holds), but starting from the strategy's
    // own values is what makes the first order after a selection do what the strategy says it will.
    b.seed_exit_from_strategy(core, id);
}

/// Resolve the Moonbot-style `Buy +0.00% · Sell +0.50% · SL ON · TS OFF` parameter summary.
///
/// Values absent from the strategy snapshot fall back to defaults from its schema kind. Missing
/// Buy or Sell fields fall back to zero; SL and TS are always included.
///
/// Args:
///     row: Selected strategy snapshot.
///     schema: Optional schema providing defaults omitted from the snapshot.
///
/// Returns:
///     The hook the strategy defers to, or its four Buy, Sell, SL and TS read-outs.
fn strat_summary(row: &StrategyRow, schema: Option<&StrategySchemaModel>) -> StratSummary {
    let field = |name: &str| crate::backend::strat_field_value(row, schema, name);
    // A strategy that hands its exits to a MoonHook has none of its own to show: the core
    // substitutes the hook at order time and takes both the sell price and the stop from it, so
    // printing this strategy's Sell and SL here would name numbers no order will ever use. Say
    // which hook instead — that is the thing whose values apply.
    if let Some(hook) = field(crate::backend::FIELD_USE_HOOK_STRATEGY).map(|v| v.trim().to_string())
        && !hook.is_empty()
    {
        return StratSummary::Hook(hook);
    }
    // Always show Buy and Sell, as Moonbot does. A default-valued field may be absent from both the
    // snapshot and some schema sections; zero then means the current price, matching the core's
    // `signal price +0%` diagnostic.
    let buy = field("BuyPrice").unwrap_or_else(|| "0".to_string());
    let sell = field("SellPrice").unwrap_or_else(|| "0".to_string());
    StratSummary::Params {
        buy: fmt_pct(&buy),
        sell: fmt_pct(&sell),
        sl_on: is_yes(field("UseStopLoss")),
        ts_on: is_yes(field("UseTrailing")),
    }
}

/// Format a numeric percentage field with an explicit non-zero sign (`"0.5"` -> `"+0.50%"`).
///
/// A value rounding to zero prints `"0.00%"`. Non-numeric or non-finite input is returned unchanged.
fn fmt_pct(v: &str) -> String {
    let raw = v.trim();
    match raw.parse::<f64>() {
        Ok(f) => moon_core::util::fmt::signed_pct(f, 2)
            .map(|(text, _)| text)
            .unwrap_or_else(|| v.to_string()),
        Err(_) => v.to_string(),
    }
}

/// Whether a `Yes` or `No` field is on: only a case-insensitive `Yes`; missing or other is off.
fn is_yes(v: Option<String>) -> bool {
    v.is_some_and(|s| s.trim().eq_ignore_ascii_case("yes"))
}

/// The read-out text of an on/off flag.
fn on_off(on: bool) -> &'static str {
    if on { "ON" } else { "OFF" }
}
