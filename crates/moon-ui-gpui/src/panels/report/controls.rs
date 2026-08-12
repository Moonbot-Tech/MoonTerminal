//! Report panel dropdown filters and the visible-column selection menu.

use super::columns::header_for;
use super::*;
use rust_i18n::t;

/// Compact word for the trade-kind filter inside the scope trigger.
///
/// Abbreviated ("реал." for "Реальные") so the two-part summary fits the field width.
fn kind_short(kind: ReportKind) -> String {
    match kind {
        ReportKind::All => t!("report.kind.all_short").to_string(),
        ReportKind::Real => t!("report.kind.real_short").to_string(),
        ReportKind::Emu => t!("report.kind.emu_short").to_string(),
    }
}

/// Lightweight retained host for the Report's composite scope dropdown.
///
/// `MoonDropdown` routes popup open/close invalidation through the entity that renders it. Keeping
/// this control as a child prevents menu-only repaint work from rebuilding the Report table, while
/// the weak owner reference avoids an entity cycle and preserves one authoritative filter state.
pub(super) struct ReportScopeControl {
    owner: WeakEntity<ReportPanel>,
    menu_open: bool,
}

impl ReportScopeControl {
    /// Construct the scope control and follow owner notifications that can change its summary.
    ///
    /// Args:
    ///     owner: Report panel whose filter and schema state this child presents.
    ///     cx: Child context used to observe owner changes without retaining it strongly.
    ///
    /// Returns:
    ///     A closed retained control with a weak owner link.
    pub(super) fn new(owner: Entity<ReportPanel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&owner, |_this, _owner, cx| cx.notify()).detach();
        Self {
            owner: owner.downgrade(),
            menu_open: false,
        }
    }

    /// Update only the retained popup state.
    ///
    /// Args:
    ///     open: Whether the dropdown popup is currently visible.
    ///     cx: Child context used to repaint the trigger and popup branch.
    ///
    /// Returns:
    ///     Nothing; the Report owner and its query invalidation state are untouched.
    fn set_menu_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.menu_open != open {
            self.menu_open = open;
            cx.notify();
        }
    }

    /// Apply a direction selection only when it changes the live owner filter.
    ///
    /// Args:
    ///     owner: Weak Report owner captured by a menu item.
    ///     side: Newly selected trade direction.
    ///     cx: Application context used to read and update the owner.
    ///
    /// Returns:
    ///     Nothing; selecting the checked row causes no owner update or query invalidation.
    fn select_side(owner: &WeakEntity<ReportPanel>, side: SideFilter, cx: &mut App) {
        let Some(owner) = owner.upgrade() else {
            return;
        };
        if owner.read(cx).side != side {
            owner.update(cx, |panel, cx| panel.set_side(side, cx));
        }
    }

    /// Apply a trade-kind selection only when it changes the live owner filter.
    ///
    /// Args:
    ///     owner: Weak Report owner captured by a menu item.
    ///     kind: Newly selected report kind.
    ///     cx: Application context used to read and update the owner.
    ///
    /// Returns:
    ///     Nothing; selecting the checked row causes no owner update or query invalidation.
    fn select_kind(owner: &WeakEntity<ReportPanel>, kind: ReportKind, cx: &mut App) {
        let Some(owner) = owner.upgrade() else {
            return;
        };
        if owner.read(cx).kind != kind {
            owner.update(cx, |panel, cx| panel.set_kind(kind, cx));
        }
    }

    /// Toggle the deleted-only filter from its live owner value.
    ///
    /// Args:
    ///     owner: Weak Report owner captured by the menu item.
    ///     cx: Application context used to read and update the owner.
    ///
    /// Returns:
    ///     Nothing; a live owner receives exactly one filter mutation.
    fn toggle_deleted(owner: &WeakEntity<ReportPanel>, cx: &mut App) {
        let Some(owner) = owner.upgrade() else {
            return;
        };
        let deleted_only = !owner.read(cx).deleted_only;
        owner.update(cx, |panel, cx| panel.set_deleted_only(deleted_only, cx));
    }

    /// Toggle the comment pane through the live owner.
    ///
    /// Args:
    ///     owner: Weak Report owner captured by a menu item.
    ///     cx: Application context used to update the owner.
    ///
    /// Returns:
    ///     Nothing; this display-only selection does not request report data.
    fn toggle_comment(owner: &WeakEntity<ReportPanel>, cx: &mut App) {
        let Some(owner) = owner.upgrade() else {
            return;
        };
        owner.update(cx, |panel, cx| panel.toggle_comment_pane(cx));
    }
}

impl Render for ReportScopeControl {
    /// Render the composite scope dropdown from the owner's current filter snapshot.
    ///
    /// Args:
    ///     window: Owning window forwarded to MoonDropdown interactions.
    ///     cx: Child render context that owns popup-only invalidation.
    ///
    /// Returns:
    ///     The existing direction, kind, deleted-only, and comment menu semantics.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(owner) = self.owner.upgrade() else {
            return div().into_any_element();
        };
        let panel = owner.read(cx);
        let side = panel.side;
        let kind = panel.kind;
        let deleted_only = panel.deleted_only;
        let show_comment = panel.show_comment;
        let no_comment_column =
            !panel.cols.is_empty() && !panel.cols.iter().any(|column| column == "comment");

        let mut label = if matches!(side, SideFilter::All) && matches!(kind, ReportKind::All) {
            t!("report.filter.all").to_string()
        } else {
            let mut label = crate::panels::side_label(side);
            label.push('/');
            label.push_str(&kind_short(kind));
            label
        };
        if deleted_only {
            label.push('/');
            label.push_str(&t!("report.filter.deleted_short"));
        }

        let side_owner = self.owner.clone();
        let mut items = crate::panels::radio_items(
            [
                (
                    SideFilter::All,
                    "rs-all".into(),
                    t!("report.filter.all_sides").to_string().into(),
                ),
                (
                    SideFilter::Long,
                    "rs-long".into(),
                    t!("report.side.long").to_string().into(),
                ),
                (
                    SideFilter::Short,
                    "rs-short".into(),
                    t!("report.side.short").to_string().into(),
                ),
            ],
            side,
            crate::panels::RadioMark::Check,
            move |app, side| Self::select_side(&side_owner, side, app),
        );
        items.push(MoonMenuItem::separator());
        let kind_owner = self.owner.clone();
        items.extend(crate::panels::radio_items(
            [
                (
                    ReportKind::All,
                    "rk-all".into(),
                    t!("report.kind.all_kinds").to_string().into(),
                ),
                (
                    ReportKind::Real,
                    "rk-real".into(),
                    t!("report.kind.real").to_string().into(),
                ),
                (
                    ReportKind::Emu,
                    "rk-emu".into(),
                    t!("report.kind.emu").to_string().into(),
                ),
            ],
            kind,
            crate::panels::RadioMark::Check,
            move |app, kind| Self::select_kind(&kind_owner, kind, app),
        ));
        items.push(MoonMenuItem::separator());
        let deleted_owner = self.owner.clone();
        items.push(
            MoonMenuItem::with_key("rd-deleted", t!("report.filter.deleted").to_string())
                .checked(deleted_only)
                .on_click(move |_, _, app| Self::toggle_deleted(&deleted_owner, app)),
        );
        items.push(MoonMenuItem::separator());
        let comment_owner = self.owner.clone();
        items.push(
            MoonMenuItem::with_key("rd-comment", t!("report.comment.show").to_string())
                .checked(show_comment && !no_comment_column)
                .disabled(no_comment_column)
                .on_click(move |_, _, app| Self::toggle_comment(&comment_owner, app)),
        );

        let control = cx.entity();
        div()
            .id("rep-scope-tip")
            .tooltip(crate::panels::common::text_tooltip(
                t!("report.filter.scope_tip").to_string(),
            ))
            .child(
                MoonDropdown::new("rep-scope")
                    .label(label)
                    .trigger_caret(true)
                    .trigger_variant(MoonButtonVariant::Soft)
                    .trigger_size(MoonButtonSize::Action)
                    .fit_trigger_width(102.0, 170.0)
                    .menu_width_scaled(210.0)
                    .menu_size(MoonMenuSize::Compact)
                    .close_on_select(false)
                    .open(self.menu_open)
                    .on_open_change(move |open, _, app| {
                        control.update(app, |this, cx| this.set_menu_open(open, cx));
                    })
                    .items(items),
            )
            .into_any_element()
    }
}

/// Maximum design-reference width of the pinned AutoCore selector.
const AUTO_CORE_TRIGGER_MAX_W: f32 = 360.0;

/// Resolve one selected core's current display name, with report history only as a fallback.
///
/// Args:
///     core: Selected effective workspace core.
///     live_cores: Current group sessions and their authoritative names.
///     report_cores: Historical report metadata, which may be stale or omit a new core.
///
/// Returns:
///     Flattened current name, historical fallback, or `None` when neither source knows the core.
pub(super) fn selected_auto_core_name(
    core: CoreId,
    live_cores: &[(CoreId, String)],
    report_cores: &[(CoreId, String)],
) -> Option<String> {
    let usable_name = |cores: &[(CoreId, String)]| {
        cores
            .iter()
            .find(|(id, _)| *id == core)
            .filter(|(_, name)| !name.trim().is_empty())
            .map(|(_, name)| crate::display_text::flatten_lines(name))
    };
    usable_name(live_cores).or_else(|| usable_name(report_cores))
}

impl ReportPanel {
    /// Render the shared core combo under the panel's current scope authority.
    ///
    /// Standalone and Classic group Reports expose the retained multi-selection. Group Auto mode
    /// displays the pinned effective workspace scope and disables the selector without changing
    /// retained state.
    ///
    /// Args:
    ///     cx: Panel context used to order database cores, read exchanges, and wire callbacks.
    ///
    /// Returns:
    ///     Interactive retained-scope selector or disabled Auto scope indicator.
    pub(super) fn core_combo(&self, cx: &Context<Self>) -> AnyElement {
        let workspace_scope = self.workspace_scope(self.backend.read(cx));
        let workspace_owned = workspace_scope
            .as_ref()
            .is_some_and(EffectiveCoreScope::is_workspace_owned);
        let effective_selection: HashSet<CoreId> = workspace_scope
            .as_ref()
            .map(|scope| scope.ids().iter().copied().collect())
            .unwrap_or_default();
        let view = cx.entity();
        let exchange_view = view.clone();
        // Rank the raw DB result at render time; the query has no config and may include
        // deleted cores with database-owned names.
        let (cores, live_cores, venues) = {
            let backend = self.backend.read(cx);
            (
                CoreOrder::new(&backend.config).from_db(self.cores.clone()),
                backend.group_cores(&self.group),
                backend.session.core_venues(),
            )
        };
        let auto_core = workspace_scope
            .as_ref()
            .is_some_and(EffectiveCoreScope::is_auto_core);
        let pinned_label = workspace_scope
            .as_ref()
            .and_then(|scope| match scope.label() {
                crate::workspace::EffectiveScopeLabel::Overview => {
                    Some(t!("workspace.overview").to_string())
                }
                crate::workspace::EffectiveScopeLabel::Core(core) => {
                    selected_auto_core_name(core, &live_cores, &cores)
                }
                crate::workspace::EffectiveScopeLabel::All
                | crate::workspace::EffectiveScopeLabel::Selection(_) => None,
            });
        let extras = crate::controls::core_combo_extras(!workspace_owned, &view, &self.backend, cx);
        let combo = crate::controls::core_combo(
            "rep-core",
            &cores,
            &venues,
            if workspace_owned {
                &effective_selection
            } else {
                &self.sel_cores
            },
            crate::controls::CoreAllRowMode::ImplicitOrComplete,
            t!("report.all_cores").to_string(),
            |n| t!("report.cores_n", n = n).to_string(),
            180.0,
            extras,
            move |uid, app| {
                view.update(app, |t, c| t.toggle_core(uid, c));
            },
            move |exchange_cores, app| {
                exchange_view.update(app, |t, c| {
                    t.toggle_exchange_cores(exchange_cores, c);
                });
            },
        )
        .disabled(workspace_owned);
        let tooltip = pinned_label.clone();
        let combo = if let Some(label) = pinned_label {
            combo.label(label).when(auto_core, |combo| {
                combo.fit_trigger_width(
                    crate::controls::CORE_COMBO_TRIGGER_W,
                    AUTO_CORE_TRIGGER_MAX_W,
                )
            })
        } else {
            combo
        };
        div()
            .id("rep-core-tip")
            .flex_none()
            .when_some(tooltip.filter(|_| auto_core), |host, label| {
                host.tooltip(crate::panels::common::text_tooltip(label))
            })
            .child(combo)
            .into_any_element()
    }

    /// Render the separate AutoCore-only strategy-name mask field.
    ///
    /// Args:
    ///     cx: Panel context used to resolve workspace scope, width, and localized tooltip.
    ///
    /// Returns:
    ///     A retained MoonUI input for AutoCore, otherwise no element.
    pub(super) fn strategy_name_mask_field(&self, cx: &Context<Self>) -> Option<AnyElement> {
        self.workspace_scope(self.backend.read(cx))
            .is_some_and(|scope| scope.is_auto_core())
            .then(|| {
                div()
                    .id("rep-strategy-mask-tip")
                    .flex_none()
                    .w(design::font_w_px(cx, 150.0))
                    .tooltip(crate::panels::common::text_tooltip(
                        t!("report.filter.strategy_mask_tip").to_string(),
                    ))
                    .child(
                        MoonInput::new("rep-strategy-mask")
                            .state(&self.strategy_name_mask_input)
                            .small()
                            .cleanable(true),
                    )
                    .into_any_element()
            })
    }

    /// Render the searchable, virtualized strategy selector grouped by core.
    ///
    /// The trigger and popup are asked to look like the `MoonDropdown` filters beside them:
    /// `trigger_variant`/`trigger_size` give it the Soft button's fill, border, hover ramp and
    /// geometry, and `menu_chrome` paints the popup on the menu surface with the check mark in the
    /// leading column. Only the mono label stays a call-site choice, matching this row's dropdowns.
    ///
    /// Args:
    ///     cx: Panel context used for responsive trigger and menu sizing.
    ///
    /// Returns:
    ///     A MoonUI combobox that renders only visible core and strategy rows.
    pub(super) fn strategy_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let (summary, _) = strategy_selection_summary(
            &self.available_strategy_keys,
            self.selected_strategies.as_ref(),
            &t!("report.all_strategies"),
            |n| t!("report.strategies_n", n = n).to_string(),
        );
        let palette = MoonPalette::active(cx);
        div()
            .w(design::font_w_px(cx, crate::controls::CORE_COMBO_TRIGGER_W))
            .child(
                MoonCombobox::new(&self.strategy_select)
                    .trigger_variant(MoonButtonVariant::Soft)
                    .trigger_size(MoonButtonSize::Action)
                    .menu_chrome(MoonComboboxMenuChrome::Menu)
                    .font_family(design::mono())
                    .placeholder(t!("report.all_strategies").to_string())
                    .cleanable(false)
                    .search_placeholder(t!("report.search_strategies").to_string())
                    .appearance(true)
                    .menu_width(design::font_w_px(cx, 380.0))
                    .menu_max_h(design::ui_px(cx, 420.0))
                    .render_trigger(move |_, _, _| {
                        // Centred label plus caret, the way a MoonDropdown button draws its
                        // trigger: pushed apart by `justify_between` the same text reads as
                        // left-aligned next to those buttons.
                        h_flex()
                            .w_full()
                            .justify_center()
                            .gap_1()
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .truncate()
                                    .child(summary.clone()),
                            )
                            // A custom MoonCombobox trigger suppresses its built-in trailing icon.
                            .child(div().text_color(rgb(palette.text_muted)).child("▾"))
                    }),
            )
    }

    /// Render the report-period filter using Moonbot-compatible presets.
    ///
    /// The presets arrive already grouped by [`Period::GROUPS`] — day, calendar, rolling, then the
    /// unbounded one — with a separator standing BETWEEN the groups and never after the last. The
    /// grouping is what tells a reader that "this month" and "30 days" answer different questions.
    ///
    /// Args:
    ///     cx: Panel context used to bind the selected period and fit localized labels.
    ///
    /// Returns:
    ///     Grouped period dropdown with separators between preset families.
    pub(super) fn period_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let mut items: Vec<MoonMenuItem> = Vec::new();
        for group in Period::GROUPS {
            if !items.is_empty() {
                items.push(MoonMenuItem::separator());
            }
            let view = view.clone();
            let options: Vec<(Period, SharedString, SharedString)> = group
                .iter()
                .map(|p| (*p, p.menu_key().into(), p.label().into()))
                .collect();
            // Per group rather than once over a flat list: `radio_items` marks the current preset
            // in whichever group holds it, so the selection is unaffected by the split.
            items.extend(crate::panels::radio_items(
                options,
                self.period,
                crate::panels::RadioMark::Highlight,
                move |app, p| {
                    view.update(app, |t, c| t.set_period(p, c));
                },
            ));
        }
        MoonDropdown::new("rep-period")
            .label(self.period.label())
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            // Fitted rather than a literal width: the longest localized labels no longer fit the
            // old figures, and MoonUI's font-aware fitting knows their advance at the live UI scale.
            .fit_trigger_width(100.0, 150.0)
            .fit_menu_width(130.0, 190.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    }

    /// Build the selection commands for the totals line.
    ///
    /// They live in the totals row because a separate bar appearing on the first click would push
    /// the table up under the cursor. They close the row, after the facts, and the
    /// fact tail's zero flex basis is what pins them to its right edge.
    ///
    /// This group is the row's one concession to a genuinely narrow dock. The facts degrade by
    /// clipping and stay reachable through the row tooltip; commands cannot, since they are the
    /// only way to act on a selection. So they wrap instead, growing the row only while rows are
    /// selected and the dock is narrower than the count plus the currently available buttons.
    ///
    /// Args:
    ///     palette: Active Moon palette used for the count label.
    ///     cx: Panel context used to wire actions and count selected replicated targets.
    ///
    /// Returns:
    ///     The count plus its commands, or `None` when nothing is selected.
    pub(super) fn selection_actions(
        &self,
        palette: MoonPalette,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let selected = self.selection.len();
        if selected == 0 {
            return None;
        }
        let mutable = self.selection.mutable_count();
        // `min_w_0` is what makes the wrap above reachable: only a group allowed to shrink below
        // its content wraps rather than overflowing. No `ml_auto` — the fact tail's zero flex basis
        // already pins this group right.
        let mut bar = h_flex()
            .min_w_0()
            .flex_wrap()
            .justify_end()
            .items_center()
            .gap_1()
            .child(
                div()
                    .flex_none()
                    .text_size(design::t_body(cx))
                    .font_bold()
                    .text_color(rgb(palette.text))
                    .child(t!("report.selection.count", n = selected).to_string()),
            )
            .child(
                MoonButton::new("report-selection-clear")
                    .size(MoonButtonSize::Micro)
                    .variant(MoonButtonVariant::Ghost)
                    .label(t!("report.selection.clear").to_string())
                    .leading_icon(MoonButtonIconSlot::new("icons/close.svg"))
                    .on_click(cx.listener(|this, _, _, cx| this.clear_report_selection(cx)))
                    .render(),
            )
            .child(
                MoonButton::new("report-selection-copy")
                    .size(MoonButtonSize::Micro)
                    .outline()
                    .label(t!("report.selection.copy").to_string())
                    .leading_icon(MoonButtonIconSlot::new("icons/copy.svg"))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.copy_report_selection(window, cx)),
                    )
                    .render(),
            );
        if mutable > 0 {
            let (label, icon) = if self.deleted_only {
                (
                    t!("report.selection.restore", n = mutable).to_string(),
                    "icons/undo-2.svg",
                )
            } else {
                (
                    t!("report.selection.delete", n = mutable).to_string(),
                    "icons/delete.svg",
                )
            };
            let mutation = MoonButton::new("report-selection-mutate")
                .size(MoonButtonSize::Micro)
                .label(label)
                .leading_icon(MoonButtonIconSlot::new(icon))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.mutate_report_selection(!this.deleted_only, window, cx)
                }));
            bar = bar.child(if self.deleted_only {
                mutation.outline().render()
            } else {
                mutation.danger().render()
            });
        }
        Some(bar.into_any_element())
    }

    /// Build the CSV/XLSX export menu for the visible or full schema.
    ///
    /// Export uses the panel's current filter and sort order; the period may be a preset or the
    /// timestamps picked in the From/To date+time fields.
    pub(super) fn export_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let item =
            |key: &'static str, label: String, fmt: super::export::Format, all_cols: bool| {
                let view = view.clone();
                MoonMenuItem::with_key(key, label).on_click(move |_, window, app| {
                    view.update(app, |t, c| t.export_report(fmt, all_cols, window, c));
                })
            };
        let items = vec![
            item(
                "exp-csv",
                t!("report.export.csv").to_string(),
                super::export::Format::Csv,
                false,
            ),
            item(
                "exp-csv-all",
                t!("report.export.csv_all").to_string(),
                super::export::Format::Csv,
                true,
            ),
            item(
                "exp-xlsx",
                t!("report.export.xlsx").to_string(),
                super::export::Format::Xlsx,
                false,
            ),
            item(
                "exp-xlsx-all",
                t!("report.export.xlsx_all").to_string(),
                super::export::Format::Xlsx,
                true,
            ),
        ];
        // Keep the glyph button alongside the column selector and explain it with a tooltip.
        div()
            .id("rep-export-tip")
            .tooltip(crate::panels::common::text_tooltip(
                t!("report.export_menu").to_string(),
            ))
            .child(
                MoonDropdown::new("rep-export")
                    .segment(moon_ui::MoonButtonSegment::new("⇩"))
                    .trigger_variant(MoonButtonVariant::Soft)
                    .trigger_size(MoonButtonSize::Action)
                    .trigger_width(design::glyph_btn_w(cx))
                    .menu_width_scaled(200.0)
                    .menu_size(MoonMenuSize::Compact)
                    .items(items),
            )
    }

    /// Build a checkbox menu from the contextually available runtime DB schema.
    ///
    /// The AutoCore display lens omits `core_name` without changing its dormant saved preference;
    /// every other host includes all dynamic runtime fields as before.
    ///
    /// Args:
    ///     cx: Panel context used to resolve workspace scope and wire menu callbacks.
    ///
    /// Returns:
    ///     Column selector whose rows and All state reflect only contextually available columns.
    pub(super) fn columns_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let hide_core_name = self.hide_core_name_column(self.backend.read(cx));
        let all_on =
            columns::all_available_columns_visible(&self.cols, &self.visible, hide_core_name);
        let all_view = view.clone();
        let mut items: Vec<MoonMenuItem> = vec![
            // "All" enables every column; when all are already enabled, this action keeps only
            // the first so it never empties the table. Use `checked`, not `selected`: selected
            // adds a light background that makes rows hard to read in the light theme, while
            // the check glyph is an explicit selection indicator.
            MoonMenuItem::with_key("col-all", t!("report.filter.all").to_string())
                .checked(all_on)
                .on_click(move |_, _, app| {
                    all_view.update(app, |t, c| t.toggle_all_columns(c));
                }),
        ];
        items.extend(
            columns::available_columns(&self.cols, hide_core_name).map(|(i, c)| {
                let on = self.visible.contains(c.as_str());
                let name = c.clone();
                let view = view.clone();
                MoonMenuItem::with_key(format!("col-{i}"), header_for(c))
                    .checked(on)
                    .on_click(move |_, _, app| {
                        let name = name.clone();
                        view.update(app, |t, c| t.toggle_column(name, c));
                    })
            }),
        );
        // Use a glyph button instead of a list field, matching other column selectors. The
        // tooltip is localized; glyphs remain outside the locale dictionary per locales/README.
        div()
            .id("rep-cols-tip")
            .tooltip(crate::panels::common::text_tooltip(
                t!("report.columns_menu").to_string(),
            ))
            .child(
                MoonDropdown::new("rep-cols")
                    .segment(moon_ui::MoonButtonSegment::new("▦"))
                    .trigger_variant(MoonButtonVariant::Soft)
                    .trigger_size(MoonButtonSize::Action)
                    .trigger_width(design::glyph_btn_w(cx))
                    .menu_width_scaled(230.0)
                    .menu_max_height_ui(420.0)
                    .menu_size(MoonMenuSize::Compact)
                    .close_on_select(false)
                    .items(items),
            )
    }
}

#[cfg(test)]
mod tests;
