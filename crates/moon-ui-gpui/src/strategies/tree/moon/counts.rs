//! MoonTree counts helpers.

use super::*;

/// What a core row's orders slot draws when the core has no open order: a dash, so an empty slot
/// cannot be mistaken for a count that failed to arrive.
pub(super) const ORDERS_NONE: &str = "—";

/// The trailing counter column of one heading row, already rendered to strings.
///
/// The counters used to be concatenated onto the end of the caption, which put them at a different
/// x on every row and made a column of fifty cores read as noise. They are their own element now,
/// so the caption keeps the flexible truncating slot and the numbers keep a fixed one.
pub(super) struct RowCounts {
    /// Left slot: "active of total on" for a core or folder, the bare count for the Deleted heading.
    pub(super) primary: String,
    /// Right slot: "N ord." on a core, [`ORDERS_NONE`] on a core with none, empty on any other row.
    /// The slot is reserved either way — see [`ORDERS_SLOT_W`].
    pub(super) orders: String,
    /// [`Self::primary`] in the compact form: `57/57`, or the same text where there is no fraction.
    pub(super) primary_short: String,
    /// [`Self::orders`] in the compact form: the bare count, the dash, or empty.
    pub(super) orders_short: String,
    /// Localized tooltip naming exactly the numbers this row actually shows.
    pub(super) tip: SharedString,
    /// Whether the row counts no strategy at all, which a core row draws dimmed.
    pub(super) empty: bool,
}

impl RowCounts {
    /// Counters for a core or folder heading, whose numbers cover its whole subtree.
    ///
    /// Spelled with words — "57 of 57 on" and "50 ord." — rather than the bare `57/57 (50)`, which
    /// read as one three-part figure. The numbers themselves are unchanged: a stopped core still
    /// counts zero running, as Moonbot's own Strategies window does.
    ///
    /// Args:
    ///     active: Checked strategies under this heading, after the kind and side filters.
    ///     total: All strategies under it, after the same filters.
    ///     open_orders: Open orders of the whole core, or `None` for a folder, which carries no
    ///         order count of its own and leaves the orders slot blank rather than drawing a dash.
    ///     engine: The core's confirmed global strategy engine: `Some(true)` running,
    ///         `Some(false)` stopped, `None` when the core has not confirmed either.
    ///
    /// Returns:
    ///     The two slot strings plus the tooltip that names whichever of them is populated.
    pub(super) fn subtree(
        active: usize,
        total: usize,
        open_orders: Option<usize>,
        engine: Option<bool>,
    ) -> Self {
        let counts_tip = match engine {
            Some(false) => rust_i18n::t!("strat.tree_counts_tip_stopped", n = active).to_string(),
            Some(true) => rust_i18n::t!("strat.tree_counts_tip_running").to_string(),
            None => rust_i18n::t!("strat.tree_counts_tip").to_string(),
        };
        let on = if matches!(engine, Some(false)) {
            0
        } else {
            active
        };
        let orders = open_orders.unwrap_or(0);
        Self {
            primary: rust_i18n::t!("strat.tree_counts_words", on = on, total = total).to_string(),
            orders: match open_orders {
                None => String::new(),
                Some(0) => ORDERS_NONE.to_string(),
                Some(n) => rust_i18n::t!("strat.tree_orders_words", n = n).to_string(),
            },
            primary_short: format!("{on}/{total}"),
            orders_short: match open_orders {
                None => String::new(),
                Some(0) => ORDERS_NONE.to_string(),
                Some(n) => n.to_string(),
            },
            tip: SharedString::from(if orders > 0 {
                format!(
                    "{counts_tip} · {}",
                    rust_i18n::t!("strat.tree_open_orders_tip")
                )
            } else {
                counts_tip
            }),
            empty: total == 0,
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
            primary_short: String::new(),
            orders_short: String::new(),
            tip: SharedString::from(tip.unwrap_or_default()),
            empty: true,
        }
    }

    /// Counters for a core's Deleted heading, which carries one number and no orders.
    pub(super) fn deleted(count: usize) -> Self {
        Self {
            primary: count.to_string(),
            orders: String::new(),
            primary_short: count.to_string(),
            orders_short: String::new(),
            tip: SharedString::from(rust_i18n::t!("strat.tree_deleted_count_tip").to_string()),
            empty: false,
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

/// The status pill a core row opens with, decided from the engine state the tree already reads.
///
/// The same `Option<bool>` that [`super::super::checks::strategy_state_style`] is handed as
/// `engine_running`, filtered to the CURRENT connection's confirmation by `build_core_root` — so the
/// pill and the strategy checkboxes cannot disagree about whether a core runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EnginePill {
    /// The core confirmed its strategy engine runs: green "running".
    Running,
    /// The core reported its strategy engine stopped: amber "stop".
    Stopped,
    /// Nothing confirmed on this connection: a neutral pill with no claim in it.
    Unknown,
}

impl EnginePill {
    /// Map the confirmed engine state to its pill.
    ///
    /// Args:
    ///     engine: `Some(true)` running, `Some(false)` stopped, `None` unconfirmed.
    ///
    /// Returns:
    ///     The pill to draw.
    pub(crate) fn from_engine(engine: Option<bool>) -> Self {
        match engine {
            Some(true) => Self::Running,
            Some(false) => Self::Stopped,
            None => Self::Unknown,
        }
    }

    /// The pill's localized word, empty for [`Self::Unknown`], which draws only its dot.
    pub(crate) fn label(self) -> String {
        match self {
            Self::Running => rust_i18n::t!("strat.tree_engine_running_tag").to_string(),
            Self::Stopped => rust_i18n::t!("strat.tree_engine_stopped_tag").to_string(),
            Self::Unknown => String::new(),
        }
    }
}

/// Render a core row's status pill: a dot plus the engine word on a tinted ground.
///
/// A minimum width keeps the core captions after it on one column whichever word a row shows; a
/// locale whose word is longer grows its own pill rather than clipping it.
///
/// Args:
///     pill: State to draw.
///     compact: Whether the tree is narrow enough to draw the dot alone; the word then lives
///         in the counters' tooltip.
///     step: Local unscaled text-size step, so the pill rides the row's own text size.
///     app: Application context used for palette and scaled geometry.
///
/// Returns:
///     A `flex_none` pill element.
pub(crate) fn engine_pill(pill: EnginePill, compact: bool, step: f32, app: &App) -> AnyElement {
    let p = MoonPalette::active(app);
    let (fg, bg) = match pill {
        EnginePill::Running => (p.green, moon_alpha(p.green, 0.16)),
        EnginePill::Stopped => (p.amber, moon_alpha(p.amber, 0.18)),
        EnginePill::Unknown => (p.text_muted, moon_alpha(p.text_muted, 0.12)),
    };
    let dot = design::ui_px(app, 6.0);
    h_flex()
        .flex_none()
        .when(!compact, |pill| pill.min_w(design::ui_px(app, PILL_MIN_W)))
        .h(design::ui_px(app, ROW_LINE_BASE + step + 2.0))
        .px(design::ui_px(app, 6.0))
        .gap(design::ui_px(app, 4.0))
        .items_center()
        .rounded(design::ui_px(app, 8.0))
        .bg(bg)
        .child(div().flex_none().size(dot).rounded_full().bg(rgb(fg)))
        .when(!compact, |el| {
            el.child(
                MoonText::new(pill.label())
                    .mono(false)
                    .uppercase(false)
                    .color(fg)
                    .weight(500.0)
                    .font_size(design::body_font_base(app, step) - 1.5)
                    .line_height(ROW_LINE_BASE + step)
                    .render(),
            )
        })
        .into_any_element()
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
