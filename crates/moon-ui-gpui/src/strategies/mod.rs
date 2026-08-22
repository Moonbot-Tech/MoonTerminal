//! Strategies window for inspecting and editing each connected core's strategies.
//! The separate OS window contains tree, versions, schema-section, and parameter panels. Its
//! core/folder/strategy tree optionally groups cores by venue and supports search, an active-only
//! display preference, staged checkboxes, and start/stop Apply; schema sections dim inactive
//! entries, while parameter rows support read-only, YES/NO, and long values. Field and section
//! dependencies load from `assets/param_deps.toml` through [`rules`] and hot-reload only when
//! `MOON_STRATEGY_RULES_HOT_RELOAD` is set.
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
mod settings;
mod split;
mod state;
// `pub(crate)` exposes `unique_name`, `set_field`, and `STRATEGY_NAME_FIELD` to the Analytics
// tuner's Make Copy operation.
pub(crate) mod tree;
mod versions;
mod window;

use split::{PanelResizeDrag, PanelSplit};
use tree::pane_cache::{LeftPaneFrame, PaneCache};
pub(crate) use window::StrategyRevealRequest;
pub use window::{RevealTarget, open, open_goto, reveal_name};
use window::{STRATEGIES_HEADER_H, strategies_header};

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBackgroundPolicy, MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox,
    MoonCheckboxSize, MoonColorPicker, MoonColorPickerEvent, MoonColorPickerState, MoonDropdown,
    MoonInput, MoonInputEvent, MoonInputState, MoonMenuItem, MoonMenuSize, MoonPalette,
    MoonTextArea, MoonTextAreaEvent, MoonTextAreaState, MoonTone, MoonTreeEvent, MoonTreeItem,
    MoonTreeState, MoonWindowFrame, Root, h_flex, v_flex,
};

use crate::design::{moon, moon_alpha};
use crate::{Backend, design};
use moon_core::feed::{SchemaField, SchemaFieldUi, SchemaSection, StrategyRow};
use moon_core::session::{CoreId, CoreStore};
use rust_i18n::t;

use filter::StrategyFilter;
use logic::*;
use rules::{Rules, Values};
use settings::StrategiesPrefs;

pub type Key = (CoreId, u64);
type FieldEditKey = (CoreId, u64, String);

/// State and renderer for the four-panel Strategies window.
pub struct StrategiesView {
    backend: Entity<Backend>,
    /// IANA zone selected by the shared header clock for user-facing version dates.
    display_zone: chrono_tz::Tz,
    /// Search input whose value is synchronized into the filter.
    search: Entity<MoonInputState>,
    /// Tree filters for kind, direction, and search text synchronized from the input.
    filter: StrategyFilter,
    /// Resolved persisted display preferences for grouping and active-row visibility.
    prefs: StrategiesPrefs,
    /// UI-thread edge proving exchange logos were prewarmed off the tree render path.
    exchange_logos_ready: bool,
    /// Whether the settings popover is open in this process.
    settings_open: bool,
    /// Concrete singleton Auto scope; `None` leaves the all-core Classic tree authoritative.
    workspace_cores: Option<Vec<CoreId>>,
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
    /// Advances whenever `deleted` actually changes.
    ///
    /// Separate from `deleted_gen`, which moves when a load STARTS: the load is asynchronous, so its
    /// result lands on a later frame, and a signature keyed on the generation would already have
    /// cached the tree built without it. See [`tree::cache::data_sig`].
    deleted_rev: u64,
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
    /// Selection and staging remain above; `MoonTreeState` only renders and exposes decorator hitboxes.
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
    clipboard: Option<Vec<tree::ops::ClipItem>>,
    /// Names submitted for creation but not yet echoed by the core, keyed by core and name.
    /// Reserving them prevents rapid pastes from reading one store snapshot and generating the same
    /// name repeatedly. A reservation is cleared once the name appears in the store.
    pending_names: HashSet<(CoreId, String)>,
    /// Click-selected folder or core root used for highlighting and subtree Ctrl+C copying.
    /// An empty path identifies the selected core root without erasing a stale strategy selection.
    selected_folder: Option<(CoreId, String)>,
    /// Empty UI folders before their first strategy is added, keyed by core and slash-separated path.
    ui_folders: HashSet<(CoreId, String)>,
    /// Active create, rename, or confirmation modal for a tree operation.
    op: Option<tree::ui::TreeOp>,
    /// Create/rename modal input, recreated on each opening to use the current initial value.
    op_input: Option<Entity<MoonInputState>>,
    /// Initial value used when rendering the next `op_input` instance.
    op_input_init: String,
    /// Strategy expected from a core echo after create/paste, keyed by core and name.
    /// Selected in the tree when it arrives, then cleared.
    pending_select: Option<StrategyRevealRequest>,
    /// Signature of strategy and schema data that materially changes the window.
    last_sig: u64,
    /// Signature of the tree shape last pushed into [`MoonTreeState`]: node ids, labels, folder
    /// flags, expansion, and whether a search is active.
    ///
    /// Re-pushing identical items incurs two deep clones of the item forest inside MoonTree because
    /// `set_items` and `set_expanded` each rebuild its flattened entries. `None` forces the next
    /// frame to push unconditionally.
    ///
    /// Only the SHAPE is cached. Row contents (`NodeData`: selection, staging, checkboxes, order
    /// counts, drag payloads) are rebuilt every frame regardless, so nothing here can render stale.
    last_tree_shape: Option<u64>,
    /// Previous frame's tree adapter, reused while its input signature is unchanged.
    /// See [`tree::cache`] for why a per-frame rebuild is the wrong cost and what keeps it honest.
    tree_cache: Option<tree::cache::TreeCache>,
    /// Previous frame's left-pane derivations: the kinds list, the Start/Stop plan and the footer
    /// label width. Same argument as `tree_cache`, for the rest of the pane — see [`pane_cache`].
    pane_cache: PaneCache,
    /// A key `sync_pending_select` resolved off-frame, waiting for render to scroll to it.
    ///
    /// Selection alone does not bring a row on screen: only `MoonTreeState::scroll_to_item`
    /// does, and it needs the tree's item index, which exists only inside render.
    pending_scroll: Option<Key>,
    focus: FocusHandle,
}

impl Render for StrategiesView {
    /// Renders the window and uses a structural signature to avoid redundant MoonTree pushes.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::bump(&crate::diag::STRAT_RENDER);
        // Drain navigation from an order-line context menu or Orders Strat click before building
        // the tree so filter, expansion, and selection changes appear in this frame.
        // A reveal by name may precede the core echo that assigns its id. Both immediate and
        // echo-resolved targets feed the same scroll path so selection and visibility stay coupled.
        let direct = self.drain_goto(window, cx);
        // Take the queued target even when a direct request wins; otherwise a later repaint could
        // consume it and move the selection away from the direct request.
        let queued = self.pending_scroll.take();
        let goto = direct.or(queued);

        // The grouped and flat tree modes consume the same connected cores in canonical order.
        let cores = {
            let b = self.backend.read(cx);
            visible_strategy_cores(self, b)
        };

        // Build the owned MoonTree adapter without leaking a store borrow, then synchronize state.
        //
        // Only when the data behind it moved: a hover highlight repaints this window at monitor
        // rate, and rebuilding the whole adapter for it measured 1.0-1.3 ms per frame on a live
        // account. `tree::cache` owns the signature and the argument for why it is complete.
        let tree_timer = crate::diag::timer();
        let sig = {
            let backend = self.backend.read(cx);
            let store = backend.session.store();
            let venues = backend.session.core_venues();
            tree::cache::data_sig(self, store, &cores, venues)
        };
        let cached = self.tree_cache.as_ref().and_then(|c| c.get(sig));
        let node_data = match cached {
            // `flat_order` is not restored: this frame is the only writer, so it already holds the
            // order that belongs to this very `node_data`.
            Some(node_data) => node_data,
            None => {
                crate::diag::bump(&crate::diag::STRAT_TREE_BUILD);
                // Attribute the miss before the entry is replaced: on a busy account this is the
                // difference between "the cores moved" and "the signature churns".
                if let Some((store_moved, view_moved)) =
                    self.tree_cache.as_ref().map(|c| c.moved(sig))
                {
                    if store_moved {
                        crate::diag::bump(&crate::diag::STRAT_SIG_STORE);
                    }
                    if view_moved {
                        crate::diag::bump(&crate::diag::STRAT_SIG_VIEW);
                    }
                }
                let build = {
                    let backend = self.backend.read(cx);
                    let store = backend.session.store();
                    let venues = backend.session.core_venues();
                    tree::moon::build(self, store, &cores, venues)
                };
                crate::diag::bump_by(&crate::diag::STRAT_TREE_NODES, build.node_data.len() as u64);
                let searching = build.searching;
                self.flat_order = build.flat;
                // Push the forest into MoonTree only when its shape actually changed. A miss does
                // not imply a new shape: staging a checkbox or selecting a row changes row CONTENT,
                // which the signature must see and the forest must not be rebuilt for.
                let shape = tree::moon::shape_sig(&build.items, &build.expanded_ids, searching);
                if self.last_tree_shape != Some(shape) {
                    self.last_tree_shape = Some(shape);
                    crate::diag::bump(&crate::diag::STRAT_TREE_PUSH);
                    self.tree_state.update(cx, |st, c| {
                        st.set_items(build.items, c);
                        st.set_force_expanded(searching, c);
                        st.set_expanded(build.expanded_ids, c);
                    });
                }
                let node_data = std::rc::Rc::new(build.node_data);
                self.tree_cache = Some(tree::cache::TreeCache::store(sig, node_data.clone()));
                node_data
            }
        };
        crate::diag::record_us(&crate::diag::STRAT_TREE_US, tree_timer);
        // The rest of the left pane, on the same terms as the adapter above. Timed into the pane's
        // counter together with its element tree below — see `strat_pane_build` in `diag.rs` for
        // how the two are told apart.
        let pane_timer = crate::diag::timer();
        let pane = self.left_pane_frame(sig, &cores, cx);
        crate::diag::record_us(&crate::diag::STRAT_TREEPANE_US, pane_timer);
        // Navigate by temporarily selecting the MoonTree item to obtain its index and scroll to
        // it, then clear that selection because `sel` renders the highlight. Independent of the
        // push above: an unchanged shape means MoonTree already holds the forest to resolve
        // against, and the reveal path expanded the target's core and folder chain before this.
        if let Some((core, id)) = goto {
            self.tree_state.update(cx, |st, c| {
                let item = MoonTreeItem::new(tree::moon::id_strat(core, id), "");
                st.set_selected_item(Some(&item), c);
                if let Some(ix) = st.selected_index() {
                    st.scroll_to_item(ix, ScrollStrategy::Center);
                }
                st.set_selected_item(None, c);
            });
        }

        // Prepare the Versions panel and deleted-strategy cache before borrowing the store because
        // they can spawn background loads.
        //
        // Each pane is timed on its own: a mouse-over repaints this whole window, so the question
        // "which pane does that cost" has to be answerable without a second measuring run.
        let t = crate::diag::timer();
        self.ensure_deleted(cx);
        let versions = self.versions_panel(cx);
        crate::diag::record_us(&crate::diag::STRAT_VERSIONS_US, t);
        let (tree, sections, params_model) = {
            let store = self.backend.read(cx).session.store();
            let t = crate::diag::timer();
            let tree = self.tree_panel(store, &cores, node_data, &pane, cx);
            crate::diag::record_us(&crate::diag::STRAT_TREEPANE_US, t);
            // Both panels read the same dependency values; build them once. The computed halves are
            // timed apart from the element trees because only they could be cached the way the tree
            // adapter is — see `strat_model_us`.
            let t = crate::diag::timer();
            let values = selected_values(self, store);
            crate::diag::record_us(&crate::diag::STRAT_MODEL_US, t);
            let t = crate::diag::timer();
            let sections = self.sections_panel(store, &values, cx);
            crate::diag::record_us(&crate::diag::STRAT_SECTIONS_US, t);
            let t = crate::diag::timer();
            let params_model = self.params_model(store, values);
            crate::diag::record_us(&crate::diag::STRAT_MODEL_US, t);
            (tree, sections, params_model)
        };
        let t = crate::diag::timer();
        let params = self.params_panel(params_model, window, cx);
        crate::diag::record_us(&crate::diag::STRAT_PARAMS_US, t);
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
