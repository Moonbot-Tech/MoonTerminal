//! The middle schema-sections panel renderer and full-mode table of contents.

use super::*;

#[cfg(test)]
mod tests;

/// Canonical runtime section titles and the locale key of their human name, in
/// `assets/param_deps.toml` order.
///
/// The schema streams at runtime, so this is an INDEX of what the repository has evidence for,
/// never a catalogue of what can arrive: a title absent here renders verbatim. Titles are matched
/// through [`section_title_eq`], never byte-for-byte -- Moonbot's own spelling differs from the
/// canonical form below in punctuation and spacing.
const SECTION_LABELS: &[(&str, &str)] = &[
    ("Main", "strat.section.Main"),
    (
        "Dynamic White/Black List",
        "strat.section.DynamicWhiteBlackList",
    ),
    ("Filters", "strat.section.Filters"),
    ("Filters / Base", "strat.section.Filters_Base"),
    ("Filters / Delta", "strat.section.Filters_Delta"),
    ("Filters / Ping", "strat.section.Filters_Ping"),
    (
        "Filters / Price/Position",
        "strat.section.Filters_PricePosition",
    ),
    ("Filters / Time", "strat.section.Filters_Time"),
    ("Filters / Volume", "strat.section.Filters_Volume"),
    ("Buy conditions", "strat.section.BuyConditions"),
    ("Delta Modifiers", "strat.section.DeltaModifiers"),
    ("Multiple Orders", "strat.section.MultipleOrders"),
    ("Sell order", "strat.section.SellOrder"),
    ("Sell order / SellShot", "strat.section.SellOrder_SellShot"),
    (
        "Sell order / SellSpread",
        "strat.section.SellOrder_SellSpread",
    ),
    ("Session", "strat.section.Session"),
    ("Stops", "strat.section.Stops"),
    ("Strategy settings", "strat.section.StrategySettings"),
    (
        "Triggers Master / Slave",
        "strat.section.Triggers_MasterSlave",
    ),
    ("User Interface", "strat.section.UserInterface"),
];

/// Compare two section titles under the punctuation the runtime actually produces.
///
/// moonproto composes a title as either Moonbot's own chapter string or `"<chapter> / <value>"`
/// (`strategy_schema.rs`), and what reaches the wire differs from the canonical spelling in ways a
/// byte compare cannot survive: `Dynamic White\Black List` arrives with a BACKSLASH, and
/// `Triggers  Master / Slave` with a doubled space. So `\` equals `/`, runs of whitespace equal one
/// space, whitespace beside a separator is ignored, and ASCII case is ignored. Allocation-free:
/// both sides are split on the separator and their segments compared word by word, so this can run
/// per frame against every candidate.
///
/// Args:
///     a: One section title, canonical or as the schema streamed it.
///     b: The other title, compared under the same normalization.
///
/// Returns:
///     Whether the two titles name the same section.
fn section_title_eq(a: &str, b: &str) -> bool {
    let separator = |c: char| c == '/' || c == '\\';
    let mut left = a.split(separator);
    let mut right = b.split(separator);
    loop {
        match (left.next(), right.next()) {
            (None, None) => return true,
            (Some(x), Some(y)) => {
                let mut words_x = x.split_whitespace();
                let mut words_y = y.split_whitespace();
                loop {
                    match (words_x.next(), words_y.next()) {
                        (None, None) => break,
                        (Some(p), Some(q)) if p.eq_ignore_ascii_case(q) => {}
                        _ => return false,
                    }
                }
            }
            _ => return false,
        }
    }
}

/// Return the locale key of the human name for a runtime section title.
///
/// Args:
///     raw_title: Section title exactly as the streamed schema produced it.
///
/// Returns:
///     A static locale key when the repository has evidence for that section, otherwise `None` --
///     the caller then renders the raw title, the same fail-closed rule the field labels use.
pub(super) fn section_label_key(raw_title: &str) -> Option<&'static str> {
    SECTION_LABELS
        .iter()
        .find(|(canonical, _)| section_title_eq(canonical, raw_title))
        .map(|(_, key)| *key)
}

/// Human name for a runtime section title, or the raw title when there is no label for it.
///
/// Args:
///     raw_title: Section title exactly as the streamed schema produced it.
///
/// Returns:
///     The localized section name, or `raw_title` unchanged.
pub(super) fn section_display_title(raw_title: &str) -> String {
    match section_label_key(raw_title) {
        Some(key) => t!(key).to_string(),
        None => raw_title.to_string(),
    }
}

impl StrategiesView {
    /// Measure the longest selected runtime section title for responsive first-run layout.
    ///
    /// The localized panel heading is the fallback when no selected runtime schema is available.
    /// Measurement matches the monospaced body text inherited by section rows.
    ///
    /// Args:
    ///     store: Live core store containing the selected strategy schema.
    ///     cx: Application context providing active text metrics and translations.
    ///
    /// Returns:
    ///     The longest measured section-title width, or the localized heading width as fallback.
    pub(super) fn longest_visible_section_label_width(&self, store: &CoreStore, cx: &App) -> f32 {
        selected_sections(self, store)
            .and_then(|sections| {
                sections
                    .iter()
                    .map(|section| {
                        design::mono_body_text_width(
                            cx,
                            &section_display_title(&section.title),
                            400.0,
                        )
                    })
                    .reduce(f32::max)
            })
            .unwrap_or_else(|| {
                design::mono_body_text_width(cx, &t!("strat.sections").to_string(), 600.0)
            })
    }

    /// Render schema sections using dependency values shared with the parameters panel.
    ///
    /// The caller computes `values` once per frame because building them normalizes every selected
    /// field and schema field name. In full mode, section clicks also queue the matching heading
    /// for the virtualized parameters list to scroll into view.
    pub(super) fn sections_panel(
        &self,
        store: &CoreStore,
        values: &Values,
        cx: &Context<Self>,
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        let border = moon(p.border);
        let mut col = v_flex()
            .w(px(self.panels.sections_w))
            .flex_none()
            .h_full()
            .bg(moon(p.shell_high))
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .line_height(design::line_px(cx, 14.0))
            .border_r_1()
            .border_color(border)
            .px(design::ui_px(cx, 10.0))
            .py(design::ui_px(cx, 12.0))
            .gap(design::ui_px(cx, 7.0))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t!("strat.sections").to_string()),
            )
            .child(div().w_full().h(px(1.0)).bg(border));

        // With nothing selected this column has nothing to list, and the parameters pane to its
        // right already says so. Repeating the sentence here made the window ask the same question
        // twice side by side, so the column keeps its heading and stays otherwise empty.
        let Some(sections) = selected_sections(self, store) else {
            return col.into_any_element();
        };
        if sections.is_empty() {
            return col
                .child(
                    div()
                        .mt_2()
                        .text_color(moon(p.text_muted))
                        .child(t!("strat.no_schema").to_string()),
                )
                .into_any_element();
        }
        // A version diff starts with the default synthetic All section, followed only by schema
        // sections containing changed fields.
        if let Some(ch) = self.version_changed_filter() {
            let ch: HashSet<String> = ch.keys().cloned().collect();
            let total = ch.len();
            let mut list = v_flex().w_full().gap_0();
            let row_base = |id: SharedString, cx: &Context<Self>| {
                div()
                    .id(id)
                    .w_full()
                    .h(design::fit_h_px(cx, 24.0, 14.0, 5.0))
                    .px(design::ui_px(cx, 6.0))
                    .rounded(design::ui_px(cx, 3.0))
                    .border_1()
                    .border_color(moon_alpha(p.border, 0.0))
                    .flex()
                    .items_center()
                    .cursor_pointer()
            };
            let on_all = self.versions.section.is_none();
            let mut all_row = row_base("sec-ver-all".into(), cx)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(moon(p.text))
                .child(t!("strat.sections_all").to_string())
                .child(
                    h_flex().ml_auto().flex_none().child(
                        MoonBadge::new(total.to_string())
                            .variant(MoonBadgeVariant::Soft)
                            .size(MoonBadgeSize::Status)
                            .tone(MoonTone::Muted)
                            .render(),
                    ),
                )
                .tooltip(crate::panels::common::text_tooltip(
                    t!("strat.sections_all_tip", n = total).to_string(),
                ))
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.versions.section.is_some() {
                        this.versions.section = None;
                        cx.notify();
                    }
                    this.request_param_scroll(0, cx);
                }));
            if on_all {
                all_row = all_row
                    .bg(moon_alpha(p.amber, 0.16))
                    .border_color(moon_alpha(p.amber, 0.55));
            } else {
                all_row = all_row.hover(move |s| s.bg(moon_alpha(p.panel, 0.74)));
            }
            list = list.child(all_row);
            for (i, sec) in sections.iter().enumerate() {
                let n = sec
                    .fields
                    .iter()
                    .filter(|f| ch.contains(&f.name.to_lowercase()))
                    .count();
                if n == 0 {
                    continue;
                }
                let on = self.versions.section == Some(i);
                let mut row = row_base(SharedString::from(format!("sec-ver-{i}")), cx)
                    .text_color(moon(p.text))
                    // The count badge beside it cannot shrink, and a Russian section name is
                    // half again as long as the schema's own: without this the title paints
                    // over the badge instead of degrading to an ellipsis.
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(section_display_title(&sec.title)),
                    )
                    .child(
                        h_flex().ml_auto().flex_none().child(
                            MoonBadge::new(n.to_string())
                                .variant(MoonBadgeVariant::Soft)
                                .size(MoonBadgeSize::Status)
                                .tone(MoonTone::Muted)
                                .render(),
                        ),
                    )
                    .tooltip(crate::panels::common::text_tooltip(
                        t!("strat.section_changed_tip", n = n).to_string(),
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.versions.section != Some(i) {
                            this.versions.section = Some(i);
                            cx.notify();
                        }
                        this.request_param_scroll(i, cx);
                    }));
                if on {
                    row = row
                        .bg(moon_alpha(p.amber, 0.16))
                        .border_color(moon_alpha(p.amber, 0.55));
                } else {
                    row = row.hover(move |s| s.bg(moon_alpha(p.panel, 0.74)));
                }
                list = list.child(row);
            }
            return col
                .child(
                    div()
                        .id("strat-sections-scroll")
                        .flex_1()
                        .w_full()
                        .overflow_y_scroll()
                        .child(list),
                )
                .into_any_element();
        }

        // Show active sections first and inactive sections second, preserving schema order within each.
        let mut order: Vec<(usize, bool)> = sections
            .iter()
            .enumerate()
            .map(|(i, sec)| (i, section_active(&self.rules, values, sec)))
            .collect();
        order.sort_by_key(|(_, active)| !active);

        let mut list = v_flex().w_full().gap_0();
        for (i, active) in order {
            let sec = &sections[i];
            let on = self.selected_section == i;
            let tcol = if !active { p.text_muted } else { p.text };
            // Resolved once: the caption and the raw-title tooltip ask the same question of the
            // same string, and this row is rebuilt on every repaint.
            let label_key = section_label_key(&sec.title);
            let mut row = div()
                .id(SharedString::from(format!("sec-{i}")))
                .w_full()
                .h(design::fit_h_px(cx, 24.0, 14.0, 5.0))
                .px(design::ui_px(cx, 6.0))
                .rounded(design::ui_px(cx, 3.0))
                .border_1()
                .border_color(moon_alpha(p.border, 0.0))
                .flex()
                .items_center()
                .cursor_pointer()
                .text_color(moon(tcol))
                // The pane is user-resizable down to a width no Russian section name fits, so the
                // caption degrades to an ellipsis rather than spilling into the splitter.
                .child(div().flex_1().min_w_0().truncate().child(match label_key {
                    Some(key) => t!(key).to_string(),
                    None => sec.title.clone(),
                }))
                // The raw schema title stays one hover away wherever a human name replaced it, so
                // a trader who knows Moonbot's own wording can still find the section by it.
                .when_some(label_key.map(|_| sec.title.clone()), |row, raw| {
                    row.tooltip(crate::panels::common::text_tooltip(raw))
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.selected_section != i {
                        this.selected_section = i;
                        this.persist_session(cx);
                        cx.notify();
                    }
                    this.request_param_scroll(i, cx);
                }));
            if on {
                row = row
                    .bg(moon_alpha(p.amber, 0.16))
                    .border_color(moon_alpha(p.amber, 0.55));
            } else {
                row = row.hover(move |s| s.bg(moon_alpha(p.panel, 0.74)));
            }
            list = list.child(row);
        }
        col = col.child(
            div()
                .id("strat-sections-scroll")
                .flex_1()
                .w_full()
                .overflow_y_scroll()
                .child(list),
        );
        col.into_any_element()
    }

    // ── Parameters for the selected section ─────────────────────────────────
}
