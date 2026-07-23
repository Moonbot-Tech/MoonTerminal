//! The middle sections panel renderer.

use super::*;

impl StrategiesView {
    pub(super) fn sections_panel(&self, store: &CoreStore, cx: &Context<Self>) -> AnyElement {
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

        let Some(sections) = selected_sections(self, store) else {
            return col
                .child(
                    div()
                        .mt_2()
                        .text_color(moon(p.text_muted))
                        .child(t!("strat.no_selection").to_string()),
                )
                .into_any_element();
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
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.versions.section.is_some() {
                        this.versions.section = None;
                        cx.notify();
                    }
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
                if !sec
                    .fields
                    .iter()
                    .any(|f| ch.contains(&f.name.to_lowercase()))
                {
                    continue;
                }
                let on = self.versions.section == Some(i);
                let mut row = row_base(SharedString::from(format!("sec-ver-{i}")), cx)
                    .text_color(moon(p.text))
                    .child(sec.title.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.versions.section != Some(i) {
                            this.versions.section = Some(i);
                            cx.notify();
                        }
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

        let values = selected_values(self, store);

        // Show active sections first and inactive sections second, preserving schema order within each.
        let mut order: Vec<(usize, bool)> = sections
            .iter()
            .enumerate()
            .map(|(i, sec)| (i, section_active(&self.rules, &values, sec)))
            .collect();
        order.sort_by_key(|(_, active)| !active);

        let mut list = v_flex().w_full().gap_0();
        for (i, active) in order {
            let sec = &sections[i];
            let on = self.selected_section == i;
            let tcol = if !active { p.text_muted } else { p.text };
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
                .child(sec.title.clone())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.selected_section != i {
                        this.selected_section = i;
                        cx.notify();
                    }
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
