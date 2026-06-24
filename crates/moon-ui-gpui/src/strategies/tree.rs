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
        order: &Arc<Vec<Key>>,
        built: &mut Vec<Key>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        let accent = moon(p.blue);
        let border = moon(p.border);

        // Поиск временно раскрывает всё; своё состояние раскрытия не трогаем.
        let force_open = self.filter.searching();

        // Узлы ядер → дерево.
        let mut list = v_flex().w_full().gap_0();
        for (core_id, core_name) in cores {
            let Some(cd) = store.core(*core_id) else {
                continue;
            };
            if cd.strategies.is_empty() || !cd.strategies.iter().any(|r| self.filter.matches(r)) {
                continue;
            }
            let open = force_open || self.expanded_cores.contains(core_id);
            let total = cd
                .strategies
                .iter()
                .filter(|r| self.filter.counts(r))
                .count();
            let active = cd
                .strategies
                .iter()
                .filter(|r| self.filter.counts(r) && r.checked)
                .count();
            let label = format!(
                "{}  {}  {}/{}",
                if open { "▼" } else { "▶" },
                core_name,
                active,
                total
            );
            let cid = *core_id;
            let core_dnd_bg = moon_alpha(p.panel, 0.85);
            list = list.child(
                div()
                    .id(SharedString::from(format!("core-{cid}")))
                    .w_full()
                    .h(design::fit_h_px(cx, 24.0, 14.0, 5.0))
                    .px(design::ui_px(cx, 6.0))
                    .rounded(design::ui_px(cx, 3.0))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(accent)
                    .hover(move |s| s.bg(moon_alpha(p.panel, 0.78)))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        toggle(&mut this.expanded_cores, cid);
                        cx.notify();
                    }))
                    // DnD: сброс на ядро = перенос/копирование в КОРЕНЬ этого ядра.
                    .drag_over::<super::tree_ui::StratDrag>(move |s, _, _, _| s.bg(core_dnd_bg))
                    .drag_over::<super::tree_ui::FolderDrag>(move |s, _, _, _| s.bg(core_dnd_bg))
                    .on_drop(
                        cx.listener(move |this, drag: &super::tree_ui::StratDrag, _w, cx| {
                            this.drop_strategies(cid, Vec::new(), drag, cx);
                        }),
                    )
                    .on_drop(cx.listener(
                        move |this, drag: &super::tree_ui::FolderDrag, _w, cx| {
                            this.drop_folder(cid, Vec::new(), drag, cx);
                        },
                    )),
            );
            if !open {
                continue;
            }
            // Вложенное дерево папок (отступ слева — как egui ui.indent).
            let mut root = build_node(cd.strategies.iter().filter(|r| self.filter.matches(r)));
            // Подмешиваем пустые UI-папки (созданные, ещё без стратегий).
            for parts in self.ui_folder_paths(*core_id) {
                ensure_folder(&mut root, &parts);
            }
            let mut prefix: Vec<String> = Vec::new();
            let mut kids: Vec<AnyElement> = Vec::new();
            self.render_node(
                &root,
                &cd.strategies,
                *core_id,
                &mut prefix,
                force_open,
                order,
                built,
                &mut kids,
                cx,
            );
            let mut body = v_flex().w_full().pl_3().gap_0();
            for k in kids {
                body = body.child(k);
            }
            list = list.child(body);
        }

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
            .w(px(380.0))
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
            // ── Прокручиваемый список ──
            .child(
                div()
                    .id("strat-tree-scroll")
                    .flex_1()
                    .w_full()
                    .overflow_y_scroll()
                    .p(px(8.0))
                    .child(list),
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

    /// Рекурсивно собирает элементы узла: подпапки (сворачиваемые, с активн./всего),
    /// затем стратегии прямо в этой папке.
    #[allow(clippy::too_many_arguments)]
    fn render_node(
        &self,
        node: &FolderNode,
        strategies: &[StrategyRow],
        core_id: CoreId,
        prefix: &mut Vec<String>,
        force_open: bool,
        order: &Arc<Vec<Key>>,
        built: &mut Vec<Key>,
        out: &mut Vec<AnyElement>,
        cx: &Context<Self>,
    ) {
        let p = MoonPalette::active(cx);
        for (name, child) in &node.children {
            prefix.push(name.clone());
            let path_key = prefix.join("/");
            let fkey = (core_id, path_key.clone());
            let fopen = force_open || self.expanded_folders.contains(&fkey);
            let (active, total) = folder_counts(strategies, &self.filter, prefix);
            let flabel = format!(
                "{}  {name}  {active}/{total}",
                if fopen { "▼" } else { "▶" }
            );
            let fkey_click = fkey.clone();
            let menu_path = prefix.clone();
            let drag_path = prefix.clone();
            let strat_drop = prefix.clone();
            let folder_drop = prefix.clone();
            let fname: SharedString = name.clone().into();
            let dnd_bg = moon_alpha(p.blue, 0.18);
            out.push(
                div()
                    .id(SharedString::from(format!("folder-{core_id}-{path_key}")))
                    .w_full()
                    .h(design::fit_h_px(cx, 23.0, 14.0, 4.5))
                    .px(design::ui_px(cx, 6.0))
                    .rounded(design::ui_px(cx, 3.0))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .text_color(moon(p.text_soft))
                    .hover(move |s| s.bg(moon_alpha(p.panel, 0.70)))
                    .child(flabel)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        toggle(&mut this.expanded_folders, fkey_click.clone());
                        cx.notify();
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.open_menu(
                                super::tree_ui::ContextMenu {
                                    core: core_id,
                                    target: super::tree_ui::MenuTarget::Folder(menu_path.clone()),
                                    pos: e.position,
                                },
                                window,
                                cx,
                            );
                        }),
                    )
                    // DnD: папка — источник перетаскивания и цель сброса (стратегий и папок).
                    .on_drag(
                        super::tree_ui::FolderDrag {
                            core: core_id,
                            path: drag_path,
                        },
                        move |_d, _p, _w, cx| {
                            cx.new(|_| super::tree_ui::DragChip {
                                label: fname.clone(),
                            })
                        },
                    )
                    .drag_over::<super::tree_ui::StratDrag>(move |s, _, _, _| s.bg(dnd_bg))
                    .drag_over::<super::tree_ui::FolderDrag>(move |s, _, _, _| s.bg(dnd_bg))
                    .on_drop(
                        cx.listener(move |this, drag: &super::tree_ui::StratDrag, _w, cx| {
                            this.drop_strategies(core_id, strat_drop.clone(), drag, cx);
                        }),
                    )
                    .on_drop(
                        cx.listener(move |this, drag: &super::tree_ui::FolderDrag, _w, cx| {
                            this.drop_folder(core_id, folder_drop.clone(), drag, cx);
                        }),
                    )
                    .into_any_element(),
            );
            if fopen {
                let mut kids: Vec<AnyElement> = Vec::new();
                self.render_node(
                    child, strategies, core_id, prefix, force_open, order, built, &mut kids, cx,
                );
                let mut body = v_flex().w_full().pl_3().gap_0();
                for k in kids {
                    body = body.child(k);
                }
                out.push(body.into_any_element());
            }
            prefix.pop();
        }
        for r in &node.strategies {
            out.push(self.strategy_row(core_id, r, order, built, cx));
        }
    }

    /// Одна строка стратегии: чекбокс (стейджинг) · индикатор запуска · имя (выбор).
    fn strategy_row(
        &self,
        core: CoreId,
        r: &StrategyRow,
        order: &Arc<Vec<Key>>,
        built: &mut Vec<Key>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let key = (core, r.id);
        built.push(key);
        let server = r.checked;
        let val = self.staged.get(&key).copied().unwrap_or(server);

        // Подсветка — для всех выбранных (мультивыбор), иначе для первичной.
        let p = MoonPalette::active(cx);
        let highlighted = if self.sel.is_empty() {
            self.selected == Some(key)
        } else {
            self.sel.contains(&key)
        };
        let dot = if server { p.green } else { p.text_muted };
        let type_col = if r.is_short { p.orange } else { p.text_muted };

        let order_c = order.clone();
        // Источник DnD: тащим весь мультивыбор этого ядра (если строка в выборе) или одну.
        let drag_ids = self.drag_ids_for(core, r.id);
        let drag_label: SharedString = if drag_ids.len() > 1 {
            t!("strat.count_strategies", n = drag_ids.len())
                .to_string()
                .into()
        } else {
            r.name.clone().into()
        };
        let mut name_row = div()
            .id(SharedString::from(format!("strat-{core}-{}", r.id)))
            .flex_1()
            .min_w_0()
            .h(design::fit_h_px(cx, 23.0, 14.0, 4.5))
            // Центрируем содержимое по вертикали (иначе текст прижат к верхнему краю строки).
            .flex()
            .items_center()
            .px(design::ui_px(cx, 6.0))
            .rounded(design::ui_px(cx, 3.0))
            .border_1()
            .border_color(moon_alpha(p.border, 0.0))
            .cursor_pointer()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(moon(p.text))
                            .child(r.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(design::t_body(cx))
                            .text_color(moon(type_col))
                            .child(r.kind.clone()),
                    ),
            )
            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                // Фокус на окно стратегий — чтобы Ctrl+C/V/Delete доходили до on_key_down.
                window.focus(&this.focus, cx);
                let m = e.modifiers();
                if this.apply_click(key, &order_c, m.shift, m.secondary()) {
                    this.clamp_selected_section(cx);
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    // ПКМ по невыбранной — выбрать только её (как в проводнике).
                    if !this.sel.contains(&key) {
                        this.sel.clear();
                        this.sel.insert(key);
                        this.selected = Some(key);
                        this.clamp_selected_section(cx);
                    }
                    this.open_menu(
                        super::tree_ui::ContextMenu {
                            core,
                            target: super::tree_ui::MenuTarget::Strategy(key.1),
                            pos: e.position,
                        },
                        window,
                        cx,
                    );
                }),
            )
            .on_drag(
                super::tree_ui::StratDrag {
                    core,
                    ids: drag_ids,
                },
                move |_d, _pos, _w, cx| {
                    cx.new(|_| super::tree_ui::DragChip {
                        label: drag_label.clone(),
                    })
                },
            );
        if highlighted {
            name_row = name_row
                .bg(moon_alpha(p.amber, 0.16))
                .border_color(moon_alpha(p.amber, 0.55));
        } else {
            name_row = name_row.hover(move |s| s.bg(moon_alpha(p.panel, 0.74)));
        }

        h_flex()
            .w_full()
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .py(design::ui_px(cx, 1.0))
            .child(
                MoonCheckbox::new(SharedString::from(format!("chk-{core}-{}", r.id)))
                    .checked(val)
                    .size(MoonCheckboxSize::Compact)
                    .on_change(cx.listener(move |this, ch: &bool, _, cx| {
                        let v = *ch;
                        let before = this.staged.get(&key).copied();
                        if v == server {
                            this.staged.remove(&key);
                        } else {
                            this.staged.insert(key, v);
                        }
                        if before != this.staged.get(&key).copied() {
                            cx.notify();
                        }
                    })),
            )
            .child(div().text_color(moon(dot)).child("●"))
            .child(name_row)
            .into_any_element()
    }

    // ── Панель 2: разделы (секции) ────────────────────────────────────────────
}
