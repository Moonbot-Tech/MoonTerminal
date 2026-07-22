//! Strategies window, ported from egui's `src/strategies/*` and `window/strategies_window.rs`.
//! The separate OS window contains tree, versions, schema-section, and parameter panels. Its
//! core/folder/strategy tree supports search, filters, staged checkboxes, and start/stop Apply;
//! schema sections dim inactive entries, while parameter rows support read-only, YES/NO, and long
//! values. Field and section dependencies load from `assets/param_deps.toml` through [`rules`] and
//! hot-reload only when `MOON_STRATEGY_RULES_HOT_RELOAD` is set.
//! The view reads the live per-core Backend store, and Apply sends checkbox changes plus start/stop
//! through `session.apply_strategies`.

mod filter;
mod logic;
mod params;
mod rules;
mod tree;
mod tree_dialogs;
mod tree_dnd;
mod tree_menu;
mod tree_moon;
// `pub(crate)` exposes `unique_name`, `set_field`, and `STRATEGY_NAME_FIELD` to the Analytics
// tuner's Make Copy operation.
pub(crate) mod tree_ops;
mod tree_ui;
mod versions;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBackgroundPolicy, MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox,
    MoonCheckboxSize, MoonColorPicker, MoonColorPickerEvent, MoonColorPickerState, MoonDropdown,
    MoonInput, MoonInputEvent, MoonInputState, MoonMenuItem, MoonMenuSize, MoonPalette,
    MoonTextArea, MoonTextAreaEvent, MoonTextAreaState, MoonTone, MoonTreeItem, MoonTreeState,
    MoonWindowFrame, Root, h_flex, v_flex,
};

use crate::design::{moon, moon_alpha};
use crate::{Backend, design};
use moon_core::feed::{SchemaField, SchemaFieldUi, SchemaSection, StrategyRow};
use moon_core::session::{CoreId, CoreStore};
use rust_i18n::t;

use filter::StrategyFilter;
use logic::*;
use rules::{Rules, Values};

pub type Key = (CoreId, u64);
type FieldEditKey = (CoreId, u64, String);

const STRATEGIES_HEADER_H: f32 = 32.0;

/// Strategies-window state, porting egui's `StrategiesState` and four-panel renderer.
pub struct StrategiesView {
    backend: Entity<Backend>,
    /// Search input whose value is synchronized into the filter.
    search: Entity<MoonInputState>,
    /// Tree filters for kind, direction, active state, and search text synchronized from the input.
    filter: StrategyFilter,
    /// Primary strategy key supplying the schema and sections.
    selected: Option<Key>,
    /// Multi-selection used for highlighting and merged parameter display.
    sel: HashSet<Key>,
    /// Versions-panel state for history and an optional persisted read-only snapshot selection.
    /// No selected snapshot means the panels display the live strategy.
    versions: versions::VersionsState,
    /// Strategies deleted from cores but retained in the local database, keyed by core.
    /// They appear flat in the tree's Deleted folder because their core paths no longer exist.
    deleted: HashMap<CoreId, Vec<moon_core::strat_db::stats::HeadRow>>,
    deleted_gen: u64,
    deleted_inflight: bool,
    /// Cores whose Deleted folder is expanded.
    expanded_deleted: HashSet<CoreId>,
    /// Resizable tree, versions, and sections widths persisted in `layout.strategies_panels`.
    panels: moon_core::config::layout::StrategiesPanels,
    /// Anchor for Shift range selection.
    anchor: Option<Key>,
    /// Previous frame's flat visible-strategy order used for Shift ranges.
    flat_order: Vec<Key>,
    /// MoonTree state for flattening, virtualization, expansion, and drag-and-drop hitboxes.
    /// Selection and staging remain above; `TreeState` only renders and exposes decorator hitboxes.
    tree_state: Entity<MoonTreeState>,
    /// Selected section index within the strategy kind's schema.
    /// Preserved across strategy changes and reset only when it falls outside the new range.
    selected_section: usize,
    /// Staged checkbox values keyed by core and strategy id.
    /// Sent with Start/Stop Checked and cleared after successful application.
    staged: HashMap<Key, bool>,
    /// Draft field edits mapping core, strategy id, and field name to the new UI string.
    field_edits: HashMap<FieldEditKey, String>,
    /// Retained single-line editor states for visible or previously visited fields.
    field_inputs: HashMap<String, Entity<MoonInputState>>,
    /// Retained memo/formula editor states for visible or previously visited fields.
    field_memos: HashMap<String, Entity<MoonTextAreaState>>,
    /// Color-field pickers keyed by row id, storing the RGB used to create each state.
    /// External RGB changes recreate the state because it has no silent synchronization; changing
    /// only the hexadecimal alpha prefix does not invalidate this RGB-based cache.
    field_colors: HashMap<String, ([u8; 3], Entity<MoonColorPickerState>)>,
    /// Field whose contextual helper or autocomplete is open.
    focused_field: Option<String>,
    /// Expanded cores in the strategy tree.
    expanded_cores: HashSet<CoreId>,
    /// Expanded tree folders keyed by core and path.
    expanded_folders: HashSet<(CoreId, String)>,
    /// Field-dependency rules from `param_deps.toml`, hot-reloaded only with the opt-in environment flag.
    rules: Rules,
    /// Copied strategy or folder source data, retained for cross-core pasting.
    clipboard: Option<Vec<tree_ops::ClipItem>>,
    /// Names submitted for creation but not yet echoed by the core, keyed by core and name.
    /// Reserving them prevents rapid pastes from reading one store snapshot and generating the same
    /// name repeatedly. A reservation is cleared once the name appears in the store.
    pending_names: HashSet<(CoreId, String)>,
    /// Click-selected folder used for highlighting and whole-folder Ctrl+C copying as in Moonbot.
    selected_folder: Option<(CoreId, String)>,
    /// Empty UI folders before their first strategy is added, keyed by core and slash-separated path.
    ui_folders: HashSet<(CoreId, String)>,
    /// Active create, rename, or confirmation modal for a tree operation.
    op: Option<tree_ui::TreeOp>,
    /// Create/rename modal input, recreated on each opening to use the current initial value.
    op_input: Option<Entity<MoonInputState>>,
    /// Initial value used when rendering the next `op_input` instance.
    op_input_init: String,
    /// Strategy expected from a core echo after create/paste, keyed by core and name.
    /// Selected in the tree when it arrives, then cleared.
    pending_select: Option<(CoreId, String)>,
    /// Signature of strategy and schema data that materially changes the window.
    last_sig: u64,
    /// Whether the parameters panel hides dependency-inactive fields.
    only_active_params: bool,
    focus: FocusHandle,
}

impl StrategiesView {
    fn new(backend: Entity<Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let panels = backend.read(cx).layout.strategies_panels;
        let search = cx
            .new(|cx| MoonInputState::new(window, cx).placeholder(t!("strat.search").to_string()));
        // Update the filter and redraw from search input events; render must not poll the input as
        // an event source.
        cx.subscribe(&search, |this, input, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                let value = input.read(cx).value().to_string();
                if this.filter.search != value {
                    this.filter.search = value;
                    cx.notify();
                }
            }
        })
        .detach();

        let initial_sig = strategies_sig(backend.read(cx));

        // Redraw for new strategy or schema snapshots. When explicitly enabled by
        // `MOON_STRATEGY_RULES_HOT_RELOAD`, rules reload on the file-mtime timer below; observing
        // backend data must not become a surrogate filesystem polling loop. A `strategies_goto`
        // request also wakes render, where it is drained with Window access.
        cx.observe(&backend, |this, backend, cx| {
            let b = backend.read(cx);
            let goto = b.strategies_goto.is_some();
            let sig = strategies_sig(b);
            if sig != this.last_sig || goto {
                this.last_sig = sig;
                this.sync_pending_select(cx);
                this.clamp_selected_section(cx);
                cx.notify();
            }
        })
        .detach();

        // Persist Strategies-window geometry in layout. The debounced save loop drains
        // `layout_dirty`, matching group windows.
        cx.observe_window_bounds(window, |this, window, cx| {
            let Some((x, y, w, h)) = crate::windowing::window_geom(window) else {
                return;
            };
            this.backend.update(cx, |b, _| {
                if b.layout.strategies_window.map(|g| (g.x, g.y, g.w, g.h)) != Some((x, y, w, h)) {
                    b.layout.strategies_window =
                        Some(moon_core::config::layout::GeomRect { x, y, w, h });
                    b.layout_dirty = true;
                }
            });
        })
        .detach();

        if std::env::var_os("MOON_STRATEGY_RULES_HOT_RELOAD").is_some() {
            cx.spawn(async move |this, cx| {
                let executor = cx.update(|cx| cx.background_executor().clone());
                loop {
                    executor.timer(Duration::from_secs(1)).await;
                    let alive = cx.update(|cx| {
                        this.update(cx, |this, cx| {
                            if this.rules.reload_if_changed() {
                                cx.notify();
                            }
                        })
                        .is_ok()
                    });
                    if !alive {
                        break;
                    }
                }
            })
            .detach();
        }

        Self {
            backend,
            search,
            filter: StrategyFilter::default(),
            selected: None,
            sel: HashSet::new(),
            versions: versions::VersionsState {
                collapsed: panels.versions_collapsed,
                ..Default::default()
            },
            panels,
            deleted: HashMap::new(),
            deleted_gen: u64::MAX,
            deleted_inflight: false,
            expanded_deleted: HashSet::new(),
            anchor: None,
            flat_order: Vec::new(),
            tree_state: cx.new(|cx| MoonTreeState::new(cx)),
            selected_section: 0,
            staged: HashMap::new(),
            field_edits: HashMap::new(),
            field_inputs: HashMap::new(),
            field_memos: HashMap::new(),
            field_colors: HashMap::new(),
            focused_field: None,
            expanded_cores: HashSet::new(),
            expanded_folders: HashSet::new(),
            rules: Rules::load(),
            clipboard: None,
            pending_names: HashSet::new(),
            selected_folder: None,
            ui_folders: HashSet::new(),
            op: None,
            op_input: None,
            op_input_init: String::new(),
            pending_select: None,
            last_sig: initial_sig,
            // Hide dependency-inactive parameters by default.
            only_active_params: true,
            focus: cx.focus_handle(),
        }
    }

    // ── Selection ───────────────────────────────────────────────────────────

    /// Apply a strategy click with selection modifiers.
    /// Shift selects the `order` range from the anchor, Ctrl/Cmd toggles one key, and an
    /// unmodified click replaces the selection.
    fn apply_click(&mut self, key: Key, order: &[Key], shift: bool, command: bool) -> bool {
        let before_selected = self.selected;
        let before_anchor = self.anchor;
        let before_sel = self.sel.clone();
        if shift {
            if let Some(a) = self.anchor {
                let ia = order.iter().position(|k| *k == a);
                let ib = order.iter().position(|k| *k == key);
                if let (Some(ia), Some(ib)) = (ia, ib) {
                    let (lo, hi) = if ia <= ib { (ia, ib) } else { (ib, ia) };
                    self.sel = order[lo..=hi].iter().copied().collect();
                } else {
                    self.sel = std::iter::once(key).collect();
                }
            } else {
                self.sel = std::iter::once(key).collect();
                self.anchor = Some(key);
            }
        } else if command {
            if !self.sel.remove(&key) {
                self.sel.insert(key);
            }
            self.anchor = Some(key);
        } else {
            self.sel.clear();
            self.sel.insert(key);
            self.anchor = Some(key);
        }
        // The clicked strategy always becomes the primary schema/section source; keep the section.
        self.selected = Some(key);
        // A strategy click clears folder selection so Ctrl+C copies the strategy selection again.
        self.selected_folder = None;
        before_selected != self.selected || before_anchor != self.anchor || before_sel != self.sel
    }

    fn sync_pending_select(&mut self, cx: &App) -> bool {
        let Some((core, name)) = self.pending_select.clone() else {
            return false;
        };
        let key = {
            let store = self.backend.read(cx).session.store();
            store.core(core).and_then(|cd| {
                cd.strategies
                    .iter()
                    .find(|row| row.name == name)
                    .map(|row| (core, row.id))
            })
        };
        let Some(key) = key else {
            return false;
        };
        self.selected = Some(key);
        self.sel.clear();
        self.sel.insert(key);
        self.pending_select = None;
        self.clamp_selected_section(cx);
        true
    }

    /// Drain a `Backend::strategies_goto` request and reveal its strategy.
    /// Disables the active-only filter, resets other filters when they still hide the target,
    /// expands its core and folders, and selects it. Returns the key so render can scroll after
    /// calling `set_items`.
    fn drain_goto(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Key> {
        let (core, strat_id) = self.backend.read(cx).strategies_goto?;
        self.backend.update(cx, |b, _| b.strategies_goto = None);
        let row = {
            let store = self.backend.read(cx).session.store();
            store
                .core(core)?
                .strategies
                .iter()
                .find(|r| r.id == strat_id)
                .cloned()?
        };
        self.filter.only_active = false;
        // Reset kind, direction, and search only if they still hide the target. `set_value` does not
        // emit Change, so update the filter explicitly alongside the search input.
        if !self.filter.matches(&row) {
            self.filter.kind = None;
            self.filter.dir = None;
            if !self.filter.search.trim().is_empty() {
                self.filter.search.clear();
                self.search
                    .update(cx, |st, cx| st.set_value(String::new(), window, cx));
            }
        }
        let key: Key = (core, strat_id);
        self.expanded_cores.insert(core);
        self.expand_path(core, tree_ops::path_segments(&row.folder_path));
        self.sel.clear();
        self.sel.insert(key);
        self.anchor = Some(key);
        self.selected = Some(key);
        self.clamp_selected_section(cx);
        Some(key)
    }

    fn clamp_selected_section(&mut self, cx: &App) -> bool {
        let store = self.backend.read(cx).session.store();
        let Some(sections) = selected_sections(self, store) else {
            return false;
        };
        if self.selected_section < sections.len() {
            return false;
        }
        self.selected_section = 0;
        true
    }

    // ── Actions for starting or stopping checked strategies ─────────────────

    /// Start or stop checked strategies on each core through `session.apply_strategies`.
    /// Sends staged checkbox differences plus the start/stop command when the core has an
    /// effectively checked strategy or any checkbox changes, then clears staging after success.
    fn apply_start_stop(
        &mut self,
        cores: &[(CoreId, String)],
        start: bool,
        cx: &mut Context<Self>,
    ) {
        // Collect actions while reading the store, then reacquire the backend to apply them.
        let mut actions: Vec<(CoreId, Vec<(u64, bool)>, bool)> = Vec::new();
        {
            let b = self.backend.read(cx);
            let store = b.session.store();
            for (core, _) in cores {
                let Some(cd) = store.core(*core) else {
                    continue;
                };
                let mut checks = Vec::new();
                let mut has_checked = false;
                for r in &cd.strategies {
                    let eff = self
                        .staged
                        .get(&(*core, r.id))
                        .copied()
                        .unwrap_or(r.checked);
                    if eff != r.checked {
                        checks.push((r.id, eff));
                    }
                    if eff {
                        has_checked = true;
                    }
                }
                if !checks.is_empty() || has_checked {
                    actions.push((*core, checks, start));
                }
            }
        }
        if actions.is_empty() {
            return;
        }
        let b = self.backend.read(cx);
        for (core, checks, st) in actions {
            if let Err(error) = b.session.apply_strategies(core, checks, Some(st)) {
                log::warn!("apply strategies failed: {error}");
                return;
            }
        }
        self.staged.clear();
        cx.notify();
    }

    fn stage_field_value(
        &mut self,
        keys: &[Key],
        field: &str,
        value: String,
        cx: &mut Context<Self>,
    ) {
        // Persisted snapshot views are read-only, including the snapshot labeled current. Live mode
        // has no selected snapshot. Controls are disabled, and this setter guard blocks indirect
        // paths such as the color picker as a backstop.
        if keys.is_empty() || self.viewing_version() {
            return;
        }
        self.focused_field = Some(field.to_string());
        for (core, id) in keys {
            self.field_edits
                .insert((*core, *id, field.to_string()), value.clone());
        }
        cx.notify();
    }

    fn apply_field_edits(&mut self, cx: &mut Context<Self>) {
        if self.field_edits.is_empty() {
            return;
        }
        // Group by core and then strategy, sending one command with all edits for each core.
        // Separate commands for multiple selected strategies on one core would let successive
        // `sync_local_strategies` calls overwrite each other and retain only one strategy's edits.
        let mut per_core: HashMap<CoreId, HashMap<u64, Vec<(String, String)>>> = HashMap::new();
        for ((core, id, field), value) in &self.field_edits {
            per_core
                .entry(*core)
                .or_default()
                .entry(*id)
                .or_default()
                .push((field.clone(), value.clone()));
        }
        let b = self.backend.read(cx);
        for (core, strat_edits) in per_core {
            let edits: Vec<(u64, Vec<(String, String)>)> = strat_edits.into_iter().collect();
            if let Err(error) = b.session.edit_strategies(core, edits) {
                log::warn!("edit strategies failed: {error}");
                return;
            }
        }
        self.clear_field_draft();
        cx.notify();
    }

    fn discard_field_edits(&mut self, cx: &mut Context<Self>) {
        if self.field_edits.is_empty() {
            return;
        }
        self.clear_field_draft();
        cx.notify();
    }

    fn clear_field_draft(&mut self) {
        self.field_edits.clear();
        self.field_inputs.clear();
        self.field_memos.clear();
        self.field_colors.clear();
        self.focused_field = None;
    }

    /// Put a strategy's full name into the search filter and focus the input for Find All by Name.
    /// Writes both the filter and input because `set_value` does not emit Change.
    pub(super) fn search_by_name(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.filter.search = name.clone();
        self.search.update(cx, |st, cx| {
            st.set_value(name, window, cx);
            st.focus(window, cx);
        });
        cx.notify();
    }

    fn field_input_state(
        &mut self,
        id: String,
        value: String,
        keys: Arc<Vec<Key>>,
        field: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        if let Some(state) = self.field_inputs.get(&id) {
            let cur = state.read(cx).value().to_string();
            if cur == value {
                return state.clone();
            }
            // The cache may lag a server echo or an edit made through another selection while
            // `value` already includes the current draft. Synchronize silently; `sync_value` does
            // not emit Change and therefore cannot loop staged edits.
            let state = state.clone();
            state.update(cx, |s, cx| s.sync_value(value.clone(), cx));
            return state;
        }
        let state = cx.new(|cx| MoonInputState::new(window, cx).default_value(value));
        cx.subscribe(&state, move |this, state, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                let value = state.read(cx).value().to_string();
                this.stage_field_value(keys.as_ref(), &field, value, cx);
            }
        })
        .detach();
        self.field_inputs.insert(id, state.clone());
        state
    }

    fn field_memo_state(
        &mut self,
        id: String,
        value: String,
        keys: Arc<Vec<Key>>,
        field: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonTextAreaState> {
        if let Some(state) = self.field_memos.get(&id) {
            let cur = state.read(cx).value().to_string();
            if cur == value {
                return state.clone();
            }
            // As in `field_input_state`, synchronize silently to the current value.
            let state = state.clone();
            state.update(cx, |s, cx| s.sync_value(value.clone(), cx));
            return state;
        }
        let state = cx.new(|cx| MoonTextAreaState::new(window, cx).default_value(value));
        cx.subscribe(&state, move |this, state, ev: &MoonTextAreaEvent, cx| {
            if matches!(ev, MoonTextAreaEvent::Change) {
                let value = state.read(cx).value().to_string();
                this.stage_field_value(keys.as_ref(), &field, value, cx);
            }
        })
        .detach();
        self.field_memos.insert(id, state.clone());
        state
    }

    /// Build a MoonUI swatch/palette picker for a Color field.
    /// A selection stages a new hexadecimal value while preserving its Moonbot `AARRGGBB` alpha
    /// prefix. States are cached by row id and recreated when the external RGB value changes.
    fn field_color_state(
        &mut self,
        id: String,
        hex: &str,
        keys: Arc<Vec<Key>>,
        field: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonColorPickerState> {
        let rgb_val = parse_hex_rgb(hex);
        if let Some((cached, state)) = self.field_colors.get(&id) {
            if *cached == rgb_val {
                return state.clone();
            }
        }
        let init: Hsla = gpui::rgb(design::rgb_to_u32(rgb_val)).into();
        let state = cx.new(|cx| MoonColorPickerState::new(window, cx).default_value(init));
        let prefix = hex_alpha_prefix(hex);
        cx.subscribe(&state, move |this, _st, ev: &MoonColorPickerEvent, cx| {
            let MoonColorPickerEvent::Change(h) = ev;
            let c = design::hsla_to_rgb8(*h);
            let hexv = format!("{prefix}{:02X}{:02X}{:02X}", c[0], c[1], c[2]);
            this.stage_field_value(keys.as_ref(), &field, hexv, cx);
        })
        .detach();
        self.field_colors.insert(id, (rgb_val, state.clone()));
        state
    }

    fn append_formula_snippet(&mut self, field: &str, snippet: &str, cx: &mut Context<Self>) {
        let needle = field_id(field);
        let state = self
            .field_memos
            .iter()
            .find_map(|(id, state)| id.contains(&needle).then_some(state.clone()));
        let current = state
            .as_ref()
            .map(|state| state.read(cx).value().to_string())
            .unwrap_or_default();
        let next = append_snippet(&current, snippet);
        if let Some(state) = state {
            state.update(cx, |state, cx| state.sync_value(next, cx));
        }
    }

    /// Insert every cumulative path prefix into `expanded_folders`.
    /// Shared by Expand All and folder creation so a newly created folder is immediately visible.
    pub(super) fn expand_path<'a>(
        &mut self,
        core: CoreId,
        segments: impl Iterator<Item = &'a str>,
    ) {
        let mut acc = String::new();
        for part in segments {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(part);
            self.expanded_folders.insert((core, acc.clone()));
        }
    }

    /// Expand every node when `collapsed` is true; otherwise collapse all nodes.
    fn expand_collapse_toggle(
        &mut self,
        cores: &[(CoreId, String)],
        store: &CoreStore,
        collapsed: bool,
    ) {
        if !collapsed {
            self.expanded_cores.clear();
            self.expanded_folders.clear();
            return;
        }
        for (c, _) in cores {
            self.expanded_cores.insert(*c);
            let Some(cd) = store.core(*c) else { continue };
            let paths: Vec<String> = cd
                .strategies
                .iter()
                .map(|r| r.folder_path.clone())
                .collect();
            for path in paths {
                self.expand_path(*c, tree_ops::path_segments(&path));
            }
        }
    }

    // ── Panel 1: strategy tree ───────────────────────────────────────────────

    fn sections_panel(&self, store: &CoreStore, cx: &Context<Self>) -> AnyElement {
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

/// Panel boundary being dragged.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PanelSplit {
    Tree,
    Versions,
    Sections,
}

/// Splitter drag payload.
#[derive(Clone)]
struct PanelResizeDrag {
    which: PanelSplit,
}

/// Empty splitter drag ghost; the panel itself follows the pointer.
struct SplitGhost;
impl Render for SplitGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// Width of a panel splitter in logical pixels.
const PANEL_SPLIT_W: f32 = 5.0;
/// Width of the collapsed Versions strip.
const VERSIONS_COLLAPSED_W: f32 = 22.0;

impl StrategiesView {
    /// Persist panel widths and Versions collapse state in layout.
    /// The debounced save loop drains `layout_dirty`, as it does for window geometry.
    pub(super) fn save_panels(&mut self, cx: &mut Context<Self>) {
        let mut p = self.panels;
        p.versions_collapsed = self.versions.collapsed;
        self.panels = p;
        self.backend.update(cx, |b, _| {
            let cur = b.layout.strategies_panels;
            if (
                cur.tree_w,
                cur.versions_w,
                cur.sections_w,
                cur.versions_collapsed,
            ) != (p.tree_w, p.versions_w, p.sections_w, p.versions_collapsed)
            {
                b.layout.strategies_panels = p;
                b.layout_dirty = true;
            }
        });
    }

    /// Build a draggable vertical splitter between panels.
    fn panel_splitter(&self, which: PanelSplit, cx: &Context<Self>) -> AnyElement {
        let p = MoonPalette::active(cx);
        div()
            .id(SharedString::from(format!("strat-split-{which:?}")))
            .w(px(PANEL_SPLIT_W))
            .h_full()
            .flex_none()
            .cursor(CursorStyle::ResizeLeftRight)
            .hover(move |s| s.bg(moon_alpha(p.blue, 0.35)))
            .on_drag(PanelResizeDrag { which }, |_, _, _, cx| {
                cx.new(|_| SplitGhost)
            })
            .into_any_element()
    }

    /// Resize a panel from the pointer's window-relative position minus the panel's left edge.
    /// Absolute positioning remains stable when intermediate drag events are missed.
    fn on_panel_split_drag(&mut self, x: f32, which: PanelSplit, cx: &mut Context<Self>) {
        match which {
            PanelSplit::Tree => {
                self.panels.tree_w = (x - PANEL_SPLIT_W / 2.0).clamp(240.0, 900.0);
            }
            PanelSplit::Versions => {
                self.panels.versions_w =
                    (x - self.panels.tree_w - PANEL_SPLIT_W * 1.5).clamp(90.0, 420.0);
            }
            PanelSplit::Sections => {
                let vers = if self.versions.collapsed {
                    VERSIONS_COLLAPSED_W
                } else {
                    self.panels.versions_w + PANEL_SPLIT_W
                };
                self.panels.sections_w =
                    (x - self.panels.tree_w - PANEL_SPLIT_W - vers - PANEL_SPLIT_W / 2.0)
                        .clamp(150.0, 520.0);
            }
        }
        self.save_panels(cx);
        cx.notify();
    }
}

fn strategies_sig(b: &Backend) -> u64 {
    let store = b.session.store();
    b.session
        .sessions()
        .iter()
        .filter_map(|s| store.core(s.id))
        .fold(0u64, |a, c| {
            a.wrapping_mul(31)
                .wrapping_add(c.strategies_rev)
                .wrapping_mul(31)
                .wrapping_add(c.schema_rev)
        })
}

impl EventEmitter<()> for StrategiesView {}
impl Focusable for StrategiesView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for StrategiesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Drain navigation from an order-line context menu or Orders Strat click before building
        // the tree so filter, expansion, and selection changes appear in this frame.
        let goto = self.drain_goto(window, cx);

        // Root nodes are connected cores in canonical order.
        let cores = {
            let b = self.backend.read(cx);
            crate::core_order::CoreOrder::new(&b.config)
                .from_sessions(b.session.sessions(), |_| true)
        };

        // Build the owned MoonTree adapter without leaking a store borrow, then synchronize state.
        let build = {
            let store = self.backend.read(cx).session.store();
            tree_moon::build(self, store, &cores)
        };
        self.flat_order = build.flat;
        let searching = build.searching;
        self.tree_state.update(cx, |st, c| {
            st.set_items(build.items, c);
            st.set_force_expanded(searching, c);
            st.set_expanded(build.expanded_ids, c);
            // Navigate by temporarily selecting the MoonTree item to obtain its index and scroll
            // to it, then clear that selection because `sel` renders the highlight.
            if let Some((core, id)) = goto {
                let item = MoonTreeItem::new(tree_moon::id_strat(core, id), "");
                st.set_selected_item(Some(&item), c);
                if let Some(ix) = st.selected_index() {
                    st.scroll_to_item(ix, ScrollStrategy::Center);
                }
                st.set_selected_item(None, c);
            }
        });
        let node_data = std::rc::Rc::new(build.node_data);

        // Prepare the Versions panel and deleted-strategy cache before borrowing the store because
        // they can spawn background loads.
        self.ensure_deleted(cx);
        let versions = self.versions_panel(cx);
        let (tree, sections, params_model) = {
            let store = self.backend.read(cx).session.store();
            (
                self.tree_panel(store, &cores, node_data, cx),
                self.sections_panel(store, cx),
                self.params_model(store),
            )
        };
        let params = self.params_panel(params_model, window, cx);
        let split_tree = self.panel_splitter(PanelSplit::Tree, cx);
        let split_versions =
            (!self.versions.collapsed).then(|| self.panel_splitter(PanelSplit::Versions, cx));
        let split_sections = self.panel_splitter(PanelSplit::Sections, cx);

        let p = MoonPalette::active(cx);
        let chrome_width = match window.window_bounds() {
            WindowBounds::Windowed(b)
            | WindowBounds::Maximized(b)
            | WindowBounds::Fullscreen(b) => f32::from(b.size.width),
        };
        let mut root = v_flex()
            .size_full()
            .relative()
            .bg(moon(p.shell))
            .text_color(moon(p.text))
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .line_height(design::line_px(cx, 14.0))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                this.handle_tree_key(ev, window, cx);
            }))
            // Resize panel splitters; resulting widths are persisted in layout.
            .on_drag_move(
                cx.listener(|this, e: &DragMoveEvent<PanelResizeDrag>, _window, cx| {
                    let which = e.drag(cx).which;
                    let x = f32::from(e.event.position.x);
                    this.on_panel_split_drag(x, which, cx);
                }),
            )
            .child(strategies_header(p, cx))
            .child(
                h_flex()
                    .flex_1()
                    .w_full()
                    .min_h_0()
                    .child(tree)
                    .child(split_tree)
                    .child(versions)
                    .children(split_versions)
                    .child(sections)
                    .child(split_sections)
                    .child(params),
            );
        root = root.child(
            MoonWindowFrame::tool("strategies-window-frame-hit", chrome_width)
                .header_height(STRATEGIES_HEADER_H)
                .leading_inset(design::titlebar_leading_inset())
                .show_controls(design::show_custom_window_controls())
                .hit_overlay(),
        );
        root
    }
}

fn strategies_header(p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .id("strategies-window-header")
        .relative()
        .flex_none()
        .w_full()
        .h(design::fit_h_px(cx, STRATEGIES_HEADER_H, 14.0, 9.0))
        .justify_between()
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        .bg(moon(p.shell_high))
        .border_b(px(1.0))
        .border_color(moon_alpha(p.border, 1.0))
        .child(
            MoonWindowFrame::tool("strategies-titlebar-title", 0.0)
                .title_cluster(t!("strat.window_title").to_string(), cx)
                .h_full()
                .flex_1()
                .min_w_0(),
        )
        .when(design::show_custom_window_controls(), |this| {
            this.child(
                MoonWindowFrame::tool("strategies-window-frame-visual", 0.0)
                    .header_height(STRATEGIES_HEADER_H)
                    .show_controls(true)
                    .visual_controls(cx),
            )
        })
}

/// Open or focus the Strategies window and navigate to `strat_id` on `core`.
/// Render drains the request, disables the active-only filter, expands the target core and folders,
/// and selects the strategy. Entry points include chart order-line context menus and the Orders
/// table's Strat column.
pub fn open_goto(
    backend: Entity<Backend>,
    core: CoreId,
    strat_id: u64,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    cx: &mut App,
) {
    backend.update(cx, |b, bcx| {
        b.strategies_goto = Some((core, strat_id));
        // Wake an existing window's observer because `open` only focuses a deduplicated window.
        bcx.notify();
    });
    open(backend, owner, owner_display, cx);
}

/// Open the Strategies tool window, deduplicated through `Backend`.
pub fn open(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    cx: &mut App,
) {
    // Focus an existing window.
    if let Some(handle) = backend.read(cx).strategies_window {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    // The tool window behaves as part of the terminal rather than a separate taskbar application.
    // Restore geometry persisted by `StrategiesView`.
    let saved = backend.read(cx).layout.strategies_window;
    let bounds = saved.map_or(
        Bounds {
            origin: point(px(120.0), px(90.0)),
            size: size(px(1180.0), px(680.0)),
        },
        |g| Bounds {
            origin: point(px(g.x as f32), px(g.y as f32)),
            size: size(px(g.w as f32), px(g.h as f32)),
        },
    );
    // Choose a display from the saved position where supported, otherwise from the owner. Without a
    // display id, GPUI creates the window on the primary display and may discard off-screen bounds.
    let display_id = crate::windowing::saved_or_owner_display_id(
        saved.map(|g| point(px(g.x as f32), px(g.y as f32))),
        owner,
        owner_display,
        cx,
    );
    let mut opts = crate::windowing::tool_window_options(
        t!("strat.window_title").to_string(),
        WindowBounds::Windowed(bounds),
        Some(size(px(920.0), px(560.0))),
        owner,
    );
    opts.display_id = display_id;
    let b = backend.clone();
    if let Ok(handle) = cx.open_window(opts, move |window, cx| {
        crate::windowing::configure_shell_clear_color(window, cx);
        let view = cx.new(|cx| StrategiesView::new(b, window, cx));
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque))
    }) {
        backend.update(cx, |bk, _| bk.strategies_window = Some(handle));
        crate::windowing::activate_new_window(handle.into(), cx);
    }
}
