//! The Core Status alert configuration popover: the gear beside the mode control. One row per warning
//! axis — name, enable, "show on chart", alert sound, then the axis's thresholds — with each control's
//! caption above its value. Split out of `mod.rs` because it is a self-contained control.
//!
//! Enable lives in `WarnAxesCfg` (kept as bare bools for config back-compat); chart visibility, sound,
//! and thresholds live in `WarnParams`. Both persist through the backend; a change bumps the engine so
//! charts rebuild their badges.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonDropdown,
    MoonMenuSize, MoonPalette, MoonPopover, MoonPopoverPlacement, h_flex, v_flex,
};
use rust_i18n::t;

use super::CoreStatusView;
use crate::Backend;
use crate::design;
use crate::panels::common::{RadioMark, popup_close_button, popup_title, radio_items};
use moon_core::config::layout::{WarnAxesCfg, WarnParams};

// Design-reference column widths, at UI scale 1.0 and Font-slider delta 0, so every control lines
// up in a table down the rows. The widest row (a latency axis: 4 params) sets the total; shorter
// rows pad empty param slots so their sound column still lines up. Gap between columns is
// `COL_GAP`. Nothing here is a rendered pixel — every one of them goes through
// [`WarnCfgMetrics`], which is the only place a scale is applied.
const NAME_W: f32 = 138.0;
const ON_W: f32 = 30.0;
const CHART_W: f32 = 34.0;
const PARAM_W: f32 = 66.0;
const SOUND_W: f32 = 128.0;
const COL_GAP: f32 = 6.0;
/// Design-reference caption-row and control-row heights, so every column's caption and control line
/// up on the same two baselines down the whole table (the checkboxes, steppers and the sound row all
/// centre in the control band).
const CAP_H: f32 = 12.0;
const CTRL_H: f32 = 28.0;
/// The most threshold columns any row has (the latency axes: yellow, red, window, hold).
const MAX_PARAMS: usize = 4;
/// One ROW: the eight columns (name, on, chart, four params, sound) plus the seven gaps between
/// them. Derived from the column constants, never restated as a literal — the trailing controls
/// ended up outside the box the last time two numbers had to agree and only one of them changed.
const WARN_CFG_ROW_W: f32 =
    NAME_W + ON_W + CHART_W + MAX_PARAMS as f32 * PARAM_W + SOUND_W + 7.0 * COL_GAP;
/// Slack over the row width.
///
/// The sound column's `MoonDropdown` resolves its trigger and menu as `(width * text_scale).max(
/// minimum readable)`, so its real width can exceed what was asked for on a long localized label.
/// This is the margin that keeps the ▶ button inside the box when it does.
const ROW_SLACK: f32 = 16.0;
/// Ratio between a rendered font size and the line box GPUI lays it out in.
///
/// Raw GPUI text with no explicit line height gets its font's own, a little over 1.25x the size.
/// A fixed band shorter than that clips the glyphs rather than overflowing visibly, which is why
/// the two band heights below take it as a floor.
const LINE_BOX: f32 = 1.3;
/// MoonUI's own HEIGHT metrics for a `MoonButtonSize::Action` control — the size the sound column's
/// `MoonDropdown` trigger runs at, and the TALLEST thing standing in the control band.
///
/// Copied from `MoonButtonMetrics::base_for_size(Size::Small)`, which is what a `MoonButton` of that
/// size actually resolves through: `height: 26`, `line_height: 14`, and a vertical pad its `scaled()`
/// derives as `(height - line_height) * 0.5`. Do NOT take these from `button_text_metrics(Action)`
/// instead — that returns `(font_size, line_height, GAP)` for the label, whose third field is a
/// HORIZONTAL icon gap, and its `(26, 16, 5)` only happens to give the same answer because
/// `16 + 2*5 == 14 + 2*6`; it would stop agreeing the moment MoonUI retunes either function.
///
/// The height resolves as `fit_height(base, line, pad) = max(ui(base), line_height(line) +
/// 2 * ui(pad))`, and `line_height` is ADDITIVE in the Font-slider delta — so the trigger is 26px at
/// delta 0, 29px at the shipped +3 and 32px at +6, past `CTRL_H` exactly where the goal says nothing
/// may clip. Deriving the floor from MoonUI's own contract rather than from a generic text ratio is
/// what makes the band track the CONTROL instead of merely the text beside it.
const ACTION_H: f32 = 26.0;
const ACTION_LINE_H: f32 = 14.0;
const ACTION_PAD_Y: f32 = (ACTION_H - ACTION_LINE_H) * 0.5;

/// Every rendered dimension of the alert table, resolved once from the active scales.
///
/// The popup mixes two scales that MoonUI moves independently: its captions and values are raw
/// GPUI text on the FONT scale (`design::t_caption` / `design::t_body`, which grow with the Font
/// slider AND its `ui_font_delta`, +3 at the shipped default), while its `MoonCheckbox`,
/// `MoonButton` steppers and the ▶ box are MoonUI widgets on the UI scale. A column sized on one
/// of the two is wrong whenever the other is larger: on the UI scale the text outgrew its column
/// at the stock config, and on the font scale a widened UI would push the widgets out.
///
/// So every HORIZONTAL length takes `max(ui, font_width)` — wide enough for whichever occupant is
/// bigger — and the popover is told the result in ALREADY-RENDERED pixels
/// (`MoonPopover::content_width`) rather than through `content_width_ui` or `content_width_font`,
/// because neither single-scale policy can bound a row that holds both.
///
/// [`Self::resolve`] is pure and takes no context, so the geometry is testable without a window.
#[derive(Clone, Copy, Debug, PartialEq)]
struct WarnCfgMetrics {
    /// The one scale every horizontal design-reference length is multiplied by.
    w_scale: f32,
    /// Rendered height of the caption band.
    cap_h: f32,
    /// Rendered height of the control band.
    ctrl_h: f32,
}

impl WarnCfgMetrics {
    /// Resolve the table's geometry for one pair of active scales.
    ///
    /// Args:
    ///     ui: The UI geometry scale (`MoonThemeTokens::ui(1.0)`).
    ///     font_w: The font WIDTH scale (`MoonThemeTokens::font_width_scale`), which is the pure
    ///         multiply `MoonPopover` itself uses — not the ADDITIVE `font()` used for text sizes.
    ///     cap_px: The rendered caption font size.
    ///     body_px: The rendered body font size.
    ///     action_h: The rendered height of a `MoonButtonSize::Action` control, from MoonUI's own
    ///         fit-height rule — see [`ACTION_H`]. The control band can never be shorter than the
    ///         tallest control standing in it.
    ///
    /// Returns:
    ///     The resolved metrics.
    fn resolve(ui: f32, font_w: f32, cap_px: f32, body_px: f32, action_h: f32) -> Self {
        Self {
            w_scale: ui.max(font_w).max(0.25),
            cap_h: (CAP_H * ui).max(cap_px * LINE_BOX),
            ctrl_h: (CTRL_H * ui).max(body_px * LINE_BOX).max(action_h),
        }
    }

    /// Scale one design-reference horizontal length into rendered pixels.
    fn w(self, base: f32) -> Pixels {
        px(base * self.w_scale)
    }

    /// The rendered width one row occupies: its eight columns and the seven gaps between them.
    fn row_w(self) -> f32 {
        WARN_CFG_ROW_W * self.w_scale
    }

    /// The rendered CONTENT width the popover is told to paint: the row plus [`ROW_SLACK`].
    ///
    /// Built ON TOP of [`Self::row_w`] rather than beside it, so the box and the row it has to
    /// hold cannot drift apart — `MoonPopover` adds its own padding and border around this, and the
    /// content root pads itself with nothing.
    fn content_w(self) -> f32 {
        self.row_w() + ROW_SLACK * self.w_scale
    }
}

/// Read the active scales and resolve [`WarnCfgMetrics`].
///
/// Args:
///     cx: Application context used to read active theme tokens.
///
/// Returns:
///     The table geometry for the current theme, font slider and UI scale.
fn warn_cfg_metrics(cx: &App) -> WarnCfgMetrics {
    WarnCfgMetrics::resolve(
        design::ui_value(cx, 1.0),
        design::font_scale(cx),
        f32::from(design::t_caption(cx)),
        f32::from(design::t_body(cx)),
        design::fit_h_value(cx, ACTION_H, ACTION_LINE_H, ACTION_PAD_Y),
    )
}

/// One row's "show on chart" state and its writer, or `None` for an axis that cannot be drawn on a
/// chart at all — which leaves that column EMPTY rather than showing a checkbox whose state would
/// mean nothing.
type ChartToggle = Option<(bool, fn(&mut WarnParams, bool))>;

/// Unit a threshold value is shown in.
#[derive(Clone, Copy)]
enum Unit {
    /// Percent, e.g. `70%`.
    Pct,
    /// Percent above a baseline, e.g. `+12%`.
    PctRel,
    /// A multiple of the baseline, e.g. `×2`.
    Mult,
    /// Seconds.
    Sec,
    /// Days.
    Day,
}

/// One threshold control on a warning row: its caption, current value, bounds, and how to read/write
/// it on `WarnParams`. Getters/setters are non-capturing `fn`s (Copy), so the row builder stays generic.
struct Param {
    /// Localized caption shown above the value.
    cap: SharedString,
    /// Read the current value from the params.
    get: fn(&WarnParams) -> u16,
    /// Write a new value into the params.
    set: fn(&mut WarnParams, u16),
    /// Inclusive bounds and step for the ± steppers.
    min: u16,
    max: u16,
    step: u16,
    /// How the value is rendered.
    unit: Unit,
}

impl CoreStatusView {
    /// The alert popover: a gear beside the mode control, one row per warning axis. Unchecking "enable"
    /// stops the backend recording that axis AND hides its history; "chart" toggles only the badge/line
    /// drawing; the thresholds retune detection live.
    pub(super) fn warn_gear(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let gear = MoonButton::new("core-status-warn-gear")
            .label("⚙")
            .size(MoonButtonSize::Micro)
            .variant(MoonButtonVariant::Ghost)
            .tooltip(t!("core_status.warn_cfg.title").to_string())
            .render();

        let mut popover = MoonPopover::new("core-status-warn-popover")
            // Open BELOW the gear, right edge aligned to it so the wide panel grows left into the
            // strip rather than off the right edge of the window. Downward is what the gear's own
            // position asks for: it sits on the tab strip at the TOP of the panel, so a popup above
            // it covered the chrome and read as detached from the control that opened it.
            .placement(MoonPopoverPlacement::BottomEnd)
            // Already-rendered pixels, resolved from the same metrics the rows lay themselves out
            // with — see `WarnCfgMetrics` for why neither `content_width_ui` nor
            // `content_width_font` can bound this row on its own.
            .content_width(warn_cfg_metrics(cx).content_w())
            .close_on_content_click(false)
            // A `MoonDropdown` menu inside this popover paints in its OWN deferred layer, outside
            // this popover's box, and `on_mouse_down_out` is bounds-based and runs in the CAPTURE
            // phase — so the click that picks an item reads as "outside" and would shut the popover
            // before the pick landed. Until MoonUI suppresses that (see the Popover entry in
            // docs-internal/FORK_BUGS.md), outside-click dismissal has to be off here; the ✕ and the
            // gear are the dismissal paths.
            .overlay_closable(false)
            .open(self.warn_cfg_open)
            .on_open_change(move |open, _window, app| {
                view.update(app, |this, cx| {
                    this.warn_cfg_open = open;
                    cx.notify();
                });
            })
            .trigger(gear);
        // Built ONLY while open. `MoonPopover` takes its content eagerly, so a shut popover would
        // otherwise rebuild six warning rows — their steppers, sound dropdowns and locale lookups —
        // on every render of a panel that repaints on the ping cadence, and throw the tree away.
        if self.warn_cfg_open {
            popover = popover.content(self.warn_cfg_content(cx));
        }
        popover
    }

    /// Builds the alert popover's body: the head row plus one row per warning axis.
    ///
    /// Split from [`Self::warn_gear`] so it is only called while the popover is open.
    ///
    /// Args:
    ///     cx: Core Status view context.
    ///
    /// Returns:
    ///     The popover content tree.
    fn warn_cfg_content(&self, cx: &Context<Self>) -> AnyElement {
        let axes = self.backend.read(cx).warn_axes();
        let params = self.backend.read(cx).warn_params();

        // Title row with an explicit close control, matching the other settings popups. It is not
        // decoration here: outside-click dismissal is off (see `warn_gear`), so the ✕ is what keeps
        // this popover closable.
        let head = h_flex()
            .w_full()
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .child(popup_title(
                t!("core_status.warn_cfg.title"),
                MoonPalette::active(cx),
                cx,
            ))
            .child(popup_close_button(
                "core-status-warn-close",
                cx.listener(|this, _, _w, cx| {
                    this.warn_cfg_open = false;
                    cx.notify();
                }),
            ));

        // Chrome is MoonPopover's; see `popover_contents_do_not_paint_a_second_surface`.
        v_flex()
            .id("core-status-warn-content")
            .w_full()
            .gap(design::ui_px(cx, 2.0))
            .child(head)
            .child(self.warn_row(
                "cpu",
                t!("core_status.warn_cfg.cpu").to_string(),
                true,
                axes.cpu,
                |a, on| a.cpu = on,
                Some((params.cpu.chart, |p, on| p.cpu.chart = on)),
                params.cpu.sound.clone(),
                |p, s| p.cpu.sound = s,
                vec![
                    Param {
                        cap: t!("core_status.warn_cfg.p.threshold").into(),
                        get: |p| u16::from(p.cpu.pct),
                        set: |p, v| p.cpu.pct = v as u8,
                        min: 40,
                        max: 100,
                        step: 5,
                        unit: Unit::Pct,
                    },
                    Param {
                        cap: t!("core_status.warn_cfg.p.hold").into(),
                        get: |p| u16::from(p.cpu.hold),
                        set: |p, v| p.cpu.hold = v as u8,
                        min: 1,
                        max: 30,
                        step: 1,
                        unit: Unit::Sec,
                    },
                ],
                cx,
            ))
            .child(self.warn_row(
                "mem",
                t!("core_status.warn_cfg.mem").to_string(),
                false,
                axes.mem,
                |a, on| a.mem = on,
                Some((params.mem.chart, |p, on| p.mem.chart = on)),
                params.mem.sound.clone(),
                |p, s| p.mem.sound = s,
                vec![
                    Param {
                        cap: t!("core_status.warn_cfg.p.growth").into(),
                        get: |p| u16::from(p.mem.pct),
                        set: |p, v| p.mem.pct = v as u8,
                        min: 2,
                        max: 50,
                        step: 1,
                        // A rise above the window minimum — a relative delta, like the ping bands.
                        unit: Unit::PctRel,
                    },
                    Param {
                        cap: t!("core_status.warn_cfg.p.window").into(),
                        get: |p| p.mem.window,
                        set: |p, v| p.mem.window = v,
                        min: 10,
                        max: 120,
                        step: 5,
                        unit: Unit::Sec,
                    },
                ],
                cx,
            ))
            .child(self.warn_row(
                "conn",
                t!("core_status.warn_cfg.conn").to_string(),
                false,
                axes.conn,
                |a, on| a.conn = on,
                Some((params.conn.chart, |p, on| p.conn.chart = on)),
                params.conn.sound.clone(),
                |p, s| p.conn.sound = s,
                vec![],
                cx,
            ))
            .child(self.warn_row(
                "ping",
                t!("core_status.warn_cfg.ping").to_string(),
                false,
                axes.ping,
                |a, on| a.ping = on,
                Some((params.ping.chart, |p, on| p.ping.chart = on)),
                params.ping.sound.clone(),
                |p, s| p.ping.sound = s,
                lat_params(
                    |p| u16::from(p.ping.yellow),
                    |p, v| p.ping.yellow = v as u8,
                    |p| u16::from(p.ping.red),
                    |p, v| p.ping.red = v as u8,
                    |p| p.ping.window,
                    |p, v| p.ping.window = v,
                    |p| u16::from(p.ping.hold),
                    |p, v| p.ping.hold = v as u8,
                ),
                cx,
            ))
            .child(self.warn_row(
                "exch",
                t!("core_status.warn_cfg.exch").to_string(),
                false,
                axes.exch,
                |a, on| a.exch = on,
                Some((params.exch.chart, |p, on| p.exch.chart = on)),
                params.exch.sound.clone(),
                |p, s| p.exch.sound = s,
                lat_params(
                    |p| u16::from(p.exch.yellow),
                    |p, v| p.exch.yellow = v as u8,
                    |p| u16::from(p.exch.red),
                    |p, v| p.exch.red = v as u8,
                    |p| p.exch.window,
                    |p, v| p.exch.window = v,
                    |p| u16::from(p.exch.hold),
                    |p, v| p.exch.hold = v as u8,
                ),
                cx,
            ))
            .child(self.warn_row(
                "api",
                t!("core_status.warn_cfg.api").to_string(),
                false,
                axes.api,
                |a, on| a.api = on,
                // No chart column: this axis has no per-second history to draw.
                None,
                params.api.sound.clone(),
                |p, s| p.api.sound = s,
                vec![Param {
                    cap: t!("core_status.warn_cfg.p.days").into(),
                    get: |p| p.api.days,
                    set: |p, v| p.api.days = v,
                    min: 0,
                    max: moon_core::config::layout::API_WARN_MAX_DAYS,
                    step: 1,
                    unit: Unit::Day,
                }],
                cx,
            ))
            .into_any_element()
    }

    /// One warning row laid out as fixed table columns: name · Вкл · Чарт · up to four thresholds ·
    /// sound. Each column is a fixed width, so the controls line up vertically down the rows; a row
    /// with fewer thresholds pads empty slots so its sound column still aligns.
    #[allow(clippy::too_many_arguments)]
    fn warn_row(
        &self,
        id: &'static str,
        name: String,
        first: bool,
        enabled: bool,
        set_enabled: fn(&mut WarnAxesCfg, bool),
        chart: ChartToggle,
        sound: Option<String>,
        set_sound: fn(&mut WarnParams, Option<String>),
        params: Vec<Param>,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let n_params = params.len();
        // Name column: an empty caption spacer keeps its control band level with the others, and the
        // name centres in that band (so it lines up with the checkboxes and steppers, not above them).
        let name_col = self.cell(
            NAME_W,
            div()
                .w_full()
                .truncate()
                .text_size(design::t_body(cx))
                .text_color(rgb(p.text))
                .child(name),
            String::new(),
            cx,
        );
        h_flex()
            .w_full()
            .items_start()
            .gap(warn_cfg_metrics(cx).w(COL_GAP))
            .py(design::ui_px(cx, 5.0))
            // Only a separator BETWEEN rows — the first row has no line above it.
            .when(!first, |el| el.border_t_1().border_color(rgb(p.border)))
            .child(name_col)
            .child(self.cell(
                ON_W,
                enable_checkbox(
                    format!("cs-warn-{id}-on"),
                    enabled,
                    &self.backend,
                    set_enabled,
                ),
                t!("core_status.warn_cfg.col.on").to_string(),
                cx,
            ))
            .child(
                self.cell(
                    CHART_W,
                    // The checkbox itself, NOT wrapped in a container: the cell already centres its
                    // control, and an extra flex box here would re-lay out the five rows that had this
                    // column before the option existed.
                    match chart {
                        Some((on, set_chart)) => chart_checkbox(
                            format!("cs-warn-{id}-chart"),
                            on,
                            &self.backend,
                            set_chart,
                        )
                        .into_any_element(),
                        None => div().into_any_element(),
                    },
                    t!("core_status.warn_cfg.col.chart").to_string(),
                    cx,
                ),
            )
            .children(params.into_iter().enumerate().map(|(i, param)| {
                let caption = caption_for(&param.cap, param.unit);
                self.cell(
                    PARAM_W,
                    self.stepper(format!("cs-warn-{id}-p{i}"), param, cx),
                    caption,
                    cx,
                )
            }))
            // Pad the missing threshold columns so the sound column lines up across every row.
            .children((n_params..MAX_PARAMS).map(|_| {
                div()
                    .w(warn_cfg_metrics(cx).w(PARAM_W))
                    .flex_none()
                    .into_any_element()
            }))
            .child(self.cell(
                SOUND_W,
                self.sound_cell(id, sound, set_sound, cx),
                t!("core_status.warn_cfg.col.sound").to_string(),
                cx,
            ))
    }

    /// A fixed-width column: a centred caption on a fixed-height row above its control, which centres
    /// in a fixed-height band. The two fixed heights make every caption and every control share the
    /// same two baselines down the whole table.
    fn cell(
        &self,
        width: f32,
        control: impl IntoElement,
        caption: String,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let m = warn_cfg_metrics(cx);
        v_flex()
            .w(m.w(width))
            .flex_none()
            .gap(design::ui_px(cx, 3.0))
            .child(
                div()
                    .h(px(m.cap_h))
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(p.text_muted))
                    .font_family(design::mono())
                    .whitespace_nowrap()
                    .child(caption),
            )
            .child(
                div()
                    .h(px(m.ctrl_h))
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(control),
            )
    }

    /// A `− value +` stepper writing one threshold through the backend on each click.
    fn stepper(&self, id: String, param: Param, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let Param {
            get,
            set,
            min,
            max,
            step,
            unit,
            ..
        } = param;
        let cur = get(&self.backend.read(cx).warn_params());
        let bump = |backend: &Entity<Backend>, delta: i32| {
            let backend = backend.clone();
            move |_: &ClickEvent, _: &mut Window, app: &mut App| {
                backend.update(app, |b, cx| {
                    let mut pr = b.warn_params();
                    let v = get(&pr);
                    let next = if delta < 0 {
                        v.saturating_sub(step).max(min)
                    } else {
                        v.saturating_add(step).min(max)
                    };
                    set(&mut pr, next);
                    b.set_warn_params(pr);
                    b.mark_backend_dirty(cx);
                });
            }
        };
        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .child(
                MoonButton::new(SharedString::from(format!("{id}-dec")))
                    .label("−")
                    .size(MoonButtonSize::Micro)
                    .variant(MoonButtonVariant::Ghost)
                    .on_click(bump(&self.backend, -1)),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .justify_center()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text))
                    .font_family(design::mono())
                    .child(fmt_value(cur, unit)),
            )
            .child(
                MoonButton::new(SharedString::from(format!("{id}-inc")))
                    .label("+")
                    .size(MoonButtonSize::Micro)
                    .variant(MoonButtonVariant::Ghost)
                    .on_click(bump(&self.backend, 1)),
            )
    }

    /// A sound dropdown (embedded stems + "none") with a preview ▶ button.
    fn sound_cell(
        &self,
        id: &'static str,
        sound: Option<String>,
        set_sound: fn(&mut WarnParams, Option<String>),
        cx: &Context<Self>,
    ) -> impl IntoElement {
        // Match the stored name to a `'static` stem so the dropdown value stays `Copy`.
        let cur: Option<&'static str> = sound
            .as_deref()
            .and_then(|s| crate::media::sound::names().find(|n| *n == s));
        let label = cur.map_or_else(
            || t!("core_status.warn_cfg.sound_none").to_string(),
            nice_sound,
        );
        let backend = self.backend.clone();
        let options = std::iter::once((
            None::<&'static str>,
            SharedString::from(format!("{id}-none")),
            SharedString::from(t!("core_status.warn_cfg.sound_none").to_string()),
        ))
        .chain(crate::media::sound::names().map(|stem| {
            (
                Some(stem),
                SharedString::from(format!("{id}-{stem}")),
                SharedString::from(nice_sound(stem)),
            )
        }));
        let items = radio_items(options, cur, RadioMark::Check, move |app, value| {
            let backend = backend.clone();
            backend.update(app, |b, cx| {
                let mut pr = b.warn_params();
                set_sound(&mut pr, value.map(String::from));
                b.set_warn_params(pr);
                b.mark_backend_dirty(cx);
            });
        });
        let play_name = cur;
        let p = MoonPalette::active(cx);
        let enabled = play_name.is_some();
        let m = warn_cfg_metrics(cx);
        h_flex()
            .w_full()
            .items_center()
            .gap(m.w(4.0))
            .child(
                MoonDropdown::new(SharedString::from(format!("cs-warn-{id}-snd")))
                    .label(label)
                    .trigger_caret(true)
                    .trigger_variant(MoonButtonVariant::Soft)
                    .trigger_size(MoonButtonSize::Action)
                    .trigger_width_scaled(94.0)
                    .menu_width_scaled(128.0)
                    .menu_size(MoonMenuSize::Compact)
                    .items(items),
            )
            // Square preview button: a fixed-size bordered box with the ▶ glyph centred.
            .child(
                div()
                    .id(SharedString::from(format!("cs-warn-{id}-play")))
                    // Square, and its side IS the control band, so it can never outgrow the row it
                    // sits in nor leave a gap under it.
                    .size(px(m.ctrl_h))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(design::ui_px(cx, 5.0))
                    .border_1()
                    .border_color(rgb(p.border))
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(if enabled { p.text_soft } else { p.text_dim }))
                    .when(enabled, |el| {
                        el.cursor_pointer()
                            .hover(|s| s.border_color(rgb(p.accent)).text_color(rgb(p.accent)))
                            .on_click(move |_, _, _| {
                                if let Some(name) = play_name {
                                    crate::media::sound::play(name);
                                }
                            })
                    })
                    .child("▶"),
            )
    }
}

/// Build the four latency-axis threshold controls (yellow ×, red × of baseline, baseline window,
/// sustain).
#[allow(clippy::too_many_arguments)]
fn lat_params(
    get_y: fn(&WarnParams) -> u16,
    set_y: fn(&mut WarnParams, u16),
    get_r: fn(&WarnParams) -> u16,
    set_r: fn(&mut WarnParams, u16),
    get_w: fn(&WarnParams) -> u16,
    set_w: fn(&mut WarnParams, u16),
    get_h: fn(&WarnParams) -> u16,
    set_h: fn(&mut WarnParams, u16),
) -> Vec<Param> {
    vec![
        Param {
            cap: t!("core_status.warn_cfg.p.yellow").into(),
            get: get_y,
            set: set_y,
            min: 2,
            max: 100,
            step: 1,
            unit: Unit::Mult,
        },
        Param {
            cap: t!("core_status.warn_cfg.p.red").into(),
            get: get_r,
            set: set_r,
            min: 2,
            max: 100,
            step: 1,
            unit: Unit::Mult,
        },
        Param {
            cap: t!("core_status.warn_cfg.p.window").into(),
            get: get_w,
            set: set_w,
            min: 15,
            max: 300,
            step: 5,
            unit: Unit::Sec,
        },
        Param {
            cap: t!("core_status.warn_cfg.p.hold").into(),
            get: get_h,
            set: set_h,
            min: 1,
            max: 15,
            step: 1,
            unit: Unit::Sec,
        },
    ]
}

/// Render a threshold value with its unit.
fn fmt_value(v: u16, unit: Unit) -> String {
    match unit {
        Unit::Pct => format!("{v}%"),
        Unit::PctRel => format!("+{v}%"),
        Unit::Mult => format!("×{v}"),
        // Seconds and days move to the caption ("Окно (сек)"), so the value is just the number.
        Unit::Sec | Unit::Day => v.to_string(),
    }
}

/// A threshold caption, with the unit appended in parentheses for a seconds or days value
/// ("Окно (сек)") so the stepper value stays a bare number.
fn caption_for(cap: &str, unit: Unit) -> String {
    match unit {
        Unit::Sec => format!("{cap} ({})", t!("core_status.warn_cfg.u.sec")),
        Unit::Day => format!("{cap} ({})", t!("core_status.warn_cfg.u.day")),
        _ => cap.to_string(),
    }
}

/// A human sound name from an embedded stem (`yes_mast` → `Yes Mast`).
fn nice_sound(stem: &str) -> String {
    stem.split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + chars.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The enable checkbox: reflects `WarnAxesCfg` and writes the flipped set back to the backend.
fn enable_checkbox(
    id: String,
    checked: bool,
    backend: &Entity<Backend>,
    set: fn(&mut WarnAxesCfg, bool),
) -> impl IntoElement {
    let backend = backend.clone();
    MoonCheckbox::new(SharedString::from(id))
        .checked(checked)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| {
            let on = *ch;
            backend.update(app, |b, cx| {
                let mut axes = b.warn_axes();
                set(&mut axes, on);
                b.set_warn_axes(axes);
                b.mark_backend_dirty(cx);
            });
        })
}

/// The "show on chart" checkbox: reflects `WarnParams` and writes the flipped set back.
fn chart_checkbox(
    id: String,
    checked: bool,
    backend: &Entity<Backend>,
    set: fn(&mut WarnParams, bool),
) -> impl IntoElement {
    let backend = backend.clone();
    MoonCheckbox::new(SharedString::from(id))
        .checked(checked)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| {
            let on = *ch;
            backend.update(app, |b, cx| {
                let mut params = b.warn_params();
                set(&mut params, on);
                b.set_warn_params(params);
                b.mark_backend_dirty(cx);
            });
        })
}

#[cfg(test)]
mod tests;
