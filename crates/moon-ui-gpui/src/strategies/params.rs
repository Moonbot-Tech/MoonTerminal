//! Правая панель окна «Стратегии»: модель панели параметров и её рендер — плашки/
//! редакторы значений выбранной стратегии (read-only YES/NO, инпут/мемо, помощник
//! формул) + попап полного значения. Методы — `impl StrategiesView` (в [`super`]).

use super::*;
use rust_i18n::t;

pub(super) enum ParamsPanelModel {
    NoSelection,
    NoSchema,
    Content {
        section: SchemaSection,
        values: Values,
        row_pairs: Vec<(Key, StrategyRow)>,
        multi: bool,
        common: Option<HashSet<String>>,
        differ: bool,
    },
}

impl StrategiesView {
    pub(super) fn params_model(&self, store: &CoreStore) -> ParamsPanelModel {
        if selected_row(self, store).is_none() {
            return ParamsPanelModel::NoSelection;
        }
        let Some(sections) = selected_sections(self, store) else {
            return ParamsPanelModel::NoSchema;
        };
        let Some(section) = sections.get(self.selected_section).cloned() else {
            return ParamsPanelModel::NoSchema;
        };
        let values = selected_values(self, store);
        let row_pairs: Vec<(Key, StrategyRow)> = multi_row_pairs(self, store)
            .into_iter()
            .map(|(key, row)| (key, row.clone()))
            .collect();
        let multi = row_pairs.len() > 1;
        let common = common_fields(self, store);
        let differ = kinds_differ(self, store);
        ParamsPanelModel::Content {
            section,
            values,
            row_pairs,
            multi,
            common,
            differ,
        }
    }

    pub(super) fn params_panel(
        &mut self,
        model: ParamsPanelModel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        let mut col = v_flex()
            .flex_1()
            .h_full()
            .min_w(px(420.0))
            .px(design::ui_px(cx, 24.0))
            .py(design::ui_px(cx, 18.0))
            .gap(design::ui_px(cx, 10.0))
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .line_height(design::line_px(cx, 14.0));

        let ParamsPanelModel::Content {
            section,
            values,
            row_pairs,
            multi,
            common,
            differ,
        } = model
        else {
            let text = match model {
                ParamsPanelModel::NoSelection => t!("strat.no_selection").to_string(),
                ParamsPanelModel::NoSchema => t!("strat.no_schema").to_string(),
                ParamsPanelModel::Content { .. } => unreachable!(),
            };
            return col
                .child(div().mt_2().text_color(moon(p.text_muted)).child(text))
                .into_any_element();
        };
        let keys: Vec<Key> = row_pairs.iter().map(|(key, _)| *key).collect();

        // Заголовок раздела + счётчик (полей / выбрано) справа.
        let count = if multi {
            t!("strat.selected_count", n = row_pairs.len()).to_string()
        } else {
            t!("strat.fields_count", n = section.fields.len()).to_string()
        };
        let dirty = self.field_edits.len();
        let mut header = h_flex()
            .w_full()
            .h(design::fit_h_px(cx, 28.0, 14.0, 7.0))
            .items_center()
            .justify_between()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(moon(p.text))
                    .child(section.title.clone()),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(design::t_body(cx))
                            .text_color(moon(p.text_muted))
                            .child(count),
                    )
                    .when(dirty > 0, |row| {
                        row.child(
                            MoonButton::new("strat-fields-apply")
                                .success()
                                .size(MoonButtonSize::Micro)
                                .label(format!("apply {dirty}"))
                                .on_click(cx.listener(|this, _, _, cx| this.apply_field_edits(cx)))
                                .render(),
                        )
                        .child(
                            MoonButton::new("strat-fields-revert")
                                .ghost()
                                .size(MoonButtonSize::Micro)
                                .label("revert")
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.discard_field_edits(cx)),
                                )
                                .render(),
                        )
                    }),
            );
        if dirty > 0 {
            header = header
                .border_l_2()
                .border_color(moon_alpha(p.amber, 0.72))
                .pl_2();
        }
        col = col
            .child(header)
            .child(
                MoonCheckbox::new("params-only-active")
                    .label(t!("strat.only_active").to_string())
                    .checked(self.only_active_params)
                    .size(MoonCheckboxSize::Compact)
                    .on_change(cx.listener(|this, ch: &bool, _, cx| {
                        if this.only_active_params != *ch {
                            this.only_active_params = *ch;
                            cx.notify();
                        }
                    })),
            )
            .child(div().w_full().h(px(1.0)).bg(moon(p.border)));

        // Порядок полей — как в схеме. Значения берём из снимка по имени.
        let mut list = v_flex().w_full().gap(design::ui_px(cx, 2.0));
        for f in &section.fields {
            let lname = f.name.to_lowercase();
            if multi && lname == "strategyname" {
                continue;
            }
            if let Some(c) = &common {
                if !c.contains(&lname) {
                    continue;
                }
            }
            if differ && lname == "signaltype" {
                continue;
            }
            let active = self.rules.field_active(&f.name, &values);
            if self.only_active_params && !active {
                continue;
            }
            let merged = merged_value_for_owned(self, &row_pairs, f);
            list = list.child(self.field_row(f, &keys, merged, active, window, cx));
        }
        let scroll = div()
            .id("strat-params-scroll")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .child(list);
        let mut body = h_flex()
            .flex_1()
            .w_full()
            .min_h_0()
            .items_start()
            .gap_2()
            .child(scroll);
        if let Some(helper) = self.formula_helper(cx) {
            body = body.child(helper);
        }
        col = col.child(body);
        col.into_any_element()
    }

    /// Строка поля: имя слева, значение справа. `active=false` — приглушаем тёмным.
    /// `merged=None` — значения у выбранных различаются (помечаем «≠», без значения).
    fn field_row(
        &mut self,
        f: &SchemaField,
        keys: &[Key],
        merged: Option<String>,
        active: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        let name_col = if active { p.text_soft } else { p.text_muted };
        let val_col = if active { p.text } else { p.text_muted };

        let dirty = keys
            .iter()
            .any(|(core, id)| self.field_edits.contains_key(&(*core, *id, f.name.clone())));
        let field_name = f.name.clone();
        let row_id = editor_state_id(keys, &field_name);
        let view = cx.entity();

        // `merged == None` → у выбранных стратегий значение РАЗНОЕ. Раньше показывали лишь «≠»
        // без правки. Теперь поле всё равно редактируемое: значок «≠» + подсветка, а ввод любого
        // значения через `stage_field_value` ложится сразу во ВСЕ выбранные ключи (унифицирует).
        let differ = merged.is_none();
        let value = merged.unwrap_or_default();
        let control: AnyElement = match f.ui {
            SchemaFieldUi::Checkbox => {
                let on = is_on(&value);
                let keys = keys.to_vec();
                let field = field_name.clone();
                MoonCheckbox::new(SharedString::from(format!("field-check-{row_id}")))
                    .checked(on)
                    .indeterminate(differ)
                    .disabled(!active)
                    .size(MoonCheckboxSize::Compact)
                    .on_change(cx.listener(move |this, ch: &bool, _, cx| {
                        this.stage_field_value(
                            &keys,
                            &field,
                            if *ch { "Yes" } else { "No" }.to_string(),
                            cx,
                        );
                    }))
                    .into_any_element()
            }
            SchemaFieldUi::Combo if !f.picklist.is_empty() => {
                let mut items = Vec::with_capacity(f.picklist.len());
                for option in &f.picklist {
                    let option_value = option.clone();
                    let label = if option.is_empty() {
                        "—".to_string()
                    } else {
                        option.clone()
                    };
                    let keys = keys.to_vec();
                    let field = field_name.clone();
                    let view = view.clone();
                    items.push(
                        MoonMenuItem::with_key(format!("field-{row_id}-{option}"), label)
                            .selected(!differ && option_value == value)
                            .on_click(move |_, _, app| {
                                view.update(app, |this, cx| {
                                    this.stage_field_value(&keys, &field, option_value.clone(), cx);
                                });
                            }),
                    );
                }
                let trigger_label = if differ {
                    "≠ ▾".to_string()
                } else {
                    format!(
                        "{} ▾",
                        if value.is_empty() {
                            "—".to_string()
                        } else {
                            value.clone()
                        }
                    )
                };
                MoonDropdown::new(SharedString::from(format!("field-combo-{row_id}")))
                    .label(trigger_label)
                    .trigger_variant(if dirty || differ {
                        MoonButtonVariant::Amber
                    } else {
                        MoonButtonVariant::Soft
                    })
                    .trigger_size(MoonButtonSize::Action)
                    .trigger_width(180.0)
                    .menu_width(220.0)
                    .menu_size(MoonMenuSize::Compact)
                    .menu_max_height(220.0)
                    .disabled(!active)
                    .items(items)
                    .into_any_element()
            }
            _ => {
                let keys_arc = Arc::new(keys.to_vec());
                // Разные значения рисуем как ПУСТОЙ инпут с плейсхолдером (не memo): набранное
                // применится ко всем выбранным сразу.
                if !differ && is_memo_field(f, &value) {
                    let state = self.field_memo_state(
                        row_id.clone(),
                        value,
                        keys_arc,
                        field_name.clone(),
                        window,
                        cx,
                    );
                    MoonTextArea::new(SharedString::from(format!("field-memo-{row_id}")))
                        .state(&state)
                        .formula()
                        .tone(MoonTone::Warning)
                        .selected(dirty)
                        .disabled(!active)
                        .into_any_element()
                } else {
                    let state = self.field_input_state(
                        row_id.clone(),
                        value,
                        keys_arc,
                        field_name.clone(),
                        window,
                        cx,
                    );
                    let mut input =
                        MoonInput::new(SharedString::from(format!("field-input-{row_id}")))
                            .state(&state)
                            .small()
                            .tone(if differ || matches!(f.ui, SchemaFieldUi::Color) {
                                MoonTone::Warning
                            } else {
                                MoonTone::Info
                            })
                            .selected(dirty || differ)
                            .disabled(!active);
                    if differ {
                        input = input.placeholder(t!("strat.mixed_values").to_string());
                    }
                    input.into_any_element()
                }
            }
        };
        // Значок «≠» перед редактируемым контролом, когда значения различаются.
        let value_el: AnyElement = if differ {
            h_flex()
                .items_center()
                .gap_1()
                .w_full()
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(moon(p.blue))
                        .child("≠"),
                )
                .child(control)
                .into_any_element()
        } else {
            control
        };

        let field_for_focus = field_name.clone();
        h_flex()
            .id(SharedString::from(format!("field-row-{row_id}")))
            .w_full()
            .items_start()
            .gap(design::ui_px(cx, 14.0))
            .min_h(design::fit_h_px(cx, 30.0, 14.0, 8.0))
            .py(design::ui_px(cx, 4.0))
            .border_l(px(2.0))
            .border_color(moon_alpha(p.amber, if dirty { 0.72 } else { 0.0 }))
            .pl(px(8.0))
            .pr_2()
            .rounded(design::ui_px(cx, 3.0))
            .when(dirty, |s| s.bg(moon_alpha(p.amber, 0.06)))
            .hover(move |s| s.bg(moon_alpha(p.panel, 0.46)))
            .child(
                div()
                    .w(px(180.0))
                    .flex_none()
                    .pt(px(5.0))
                    .truncate()
                    .text_color(moon(name_col))
                    .child(f.name.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    // Клип: значение НЕ должно вылезать за свою ячейку и просвечивать на
                    // соседние поля (memo с длинным текстом раньше перекрывал строки ниже).
                    .overflow_hidden()
                    .text_color(moon(val_col))
                    .child(value_el),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.focused_field = Some(field_for_focus.clone());
                cx.notify();
            }))
            .into_any_element()
    }

    fn formula_helper(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let field = self.focused_field.clone()?;
        if !is_formula_field(&field) {
            return None;
        }
        let p = MoonPalette::active(cx);
        let snippets = formula_snippets();
        let mut list = v_flex().w_full().gap_1();
        for (label, detail, insert) in snippets {
            let field = field.clone();
            list = list.child(
                v_flex()
                    .id(SharedString::from(format!("helper-{label}")))
                    .w_full()
                    .rounded(design::ui_px(cx, 4.0))
                    .border_1()
                    .border_color(moon(p.border))
                    .bg(moon(p.panel))
                    .px(design::ui_px(cx, 8.0))
                    .py(design::ui_px(cx, 6.0))
                    .cursor_pointer()
                    .hover(move |s| s.border_color(moon_alpha(p.amber, 0.72)))
                    .child(
                        div()
                            .font_family(design::mono())
                            .text_size(design::t_body(cx))
                            .text_color(moon(p.text))
                            .child(label),
                    )
                    .child(
                        div()
                            .font_family(design::mono())
                            .text_size(design::t_body(cx))
                            .text_color(moon(p.text_muted))
                            .child(detail),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.append_formula_snippet(&field, insert, cx);
                    })),
            );
        }
        Some(
            v_flex()
                .w(px(280.0))
                .h_full()
                .flex_none()
                .gap(design::ui_px(cx, 10.0))
                .px(design::ui_px(cx, 16.0))
                .py(design::ui_px(cx, 14.0))
                .bg(moon(p.shell_high))
                .border_l_1()
                .border_color(moon(p.border))
                .child(
                    div()
                        .text_size(design::t_body(cx))
                        .text_color(moon(p.text_muted))
                        .child(format!("{field} · formula helper")),
                )
                .child(list)
                .into_any_element(),
        )
    }
}
