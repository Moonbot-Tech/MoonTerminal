//! The crowd's rule: its lines, how long its cards stay, and the controls that set them.
//!
//! Split out of the screen because it is not one — the rule draws nothing on an empty Main. It is
//! configured from that screen's popup because that is where the crowd's other switches are, and
//! everything else about it belongs together, here.
//!
//! **A zero switches a line off**, on either of the two: the money line at zero asks about the
//! number of trades whatever the money did, the count at zero asks about the money whatever the
//! count was, and both at zero ask nothing — at which point nothing is read at all, not even a
//! socket. **The money line may be negative** and is read as written: `> -500` is "not worse than
//! five hundred down", which widens the rule rather than inverting it.
//!
//! **What is set here reaches a card when it fires, and never after.** A card freezes its own
//! lifetime exactly as a core's detection freezes its `KeepAlert`: shortening the setting does not
//! cut short what is already on screen, and lengthening it does not extend it. Otherwise a card
//! would state one countdown and obey another.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext, Context, Entity, IntoElement, ParentElement, SharedString, Styled,
    Window, div, rgb,
};
use moon_core::config::layout::{EmptyPlaces, WindowLayout};
use moon_core::crowd::CrowdRule;
use moon_core::crowd::detect::{DEFAULT_PROFIT, DEFAULT_TRADES, SEATS};
use moon_ui::{
    MoonCheckbox, MoonCheckboxSize, MoonInput, MoonInputEvent, MoonInputState, MoonPalette, h_flex,
};
use rust_i18n::t;

use crate::chart_tabs::MainChartStack;
use crate::design;

/// Width of one field, in design units. Sized for six digits and a decimal point.
const FIELD_WIDTH: f32 = 84.0;
/// Gap between a caption and its field, in design units.
const FIELD_GAP: f32 = 8.0;

/// Largest profit line accepted, in dollars, either side of zero.
///
/// A line nothing can ever cross is a rule that is silently off, and a reader who typed one extra
/// zero is better served by the biggest line that still means something than by a field that
/// quietly did nothing.
const PROFIT_MAX: f64 = 1e9;
/// Largest trade-count line accepted. One trade is a person rather than a crowd, but it is a rule
/// somebody may genuinely want, so anything from one upwards is allowed.
const TRADES_MAX: u32 = 100_000;

/// How long a card stays by default, in seconds — the width of the window it was computed from.
const KEEP_DEFAULT_S: u32 = 60;
/// The longest a card may be kept, in seconds.
///
/// Five minutes. Past that a card is describing a minute that ended long ago while occupying one of
/// [`SEATS`] seats the whole time, so a longer setting would quietly turn the feed into a list of
/// history.
const KEEP_MAX_S: u32 = 300;
/// The shortest, in seconds. A card has to survive being noticed and clicked; anything less is a
/// flicker rather than a setting.
const KEEP_MIN_S: u32 = 5;

/// Whether a profile that has never opened this popup is watching the rule.
///
/// Named rather than read out of the switch table by position: the rule's default is asked for from
/// two places, and a switch appended to that table would otherwise silently redefine it.
pub(super) const DETECT_DEFAULT: bool = false;

/// Whether a fresh crossing takes the seat of the oldest card when every seat is full.
///
/// On by default: the feed answers "what is happening now", and holding the oldest five while the
/// market moves on answers something else.
const EVICT_DEFAULT: bool = true;

/// The rule this RUN watches.
///
/// A bench (`--fixture`) and FireTest both assert that nothing reaches the network and both feed
/// the seeded stand-in, so a profile that had switched the rule on would drop invented crowd cards
/// into a panel measuring something else. Applied at the READER: applied at one caller instead, a
/// click on any checkbox re-armed what the run had refused to arm.
///
/// Args:
///     layout: Persisted window layout.
pub(crate) fn crowd_rule_for_run(layout: &WindowLayout) -> CrowdRule {
    if moon_core::fixture::active().is_some() || crate::firetest::scripted() {
        return CrowdRule::default();
    }
    crowd_rule(layout)
}

/// The rule as this profile has saved it, held to what a rule can be.
///
/// The one reader of those keys, so the switch that turns the rule on and the service that watches
/// the market cannot end up disagreeing about what is configured. An absent key means "never
/// chosen" and takes the rule's own default, exactly as every switch on this screen does.
///
/// Args:
///     layout: Persisted window layout.
pub(crate) fn crowd_rule(layout: &WindowLayout) -> CrowdRule {
    sane(CrowdRule {
        enabled: layout.main_empty_detect.unwrap_or(DETECT_DEFAULT),
        profit: layout.main_empty_detect_profit.unwrap_or(DEFAULT_PROFIT),
        trades: layout.main_empty_detect_trades.unwrap_or(DEFAULT_TRADES),
    })
}

/// Hold a rule to what a rule can be.
///
/// The file is a text file a person can edit, and the popup is not the only way a value reaches
/// this: a hand-written `nan` would compare unequal to itself, so the service would be handed a
/// "changed" rule on every frame and would re-seed itself into permanent silence. So the read path
/// holds the same lines the typed path does, and both come through here.
///
/// A zero on either line is NOT clamped away: zero is what switches that line off, and rounding it
/// up to one would silently switch it back on.
///
/// Args:
///     rule: What was read or typed.
fn sane(rule: CrowdRule) -> CrowdRule {
    CrowdRule {
        enabled: rule.enabled,
        profit: if rule.profit.is_finite() {
            let profit = rule.profit.clamp(-PROFIT_MAX, PROFIT_MAX);
            // A negative zero would persist as `-0`, print as `-0`, and — since zero is what
            // switches this line off — leave the rule looking switched on while behaving as though
            // it were not. Compared rather than `max`ed, which is documented as free to return
            // either input for two zeros.
            if profit == 0.0 { 0.0 } else { profit }
        } else {
            DEFAULT_PROFIT
        },
        trades: rule.trades.min(TRADES_MAX),
    }
}

/// How a card behaves once the rule has fired it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CrowdCards {
    /// How long it stays, in seconds.
    pub(crate) keep_secs: u32,
    /// Whether a fresh crossing may take the seat of the oldest card when every seat is full. With
    /// this off the newcomer is dropped instead, and what is on screen lives out its time.
    pub(crate) evict: bool,
}

impl CrowdCards {
    /// The same lifetime in milliseconds, which is what a card's countdown is measured in.
    pub(crate) fn keep_ms(&self) -> f64 {
        f64::from(self.keep_secs) * 1000.0
    }
}

/// How this profile has asked the crowd's cards to behave.
///
/// Args:
///     layout: Persisted window layout.
pub(crate) fn crowd_cards(layout: &WindowLayout) -> CrowdCards {
    CrowdCards {
        keep_secs: layout
            .main_empty_detect_keep
            .unwrap_or(KEEP_DEFAULT_S)
            .clamp(KEEP_MIN_S, KEEP_MAX_S),
        evict: layout.main_empty_detect_evict.unwrap_or(EVICT_DEFAULT),
    }
}

/// Whether this text could still become a decimal number.
///
/// Refused at the keystroke rather than at the commit: a comma, a space or a letter parses as
/// nothing, and a field that accepted them would keep the old line while showing a number the
/// reader believes they set. Partial entries — "", "-", "12", "12." — pass, because that is what
/// typing looks like, and what the commit does with a blank or a bare "-" is keep the value in
/// force. Such a field goes on showing what was typed until the popup is next opened, at which
/// point it reads back as the value that actually took effect — the same answer the chart-layout
/// fields give.
fn decimal(text: &str, _: &mut Context<MoonInputState>) -> bool {
    // A leading minus, and only leading: the money line may be negative, and `> -500` reads as
    // "not worse than five hundred down".
    let body = text.strip_prefix('-').unwrap_or(text);
    body.chars()
        .all(|glyph| glyph.is_ascii_digit() || glyph == '.')
        && body.matches('.').count() <= 1
}

/// Whether this text could still become a whole number.
fn whole(text: &str, _: &mut Context<MoonInputState>) -> bool {
    text.chars().all(|glyph| glyph.is_ascii_digit())
}

/// A number as it is written into its field.
///
/// The shortest text that reads back as the SAME value: whole dollars where the figure is whole,
/// which is what a line nearly always is, and the hundredths only for somebody who typed them. A
/// fixed two decimals would round the value on its way to the screen, and the popup writes back
/// what it shows — so merely opening and closing it would edit the setting.
pub(super) fn field_text(value: f64) -> String {
    format!("{value}")
}

/// What each field was last SEEDED with.
///
/// A field still showing what it was given is not an edit, and must not be written back. Two group
/// windows are two of these popups: without this, closing the one nobody touched would write its
/// stale text over the settings the other had just saved.
#[derive(Default)]
struct Seeds {
    profit: String,
    trades: String,
    keep: String,
}

/// The rule's fields, built the first time the popup is opened.
///
/// Built late because a `MoonInputState` needs a `Window` and this stack is constructed without one;
/// and built ONCE because the subscription that commits them is attached at the same time.
#[derive(Clone)]
pub(crate) struct DetectInputs {
    profit: Entity<MoonInputState>,
    trades: Entity<MoonInputState>,
    keep: Entity<MoonInputState>,
    seeded: Rc<RefCell<Seeds>>,
}

impl MainChartStack {
    /// Hand the saved rule to the service.
    ///
    /// Called from the places that can CHANGE it — the checkbox, and the fields when their edit is
    /// finished — and from nowhere else. Publishing from `render` unconditionally, which is where
    /// this began, put a call that starts and stops operating-system threads on the frame path for
    /// a value only a click can move. One render path still reaches it: the frame on which a chart
    /// covers an open popup commits that popup, which is a way of finishing an edit like any other,
    /// and it happens once rather than per frame.
    ///
    /// Args:
    ///     cx: Stack context used to read the layout and reach the service.
    pub(super) fn publish_crowd_rule(&mut self, cx: &mut Context<Self>) {
        let rule = crowd_rule_for_run(&self.backend.read(cx).layout);
        let service = self.backend.read(cx).crowd();
        service.update(cx, |service, service_cx| service.set_rule(rule, service_cx));
    }

    /// Write the eviction choice.
    ///
    /// It reaches a card when that card FIRES, so this changes what happens to the next crossing
    /// rather than to what is already on screen.
    ///
    /// Args:
    ///     evict: What the checkbox now says.
    ///     cx: Stack context used to persist and repaint.
    pub(super) fn set_crowd_evict(&mut self, evict: bool, cx: &mut Context<Self>) {
        if crowd_cards(&self.backend.read(cx).layout).evict == evict {
            return;
        }
        self.backend.update(cx, |backend, _| {
            backend.layout.main_empty_detect_evict = Some(evict);
            backend.layout_dirty = true;
        });
        cx.notify();
    }

    /// Build the popup's window-bound controls on first use, and fill them with the settings as
    /// they stand.
    ///
    /// The rule's fields and the blocks' position dropdowns are built together because they need
    /// the same thing and only the popup has it: a `MoonInputState` and a `MoonSelectState` cannot
    /// exist without a `Window`, and this stack is constructed without one.
    ///
    /// Args:
    ///     window: The window the controls belong to.
    ///     cx: Stack context used to create, subscribe and seed.
    pub(super) fn seed_empty_detect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let places = EmptyPlaces::restore(&self.backend.read(cx).layout);
        if let Some(selects) = &self.empty_places {
            selects.show(places, cx);
        } else {
            self.empty_places = Some(super::arrange::PlaceSelects::seed(places, window, cx));
        }
        let inputs = match &self.empty_detect {
            Some(inputs) => inputs.clone(),
            None => {
                // The guard goes on the STATE. `MoonInput::validate` is applied only to a state the
                // element builds for itself (`moon-ui-components/src/moon/input.rs:202`), and these
                // are built here so they can be seeded and subscribed to — so a validator written
                // on the element would be silently dropped.
                let profit = cx.new(|cx| MoonInputState::new(window, cx).validate(decimal));
                let trades = cx.new(|cx| MoonInputState::new(window, cx).validate(whole));
                let keep = cx.new(|cx| MoonInputState::new(window, cx).validate(whole));
                for input in [&profit, &trades, &keep] {
                    cx.subscribe(input, |this, _input, event: &MoonInputEvent, cx| {
                        if matches!(
                            event,
                            MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }
                        ) {
                            this.commit_empty_detect(cx);
                        }
                    })
                    .detach();
                }
                let inputs = DetectInputs {
                    profit,
                    trades,
                    keep,
                    seeded: Rc::new(RefCell::new(Seeds::default())),
                };
                self.empty_detect = Some(inputs.clone());
                inputs
            }
        };
        // Seeded from what is SAVED every time the popup opens, so a value that was refused —
        // blank, or not a number — is replaced by the one actually in force rather than left on
        // screen looking configured.
        let layout = &self.backend.read(cx).layout;
        let rule = crowd_rule(layout);
        let cards = crowd_cards(layout);
        let seeds = Seeds {
            profit: field_text(rule.profit),
            trades: rule.trades.to_string(),
            keep: cards.keep_secs.to_string(),
        };
        let (profit, trades, keep) = (
            seeds.profit.clone(),
            seeds.trades.clone(),
            seeds.keep.clone(),
        );
        *inputs.seeded.borrow_mut() = seeds;
        inputs.profit.update(cx, |input, input_cx| {
            input.set_value(profit, window, input_cx)
        });
        inputs.trades.update(cx, |input, input_cx| {
            input.set_value(trades, window, input_cx)
        });
        inputs.keep.update(cx, |input, input_cx| {
            input.set_value(keep, window, input_cx)
        });
    }

    /// Take what the fields say, and keep what is in force where they say nothing.
    ///
    /// An unreadable field keeps the value already in force rather than resolving to a zero: on the
    /// two lines a zero means "not watched", which is the loudest possible reading of "I typed
    /// something you did not understand".
    ///
    /// Args:
    ///     cx: Stack context used to read the fields and persist the result.
    pub(super) fn commit_empty_detect(&mut self, cx: &mut Context<Self>) {
        let Some(inputs) = self.empty_detect.clone() else {
            return;
        };
        let (rule, cards) = {
            let layout = &self.backend.read(cx).layout;
            (crowd_rule(layout), crowd_cards(layout))
        };
        let seeded = inputs.seeded.borrow();
        // A field still showing what it was seeded with is not an edit. Taking it anyway would let
        // a second window's untouched popup write its stale text over what this one just saved.
        let profit = edited(&inputs.profit, &seeded.profit, cx)
            .and_then(|text| text.trim().parse::<f64>().ok())
            .filter(|value| value.is_finite())
            .map_or(rule.profit, |value| value.clamp(-PROFIT_MAX, PROFIT_MAX));
        // Parsed WIDE and then held to the range, so an over-large count is the biggest rule that
        // still means something rather than a number the field quietly ignored.
        let trades = edited(&inputs.trades, &seeded.trades, cx)
            .and_then(|text| text.trim().parse::<u64>().ok())
            .map_or(rule.trades, |value| value.min(u64::from(TRADES_MAX)) as u32);
        let keep = edited(&inputs.keep, &seeded.keep, cx)
            .and_then(|text| text.trim().parse::<u64>().ok())
            .map_or(cards.keep_secs, |value| {
                value.clamp(u64::from(KEEP_MIN_S), u64::from(KEEP_MAX_S)) as u32
            });
        drop(seeded);
        if profit == rule.profit && trades == rule.trades && keep == cards.keep_secs {
            return;
        }
        self.backend.update(cx, |backend, _| {
            backend.layout.main_empty_detect_profit = Some(profit);
            backend.layout.main_empty_detect_trades = Some(trades);
            backend.layout.main_empty_detect_keep = Some(keep);
            backend.layout_dirty = true;
        });
        self.publish_crowd_rule(cx);
        cx.notify();
    }
}

/// One field's text, but only when it differs from what that field was seeded with.
///
/// Args:
///     input: The field.
///     seed: What it was filled with when the popup opened.
///     cx: Application context.
///
/// Returns:
///     The typed text, or `None` when nothing was typed into it.
fn edited(input: &Entity<MoonInputState>, seed: &str, cx: &App) -> Option<String> {
    let typed = input.read(cx).value().to_string();
    (typed != seed).then_some(typed)
}

/// The rule's own controls: its two lines, how long a card stays, and what happens at the last seat.
///
/// Dead while the rule is off — the fields grey out rather than disappearing, so the popup does not
/// change height under the cursor and the reader can see what they would be configuring.
///
/// Args:
///     enabled: Whether the rule is being watched at all.
///     cards: How the cards are set to behave.
///     inputs: The fields, once the popup has been opened at least once.
///     view: Stack entity receiving the edits.
///     id: Group-scoped element identities.
///     palette: Active MoonUI palette.
///     cx: Application context supplying scaled geometry.
pub(super) fn block(
    enabled: bool,
    cards: CrowdCards,
    inputs: &DetectInputs,
    view: Entity<MainChartStack>,
    id: &dyn Fn(&str) -> SharedString,
    palette: MoonPalette,
    cx: &App,
) -> AnyElement {
    moon_ui::v_flex()
        .w_full()
        .gap(design::ui_px(cx, FIELD_GAP))
        .child(field_row(
            id("detect-profit"),
            t!("crowd.settings.detect_profit").to_string(),
            &inputs.profit,
            enabled,
            palette,
            cx,
        ))
        .child(field_row(
            id("detect-trades"),
            t!("crowd.settings.detect_trades").to_string(),
            &inputs.trades,
            enabled,
            palette,
            cx,
        ))
        .child(field_row(
            id("detect-keep"),
            t!("crowd.settings.detect_keep").to_string(),
            &inputs.keep,
            enabled,
            palette,
            cx,
        ))
        .child(
            MoonCheckbox::new(id("detect-evict"))
                .label(t!("crowd.settings.detect_evict", max = SEATS.to_string()).to_string())
                .checked(cards.evict)
                .disabled(!enabled)
                .size(MoonCheckboxSize::Compact)
                .on_change(move |checked: &bool, _window, app| {
                    let checked = *checked;
                    view.update(app, |this, cx| this.set_crowd_evict(checked, cx));
                }),
        )
        // A zero does not mean "a line at zero", and nothing on the rows above could say so: they
        // are a caption and a number.
        .child(
            div()
                .w_full()
                .text_size(design::t_caption(cx))
                .text_color(rgb(palette.text_muted))
                .child(t!("crowd.settings.detect_hint").to_string()),
        )
        .into_any_element()
}

/// One setting: what it is, and the field that sets it.
///
/// Args:
///     id: Element identity, already group-scoped.
///     label: What this line means.
///     input: The field's state.
///     enabled: Whether the rule is being watched at all.
///     palette: Active MoonUI palette.
///     cx: Application context supplying scaled geometry.
fn field_row(
    id: SharedString,
    label: String,
    input: &Entity<MoonInputState>,
    enabled: bool,
    palette: MoonPalette,
    cx: &App,
) -> AnyElement {
    h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, FIELD_GAP))
        .child(
            div()
                .flex_1()
                .text_size(design::t_body(cx))
                .text_color(rgb(if enabled {
                    palette.text
                } else {
                    palette.text_muted
                }))
                .child(label),
        )
        .child(
            div().w(design::ui_px(cx, FIELD_WIDTH)).child(
                MoonInput::new(id)
                    .state(input)
                    .small()
                    .mono(true)
                    .disabled(!enabled),
            ),
        )
        .into_any_element()
}
