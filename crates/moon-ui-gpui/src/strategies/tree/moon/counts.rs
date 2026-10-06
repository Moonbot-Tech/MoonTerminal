//! MoonTree counts helpers.

use super::*;

/// The trailing counter column of one heading row, already rendered to strings.
///
/// The counters used to be concatenated onto the end of the caption, which put them at a different
/// x on every row and made a column of fifty cores read as noise. They are their own element now,
/// so the caption keeps the flexible truncating slot and the numbers keep a fixed one.
pub(super) struct RowCounts {
    /// Left slot: `active/total` for a core or folder, the bare count for the Deleted heading.
    pub(super) primary: String,
    /// Right slot: the open-orders `(N)`, empty when the row has none. The slot is reserved either
    /// way — see [`ORDERS_SLOT_W`].
    pub(super) orders: String,
    /// Localized tooltip naming exactly the numbers this row actually shows.
    pub(super) tip: SharedString,
}

impl RowCounts {
    /// Counters for a core or folder heading, whose numbers cover its whole subtree.
    ///
    /// Args:
    ///     active: Checked strategies under this heading, after the kind and side filters.
    ///     total: All strategies under it, after the same filters.
    ///     open_orders: Open orders of the whole core; always zero for a folder, which does not
    ///         carry an order count of its own.
    ///     engine: The core's confirmed global strategy engine: `Some(true)` running,
    ///         `Some(false)` stopped, `None` when the core has not confirmed either.
    ///
    /// Returns:
    ///     The two slot strings plus the tooltip that names whichever of them is populated.
    pub(super) fn subtree(
        active: usize,
        total: usize,
        open_orders: usize,
        engine: Option<bool>,
    ) -> Self {
        let counts_tip = match engine {
            Some(false) => rust_i18n::t!("strat.tree_counts_tip_stopped", n = active).to_string(),
            Some(true) => rust_i18n::t!("strat.tree_counts_tip_running").to_string(),
            None => rust_i18n::t!("strat.tree_counts_tip").to_string(),
        };
        Self {
            primary: if matches!(engine, Some(false)) {
                format!("0/{total}")
            } else {
                format!("{active}/{total}")
            },
            orders: if open_orders > 0 {
                format!("({open_orders})")
            } else {
                String::new()
            },
            tip: SharedString::from(if open_orders > 0 {
                format!(
                    "{counts_tip} · {}",
                    rust_i18n::t!("strat.tree_open_orders_tip")
                )
            } else {
                counts_tip
            }),
        }
    }

    /// Counters for a folder holding nothing: no numbers, and a tooltip that says why.
    ///
    /// Both slots stay reserved by the row itself, so an empty folder's caption keeps the column
    /// its siblings' captions sit on.
    ///
    /// Args:
    ///     tip: What this row is, from [`FolderFill::empty_tip`].
    ///
    /// Returns:
    ///     Empty counters carrying that tooltip.
    pub(super) fn empty_folder(tip: Option<String>) -> Self {
        Self {
            primary: String::new(),
            orders: String::new(),
            tip: SharedString::from(tip.unwrap_or_default()),
        }
    }

    /// Counters for a core's Deleted heading, which carries one number and no orders.
    pub(super) fn deleted(count: usize) -> Self {
        Self {
            primary: count.to_string(),
            orders: String::new(),
            tip: SharedString::from(rust_i18n::t!("strat.tree_deleted_count_tip").to_string()),
        }
    }
}

/// Render one right-aligned counter slot of a heading row's trailing column.
///
/// Args:
///     text: The slot's number, or empty to reserve the width without drawing anything.
///     width: Minimum slot width in design units — [`COUNTS_SLOT_W`] or [`ORDERS_SLOT_W`].
///     color: Palette token for the number, so the two slots can differ.
///     step: Local unscaled text-size step, so the number rides the row's own text size.
///     app: Application context used for palette and scaled geometry.
///
/// Returns:
///     A `flex_none` slot whose content sits on its right edge.
pub(super) fn counts_slot(
    text: String,
    width: f32,
    color: u32,
    step: f32,
    app: &App,
) -> impl IntoElement {
    h_flex()
        .flex_none()
        .min_w(design::ui_px(app, width))
        .justify_end()
        .child(
            MoonText::new(text)
                .mono(true)
                .uppercase(false)
                .color(color)
                .font_size(design::body_font_base(app, step))
                .line_height(ROW_LINE_BASE + step)
                .render(),
        )
}

/// What a folder row has inside it, which decides how much of a row it draws.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FolderFill {
    /// Holds at least one strategy, filtered away or not.
    Populated,
    /// Holds nothing, and the CORE says so — it keeps this folder in its own tree.
    EmptyOnCore,
    /// Holds nothing and exists only in this window: either the core cannot keep empty folders, or
    /// it has not confirmed this one yet.
    EmptyLocal,
}

impl FolderFill {
    /// Whether this row has anything to expand or to check.
    ///
    /// Both controls are omitted when it does not, because both would otherwise be live controls
    /// that cannot do anything: a caret that opens onto nothing, and a box whose "check every
    /// strategy below" covers no strategy. Their space is still reserved, so the caption of an
    /// empty folder stays on the same column as its siblings'.
    pub(super) fn has_contents(self) -> bool {
        matches!(self, Self::Populated)
    }

    /// The tooltip an empty folder's counter carries, or `None` for a populated one.
    pub(super) fn empty_tip(self) -> Option<String> {
        match self {
            Self::Populated => None,
            Self::EmptyOnCore => Some(rust_i18n::t!("strat.folder_empty_tip").to_string()),
            Self::EmptyLocal => Some(rust_i18n::t!("strat.folder_empty_local_tip").to_string()),
        }
    }
}

pub(super) enum ToggleTarget {
    Core(CoreId),
    Folder(CoreId, Vec<String>),
    /// The core's Deleted folder.
    Deleted(CoreId),
}

/// Resolve the heading's bundled MoonUI icon and passive caret pose.
///
/// Empty folders keep their identity even if retained expansion state says open, but never
/// advertise children. Core and Deleted headings retain their existing disclosure-only chrome.
pub(super) fn heading_chrome(
    target: &ToggleTarget,
    fill: FolderFill,
    expanded: bool,
) -> (Option<&'static str>, Option<bool>) {
    let caret = fill.has_contents().then_some(expanded);
    let icon = matches!(target, ToggleTarget::Folder(..)).then_some(match caret {
        Some(true) => "icons/folder-open.svg",
        _ => "icons/folder-closed.svg",
    });
    (icon, caret)
}

impl ToggleTarget {
    /// Return the core and folder segments this row acts on, or `None` for Deleted.
    ///
    /// One place deciding what a row addresses, so the context menu and the bulk checkbox cannot
    /// disagree about which rows carry a folder identity at all.
    pub(super) fn folder_key(&self) -> Option<(CoreId, Vec<String>)> {
        match self {
            Self::Core(core) => Some((*core, Vec::new())),
            Self::Folder(core, path) => Some((*core, path.clone())),
            Self::Deleted(_) => None,
        }
    }
}
