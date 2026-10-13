//! The Badges tab edits badge codes and colors by strategy kind (detection type). Each row
//! controls whether the badge is drawn, its long code, an optional distinct short code,
//! theme-specific colors, and a per-row outline with long/short colors. Edits update the draft
//! for live preview; Save writes the portable `badges.json` file.
//!
//! [`BadgesEd`] stores editor state. Rows are rebuilt after add/delete and after a successful
//! badge-config paste so subscriptions capture current indices, matching the Connections tab.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonAlert, MoonBadge, MoonBadgeVariant, MoonButton, MoonColorPickerState, MoonDisclosure,
    MoonInput, MoonInputEvent, MoonInputState, MoonPalette, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use std::rc::Rc;

use super::SettingsView;
use crate::{Backend, design};
use moon_core::config::BadgeEntry;

/// Editor state for one badge row: text inputs and colour pickers for the colour set active when
/// the state is built.
pub(super) struct BadgeRowEd {
    /// Index in the draft's `badges.entries`.
    ///
    /// This may differ from the row position because the editor hides the `Unknown` service
    /// entry with ordinal 0 while retaining it in the data.
    idx: usize,
    ordinal: Entity<MoonInputState>,
    name: Entity<MoonInputState>,
    code: Entity<MoonInputState>,
    code_short: Entity<MoonInputState>,
    color: Entity<MoonColorPickerState>,
    outline_long: Entity<MoonColorPickerState>,
    outline_short: Entity<MoonColorPickerState>,
}

/// Editor state for the Badges tab.
pub(super) struct BadgesEd {
    rows: Vec<BadgeRowEd>,
}

/// Build a text input bound to a field in draft `badges.entries[idx]`.
fn badge_input(
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    idx: usize,
    init: String,
    apply: impl Fn(&mut BadgeEntry, String) + 'static,
) -> Entity<MoonInputState> {
    let st = cx.new(|cx| MoonInputState::new(window, cx).default_value(init));
    cx.subscribe(&st, move |this, emitter, ev: &MoonInputEvent, cx| {
        if matches!(ev, MoonInputEvent::Change) {
            let val = emitter.read(cx).value().to_string();
            this.backend.update(cx, |b, bcx| {
                if let Some(p) = b.preview.as_mut()
                    && let Some(e) = p.badges.entries.get_mut(idx)
                {
                    apply(e, val);
                    bcx.notify();
                }
            });
        }
    })
    .detach();
    st
}

/// Bind a badge color to its draft field through the shared app-global history factory.
/// Theme selection is resolved from the current draft on each read and write.
fn entry_color(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    idx: usize,
    get: impl Fn(&BadgeEntry, bool) -> [u8; 3] + Copy + 'static,
    set: impl Fn(&mut BadgeEntry, bool, [u8; 3]) + 'static,
) -> Entity<MoonColorPickerState> {
    // A badge entry holds one colour per colour set, so which half these closures address depends on
    // the mode that is LIVE at the moment of the read or the write — never on the mode that was
    // saved when this editor was built. The entry itself cannot answer that, so the mode is
    // resolved here, where the draft is in scope, and handed down: in the getter from the same
    // snapshot the entry is read out of, in the setter from the draft being written.
    let init = {
        let b = backend.read(cx);
        let cfg = b.preview.as_ref().unwrap_or(&b.config);
        let is_light = cfg.ui_theme_mode.is_light();
        cfg.badges
            .entries
            .get(idx)
            .map(|e| get(e, is_light))
            .unwrap_or([0x97, 0x92, 0x8A])
    };
    super::draft_color(window, cx, init, move |p, cc| {
        let is_light = p.ui_theme_mode.is_light();
        if let Some(e) = p.badges.entries.get_mut(idx)
            && get(e, is_light) != cc
        {
            set(e, is_light, cc);
            return true;
        }
        false
    })
}

/// Build badge editor state from the current draft.
///
/// Called during settings creation, after add/delete, and after a successful Badges-tab paste.
pub(super) fn build(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> BadgesEd {
    let entries = {
        let b = backend.read(cx);
        b.preview
            .as_ref()
            .unwrap_or(&b.config)
            .badges
            .entries
            .clone()
    };
    let rows = entries
        .iter()
        .enumerate()
        // `Unknown` (ordinal 0) is the fallback bucket for unrecognized detection types. Hide it
        // from the editor while retaining it in the data and `badges.json`.
        .filter(|(_, e)| e.ordinal != 0)
        .map(|(idx, e)| BadgeRowEd {
            idx,
            ordinal: badge_input(window, cx, idx, e.ordinal.to_string(), |e, v| {
                if let Ok(n) = v.trim().parse::<u8>() {
                    e.ordinal = n;
                }
            }),
            name: badge_input(window, cx, idx, e.name.clone(), |e, v| e.name = v),
            code: badge_input(window, cx, idx, e.code.clone(), |e, v| {
                e.code = v.chars().take(3).collect();
            }),
            code_short: badge_input(window, cx, idx, e.code_short.clone(), |e, v| {
                e.code_short = v.chars().take(3).collect();
            }),
            color: entry_color(
                backend,
                window,
                cx,
                idx,
                move |e, is_light| e.color(is_light),
                move |e, is_light, cc| {
                    if is_light {
                        e.color_light = cc;
                    } else {
                        e.color_dark = cc;
                    }
                },
            ),
            outline_long: entry_color(
                backend,
                window,
                cx,
                idx,
                move |e, is_light| {
                    if is_light {
                        e.outline_long_light
                    } else {
                        e.outline_long_dark
                    }
                },
                move |e, is_light, cc| {
                    if is_light {
                        e.outline_long_light = cc;
                    } else {
                        e.outline_long_dark = cc;
                    }
                },
            ),
            outline_short: entry_color(
                backend,
                window,
                cx,
                idx,
                move |e, is_light| {
                    if is_light {
                        e.outline_short_light
                    } else {
                        e.outline_short_dark
                    }
                },
                move |e, is_light, cc| {
                    if is_light {
                        e.outline_short_light = cc;
                    } else {
                        e.outline_short_dark = cc;
                    }
                },
            ),
        })
        .collect();

    BadgesEd { rows }
}

impl SettingsView {
    /// Add a kind to the draft with `ordinal = max + 1`, then rebuild editor state.
    fn add_badge(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let default_color = design::u32_to_rgb(MoonPalette::active(cx).accent);
        self.backend.update(cx, |b, bcx| {
            if let Some(p) = b.preview.as_mut() {
                let next = p
                    .badges
                    .entries
                    .iter()
                    .map(|e| e.ordinal)
                    .max()
                    .unwrap_or(0)
                    .saturating_add(1);
                p.badges.entries.push(BadgeEntry {
                    ordinal: next,
                    name: String::new(),
                    code: "???".to_string(),
                    color_dark: default_color,
                    color_light: default_color,
                    ..BadgeEntry::default()
                });
                bcx.notify();
            }
        });
        self.badges = build(&self.backend, window, cx);
        cx.notify();
    }

    /// Delete draft entry `idx`, then rebuild editor state.
    fn delete_badge(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.backend.update(cx, |b, bcx| {
            if let Some(p) = b.preview.as_mut()
                && idx < p.badges.entries.len()
            {
                p.badges.entries.remove(idx);
                bcx.notify();
            }
        });
        self.badges = build(&self.backend, window, cx);
        cx.notify();
    }

    /// Render one badge editor card.
    /// Render one badge row of the table.
    ///
    /// Columns: [ordinal, expert only], shown, name, code, short code (or the button that adds
    /// one), colour, outline, a preview, delete. With the outline on, its long and short colours
    /// take a second line under the row.
    fn badge_row(
        &self,
        cx: &Context<Self>,
        idx: usize,
        row: &BadgeRowEd,
        wrapped: bool,
    ) -> AnyElement {
        let expert = self.badges_ui.expert_open;
        let (active, distinguish, outline, code, color) = {
            let b = self.backend.read(cx);
            let is_light = b
                .preview
                .as_ref()
                .unwrap_or(&b.config)
                .ui_theme_mode
                .is_light();
            let bcfg = &b.preview.as_ref().unwrap_or(&b.config).badges;
            bcfg.entries
                .get(idx)
                .map(|e| {
                    (
                        e.active,
                        e.distinguish_dir,
                        e.outline,
                        e.code.clone(),
                        e.color(is_light),
                    )
                })
                .unwrap_or((true, false, false, "UNK".to_string(), [0x97, 0x92, 0x8A]))
        };
        let bcol = design::rgb_to_u32(color);
        let set_flag = move |get: fn(&mut BadgeEntry) -> &mut bool, v: bool| {
            move |p: &mut moon_core::config::AppConfig, _: bool| {
                if let Some(e) = p.badges.entries.get_mut(idx)
                    && *get(e) != v
                {
                    *get(e) = v;
                    return true;
                }
                false
            }
        };
        let active_chk = self.draft_checkbox(
            cx,
            SharedString::from(format!("badge-active-{idx}")),
            active,
            move |p, v| {
                if let Some(e) = p.badges.entries.get_mut(idx)
                    && e.active != v
                {
                    e.active = v;
                    return true;
                }
                false
            },
        );
        let outline_chk = self.draft_checkbox(
            cx,
            SharedString::from(format!("badge-outline-{idx}")),
            outline,
            move |p, v| {
                if let Some(e) = p.badges.entries.get_mut(idx)
                    && e.outline != v
                {
                    e.outline = v;
                    return true;
                }
                false
            },
        );
        // A separate short code is an opt-in: "+ separate" turns it on, the cross turns it off.
        let flip_short = move |on: bool| {
            let apply = set_flag(|e| &mut e.distinguish_dir, on);
            cx.listener(move |this: &mut SettingsView, _: &ClickEvent, _, cx| {
                let changed = this.backend.update(cx, |b, bcx| {
                    let changed = b.preview.as_mut().is_some_and(|p| apply(p, on));
                    if changed {
                        bcx.notify();
                    }
                    changed
                });
                if changed {
                    cx.notify();
                }
            })
        };
        let short = if distinguish {
            h_flex()
                .gap(design::ui_px(cx, 4.0))
                .items_center()
                .child(
                    div().w(design::ui_px(cx, BADGE_COLS.code)).child(
                        MoonInput::new(SharedString::from(format!("badge-codeshort-{idx}")))
                            .state(&row.code_short)
                            .size(design::INPUT_SIZE)
                            .mono(true),
                    ),
                )
                .child(
                    MoonButton::new(SharedString::from(format!("badge-codeshort-off-{idx}")))
                        .ghost()
                        .width(22.0)
                        .label("\u{00d7}")
                        .on_click(flip_short(false))
                        .render(),
                )
                .into_any_element()
        } else {
            MoonButton::new(SharedString::from(format!("badge-codeshort-on-{idx}")))
                .ghost()
                .size(design::CONTROL_TIER)
                .label(t!("badges.short_separate").to_string())
                .on_click(flip_short(true))
                .render()
                .into_any_element()
        };
        let outline_colors = outline.then(|| {
            (
                super::common::swatch_picker(&row.outline_long).into_any_element(),
                super::common::swatch_picker(&row.outline_short).into_any_element(),
            )
        });
        let input = |id: String, state: &Entity<moon_ui::MoonInputState>| {
            MoonInput::new(SharedString::from(id))
                .state(state)
                .size(design::INPUT_SIZE)
                .mono(true)
                .into_any_element()
        };
        let cells = BadgeCells {
            ordinal: expert.then(|| input(format!("badge-ord-{idx}"), &row.ordinal)),
            active: active_chk.into_any_element(),
            name: input(format!("badge-name-{idx}"), &row.name),
            code: input(format!("badge-code-{idx}"), &row.code),
            short,
            color: super::common::swatch_picker(&row.color).into_any_element(),
            outline: outline_chk.into_any_element(),
            look: MoonBadge::new(code)
                .size(moon_ui::MoonSize::Xs.into())
                .variant(MoonBadgeVariant::Soft)
                .bg_color(bcol)
                .text_color(bcol)
                .mono(true)
                .into_any_element(),
            delete: MoonButton::new(SharedString::from(format!("badge-del-{idx}")))
                .ghost()
                .width(24.0)
                .label("\u{2715}")
                .tooltip(t!("badges.delete").to_string())
                .on_click(cx.listener(move |this, _, w, cx| this.delete_badge(idx, w, cx)))
                .render()
                .into_any_element(),
        };
        badge_row_el(idx, cells, outline_colors, wrapped, cx)
    }

    /// Render the Badges page body: the table, Add, and the expert disclosure.
    pub(super) fn badges_tab(&self, body_width: f32, cx: &Context<Self>) -> AnyElement {
        let expert = self.badges_ui.expert_open;
        let content_w = body_width - 2.0 * design::ui_value(cx, super::render::BODY_PAD);
        let wrapped = badges_wrapped(content_w, expert, cx);
        // Use `row.idx`, the position in `entries`, rather than enumerating visible rows. The
        // hidden `Unknown` entry can make the visible list shorter and would shift edits.
        let rows = self
            .badges
            .rows
            .iter()
            .map(|row| self.badge_row(cx, row.idx, row, wrapped))
            .collect();
        let add = MoonButton::new("badge-add")
            .variant(moon_ui::MoonButtonVariant::Neutral)
            .label(format!("+ {}", t!("badges.add")))
            .on_click(cx.listener(|this, _, w, cx| this.add_badge(w, cx)))
            .render()
            .into_any_element();
        let this = cx.entity().downgrade();
        let toggle: super::common::Action = Rc::new(move |_, cx| {
            let _ = this.update(cx, |view, cx| {
                view.badges_ui.expert_open = !view.badges_ui.expert_open;
                cx.notify();
            });
        });
        badges_body(
            content_w,
            badges_table(expert, wrapped, rows, cx),
            add,
            expert,
            toggle,
            cx,
        )
    }
}

/// Badge table column widths in pixels.
struct BadgeCols {
    ordinal: f32,
    active: f32,
    name: f32,
    code: f32,
    short: f32,
    color: f32,
    outline: f32,
    look: f32,
    delete: f32,
}

/// The MoonUI colour picker's trigger is a fixed 128 px, so the colour column is too; a body
/// narrower than the columns wraps each row into two lines rather than squeezing them.
const BADGE_COLS: BadgeCols = BadgeCols {
    ordinal: 44.0,
    active: 34.0,
    name: 140.0,
    code: 52.0,
    short: 110.0,
    color: 128.0,
    outline: 64.0,
    look: 48.0,
    delete: 28.0,
};

/// Gap between badge table columns, in pixels.
const BADGE_GAP: f32 = 8.0;

/// Left indent of a row's outline colours: under the colour column.
fn badge_outline_indent(expert: bool) -> f32 {
    let c = &BADGE_COLS;
    let before = [c.active, c.name, c.code, c.short];
    let ord = if expert { c.ordinal + BADGE_GAP } else { 0.0 };
    ord + before.iter().map(|w| w + BADGE_GAP).sum::<f32>()
}

/// Horizontal room the table's box takes beyond its columns: padding and border.
const BADGE_BOX_PAD: f32 = 26.0;

/// Whether rows wrap into two lines: the one-line table, at the UI zoom, is wider than the
/// body's content.
///
/// Args:
///     content_w: The body's content width in logical pixels.
///     expert: Whether the ordinal column shows.
///     cx: Application context for the UI zoom.
pub(super) fn badges_wrapped(content_w: f32, expert: bool, cx: &App) -> bool {
    wraps_at(content_w, expert, design::ui_value(cx, 1.0))
}

/// [`badges_wrapped`] at an explicit zoom factor.
fn wraps_at(content_w: f32, expert: bool, zoom: f32) -> bool {
    content_w < (badge_table_w(expert) + BADGE_BOX_PAD) * zoom + picker_excess(zoom)
}

/// The colour picker's trigger stays 128 px at every zoom: how much it outgrows its scaled
/// column below 100%.
fn picker_excess(zoom: f32) -> f32 {
    (BADGE_COLS.color - BADGE_COLS.color * zoom).max(0.0)
}

/// The colour column: scaled with the zoom, never narrower than the picker's fixed trigger.
fn color_cell_w(cx: &App) -> Pixels {
    px(design::ui_value(cx, BADGE_COLS.color).max(BADGE_COLS.color))
}

/// A badge row's cells, built by the page; [`badge_row_el`] places them.
pub(super) struct BadgeCells {
    pub(super) ordinal: Option<AnyElement>,
    pub(super) active: AnyElement,
    pub(super) name: AnyElement,
    pub(super) code: AnyElement,
    pub(super) short: AnyElement,
    pub(super) color: AnyElement,
    pub(super) outline: AnyElement,
    pub(super) look: AnyElement,
    pub(super) delete: AnyElement,
}

/// One badge row: one line when the table fits, else what the badge is over how it looks.
///
/// Args:
///     idx: The entry's position, for the row's selector.
///     cells: The row's controls.
///     outline_colors: The long and short outline pickers when the outline is on.
///     wrapped: Whether the row takes two lines.
///     cx: Application context for theme.
pub(super) fn badge_row_el(
    idx: usize,
    cells: BadgeCells,
    outline_colors: Option<(AnyElement, AnyElement)>,
    wrapped: bool,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    let c = &BADGE_COLS;
    let expert = cells.ordinal.is_some();
    let cell = |w: f32, el: AnyElement| div().w(design::ui_px(cx, w)).flex_none().child(el);
    let line = || h_flex().gap(design::ui_px(cx, BADGE_GAP)).items_center();
    let ordinal = cells.ordinal.map(|el| cell(c.ordinal, el));
    let one = [
        cell(c.active, cells.active),
        cell(c.name, cells.name),
        cell(c.code, cells.code),
        cell(c.short, cells.short),
    ];
    let two = [
        div().w(color_cell_w(cx)).flex_none().child(cells.color),
        cell(c.outline, cells.outline),
        cell(c.look, cells.look).flex().justify_center(),
        cell(c.delete, cells.delete),
    ];
    let lines = if wrapped {
        v_flex()
            .gap(design::ui_px(cx, 4.0))
            .child(line().children(one))
            .child(line().children(two).children(ordinal))
            .into_any_element()
    } else {
        line()
            .children(ordinal)
            .children(one)
            .children(two)
            .into_any_element()
    };
    // Under the colour column: the line's start when wrapped, past line one's columns when not.
    let indent = if wrapped {
        0.0
    } else {
        badge_outline_indent(expert)
    };
    let soft = rgba_from(p.text_soft, 1.0);
    let cap = |text: &str| div().flex_none().text_color(soft).child(text.to_string());
    v_flex()
        .debug_selector(move || format!("badge-row-{idx}"))
        .py(design::ui_px(cx, 4.0))
        .border_t_1()
        .border_color(rgba_from(p.border, 1.0))
        .child(lines)
        .children(outline_colors.map(|(long, short)| {
            h_flex()
                .pt(design::ui_px(cx, 4.0))
                .pl(design::ui_px(cx, indent))
                .gap(design::ui_px(cx, 6.0))
                .items_center()
                .child(cap("L"))
                .child(long)
                .child(cap("S"))
                .child(short)
        }))
        .into_any_element()
}

/// The table's full width for the visible columns, in pixels.
fn badge_table_w(expert: bool) -> f32 {
    let c = &BADGE_COLS;
    let cols = [
        c.active, c.name, c.code, c.short, c.color, c.outline, c.look, c.delete,
    ];
    let ord = if expert { c.ordinal + BADGE_GAP } else { 0.0 };
    ord + cols.iter().sum::<f32>() + BADGE_GAP * (cols.len() - 1) as f32
}

/// The badge table: column headers over the rows. Wrapped, the header names line one only.
fn badges_table(expert: bool, wrapped: bool, rows: Vec<AnyElement>, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    let head = |w: f32, key: &'static str| {
        div()
            .w(design::ui_px(cx, w))
            .flex_none()
            .truncate()
            .child(t!(key).to_string())
    };
    let c = &BADGE_COLS;
    let header = h_flex()
        .gap(design::ui_px(cx, BADGE_GAP))
        .pb(design::ui_px(cx, 4.0))
        .text_size(design::t_caption(cx))
        .text_color(rgba_from(p.text_muted, 1.0))
        .when(expert && !wrapped, |el| {
            el.child(div().w(design::ui_px(cx, c.ordinal)).flex_none().child("#"))
        })
        .child(head(c.active, "badges.col.active"))
        .child(head(c.name, "badges.col.type"))
        .child(head(c.code, "badges.col.code"))
        .child(head(c.short, "badges.col.short_code"))
        .when(!wrapped, |el| {
            el.child(
                div()
                    .w(color_cell_w(cx))
                    .flex_none()
                    .truncate()
                    .child(t!("badges.col.color").to_string()),
            )
            .child(head(c.outline, "badges.col.outline"))
            .child(head(c.look, "badges.col.look"))
            .child(div().w(design::ui_px(cx, c.delete)).flex_none())
        });
    div()
        .w_full()
        .min_w_0()
        .debug_selector(|| "badges-table".into())
        .child(
            super::general::page_box(Vec::new(), cx)
                .debug_selector(|| "badges-table-box".into())
                // The card takes the page's width; the table inside keeps its own column widths.
                .w_full()
                .py(design::ui_px(cx, 8.0))
                .child(header)
                .children(rows),
        )
        .into_any_element()
}

/// The Badges page body.
///
/// Args:
///     content_w: The body's content width in logical pixels.
///     table: The badge table.
///     add: The Add button.
///     expert_open: Whether the expert disclosure is unfolded.
///     toggle: Folds or unfolds it.
///     cx: Application context for theme and scale.
fn badges_body(
    content_w: f32,
    table: AnyElement,
    add: AnyElement,
    expert_open: bool,
    toggle: super::common::Action,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    let on_caret = toggle.clone();
    let title = t!("badges.expert.title").to_string();
    let (title_w, hint_w) = expert_widths(content_w, &title, cx);
    let disclosure = h_flex()
        .id("badges-expert")
        .debug_selector(|| "badges-expert".into())
        .w_full()
        .flex_wrap()
        .gap(design::ui_px(cx, 6.0))
        .items_center()
        .px(design::ui_px(cx, EXPERT_PAD_X))
        .py(design::ui_px(cx, 6.0))
        .border_1()
        .border_color(rgba_from(p.border, 1.0))
        .rounded(design::r_container(cx))
        .cursor_pointer()
        .on_click(move |_, window, cx| toggle(window, cx))
        .child(
            MoonDisclosure::button("badges-expert-caret", expert_open)
                .size(design::DISCLOSURE_GLYPH)
                .box_size(design::DISCLOSURE_BOX)
                .on_toggle(move |_, window, cx| on_caret(window, cx)),
        )
        .child(
            super::common::prose("badges-expert-title".into(), title, false, cx)
                .flex_none()
                .w(px(title_w))
                .max_w_full(),
        )
        .child(
            super::common::prose(
                "badges-expert-hint".into(),
                t!("badges.expert.hint").to_string(),
                false,
                cx,
            )
            .flex_none()
            .w(px(hint_w))
            .text_color(rgba_from(p.text_muted, 1.0)),
        );
    v_flex()
        .w_full()
        .min_w_0()
        .gap(design::ui_px(cx, 12.0))
        .child(table)
        .child(div().debug_selector(|| "badges-add".into()).child(add))
        .child(disclosure)
        .when(expert_open, |el| {
            el.child(
                div().debug_selector(|| "badges-expert-warn".into()).child(
                    MoonAlert::warning("badges-expert-warn", t!("badges.expert.warn").to_string())
                        .render(),
                ),
            )
        })
        .into_any_element()
}

/// Horizontal padding of the expert disclosure row, unscaled.
const EXPERT_PAD_X: f32 = 10.0;

/// Widths of the expert disclosure's title and caption on a body `content_w` wide.
///
/// Both are explicit: GPUI paints a text with the lines of its LAST measure, and a content-sized
/// flex item is last measured at its min-content width, which stacked the caption one word per
/// line on a full-width card. The caption takes the rest of the row beside the title; a row too
/// narrow for its longest word wraps it under the title at the full row width instead.
///
/// Args:
///     content_w: The body's content width in logical pixels.
///     title: The disclosure title.
///     cx: Application context for scale and text measures.
///
/// Returns:
///     The title's and the caption's widths in logical pixels.
fn expert_widths(content_w: f32, title: &str, cx: &App) -> (f32, f32) {
    let title_w = design::ui_body_text_width(cx, title, 400.0) + 2.0;
    // The row's padding, border and the caret with the two gaps beside it.
    let row_w = (content_w - 2.0 * design::ui_value(cx, EXPERT_PAD_X) - 2.0).max(0.0);
    let caret = design::ui_value(cx, design::DISCLOSURE_BOX + 2.0 * 6.0);
    let beside = row_w - caret - title_w;
    let floor = super::common::longest_word_w(&t!("badges.expert.hint"), false, cx);
    let hint_w = if beside >= floor { beside } else { row_w };
    (title_w.min(row_w), hint_w)
}

impl SettingsView {
    /// The Detect badges page, Copy and Paste in its header.
    pub(super) fn badges_page(
        &self,
        body_width: f32,
        cx: &Context<Self>,
    ) -> super::frame::PageParts {
        let body = self.badges_tab(body_width, cx);
        self.page_parts(super::Tab::Badges, body, self.share_actions(cx), cx)
    }
}

/// UI state of the Badges page that the draft does not hold. Branch-owned: fields are added here,
/// never on `SettingsView`.
#[derive(Default)]
pub(super) struct BadgesUi {
    /// Whether the expert disclosure (type ordinals) is unfolded. Per Settings session.
    pub(super) expert_open: bool,
}

#[cfg(test)]
mod tests;
