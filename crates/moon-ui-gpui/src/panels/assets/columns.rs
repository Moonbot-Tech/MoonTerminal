//! Asset-table field descriptor: the selectable columns, their persisted visibility, and the
//! header-click sort.
//!
//! The action buttons are a field like any other, so they can be turned off from the same selector.
//! Everything follows the shared per-context descriptor
//! ([`crate::persistence::table_persist`]) exactly like Orders and Report, so a docked tab and a
//! window keep independent field sets.

use super::*;

/// Validate a stored Assets sort against current sortable and visible columns.
fn restore_sort(
    preference: Option<moon_core::config::TableSortPreference>,
    visible: &[AssetCol],
) -> Option<(AssetCol, bool)> {
    preference.and_then(|preference| {
        AssetCol::from_key(&preference.column)
            .filter(|column| *column != AssetCol::Actions && visible.contains(column))
            .map(|column| (column, preference.ascending))
    })
}

/// One selectable column of the asset table, in canonical (left-to-right) order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum AssetCol {
    Core,
    Coin,
    Qty,
    Value,
    /// Unrealized PnL of the row's position, in the market's USD-stable quote currency.
    Pnl,
    /// Market Sell and Order buttons. It carries no header text and no sort — but it IS selectable,
    /// so a user who only watches balances can drop the trading controls entirely.
    Actions,
}

impl AssetCol {
    /// Canonical column order, also the order the selector menu lists.
    pub(super) const ALL: [AssetCol; 6] = [
        AssetCol::Core,
        AssetCol::Coin,
        AssetCol::Qty,
        AssetCol::Value,
        AssetCol::Pnl,
        AssetCol::Actions,
    ];

    /// Stable storage and table key. It is persisted in `layout.toml`, so it must not change.
    pub(super) fn key(self) -> &'static str {
        match self {
            AssetCol::Core => "core",
            AssetCol::Coin => "coin",
            AssetCol::Qty => "qty",
            AssetCol::Value => "value",
            AssetCol::Pnl => "pnl",
            AssetCol::Actions => "actions",
        }
    }

    /// Resolve a persisted key back to its column, ignoring keys left by a renamed column.
    pub(super) fn from_key(key: &str) -> Option<AssetCol> {
        AssetCol::ALL.into_iter().find(|c| c.key() == key)
    }

    /// Name shown in the FIELD SELECTOR. The action column has no header caption of its own
    /// ([`Self::column`] leaves it blank), so the menu needs its own word for it.
    pub(super) fn title(self) -> String {
        match self {
            AssetCol::Core => t!("assets.col.core").to_string(),
            AssetCol::Coin => t!("assets.col.coin").to_string(),
            AssetCol::Qty => t!("assets.col.qty").to_string(),
            AssetCol::Value => t!("assets.col.value").to_string(),
            AssetCol::Pnl => t!("assets.col.pnl").to_string(),
            AssetCol::Actions => t!("assets.col.actions").to_string(),
        }
    }

    /// Default width in design pixels; a user-dragged width overrides it through `table_persist`.
    fn width(self) -> f32 {
        match self {
            AssetCol::Core => 90.0,
            AssetCol::Coin => 80.0,
            AssetCol::Qty => 130.0,
            AssetCol::Value => 110.0,
            AssetCol::Pnl => 100.0,
            AssetCol::Actions => 170.0,
        }
    }

    /// Whether the column renders right-aligned numbers.
    fn numeric(self) -> bool {
        matches!(self, AssetCol::Qty | AssetCol::Value | AssetCol::Pnl)
    }

    /// Build the MoonUI column.
    ///
    /// Every data column sorts through the panel's header-click handler. The action column does
    /// not: it holds two fixed-width buttons and no value to order by, so it also keeps its title
    /// blank and stays out of the auto-width pool (`no_grow`) — a share of a wide viewport buys it
    /// nothing and only pushes coin/qty/value apart.
    pub(super) fn column(self) -> MoonDataTableColumn {
        if self == AssetCol::Actions {
            return MoonDataTableColumn::new(self.key(), String::new(), self.width()).no_grow();
        }
        let col = MoonDataTableColumn::new(self.key(), self.title(), self.width()).sortable(true);
        if self.numeric() { col.right() } else { col }
    }

    /// Whether this column's cell shows a dash for the row instead of a number to order by.
    ///
    /// PnL only, deliberately. A spot balance has no unrealized PnL, so its dash means "not
    /// applicable" and belongs below comparable positions under either arrow. Value also prints a
    /// dash for a broken price, while Qty prints its non-finite number outright; those columns keep
    /// their previous direction-dependent `cmp_f64` ordering instead of inheriting PnL's domain rule.
    ///
    /// Args:
    ///     e: Asset row whose cell is being classified.
    ///
    /// Returns:
    ///     `true` only when this is the PnL column and the row has no displayable live PnL.
    fn missing_value(self, e: &AssetEntry) -> bool {
        matches!(self, AssetCol::Pnl) && pnl_display(e).is_none()
    }

    /// Compare two rows by this column's displayed value, before applying sort direction.
    ///
    /// Numeric columns compare the SAME number their cell prints (`display_value` for Value, so the
    /// sorted order matches the column and the footer's Σ), never the raw `value` behind it. A
    /// non-finite number sorts last in ascending order instead of poisoning the comparison, which
    /// `partial_cmp` would otherwise leave as `Equal` and scatter broken rows through the table.
    /// Ties break by core and coin so equal values (a table full of dashes, for example) keep a
    /// stable order across rebuilds instead of shuffling every second.
    ///
    /// [`order_rows`] intercepts a PnL row with nothing to display before comparing it with an
    /// ordinary row, so the missing-value rule survives the descending reverse. A direct caller can
    /// still compare such rows here and receives the historical ascending-last ordering.
    ///
    /// Args:
    ///     a: Left row.
    ///     b: Right row.
    ///
    /// Returns:
    ///     Ascending comparison with stable core-and-coin tie-breaking.
    pub(super) fn compare(self, a: &AssetEntry, b: &AssetEntry) -> std::cmp::Ordering {
        let primary = match self {
            AssetCol::Core => a.core_name.cmp(&b.core_name),
            // Case-insensitive: wallet rows carry raw exchange casing, so a byte comparison would
            // sort every mixed-case token after the uppercase tickers.
            AssetCol::Coin => cmp_coin(a, b),
            AssetCol::Qty => cmp_f64(row_qty(a), row_qty(b)),
            AssetCol::Value => cmp_f64(a.display_value, b.display_value),
            AssetCol::Pnl => match (pnl_display(a), pnl_display(b)) {
                (Some(x), Some(y)) => cmp_f64(x, y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            },
            // Not sortable (`column` sets no `sortable`), so no header click can reach this.
            AssetCol::Actions => std::cmp::Ordering::Equal,
        };
        match self {
            // Its own key already IS the tiebreaker, so only the other one is left to apply.
            AssetCol::Core => primary.then_with(|| cmp_coin(a, b)),
            AssetCol::Coin => primary.then_with(|| a.core_name.cmp(&b.core_name)),
            _ => primary
                .then_with(|| a.core_name.cmp(&b.core_name))
                .then_with(|| cmp_coin(a, b)),
        }
    }
}

/// The unrealized PnL a row may DISPLAY, or `None` when it has none to show.
///
/// One source for the cell and the sort: `AssetRow::pnl_live` is the feed's own statement that
/// `pnl_usdt` is `(mark − entry) × size` at a known rate. Without it the field holds the server's
/// period-accumulated profit or an unpriced zero — numbers that must not be printed as unrealized
/// PnL, and must not order rows whose cell shows a dash either.
pub(super) fn pnl_display(e: &AssetEntry) -> Option<f64> {
    (e.row.pnl_live && e.row.pnl_usdt.is_finite()).then_some(e.row.pnl_usdt)
}

/// Order two rows for the header sort, pinning this column's designated missing rows last.
///
/// The pin is applied here rather than inside [`AssetCol::compare`] because direction is applied by
/// reversing the comparison: a "sorts last" rule expressed as an `Ordering` reverses along with
/// everything else and lands those rows FIRST under the descending arrow, which is what the user
/// sees as empty PnL cells at the top. Outside the reverse it holds under both arrows.
///
/// Two pinned rows keep a direction-INDEPENDENT tiebreak, so the block of dashes at the bottom does
/// not reshuffle when the arrow flips. Two ordinary rows keep the existing behaviour exactly: the
/// whole ordering, primary key and core/coin tiebreak alike, reverses under `!ascending`.
///
/// Args:
///     col: Active sort column.
///     ascending: Whether ordinary comparisons retain ascending direction.
///     a: Left row.
///     b: Right row.
///
/// Returns:
///     Comparison that keeps designated missing rows last under either direction.
pub(super) fn order_rows(
    col: AssetCol,
    ascending: bool,
    a: &AssetEntry,
    b: &AssetEntry,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (col.missing_value(a), col.missing_value(b)) {
        (false, true) => Ordering::Less,
        (true, false) => Ordering::Greater,
        // Both pinned: `compare` remains unreversed, so its equal PnL primary falls through to the
        // same core-then-coin tiebreak under either arrow.
        (true, true) => col.compare(a, b),
        (false, false) => {
            let ordering = col.compare(a, b);
            if ascending {
                ordering
            } else {
                ordering.reverse()
            }
        }
    }
}

/// Compare two coin tickers case-insensitively, falling back to the raw bytes for a stable order.
///
/// Byte-wise and allocation-free: this is the Coin sort key AND the universal tiebreaker, so it
/// runs `O(n log n)` times per cache rebuild — a temporary `String` per comparison would allocate
/// thousands of times a second on a many-core scope.
pub(super) fn cmp_coin(a: &AssetEntry, b: &AssetEntry) -> std::cmp::Ordering {
    let (x, y) = (a.row.coin.as_str(), b.row.coin.as_str());
    x.bytes()
        .map(|c| c.to_ascii_uppercase())
        .cmp(y.bytes().map(|c| c.to_ascii_uppercase()))
        .then_with(|| x.cmp(y))
}

/// The quantity a row DISPLAYS: a position's remaining size, otherwise the held spot balance.
///
/// Kept beside [`AssetCol::compare`] and used by the Qty cell so the sort cannot order rows by one
/// number while the column shows another.
pub(super) fn row_qty(e: &AssetEntry) -> f64 {
    if e.row.pos_size != 0.0 {
        e.row.pos_size
    } else if e.row.qty_full.abs() > e.row.qty.abs() {
        e.row.qty_full
    } else {
        e.row.qty
    }
}

/// Compare two numbers, sorting a non-finite one last in ascending order.
fn cmp_f64(a: f64, b: f64) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a.is_finite(), b.is_finite()) {
        (true, true) => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => Ordering::Equal,
    }
}

impl AssetsView {
    /// Visible columns in canonical order, always at least one.
    pub(super) fn visible_cols(&self) -> Vec<AssetCol> {
        AssetCol::ALL
            .into_iter()
            .filter(|c| self.hidden_cols.iter().all(|h| h != c))
            .collect()
    }

    /// Restore the persisted field set for this context (`assets-table:dock` / `:win`).
    ///
    /// An entry listing no known column is ignored: it would hide every field and leave a table
    /// with only the action buttons.
    pub(super) fn apply_ctx_columns(&mut self, cx: &App) {
        let Some(keys) =
            crate::persistence::table_persist::visible(self.backend.read(cx), &self.widths_id)
        else {
            return;
        };
        let shown: Vec<AssetCol> = keys.iter().filter_map(|k| AssetCol::from_key(k)).collect();
        if shown.is_empty() {
            return;
        }
        self.hidden_cols = AssetCol::ALL
            .into_iter()
            .filter(|c| !shown.contains(c))
            .collect();
    }

    /// Restore this dock/window context's valid visible-column sort and MoonUI arrow.
    pub(super) fn apply_ctx_sort(&mut self, cx: &mut Context<Self>) {
        self.sort = restore_sort(
            crate::persistence::table_persist::saved_sort(self.backend.read(cx), &self.widths_id),
            &self.visible_cols(),
        );
        self.table_state.update(cx, |state, _| match self.sort {
            Some((column, ascending)) => state.set_sort(column.key(), ascending),
            None => state.sort_column = None,
        });
    }

    /// Persist the current field set under the context-qualified table ID.
    fn save_ctx_columns(&self, cx: &mut App) {
        let keys: Vec<String> = self
            .visible_cols()
            .into_iter()
            .map(|c| c.key().to_string())
            .collect();
        crate::persistence::table_persist::set_visible(&self.backend, &self.widths_id, keys, cx);
    }

    /// Toggle one field, refusing to hide the last visible one so the table cannot go blank.
    ///
    /// Hiding the column the table is sorted by also clears the sort: its header is gone, so the
    /// order would be frozen on an invisible key with no way back to the default.
    pub(super) fn toggle_col(&mut self, col: AssetCol, cx: &mut Context<Self>) {
        if let Some(at) = self.hidden_cols.iter().position(|c| *c == col) {
            self.hidden_cols.remove(at);
        } else {
            if self.visible_cols().len() == 1 {
                return;
            }
            self.hidden_cols.push(col);
            if self.sort.map(|(c, _)| c) == Some(col) {
                self.clear_sort(cx);
            }
        }
        self.save_ctx_columns(cx);
        cx.notify();
    }

    /// Drop the header sort and restore the collector's default order, largest raw value first.
    ///
    /// It re-applies that default to the rows already in the cache instead of rebuilding: a full
    /// rebuild would also re-request transfer assets from every silent core, and hiding a column
    /// must not send network commands. MoonUI's own sort indicator is cleared too — otherwise the
    /// column would come back carrying a live ↑/↓ arrow over an unsorted table, and its next click
    /// would start descending while every other column starts ascending.
    fn clear_sort(&mut self, cx: &mut Context<Self>) {
        if self.sort.is_none() {
            return;
        }
        self.sort = None;
        let mut rows = (*self.cached_entries).clone();
        super::collect::sort_by_value(&mut rows);
        self.cached_entries = Rc::new(rows);
        self.table_state.update(cx, |state, _| {
            state.sort_column = None;
        });
        crate::persistence::table_persist::set_sort(&self.backend, &self.widths_id, None, cx);
    }

    /// Show every field, or — when all are already shown — leave only the first canonical one.
    ///
    /// Collapsing to one field clears a sort on any field it hides, for the same reason
    /// [`Self::toggle_col`] does: no header, no way back.
    pub(super) fn toggle_all_cols(&mut self, cx: &mut Context<Self>) {
        self.hidden_cols = if self.hidden_cols.is_empty() {
            AssetCol::ALL.into_iter().skip(1).collect()
        } else {
            Vec::new()
        };
        if self
            .sort
            .is_some_and(|(c, _)| self.hidden_cols.contains(&c))
        {
            self.clear_sort(cx);
        }
        self.save_ctx_columns(cx);
        cx.notify();
    }

    /// Apply a header-click sort and reorder the cached rows once.
    ///
    /// Sorting happens in the CACHE, not in `render`: the table is rebuilt on data changes only
    /// (about once a second), so a repaint never re-sorts. The selected column and direction are
    /// persisted per dock/window context; no saved value retains the default largest-value-first
    /// order.
    ///
    /// It reorders the EXISTING rows instead of calling `rebuild_cache`: that path also re-requests
    /// transfer assets from every core that has not answered yet, and a header click must not send
    /// network commands. `cx.notify()` runs even when the order is unchanged, because MoonUI has
    /// already flipped its own header arrow by the time this is called.
    pub(super) fn set_sort(&mut self, key: &str, ascending: bool, cx: &mut Context<Self>) {
        let next = AssetCol::from_key(key).map(|c| (c, ascending));
        if next.is_some() && self.sort != next {
            self.sort = next;
            let mut rows = (*self.cached_entries).clone();
            self.sort_entries(&mut rows);
            self.cached_entries = Rc::new(rows);
            let preference =
                self.sort.map(
                    |(column, ascending)| moon_core::config::TableSortPreference {
                        column: column.key().to_string(),
                        ascending,
                    },
                );
            crate::persistence::table_persist::set_sort(
                &self.backend,
                &self.widths_id,
                preference,
                cx,
            );
        }
        cx.notify();
    }

    /// Order collected rows by the active header sort, leaving `collect`'s own order when there is
    /// none.
    ///
    /// That default is descending RAW `AssetEntry::value`, which is not the Value column: a
    /// USDT-margined position holds no coin balance, so its raw value is ~0 while the column shows
    /// its notional. Clicking the Value header is what orders the table by the number it prints.
    pub(super) fn sort_entries(&self, rows: &mut [AssetEntry]) {
        let Some((col, ascending)) = self.sort else {
            return;
        };
        rows.sort_by(|a, b| order_rows(col, ascending, a, b));
    }

    /// Build the field-selector menu: one checkbox item per column plus All.
    ///
    /// The menu stays open across clicks (`close_on_select(false)`) because choosing fields is
    /// usually several toggles, and the last visible field is disabled so the table keeps a column.
    pub(super) fn columns_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let all_on = self.hidden_cols.is_empty();
        let all_view = view.clone();
        let mut menu = MoonDropdown::new("assets-columns")
            // The shared icon trigger used by every column selector (Orders, Report, Screener);
            // the choice and the childless trigger are `design::COLUMN_SELECTOR_ICON`'s contract.
            .trigger_icon(design::COLUMN_SELECTOR_ICON)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(design::glyph_btn_w(cx))
            .menu_width_scaled(170.0)
            .menu_size(MoonMenuSize::Compact)
            .close_on_select(false)
            .item(
                MoonMenuItem::with_key("col-all", t!("report.filter.all").to_string())
                    .checked(all_on)
                    .selected(all_on)
                    .on_click(move |_, _, app| {
                        all_view.update(app, |this, cx| this.toggle_all_cols(cx));
                    }),
            );
        let visible = self.visible_cols();
        for col in AssetCol::ALL {
            let shown = visible.contains(&col);
            let last_visible = shown && visible.len() == 1;
            let view = view.clone();
            menu = menu.item(
                MoonMenuItem::with_key(format!("col-{}", col.key()), col.title())
                    .checked(shown)
                    .disabled(last_visible)
                    .on_click(move |_, _, app| {
                        view.update(app, |this, cx| this.toggle_col(col, cx));
                    }),
            );
        }
        div()
            .id("assets-cols-tip")
            .tooltip(|_window, cx| {
                cx.new(|_| moon_ui::MoonTooltipView::new(t!("assets.columns").to_string()))
                    .into()
            })
            .child(menu)
    }
}

#[cfg(test)]
/// Sort/display contracts of the asset fields.
mod tests;
