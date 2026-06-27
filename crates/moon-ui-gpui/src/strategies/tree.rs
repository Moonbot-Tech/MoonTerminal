//! Левая панель окна «Стратегии»: дерево ядро→папка→стратегия с поиском/фильтрами
//! (вид/L/S), чекбоксами-стейджингом и кнопками старт/стоп («Применить»). Это методы
//! `impl StrategiesView`; состояние и чистые помощники — в [`super`]/[`super::logic`].

use super::*;
use rust_i18n::t;

impl StrategiesView {
    pub(super) fn tree_panel(
        &self,
        store: &CoreStore,
        cores: &[(CoreId, String)],
        node_data: std::rc::Rc<std::collections::HashMap<SharedString, super::tree_moon::NodeData>>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        let border = moon(p.border);

        // Само дерево (MoonTree, headless) — флэттинг/виртуализация/DnD внутри компонента.
        // Адаптер `CoreStore → MoonTreeItem` + строки/DnD/меню живут в [`super::tree_moon`].
        let tree_el = self.moon_tree_el(node_data, cx);

        // Поиск + фильтр вида + фильтр направления.
        let kinds = kinds_present(cores, store);
        let kind_text = self
            .filter
            .kind
            .and_then(|k| kinds.iter().find(|(o, _)| *o == k))
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| t!("strat.all_kinds").to_string());
        let dir_text = match self.filter.dir {
            None => t!("strat.all_dirs").to_string(),
            Some(true) => "SHORT".to_string(),
            Some(false) => "LONG".to_string(),
        };

        let collapsed = self.expanded_cores.is_empty() && self.expanded_folders.is_empty();
        let cores_owned: Arc<Vec<(CoreId, String)>> = Arc::new(cores.to_vec());

        v_flex()
            // +10% к 380 — чтобы справа влезала плашка «вид(N)» с числом ордеров.
            .w(px(418.0))
            .h_full()
            .bg(moon(p.shell_high))
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .line_height(design::line_px(cx, 14.0))
            .border_r_1()
            .border_color(border)
            // ── Фильтры сверху ──
            .child(
                v_flex()
                    .w_full()
                    .px(design::ui_px(cx, 10.0))
                    .py(design::ui_px(cx, 10.0))
                    .gap(design::ui_px(cx, 7.0))
                    .child(
                        div().w_full().child(
                            MoonInput::new("strat-search")
                                .state(&self.search)
                                .small()
                                .cleanable(true),
                        ),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .gap(design::ui_px(cx, 7.0))
                            .items_center()
                            .child(self.combo_kind(kind_text, kinds, cx))
                            .child(self.combo_dir(dir_text, cx))
                            .child({
                                let (cc, ct) = self.default_target(store, cores);
                                self.create_dropdown(cc, ct, cx)
                            }),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .child(
                                MoonCheckbox::new("flt-active")
                                    .label(t!("strat.only_active").to_string())
                                    .checked(self.filter.only_active)
                                    .size(MoonCheckboxSize::Compact)
                                    .on_change(cx.listener(|this, ch: &bool, _, cx| {
                                        if this.filter.only_active != *ch {
                                            this.filter.only_active = *ch;
                                            cx.notify();
                                        }
                                    })),
                            )
                            .child(
                                MoonButton::new("expand-all")
                                    .ghost()
                                    .size(MoonButtonSize::Micro)
                                    .label(if collapsed { "▼" } else { "▲" })
                                    .on_click({
                                        let cores = cores_owned.clone();
                                        cx.listener(move |this, _, _, cx| {
                                            let store = this.backend.read(cx).session.store();
                                            let coll = this.expanded_cores.is_empty()
                                                && this.expanded_folders.is_empty();
                                            // store borrow tied to cx; clone cores for &-call.
                                            let cores_v = cores.as_ref().clone();
                                            this.expand_collapse_toggle(&cores_v, store, coll);
                                            cx.notify();
                                        })
                                    })
                                    .render(),
                            ),
                    ),
            )
            .child(div().w_full().h(px(1.0)).bg(border))
            // ── Дерево (MoonTree сам виртуализирует/скроллит) ──
            .child(
                div()
                    .id("strat-tree-scroll")
                    .flex_1()
                    .w_full()
                    .min_h_0()
                    .p(px(8.0))
                    .child(tree_el),
            )
            // ── Нижняя панель действий ──
            .child(div().w_full().h(px(1.0)).bg(border))
            .child(self.action_bar(cores_owned, store, cx))
            .into_any_element()
    }

    /// Комбобокс фильтра вида (попап-список: «все типы» + присутствующие виды).
    fn combo_kind(
        &self,
        current: String,
        kinds: Vec<(u8, String)>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let view = cx.entity();
        let selected_kind = self.filter.kind;
        let mut items = vec![
            MoonMenuItem::with_key("kind-all", t!("strat.all_kinds").to_string())
                .selected(selected_kind.is_none())
                .on_click({
                    let view = view.clone();
                    move |_, _, app| {
                        view.update(app, |this, c| {
                            if this.filter.kind.is_some() {
                                this.filter.kind = None;
                                c.notify();
                            }
                        });
                    }
                }),
        ];
        for (ord, name) in kinds {
            let view = view.clone();
            items.push(
                MoonMenuItem::with_key(format!("kind-{ord}"), name.clone())
                    .selected(selected_kind == Some(ord))
                    .on_click({
                        let name_ord = ord;
                        move |_, _, app| {
                            view.update(app, |this, c| {
                                if this.filter.kind != Some(name_ord) {
                                    this.filter.kind = Some(name_ord);
                                    c.notify();
                                }
                            });
                        }
                    }),
            );
        }
        MoonDropdown::new("strat-kind-filter")
            .label(format!("{current} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(116.0)
            .menu_width(180.0)
            .menu_size(MoonMenuSize::Compact)
            .menu_max_height(240.0)
            .items(items)
            .into_any_element()
    }

    /// Комбобокс фильтра направления (все/LONG/SHORT).
    fn combo_dir(&self, current: String, cx: &Context<Self>) -> AnyElement {
        let view = cx.entity();
        let opts: [(&str, String, Option<bool>); 3] = [
            ("all", t!("strat.all_dirs").to_string(), None),
            ("LONG", "LONG".to_string(), Some(false)),
            ("SHORT", "SHORT".to_string(), Some(true)),
        ];
        let mut items = Vec::with_capacity(opts.len());
        for (id, label, val) in opts {
            let view = view.clone();
            items.push(
                MoonMenuItem::with_key(format!("dir-{id}"), label)
                    .selected(self.filter.dir == val)
                    .on_click(move |_, _, app| {
                        view.update(app, |this, c| {
                            if this.filter.dir != val {
                                this.filter.dir = val;
                                c.notify();
                            }
                        });
                    }),
            );
        }
        MoonDropdown::new("strat-dir-filter")
            .label(format!("{current} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(80.0)
            .menu_width(120.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
            .into_any_element()
    }

    /// Нижняя панель действий: СЛЕВА группа выделения (копировать/вставить, под ними —
    /// удалить во всю ширину), СПРАВА старт/стоп отмеченных стопкой. Счётчик стейджинга — по центру.
    fn action_bar(
        &self,
        cores: Arc<Vec<(CoreId, String)>>,
        store: &CoreStore,
        cx: &Context<Self>,
    ) -> AnyElement {
        let cs = cores.clone();
        // Правая группа: старт/стоп друг под другом, прижата вправо.
        let right = v_flex()
            .gap_1()
            .items_end()
            .child(
                MoonButton::new("start-checked")
                    .primary()
                    .size(MoonButtonSize::Micro)
                    .label(format!("▶ {}", t!("strat.start_checked")))
                    .on_click({
                        let cs = cs.clone();
                        cx.listener(move |this, _, _, cx| {
                            let cores_v = cs.as_ref().clone();
                            this.apply_start_stop(&cores_v, true, cx);
                        })
                    })
                    .render(),
            )
            .child(
                MoonButton::new("stop-checked")
                    .outline()
                    .size(MoonButtonSize::Micro)
                    .label(format!("■ {}", t!("strat.stop_checked")))
                    .on_click({
                        let cs = cs.clone();
                        cx.listener(move |this, _, _, cx| {
                            let cores_v = cs.as_ref().clone();
                            this.apply_start_stop(&cores_v, false, cx);
                        })
                    })
                    .render(),
            );

        let mut bar = h_flex()
            .w_full()
            .p_2()
            .gap_2()
            .items_start()
            .justify_between()
            .child(self.selection_toolbar(store, cx));
        if !self.staged.is_empty() {
            bar = bar.child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(MoonPalette::active(cx).amber))
                    .child(t!("strat.staged", n = self.staged.len()).to_string()),
            );
        }
        bar.child(right).into_any_element()
    }

    // ── Панель 2: разделы (секции) ────────────────────────────────────────────
}
