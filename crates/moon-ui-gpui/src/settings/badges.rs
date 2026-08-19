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
    MoonBadge, MoonBadgeSize, MoonBadgeVariant, MoonButton, MoonButtonSize, MoonCheckboxSize,
    MoonColorPicker, MoonColorPickerState, MoonInput, MoonInputEvent, MoonInputState, MoonPalette,
    MoonTooltipView, StyledExt, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::{SettingsView, separator};
use crate::{Backend, design};
use moon_core::config::{BadgeEntry, UiThemeMode};

/// Editor state for one badge row: text inputs and color pickers for the saved theme mode selected
/// when the state is built.
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
                if let Some(p) = b.preview.as_mut() {
                    if let Some(e) = p.badges.entries.get_mut(idx) {
                        apply(e, val);
                        bcx.notify();
                    }
                }
            });
        }
    })
    .detach();
    st
}

/// Build a color picker bound through `BadgeEntry` accessors to draft `badges.entries[idx]`.
fn entry_color(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    idx: usize,
    get: impl Fn(&BadgeEntry, bool) -> [u8; 3] + Copy + 'static,
    set: impl Fn(&mut BadgeEntry, bool, [u8; 3]) + 'static,
) -> Entity<MoonColorPickerState> {
    // A badge entry holds one colour per UI mode, so which half these closures address depends on
    // the mode that is LIVE at the moment of the read or the write — never on the mode that was
    // saved when this editor was built. The entry itself cannot answer that, so the mode is
    // resolved here, where the draft is in scope, and handed down: in the getter from the same
    // snapshot the entry is read out of, in the setter from the draft being written.
    let init = {
        let b = backend.read(cx);
        let cfg = b.preview.as_ref().unwrap_or(&b.config);
        let is_light = cfg.ui_theme_mode == UiThemeMode::Light;
        cfg.badges
            .entries
            .get(idx)
            .map(|e| get(e, is_light))
            .unwrap_or([0x97, 0x92, 0x8A])
    };
    super::draft_color(window, cx, init, move |p, cc| {
        let is_light = p.ui_theme_mode == UiThemeMode::Light;
        if let Some(e) = p.badges.entries.get_mut(idx) {
            if get(e, is_light) != cc {
                set(e, is_light, cc);
                return true;
            }
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
            if let Some(p) = b.preview.as_mut() {
                if idx < p.badges.entries.len() {
                    p.badges.entries.remove(idx);
                    bcx.notify();
                }
            }
        });
        self.badges = build(&self.backend, window, cx);
        cx.notify();
    }

    /// Render one badge editor card.
    ///
    /// The row contains ordinal, name, active state, code, optional short code, color, preview,
    /// outline controls with optional long/short colors, and delete action.
    fn badge_row(&self, cx: &Context<Self>, idx: usize, row: &BadgeRowEd) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let (active, distinguish, outline, code, color) = {
            let b = self.backend.read(cx);
            let is_light =
                b.preview.as_ref().unwrap_or(&b.config).ui_theme_mode == UiThemeMode::Light;
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

        let active_chk = self
            .draft_checkbox(
                cx,
                SharedString::from(format!("badge-active-{idx}")),
                active,
                move |p, v| {
                    if let Some(e) = p.badges.entries.get_mut(idx) {
                        if e.active != v {
                            e.active = v;
                            return true;
                        }
                    }
                    false
                },
            )
            .label(t!("badges.col_active").to_string())
            .size(MoonCheckboxSize::Compact);

        let distinguish_chk = self
            .draft_checkbox(
                cx,
                SharedString::from(format!("badge-dist-{idx}")),
                distinguish,
                move |p, v| {
                    if let Some(e) = p.badges.entries.get_mut(idx) {
                        if e.distinguish_dir != v {
                            e.distinguish_dir = v;
                            return true;
                        }
                    }
                    false
                },
            )
            .label(t!("badges.distinguish").to_string())
            .size(MoonCheckboxSize::Compact);

        let outline_chk = self
            .draft_checkbox(
                cx,
                SharedString::from(format!("badge-outline-{idx}")),
                outline,
                move |p, v| {
                    if let Some(e) = p.badges.entries.get_mut(idx) {
                        if e.outline != v {
                            e.outline = v;
                            return true;
                        }
                    }
                    false
                },
            )
            .label(t!("badges.use_outline").to_string())
            .size(MoonCheckboxSize::Compact);

        // Keep everything on one row. Color pickers use `flex_none` at their natural 128px width
        // so they do not overlap the next badge. Outline colors appear to the right as L for long
        // and S for short when the outline checkbox is enabled.
        let cap = |t: &str| {
            div()
                .flex_none()
                .text_color(rgba_from(p.text_soft, 1.0))
                .child(t.to_string())
        };
        // Explain cryptic row fields with tooltips because Badges has no column header.
        let tip = |id: &str, key: &'static str, el: gpui::Div| {
            el.id(SharedString::from(format!("{id}-{idx}")))
                .tooltip(move |_w, cx| {
                    cx.new(|_| MoonTooltipView::new(t!(key).to_string()).max_width(320.0))
                        .into()
                })
        };
        let row_el = h_flex()
            .gap(px(6.0))
            .items_center()
            .child(
                tip(
                    "badge-ord-tip",
                    "badges.ordinal_tip",
                    div().flex_none().w(px(42.0)),
                )
                .child(
                    MoonInput::new(SharedString::from(format!("badge-ord-{idx}")))
                        .state(&row.ordinal)
                        .small(),
                ),
            )
            .child(
                div().flex_none().w(px(120.0)).child(
                    MoonInput::new(SharedString::from(format!("badge-name-{idx}")))
                        .state(&row.name)
                        .small(),
                ),
            )
            .child(active_chk)
            .child(
                tip(
                    "badge-code-tip",
                    "badges.code_tip",
                    div().flex_none().w(px(44.0)),
                )
                .child(
                    MoonInput::new(SharedString::from(format!("badge-code-{idx}")))
                        .state(&row.code)
                        .small(),
                ),
            )
            .child(distinguish_chk)
            .when(distinguish, |el| {
                el.child(
                    tip(
                        "badge-codeshort-tip",
                        "badges.code_short_tip",
                        div().flex_none().w(px(44.0)),
                    )
                    .child(
                        MoonInput::new(SharedString::from(format!("badge-codeshort-{idx}")))
                            .state(&row.code_short)
                            .small(),
                    ),
                )
            })
            .child(div().flex_none().child(MoonColorPicker::new(&row.color)))
            .child(
                div().flex_none().w(px(42.0)).flex().justify_center().child(
                    MoonBadge::new(code)
                        .variant(MoonBadgeVariant::Soft)
                        .size(MoonBadgeSize::Status)
                        .bg_color(bcol)
                        .text_color(bcol)
                        .mono(true),
                ),
            )
            .child(outline_chk)
            .when(outline, |el| {
                el.child(cap("L"))
                    .child(
                        div()
                            .flex_none()
                            .child(MoonColorPicker::new(&row.outline_long)),
                    )
                    .child(cap("S"))
                    .child(
                        div()
                            .flex_none()
                            .child(MoonColorPicker::new(&row.outline_short)),
                    )
            })
            .child(
                MoonButton::new(SharedString::from(format!("badge-del-{idx}")))
                    .danger()
                    .size(MoonButtonSize::Micro)
                    .width(24.0)
                    .label("x")
                    .on_click(cx.listener(move |this, _, w, cx| this.delete_badge(idx, w, cx)))
                    .render(),
            );

        // Omitting `w_full` makes the card fit its contents, so enabling outline colors widens
        // its border. The parent list keeps cards left-aligned.
        div()
            .px_1()
            .py_0p5()
            .rounded(design::r_button(cx))
            .border_1()
            .border_color(rgba_from(p.border, 1.0))
            .child(row_el)
    }

    /// Render the Badges tab as one card per kind followed by the Add Type button.
    pub(super) fn badges_tab(&self, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let mut col = v_flex()
            .w_full()
            .items_start()
            .gap_1()
            .child(div().font_bold().child(t!("badges.title").to_string()))
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(rgba_from(p.text_soft, 1.0))
                    .child(t!("badges.theme_hint").to_string()),
            );
        // Use `row.idx`, the position in `entries`, rather than enumerating visible rows. The
        // hidden `Unknown` entry can make the visible list shorter and would shift edits.
        for row in self.badges.rows.iter() {
            col = col.child(self.badge_row(cx, row.idx, row));
        }
        col.child(separator(p, cx)).child(
            div().mt_1().child(
                MoonButton::new("badge-add")
                    .small()
                    .width(150.0)
                    .label(t!("badges.add").to_string())
                    .on_click(cx.listener(|this, _, w, cx| this.add_badge(w, cx)))
                    .render(),
            ),
        )
    }
}
