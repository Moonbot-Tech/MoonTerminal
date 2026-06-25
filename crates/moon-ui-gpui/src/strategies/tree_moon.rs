//! Дерево стратегий на компоненте `MoonTree` (форк MoonUI, headless `Tree::custom`).
//! Заменяет ручной флэттинг/виртуализацию/hitbox-DnD прежней `tree.rs`: MoonTree даёт
//! уплощение по `expanded_ids`, виртуальный список, клавиатуру и row-hitbox под декораторы
//! DnD. Выбор/стейджинг/раскрытие остаются в полях `StrategiesView` — здесь только адаптер
//! `CoreStore → MoonTreeItem` + side-map `id → NodeData` (данные строки и drag-нагрузка) и
//! рендер строк/декораторов. Мутации идут через `Entity::update` (callbacks вне `Context<Self>`).

use std::collections::HashMap;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBadge, MoonBadgeSize, MoonBadgeVariant, MoonCheckbox, MoonCheckboxSize, MoonPalette,
    MoonText, MoonTone, MoonTree, MoonTreeEntry, MoonTreeItem, MoonTreeRowMeta, h_flex,
};

use super::logic::{build_node, ensure_folder, folder_counts, toggle};
use super::tree_ui::{ContextMenu, DragChip, FolderDrag, MenuTarget, StratDrag};
use super::{Key, StrategiesView, moon_alpha};
use crate::design;
use moon_core::feed::StrategyRow;
use moon_core::session::{CoreId, CoreStore};

// ── id-кодировка узлов (стабильные строковые id для MoonTree) ────────────────
fn id_core(core: CoreId) -> SharedString {
    SharedString::from(format!("c:{core}"))
}
fn id_folder(core: CoreId, path: &str) -> SharedString {
    SharedString::from(format!("f:{core}:{path}"))
}
fn id_strat(core: CoreId, id: u64) -> SharedString {
    SharedString::from(format!("s:{core}:{id}"))
}

/// Данные одной строки дерева (берёт `render_row`/декораторы по id узла).
pub(super) enum NodeData {
    Core {
        core: CoreId,
        label: String,
        active: usize,
        total: usize,
        open_orders: usize,
    },
    Folder {
        core: CoreId,
        path: Vec<String>,
        label: String,
        active: usize,
        total: usize,
    },
    Strategy {
        core: CoreId,
        id: u64,
        name: String,
        kind: String,
        open_orders: usize,
        server_checked: bool,
        staged: Option<bool>,
        highlighted: bool,
        is_short: bool,
        drag_ids: Vec<u64>,
    },
}

/// Результат адаптера: элементы дерева + side-map + раскрытые id + видимый плоский порядок.
pub(super) struct MoonTreeBuild {
    pub(super) items: Vec<MoonTreeItem>,
    pub(super) node_data: HashMap<SharedString, NodeData>,
    pub(super) expanded_ids: Vec<SharedString>,
    pub(super) flat: Vec<Key>,
    pub(super) searching: bool,
}

/// Построить дерево MoonTree из стора (owned — без заимствований стора наружу).
pub(super) fn build(
    view: &StrategiesView,
    store: &CoreStore,
    cores: &[(CoreId, String)],
) -> MoonTreeBuild {
    let filter = &view.filter;
    let searching = filter.searching();
    let mut items = Vec::new();
    let mut data: HashMap<SharedString, NodeData> = HashMap::new();
    let mut expanded: Vec<SharedString> = Vec::new();
    let mut flat: Vec<Key> = Vec::new();

    for (core_id, core_name) in cores {
        let core = *core_id;
        let Some(cd) = store.core(core) else { continue };
        if cd.strategies.is_empty() || !cd.strategies.iter().any(|r| filter.matches(r)) {
            continue;
        }
        let core_open = searching || view.expanded_cores.contains(&core);
        let total = cd.strategies.iter().filter(|r| filter.counts(r)).count();
        let active = cd
            .strategies
            .iter()
            .filter(|r| filter.counts(r) && r.checked)
            .count();
        let open_orders_total = cd.orders.iter().filter(|o| !o.job_is_done).count();
        // открытые ордера по стратегиям ядра (strat_id → кол-во)
        let order_counts: HashMap<u64, usize> = {
            let mut m = HashMap::new();
            for o in cd.orders.iter().filter(|o| !o.job_is_done) {
                *m.entry(o.strat_id).or_insert(0) += 1;
            }
            m
        };

        let cid = id_core(core);
        if core_open {
            expanded.push(cid.clone());
        }

        // дерево папок из видимых стратегий + пустые UI-папки
        let mut root = build_node(cd.strategies.iter().filter(|r| filter.matches(r)));
        for parts in view.ui_folder_paths(core) {
            ensure_folder(&mut root, &parts);
        }

        let mut children = Vec::new();
        let mut prefix: Vec<String> = Vec::new();
        convert_node(
            &root,
            core,
            &cd.strategies,
            &order_counts,
            &mut prefix,
            view,
            searching,
            core_open,
            &mut children,
            &mut data,
            &mut flat,
            &mut expanded,
        );

        data.insert(
            cid.clone(),
            NodeData::Core {
                core,
                label: core_name.clone(),
                active,
                total,
                open_orders: open_orders_total,
            },
        );
        items.push(
            MoonTreeItem::new(cid, core_name.clone())
                .folder(true)
                .children(children),
        );
    }

    MoonTreeBuild {
        items,
        node_data: data,
        expanded_ids: expanded,
        flat,
        searching,
    }
}

#[allow(clippy::too_many_arguments)]
fn convert_node(
    node: &super::logic::FolderNode,
    core: CoreId,
    all_strats: &[StrategyRow],
    order_counts: &HashMap<u64, usize>,
    prefix: &mut Vec<String>,
    view: &StrategiesView,
    searching: bool,
    ancestors_visible: bool,
    out: &mut Vec<MoonTreeItem>,
    data: &mut HashMap<SharedString, NodeData>,
    flat: &mut Vec<Key>,
    expanded: &mut Vec<SharedString>,
) {
    for (name, child) in &node.children {
        prefix.push(name.clone());
        let path = prefix.join("/");
        let fid = id_folder(core, &path);
        let fopen = searching || view.expanded_folders.contains(&(core, path.clone()));
        if fopen {
            expanded.push(fid.clone());
        }
        let (active, total) = folder_counts(all_strats, &view.filter, prefix);
        let mut fchildren = Vec::new();
        convert_node(
            child,
            core,
            all_strats,
            order_counts,
            prefix,
            view,
            searching,
            ancestors_visible && fopen,
            &mut fchildren,
            data,
            flat,
            expanded,
        );
        data.insert(
            fid.clone(),
            NodeData::Folder {
                core,
                path: prefix.clone(),
                label: name.clone(),
                active,
                total,
            },
        );
        out.push(
            MoonTreeItem::new(fid, name.clone())
                .folder(true)
                .children(fchildren),
        );
        prefix.pop();
    }

    for r in &node.strategies {
        let key: Key = (core, r.id);
        let sid = id_strat(core, r.id);
        let staged = view.staged.get(&key).copied();
        let highlighted = if view.sel.is_empty() {
            view.selected == Some(key)
        } else {
            view.sel.contains(&key)
        };
        if ancestors_visible {
            flat.push(key);
        }
        data.insert(
            sid.clone(),
            NodeData::Strategy {
                core,
                id: r.id,
                name: r.name.clone(),
                kind: r.kind.clone(),
                open_orders: order_counts.get(&r.id).copied().unwrap_or(0),
                server_checked: r.checked,
                staged,
                highlighted,
                is_short: r.is_short,
                drag_ids: view.drag_ids_for(core, r.id),
            },
        );
        out.push(MoonTreeItem::new(sid, r.name.clone()));
    }
}

impl StrategiesView {
    /// Элемент дерева стратегий на `MoonTree` (headless). `data` — side-map текущего кадра.
    pub(super) fn moon_tree_el(
        &self,
        data: Rc<HashMap<SharedString, NodeData>>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let view = cx.entity();

        // ── рендер строки ──
        let row_data = data.clone();
        let row_view = view.clone();
        let tree = MoonTree::custom(&self.tree_state, move |entry, meta, _window, app| {
            render_row(&row_data, &row_view, entry, meta, app)
        })
        // ── DnD: стратегии ──
        .draggable::<StratDrag, DragChip, _, _>(
            {
                let data = data.clone();
                move |entry, _meta| match data.get(entry.item().id()) {
                    Some(NodeData::Strategy { core, drag_ids, .. }) => Some(StratDrag {
                        core: *core,
                        ids: drag_ids.clone(),
                    }),
                    _ => None,
                }
            },
            |drag: &StratDrag, _pos, _window, app| {
                let n = drag.ids.len();
                app.new(|_| DragChip {
                    label: SharedString::from(if n > 1 {
                        format!("{n}×")
                    } else {
                        "≡".to_string()
                    }),
                })
            },
        )
        // ── DnD: папки ──
        .draggable::<FolderDrag, DragChip, _, _>(
            {
                let data = data.clone();
                move |entry, _meta| match data.get(entry.item().id()) {
                    Some(NodeData::Folder { core, path, label, .. }) => {
                        let _ = label;
                        Some(FolderDrag {
                            core: *core,
                            path: path.clone(),
                        })
                    }
                    _ => None,
                }
            },
            |_drag: &FolderDrag, _pos, _window, app| {
                app.new(|_| DragChip {
                    label: SharedString::from("▣"),
                })
            },
        )
        // ── Цель сброса: ядро/папка. Единый can_drop на ОБА типа (gpui `can_drop` —
        // один слот; два drop_target перетёрли бы друг друга → дроп не срабатывал), плюс
        // drag_over-подсветка и on_drop по типу нагрузки. ──
        .row_decorator({
            let data = data.clone();
            let view = view.clone();
            move |row, entry, _meta, _w, app| {
                let Some((core, target)) = drop_dest(&data, entry) else {
                    return row;
                };
                let p = MoonPalette::active(app);
                let hl = moon_alpha(p.blue, 0.22);
                let (vs, ts) = (view.clone(), target.clone());
                let (vf, tf) = (view.clone(), target.clone());
                row.can_drop(|drag, _w, _a| drag.is::<StratDrag>() || drag.is::<FolderDrag>())
                    .drag_over::<StratDrag>(move |s, _d, _w, _a| s.bg(hl))
                    .drag_over::<FolderDrag>(move |s, _d, _w, _a| s.bg(hl))
                    .on_drop::<StratDrag>(move |drag: &StratDrag, _w, app| {
                        let d = drag.clone();
                        vs.update(app, |this, cx| this.drop_strategies(core, ts.clone(), &d, cx));
                    })
                    .on_drop::<FolderDrag>(move |drag: &FolderDrag, _w, app| {
                        let d = drag.clone();
                        vf.update(app, |this, cx| this.drop_folder(core, tf.clone(), &d, cx));
                    })
            }
        });

        tree.into_any_element()
    }
}

/// Цель сброса = ядро (корень) или папка. Возвращает (целевое ядро, путь).
fn drop_dest(
    data: &HashMap<SharedString, NodeData>,
    entry: &MoonTreeEntry,
) -> Option<(CoreId, Vec<String>)> {
    match data.get(entry.item().id())? {
        NodeData::Core { core, .. } => Some((*core, Vec::new())),
        NodeData::Folder { core, path, .. } => Some((*core, path.clone())),
        NodeData::Strategy { .. } => None,
    }
}

/// Рендер одной строки по `NodeData`.
fn render_row(
    data: &HashMap<SharedString, NodeData>,
    view: &Entity<StrategiesView>,
    entry: &MoonTreeEntry,
    meta: MoonTreeRowMeta,
    app: &mut App,
) -> AnyElement {
    let _ = meta;
    let p = MoonPalette::active(app);
    let depth = entry.depth();
    let indent = design::ui_px(app, 6.0 + 12.0 * depth as f32);
    let Some(node) = data.get(entry.item().id()) else {
        return div().into_any_element();
    };

    match node {
        NodeData::Core {
            core,
            label,
            active,
            total,
            open_orders,
        } => {
            let core = *core;
            let txt = if *open_orders > 0 {
                format!("{label}  {active}/{total}  ({open_orders})")
            } else {
                format!("{label}  {active}/{total}")
            };
            core_folder_row(
                view,
                entry.is_expanded(),
                indent,
                txt,
                p.blue,
                600.0,
                ToggleTarget::Core(core),
                app,
            )
        }
        NodeData::Folder {
            core,
            path,
            label,
            active,
            total,
        } => {
            let core = *core;
            let path = path.clone();
            let txt = format!("{label}  {active}/{total}");
            core_folder_row(
                view,
                entry.is_expanded(),
                indent,
                txt,
                p.text_soft,
                400.0,
                ToggleTarget::Folder(core, path),
                app,
            )
        }
        NodeData::Strategy {
            core,
            id,
            name,
            kind,
            open_orders,
            server_checked,
            staged,
            highlighted,
            is_short,
            ..
        } => strategy_row(
            view,
            *core,
            *id,
            name,
            kind,
            *open_orders,
            *server_checked,
            *staged,
            *highlighted,
            *is_short,
            indent,
            app,
        ),
    }
}

enum ToggleTarget {
    Core(CoreId),
    Folder(CoreId, Vec<String>),
}

#[allow(clippy::too_many_arguments)]
fn core_folder_row(
    view: &Entity<StrategiesView>,
    expanded: bool,
    indent: Pixels,
    text: String,
    color: u32,
    weight: f32,
    target: ToggleTarget,
    app: &App,
) -> AnyElement {
    let p = MoonPalette::active(app);
    let marker = if expanded { "▼" } else { "▶" };
    // ПКМ-меню — только у папок (как в egui): переименовать/копировать/вставить/новая/удалить.
    let menu = match &target {
        ToggleTarget::Folder(c, path) => Some((*c, path.clone())),
        ToggleTarget::Core(_) => None,
    };
    let view_click = view.clone();
    let view_menu = view.clone();
    h_flex()
        .id(SharedString::from(format!("strat-tree-cf-{text}")))
        .w_full()
        .h(design::fit_h_px(app, 23.0, 14.0, 4.5))
        .pl(indent)
        .pr(design::ui_px(app, 6.0))
        .items_center()
        .gap(design::ui_px(app, 5.0))
        .cursor_pointer()
        .rounded(design::ui_px(app, 3.0))
        .hover(move |s| s.bg(moon_alpha(p.panel, 0.74)))
        .child(
            MoonText::new(marker)
                .mono(true)
                .uppercase(false)
                .color(p.text_muted)
                .render(),
        )
        .child(
            MoonText::new(text)
                .mono(true)
                .uppercase(false)
                .color(color)
                .weight(weight)
                .render(),
        )
        .on_click(move |_e, _window, app| {
            view_click.update(app, |this, cx| {
                match &target {
                    ToggleTarget::Core(c) => toggle(&mut this.expanded_cores, *c),
                    ToggleTarget::Folder(c, path) => {
                        toggle(&mut this.expanded_folders, (*c, path.join("/")))
                    }
                }
                cx.notify();
            });
        })
        .when_some(menu, |row, (core, path)| {
            row.on_mouse_down(MouseButton::Right, move |e: &MouseDownEvent, window, app| {
                app.stop_propagation();
                let pos = e.position;
                let path = path.clone();
                view_menu.update(app, |this, cx| {
                    this.open_menu(
                        ContextMenu {
                            core,
                            target: MenuTarget::Folder(path),
                            pos,
                        },
                        window,
                        cx,
                    );
                });
            })
        })
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn strategy_row(
    view: &Entity<StrategiesView>,
    core: CoreId,
    id: u64,
    name: &str,
    kind: &str,
    open_orders: usize,
    server_checked: bool,
    staged: Option<bool>,
    highlighted: bool,
    is_short: bool,
    indent: Pixels,
    app: &App,
) -> AnyElement {
    let p = MoonPalette::active(app);
    let key: Key = (core, id);
    let val = staged.unwrap_or(server_checked);
    let dot = if server_checked { p.green } else { p.text_muted };
    let kind_txt = if open_orders > 0 {
        format!("{kind}({open_orders})")
    } else {
        kind.to_string()
    };

    // имя + вид(N) — кликабельная зона выбора
    let view_click = view.clone();
    let view_menu = view.clone();
    let mut name_row = h_flex()
        .id(SharedString::from(format!("strat-{core}-{id}")))
        .flex_1()
        .min_w_0()
        .h(design::fit_h_px(app, 23.0, 14.0, 4.5))
        .items_center()
        .justify_between()
        .gap(design::ui_px(app, 6.0))
        .px(design::ui_px(app, 6.0))
        .rounded(design::ui_px(app, 3.0))
        .border_1()
        .border_color(moon_alpha(p.border, 0.0))
        .cursor_pointer()
        .child(
            div().flex_1().min_w_0().truncate().child(
                MoonText::new(name.to_string())
                    .mono(true)
                    .uppercase(false)
                    .color(p.text)
                    .render(),
            ),
        )
        .child(
            // Вид-бейдж различает направление: SHORT — оранжевый (`Negative`),
            // LONG — зеленоватый (`Positive`).
            MoonBadge::new(kind_txt)
                .tone(if is_short {
                    MoonTone::Negative
                } else {
                    MoonTone::Positive
                })
                .variant(MoonBadgeVariant::Soft)
                .size(MoonBadgeSize::Tiny)
                .render_with_palette(p),
        )
        .on_click(move |e: &ClickEvent, window, app| {
            let m = e.modifiers();
            let shift = m.shift;
            let cmd = m.secondary();
            view_click.update(app, |this, cx| {
                window.focus(&this.focus, cx);
                let order = this.flat_order.clone();
                if this.apply_click(key, &order, shift, cmd) {
                    this.clamp_selected_section(cx);
                    cx.notify();
                }
            });
        })
        .on_mouse_down(MouseButton::Right, move |e: &MouseDownEvent, window, app| {
            app.stop_propagation();
            let pos = e.position;
            view_menu.update(app, |this, cx| {
                if !this.sel.contains(&key) {
                    this.sel.clear();
                    this.sel.insert(key);
                    this.selected = Some(key);
                    this.clamp_selected_section(cx);
                }
                this.open_menu(
                    ContextMenu {
                        core,
                        target: MenuTarget::Strategy(id),
                        pos,
                    },
                    window,
                    cx,
                );
            });
        });
    if highlighted {
        name_row = name_row
            .bg(moon_alpha(p.amber, 0.16))
            .border_color(moon_alpha(p.amber, 0.55));
    } else {
        name_row = name_row.hover(move |s| s.bg(moon_alpha(p.panel, 0.74)));
    }

    let view_chk = view.clone();
    h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(app, 6.0))
        .pl(indent)
        .pr(design::ui_px(app, 2.0))
        .py(design::ui_px(app, 1.0))
        .child(
            MoonCheckbox::new(SharedString::from(format!("chk-{core}-{id}")))
                .checked(val)
                .size(MoonCheckboxSize::Compact)
                .on_change(move |ch: &bool, _window, app| {
                    let v = *ch;
                    view_chk.update(app, |this, cx| {
                        let before = this.staged.get(&key).copied();
                        if v == server_checked {
                            this.staged.remove(&key);
                        } else {
                            this.staged.insert(key, v);
                        }
                        if before != this.staged.get(&key).copied() {
                            cx.notify();
                        }
                    });
                }),
        )
        .child(
            MoonText::new("●")
                .mono(true)
                .uppercase(false)
                .color(dot)
                .render(),
        )
        .child(name_row)
        .into_any_element()
}
