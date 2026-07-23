//! Strategies window, ported from egui's `src/strategies/*` and `window/strategies_window.rs`.
//! The separate OS window contains tree, versions, schema-section, and parameter panels. Its
//! core/folder/strategy tree supports search, filters, staged checkboxes, and start/stop Apply;
//! schema sections dim inactive entries, while parameter rows support read-only, YES/NO, and long
//! values. Field and section dependencies load from `assets/param_deps.toml` through [`rules`] and
//! hot-reload only when `MOON_STRATEGY_RULES_HOT_RELOAD` is set.
//! The view reads the live per-core Backend store, and Apply sends checkbox changes plus start/stop
//! through `session.apply_strategies`.

mod actions;
mod fields;
mod filter;
mod logic;
mod params;
mod rules;
mod sections;
mod selection;
mod split;
mod state;
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
mod window;

use split::{PanelResizeDrag, PanelSplit};
use window::{STRATEGIES_HEADER_H, strategies_header};
pub use window::{open, open_goto};

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
