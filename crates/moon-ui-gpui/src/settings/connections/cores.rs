//! The Cores page builders: the list of cores by group and exchange, the card of the selected
//! core, the empty state, and the two layouts that place them (side by side in a wide window,
//! stacked in one scroll below the navigation breakpoint).
//!
//! Every builder here is a free function over a model ([`CoresModel`], [`CoreCard`]) and one
//! action callback ([`CoreAct`]): the page cannot be built in a test as a whole `SettingsView`
//! (it needs a `Backend`), so the layout probe draws these with synthetic data, and
//! `SettingsView::on_core_action` is the one place an action touches the draft.

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonAlert, MoonButton, MoonButtonIconSlot, MoonButtonSize, MoonButtonVariant, MoonCheckbox,
    MoonColorPicker, MoonColorPickerState, MoonDisclosure, MoonDisclosureDirection, MoonDropdown,
    MoonInput, MoonInputState, MoonMenuItem, MoonPalette, MoonSegmentItem, MoonSegmentSelection,
    MoonSegmentedControl, MoonTag, MoonToggle, MoonTone, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

#[cfg(test)]
use super::entries::CoreDot;
use super::entries::{CoreListRow, CoresModel, GroupBlock};
use super::table::{FEED_FLAGS, FEED_TOTAL};
use crate::design;
use crate::settings::common::{Action, hint_row};
use moon_core::config::{TotalMode, TransportVersion, WorkspaceMembership};

/// Something the user did on the Cores page.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CoreAction {
    /// Show this core's card.
    Select(u64),
    /// Switch a window group on or off.
    GroupActive(String, bool),
    /// Bring a group's main window up.
    ShowGroup(String),
    /// Open a group's "⋯" menu.
    GroupMenu(String),
    /// Open a group's icon picker.
    PickIcon(String),
    /// Set every core of a group to one transport mode.
    GroupMode(String, TransportVersion),
    /// Add a core to a group.
    AddCore(String),
    /// Import cores from MoonBot.
    Import,
    /// Switch the selected core on or off.
    SetActive(bool),
    /// Whether the station dials the selected core's typed address too.
    ForStation(bool),
    /// Move the selected core to a group.
    SetGroup(String),
    /// Ask for a new group's name and move the selected core there.
    NewGroup,
    /// Turn data kind `n` (0..8 filters, 8 `log_delivery`) on or off.
    Feed(usize, bool),
    Proto(Option<TransportVersion>),
    Preset(WorkspaceMembership),
    Total(TotalMode),
    /// Fold or unfold a disclosure or a long help text.
    Disclose(&'static str),
    /// Put the clipboard's key into the selected core.
    PasteKey,
    Reconnect,
    /// The delete button: asks first.
    AskDelete,
    /// The dialog's confirm: deletes the core at this draft index.
    DeleteConfirmed(usize),
}

/// The draft index an action deletes. Only the dialog's confirm deletes; the card's button only
/// asks.
pub(crate) fn deletes(action: &CoreAction) -> Option<usize> {
    match action {
        CoreAction::DeleteConfirmed(i) => Some(*i),
        _ => None,
    }
}

/// Runs a [`CoreAction`].
pub(crate) type CoreAct = Rc<dyn Fn(CoreAction, &mut Window, &mut App)>;

/// Disclosure keys of the card (`SettingsView::more_open`).
pub(crate) const FEEDS_KEY: &str = "core-feeds";
pub(crate) const ADVANCED_KEY: &str = "core-advanced";

/// Whether the list marks a core with "n/9": it receives fewer than all nine data kinds.
pub(crate) fn feeds_partial(feeds_on: u8) -> bool {
    feeds_on < FEED_TOTAL
}

/// The dot's color.
#[cfg(test)]
pub(crate) fn dot_color(dot: CoreDot, p: MoonPalette) -> u32 {
    match dot {
        CoreDot::Ok => p.green,
        CoreDot::Wait => p.amber,
        CoreDot::Bad => p.red,
        CoreDot::Off | CoreDot::Unknown => p.text_soft,
    }
}

/// A status dot without the live verdict tooltip (the probe's).
#[cfg(test)]
pub(crate) fn plain_dot(dot: CoreDot, cx: &App) -> AnyElement {
    div()
        .flex_none()
        .w(design::ui_px(cx, 8.0))
        .h(design::ui_px(cx, 8.0))
        .rounded_full()
        .bg(rgb(dot_color(dot, MoonPalette::active(cx))))
        .into_any_element()
}

/// Run `action` from a click.
fn on(act: &CoreAct, action: CoreAction) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let act = act.clone();
    move |_, window, cx| act(action.clone(), window, cx)
}

/// An [`Action`] that runs `action`, for `hint_row`'s "подробнее".
fn toggle(act: &CoreAct, key: &'static str) -> Action {
    let act = act.clone();
    Rc::new(move |window, cx| act(CoreAction::Disclose(key), window, cx))
}

/// Elements the page hands in where the live app draws more than the model knows: the status dot
/// with its verdict tooltip, and a group's icon with its picker.
pub(crate) struct ListParts<'a> {
    pub(crate) dot: &'a dyn Fn(&CoreListRow, &App) -> AnyElement,
    pub(crate) icon: &'a dyn Fn(&GroupBlock, &App) -> AnyElement,
}

/// One group's header: on/off switch, icon, name, core count, window button, "⋯" menu.
fn group_header(g: &GroupBlock, parts: &ListParts<'_>, act: &CoreAct, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    let name = g.name.clone();
    let switch_act = act.clone();
    h_flex()
        .id(SharedString::from(format!("cores-group-{}", g.name)))
        .w_full()
        .gap(design::ui_px(cx, 6.0))
        .items_center()
        .px(design::ui_px(cx, 8.0))
        .py(design::ui_px(cx, 6.0))
        .bg(rgba_from(p.shell_high, 1.0))
        .border_b_1()
        .border_color(rgba_from(p.border, 1.0))
        .child(
            MoonToggle::new(SharedString::from(format!("cores-group-on-{}", g.name)))
                .checked(g.active)
                .on_change(move |v: &bool, window, cx| {
                    switch_act(CoreAction::GroupActive(name.clone(), *v), window, cx)
                }),
        )
        .child((parts.icon)(g, cx))
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::SEMIBOLD)
                .child(g.name.clone()),
        )
        .child(
            div()
                .flex_none()
                .text_color(rgba_from(p.text_muted, 1.0))
                .child(super::super::common::count_label(
                    "conn.list.group_count",
                    g.count() as u64,
                )),
        )
        .child(div().flex_1())
        .child(
            MoonButton::new(SharedString::from(format!("cores-group-win-{}", g.name)))
                .ghost()
                .size(MoonButtonSize::density(cx))
                .leading_icon(MoonButtonIconSlot::new("icons/window-restore.svg"))
                .tooltip(t!("conn.list.group_window").to_string())
                .on_click(on(act, CoreAction::ShowGroup(g.name.clone())))
                .render(),
        )
        .child(
            MoonButton::new(SharedString::from(format!("cores-group-menu-{}", g.name)))
                .ghost()
                .size(MoonButtonSize::density(cx))
                .label("⋯")
                .tooltip(t!("conn.list.group_menu").to_string())
                .on_click(on(act, CoreAction::GroupMenu(g.name.clone())))
                .render(),
        )
        .into_any_element()
}

/// One core in the list.
fn core_item(
    row: &CoreListRow,
    selected: bool,
    parts: &ListParts<'_>,
    act: &CoreAct,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    let tag = feeds_partial(row.feeds_on).then(|| {
        div()
            .id(("cores-feeds-tag", row.row_key))
            .flex_none()
            .tooltip(|_, cx| {
                cx.new(|_| moon_ui::MoonTooltipView::new(t!("conn.list.feeds_partial").to_string()))
                    .into()
            })
            .child(
                MoonTag::warning()
                    .mono(true)
                    .label(format!("{}/{}", row.feeds_on, FEED_TOTAL)),
            )
    });
    h_flex()
        .id(("cores-row", row.row_key))
        .w_full()
        .gap(design::ui_px(cx, 8.0))
        .items_center()
        .px(design::ui_px(cx, 10.0))
        .py(design::ui_px(cx, 5.0))
        .cursor_pointer()
        .when(selected, |el| el.bg(rgba_from(p.blue, 0.16)))
        .when(!selected, |el| {
            el.hover(|s| s.bg(rgba_from(p.panel_high, 1.0)))
        })
        .when(!row.active, |el| el.opacity(0.6))
        .on_click(on(act, CoreAction::Select(row.row_key)))
        .child((parts.dot)(row, cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(design::mono())
                .child(row.name.clone()),
        )
        .child(
            div()
                .flex_shrink(1.0)
                .min_w_0()
                .max_w(design::ui_px(cx, 130.0))
                .truncate()
                .font_family(design::mono())
                .text_color(rgba_from(p.text_muted, 1.0))
                .child(row.addr.clone()),
        )
        .children(tag)
        .into_any_element()
}

/// The list of cores: per group a header, per exchange a sub-header, then the cores.
///
/// Args:
///     model: Groups, exchanges, cores and the selection.
///     parts: The status dot and group icon builders.
///     act: Runs what the user clicks.
///     cx: Application context.
///
/// Returns:
///     The list, without its own scroll.
pub(crate) fn cores_list(
    model: &CoresModel,
    parts: &ListParts<'_>,
    act: &CoreAct,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    let mut list = v_flex()
        .w_full()
        .debug_selector(|| "cores-list".into())
        .border_1()
        .border_color(rgba_from(p.border, 1.0))
        .rounded(design::r_container(cx))
        .overflow_hidden();
    for g in &model.groups {
        list = list.child(group_header(g, parts, act, cx));
        for (ex_index, ex) in g.exchanges.iter().enumerate() {
            list = list.child(
                h_flex()
                    .id(SharedString::from(format!(
                        "cores-ex-{}-{ex_index}",
                        g.name
                    )))
                    .w_full()
                    .gap(design::ui_px(cx, 6.0))
                    .items_center()
                    .px(design::ui_px(cx, 10.0))
                    .pt(design::ui_px(cx, 6.0))
                    .pb(design::ui_px(cx, 2.0))
                    .text_color(rgba_from(p.text_muted, 1.0))
                    .child(
                        div().min_w_0().truncate().child(
                            t!(
                                "conn.list.exchange_count",
                                exchange = ex.caption,
                                n = ex.rows.len()
                            )
                            .to_string(),
                        ),
                    ),
            );
            for row in &ex.rows {
                list = list.child(core_item(
                    row,
                    model.selected == Some(row.row_key),
                    parts,
                    act,
                    cx,
                ));
            }
        }
    }
    list.into_any_element()
}

/// The selected core as its card shows it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CoreCard {
    /// Draft index of the core.
    pub(crate) draft_index: usize,
    pub(crate) name: String,
    pub(crate) active: bool,
    /// The key cannot be read: the card says so above the fields.
    pub(crate) key_bad: bool,
    /// The address field's state: `Some(text)` explains a problem (danger when `addr_invalid`).
    pub(crate) addr_note: Option<String>,
    pub(crate) addr_invalid: bool,
    /// A typed address exists, so the station box means something.
    pub(crate) has_override: bool,
    pub(crate) for_station: bool,
    pub(crate) group: String,
    /// Every group name in the draft, sorted.
    pub(crate) groups: Vec<String>,
    /// The nine data kinds (eight filters, then `log_delivery`).
    pub(crate) feeds: [bool; 9],
    /// An older core keeps sending its log although `log_delivery` is off.
    pub(crate) log_ignored: bool,
    pub(crate) proto: Option<TransportVersion>,
    pub(crate) preset: WorkspaceMembership,
    pub(crate) total: TotalMode,
}

impl CoreCard {
    /// Data kinds on.
    pub(crate) fn feeds_on(&self) -> usize {
        self.feeds.iter().filter(|on| **on).count()
    }
}

/// The card's input states, owned by the page's row editors.
pub(crate) struct CardInputs<'a> {
    pub(crate) name: &'a Entity<MoonInputState>,
    pub(crate) key: &'a Entity<MoonInputState>,
    pub(crate) address: &'a Entity<MoonInputState>,
    pub(crate) bundle: &'a Entity<MoonInputState>,
    pub(crate) color: &'a Entity<MoonColorPickerState>,
    /// When the first-run hint was armed, while no core was ever configured: the key field
    /// carries the attention ring while it is still empty.
    pub(crate) key_ring: Option<std::time::Instant>,
}

/// Width of the card's text fields and pickers, unscaled.
const FIELD_W: f32 = 200.0;

/// A disclosure header of the card: caret, title, muted summary.
fn disclosure(
    id: &'static str,
    title: String,
    summary: String,
    open: bool,
    act: &CoreAct,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    let caret_act = act.clone();
    h_flex()
        .id(id)
        .w_full()
        .flex_wrap()
        .gap(design::ui_px(cx, 6.0))
        .items_center()
        .py(design::ui_px(cx, 8.0))
        .cursor_pointer()
        .border_t_1()
        .border_color(rgba_from(p.border, 1.0))
        .on_click(on(act, CoreAction::Disclose(id)))
        .child(
            MoonDisclosure::button(SharedString::from(format!("{id}-caret")), open)
                .direction(MoonDisclosureDirection::RightDown)
                .size(design::DISCLOSURE_GLYPH)
                .box_size(design::DISCLOSURE_BOX)
                .on_toggle(move |_, window, cx| caret_act(CoreAction::Disclose(id), window, cx)),
        )
        .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
        .child(
            div()
                .min_w_0()
                .text_color(rgba_from(p.text_muted, 1.0))
                .child(summary),
        )
        .into_any_element()
}

/// A dropdown over `items`, with `label` on the trigger.
fn dropdown(id: &'static str, label: String, items: Vec<MoonMenuItem>, cx: &App) -> AnyElement {
    MoonDropdown::new(id)
        .label(label)
        .trigger_caret(true)
        .trigger_variant(MoonButtonVariant::Neutral)
        .trigger_size(MoonButtonSize::density(cx))
        .trigger_width_scaled(FIELD_W)
        .fit_menu_width(0.0, f32::INFINITY)
        .items(items)
        .into_any_element()
}

/// Menu items for `values`, the current one checked, each running `to(value)`.
fn choice<T: Copy + PartialEq + 'static>(
    id: &str,
    values: impl IntoIterator<Item = (T, String)>,
    current: T,
    to: impl Fn(T) -> CoreAction + Clone + 'static,
    act: &CoreAct,
) -> Vec<MoonMenuItem> {
    values
        .into_iter()
        .enumerate()
        .map(|(index, (value, label))| {
            let to = to.clone();
            let act = act.clone();
            MoonMenuItem::with_key(format!("{id}-{index}"), label)
                .checked(value == current)
                .on_click(move |_, window, cx| act(to(value), window, cx))
        })
        .collect()
}

/// Localized label of a workspace preset.
pub(crate) fn preset_label(m: WorkspaceMembership) -> String {
    match m {
        WorkspaceMembership::Both => t!("conn.preset.both"),
        WorkspaceMembership::ClassicOnly => t!("conn.preset.classic"),
        WorkspaceMembership::AutoOnly => t!("conn.preset.auto"),
    }
    .to_string()
}

/// Localized label of a footer-total mode.
pub(crate) fn total_label(m: TotalMode) -> String {
    match m {
        TotalMode::Auto => t!("conn.total.auto"),
        TotalMode::Exclude => t!("conn.total.exclude"),
        TotalMode::Separate => t!("conn.total.separate"),
    }
    .to_string()
}

/// The card of the selected core.
///
/// Args:
///     card: What the card shows.
///     inputs: The core's text fields and color picker.
///     dot: The status dot for the header.
///     open: Whether a disclosure or long help (by key) is unfolded.
///     act: Runs what the user clicks.
///     cx: Application context.
///
/// Returns:
///     The card.
pub(crate) fn core_card(
    card: &CoreCard,
    inputs: &CardInputs<'_>,
    dot: AnyElement,
    open: &dyn Fn(&'static str) -> bool,
    act: &CoreAct,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    let field_w = design::ui_px(cx, FIELD_W);
    let active_act = act.clone();
    let header = h_flex()
        .id("core-card-head")
        .w_full()
        .gap(design::ui_px(cx, 8.0))
        .items_center()
        .pb(design::ui_px(cx, 8.0))
        .child(dot)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(design::mono())
                .font_weight(FontWeight::SEMIBOLD)
                .child(card.name.clone()),
        )
        .child(
            MoonToggle::new("core-card-active")
                .label(t!("conn.card.active").to_string())
                .mono(false)
                .checked(card.active)
                .on_change(move |v: &bool, window, cx| {
                    active_act(CoreAction::SetActive(*v), window, cx)
                }),
        );

    let row = |id: &'static str,
               label: String,
               hint: Option<String>,
               more: Option<String>,
               control: AnyElement| {
        hint_row(
            id,
            label,
            hint,
            more,
            control,
            open(id),
            toggle(act, id),
            cx,
        )
    };

    // Only an EMPTY key is worth pointing at: a ring around a filled field reads as an error.
    let key_empty = inputs.key.read(cx).value().trim().is_empty();
    let key = h_flex()
        .gap(design::ui_px(cx, 6.0))
        .items_center()
        .child(
            div()
                .debug_selector(|| "core-card-key".into())
                .relative()
                .w(field_w)
                .child(
                    MoonInput::new("core-card-key")
                        .state(inputs.key)
                        .small()
                        .mask_toggle()
                        .cleanable(true)
                        .when(card.key_bad, |i| i.tone(MoonTone::Danger).selected(true)),
                )
                .children(
                    inputs
                        .key_ring
                        .filter(|_| key_empty)
                        .and_then(|at| crate::pulse::attention_ring(p.accent, at)),
                ),
        )
        .child(
            div().debug_selector(|| "core-card-paste".into()).child(
                MoonButton::new("core-card-paste")
                    .variant(moon_ui::MoonButtonVariant::Neutral)
                    .size(MoonButtonSize::density(cx))
                    .label(t!("conn.card.paste").to_string())
                    .on_click(on(act, CoreAction::PasteKey))
                    .render(),
            ),
        )
        .into_any_element();

    let station_act = act.clone();
    let address = v_flex()
        .gap(design::ui_px(cx, 4.0))
        .child(
            div().w(field_w).child(
                MoonInput::new("core-card-addr")
                    .state(inputs.address)
                    .small()
                    .cleanable(true)
                    .when(card.addr_invalid, |i| {
                        i.tone(MoonTone::Danger).selected(true)
                    })
                    .when(!card.addr_invalid && card.addr_note.is_some(), |i| {
                        i.tone(MoonTone::Warning).selected(true)
                    }),
            ),
        )
        .child(
            MoonCheckbox::new("core-card-station")
                .label(t!("conn.card.for_station").to_string())
                .checked(card.for_station)
                .disabled(!card.has_override)
                .on_change(move |v: &bool, window, cx| {
                    station_act(CoreAction::ForStation(*v), window, cx)
                }),
        )
        .into_any_element();

    let mut group_items = card
        .groups
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let act = act.clone();
            let name = name.clone();
            MoonMenuItem::with_key(format!("core-card-group-{index}"), name.clone())
                .checked(name == card.group)
                .on_click(move |_, window, cx| act(CoreAction::SetGroup(name.clone()), window, cx))
        })
        .collect::<Vec<_>>();
    group_items.push(MoonMenuItem::separator());
    {
        let act = act.clone();
        group_items.push(
            MoonMenuItem::with_key("core-card-group-new", t!("conn.card.new_group").to_string())
                .on_click(move |_, window, cx| act(CoreAction::NewGroup, window, cx)),
        );
    }

    let feeds_open = open(FEEDS_KEY);
    let feeds = feeds_open.then(|| {
        let mut col = v_flex()
            .id("core-card-feeds")
            .gap(design::ui_px(cx, 6.0))
            .pb(design::ui_px(cx, 8.0));
        for (index, on_now) in card.feeds.iter().copied().enumerate() {
            let label = match FEED_FLAGS.get(index) {
                Some((key, _, _)) => t!(*key).to_string(),
                None if card.log_ignored => format!(
                    "{} ({})",
                    t!("conn.tip.log_delivery"),
                    t!("conn.log_delivery_ignored")
                ),
                None => t!("conn.tip.log_delivery").to_string(),
            };
            let act = act.clone();
            col = col.child(
                MoonCheckbox::new(SharedString::from(format!("core-card-feed-{index}")))
                    .label(label)
                    .checked(on_now)
                    .on_change(move |v: &bool, window, cx| {
                        act(CoreAction::Feed(index, *v), window, cx)
                    }),
            );
        }
        col.child(
            div()
                .text_color(rgba_from(p.text_muted, 1.0))
                .child(t!("conn.card.feeds_hint").to_string()),
        )
    });

    let advanced_open = open(ADVANCED_KEY);
    let advanced = advanced_open.then(|| {
        let preset_act = act.clone();
        let preset = MoonSegmentedControl::new("core-card-preset")
            .selection(MoonSegmentSelection::Filled)
            .item_gap(0.0)
            .items(WorkspaceMembership::ALL.map(|m| {
                MoonSegmentItem::new("", preset_label(m))
                    .fit_width(cx, 0.0, f32::MAX)
                    .selected(card.preset == m)
            }))
            .on_click(move |index, _, window, cx| {
                if let Some(m) = WorkspaceMembership::ALL.get(index) {
                    preset_act(CoreAction::Preset(*m), window, cx)
                }
            })
            .render()
            .into_any_element();
        v_flex()
            .id("core-card-advanced")
            .child(row(
                "core-card-proto",
                t!("conn.card.proto").to_string(),
                Some(t!("conn.card.proto_hint").to_string()),
                Some(t!("conn.tip.proto").to_string()),
                dropdown(
                    "core-card-proto-pick",
                    card.proto
                        .map_or_else(|| "-".to_string(), |v| v.label().to_string()),
                    choice(
                        "core-card-proto",
                        TransportVersion::ALL
                            .into_iter()
                            .map(|v| (Some(v), v.label().to_string())),
                        card.proto,
                        CoreAction::Proto,
                        act,
                    ),
                    cx,
                ),
            ))
            .child(row(
                "core-card-preset-row",
                t!("conn.card.preset").to_string(),
                Some(t!("conn.card.preset_hint").to_string()),
                Some(t!("conn.tip.preset").to_string()),
                preset,
            ))
            .child(row(
                "core-card-total",
                t!("conn.card.total").to_string(),
                Some(t!("conn.card.total_hint").to_string()),
                Some(t!("conn.tip.total").to_string()),
                dropdown(
                    "core-card-total-pick",
                    total_label(card.total),
                    choice(
                        "core-card-total",
                        TotalMode::ALL.into_iter().map(|m| (m, total_label(m))),
                        card.total,
                        CoreAction::Total,
                        act,
                    ),
                    cx,
                ),
            ))
            .child(row(
                "core-card-bundle",
                t!("conn.card.bundle").to_string(),
                Some(t!("conn.card.bundle_hint").to_string()),
                Some(t!("conn.tip.bundle").to_string()),
                div()
                    .w(field_w)
                    .child(
                        MoonInput::new("core-card-bundle-input")
                            .state(inputs.bundle)
                            .small(),
                    )
                    .into_any_element(),
            ))
    });

    let footer = h_flex()
        .id("core-card-foot")
        .debug_selector(|| "core-card-foot".into())
        .w_full()
        .flex_wrap()
        .gap(design::ui_px(cx, 8.0))
        .items_center()
        .pt(design::ui_px(cx, 10.0))
        .border_t_1()
        .border_color(rgba_from(p.border, 1.0))
        .when(card.active, |el| {
            el.child(
                MoonButton::new("core-card-reconnect")
                    .variant(moon_ui::MoonButtonVariant::Neutral)
                    .size(MoonButtonSize::density(cx))
                    .label(t!("conn.card.reconnect").to_string())
                    .on_click(on(act, CoreAction::Reconnect))
                    .render(),
            )
        })
        .child(div().flex_1())
        .child(
            MoonButton::new("core-card-delete")
                .variant(MoonButtonVariant::OutlineRed)
                .size(MoonButtonSize::density(cx))
                .label(t!("conn.card.delete").to_string())
                .on_click(on(act, CoreAction::AskDelete))
                .render(),
        );

    v_flex()
        .id("core-card")
        .debug_selector(|| "core-card".into())
        .w_full()
        .min_w_0()
        .p(design::ui_px(cx, 14.0))
        .border_1()
        .border_color(rgba_from(p.border, 1.0))
        .rounded(design::r_container(cx))
        .bg(rgba_from(p.shell_high, 1.0))
        .child(header)
        .when(card.key_bad, |el| {
            el.child(
                div()
                    .debug_selector(|| "core-bad-key".into())
                    .pb(design::ui_px(cx, 6.0))
                    .child(MoonAlert::error(
                        "core-card-bad-key",
                        t!("conn.card.bad_key").to_string(),
                    )),
            )
        })
        .child(row(
            "core-card-name",
            t!("conn.card.name").to_string(),
            None,
            None,
            div()
                .w(field_w)
                .child(
                    MoonInput::new("core-card-name-input")
                        .state(inputs.name)
                        .small(),
                )
                .into_any_element(),
        ))
        .child(row(
            "core-card-key-row",
            t!("conn.card.key").to_string(),
            Some(t!("conn.card.key_hint").to_string()),
            Some(t!("conn.tip.key").to_string()),
            key,
        ))
        .child(row(
            "core-card-addr-row",
            t!("conn.card.addr").to_string(),
            Some(
                card.addr_note
                    .clone()
                    .unwrap_or_else(|| t!("conn.card.addr_hint").to_string()),
            ),
            Some(t!("conn.card.addr_more").to_string()),
            address,
        ))
        .child(row(
            "core-card-group",
            t!("conn.card.group").to_string(),
            Some(t!("conn.card.group_hint").to_string()),
            Some(t!("conn.tip.group").to_string()),
            dropdown("core-card-group-pick", card.group.clone(), group_items, cx),
        ))
        .child(row(
            "core-card-color",
            t!("conn.card.color").to_string(),
            Some(t!("conn.card.color_hint").to_string()),
            None,
            MoonColorPicker::new(inputs.color)
                .colors(design::picker_palette())
                .into_any_element(),
        ))
        .child(disclosure(
            FEEDS_KEY,
            t!("conn.card.feeds").to_string(),
            t!(
                "conn.card.feeds_count",
                n = card.feeds_on(),
                total = FEED_TOTAL
            )
            .to_string(),
            feeds_open,
            act,
            cx,
        ))
        .children(feeds)
        .child(disclosure(
            ADVANCED_KEY,
            t!("conn.card.advanced").to_string(),
            t!("conn.card.advanced_hint").to_string(),
            advanced_open,
            act,
            cx,
        ))
        .children(advanced)
        .child(footer)
        .into_any_element()
}

/// The page with no cores: what to do, and the two ways to start.
pub(crate) fn cores_empty(act: &CoreAct, add: AnyElement, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    v_flex()
        .id("cores-empty")
        .debug_selector(|| "cores-empty".into())
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 10.0))
        .py(design::ui_px(cx, 32.0))
        .px(design::ui_px(cx, 16.0))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(design::ui_px(cx, 16.0))
                .child(t!("conn.empty.title").to_string()),
        )
        .child(
            div()
                .debug_selector(|| "cores-empty-body".into())
                .w_full()
                .max_w(design::ui_px(cx, 460.0))
                .text_center()
                .text_color(rgba_from(p.text_muted, 1.0))
                .child(t!("conn.empty.body").to_string()),
        )
        .child(
            h_flex()
                .flex_wrap()
                .justify_center()
                .gap(design::ui_px(cx, 8.0))
                .child(add)
                .child(
                    MoonButton::new("cores-empty-import")
                        .variant(moon_ui::MoonButtonVariant::Neutral)
                        .label(t!("conn.empty.import").to_string())
                        .on_click(on(act, CoreAction::Import))
                        .render(),
                ),
        )
        .into_any_element()
}

/// Height the list keeps in a narrow window, where the card sits under it, unscaled.
const NARROW_LIST_H: f32 = 240.0;
/// Share of the body the list takes beside the card, clamped to [`LIST_W_MIN`]..[`LIST_W_MAX`].
const LIST_SHARE: f32 = 0.38;
/// Narrowest list beside the card, unscaled: a core name and its status still read.
const LIST_W_MIN: f32 = 200.0;
/// Widest list beside the card, unscaled.
const LIST_W_MAX: f32 = 280.0;
/// Gap between the list and the card, unscaled.
const SPLIT_GAP: f32 = 12.0;
/// The least a card row's caption keeps beside its control, unscaled.
const CARD_LABEL_MIN_W: f32 = 64.0;

/// How the list and the card share the body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CoresLayout {
    /// The list above the card.
    Stacked,
    /// Side by side, the list this many logical pixels wide.
    Side { list_w: f32 },
}

/// The card's narrowest width that keeps every control inside it: padding and border, a row
/// caption, the row gap, and the widest control — the key field with Paste beside it.
pub(crate) fn card_min_w(cx: &App) -> f32 {
    let paste =
        design::ui_text_width_zoomed(cx, &t!("conn.card.paste"), design::BODY_TEXT, 400.0, false)
            + 2.0 * design::ui_value(cx, design::CONTROL_TIER.control_metrics().pad_x)
            + 2.0;
    design::ui_value(cx, 2.0 * 14.0 + CARD_LABEL_MIN_W + 16.0 + FIELD_W + 6.0) + 2.0 + paste
}

/// Side by side only when the list at its share and the card at its minimum both fit
/// `content_w`; a window below the navigation breakpoint always stacks.
///
/// Args:
///     narrow: The window is below the navigation breakpoint.
///     content_w: The body's content width, logical pixels.
///     card_min: [`card_min_w`].
///     zoom: The UI zoom factor.
pub(crate) fn cores_layout(narrow: bool, content_w: f32, card_min: f32, zoom: f32) -> CoresLayout {
    if narrow {
        return CoresLayout::Stacked;
    }
    let list_w = (content_w * LIST_SHARE).clamp(LIST_W_MIN * zoom, LIST_W_MAX * zoom);
    if content_w - list_w - SPLIT_GAP * zoom >= card_min {
        CoresLayout::Side { list_w }
    } else {
        CoresLayout::Stacked
    }
}

/// Place the list and the card: side by side, each scrolling on its own, in a wide window;
/// stacked in a narrow one, the list capped so the card shows under it (the page then scrolls as a
/// whole, see `SettingsView::cores_page`).
pub(crate) fn cores_split(
    list: AnyElement,
    card: Option<AnyElement>,
    layout: CoresLayout,
    cx: &App,
) -> AnyElement {
    let gap = design::ui_px(cx, SPLIT_GAP);
    let CoresLayout::Side { list_w } = layout else {
        return v_flex()
            .w_full()
            .flex_none()
            .gap(gap)
            .child(
                div()
                    .id("cores-list-scroll")
                    .debug_selector(|| "cores-list-box".into())
                    .w_full()
                    .max_h(design::ui_px(cx, NARROW_LIST_H))
                    .overflow_y_scroll()
                    .child(list),
            )
            .children(card)
            .into_any_element();
    };
    {
        h_flex()
            .w_full()
            .flex_1()
            .min_h(px(0.0))
            .items_start()
            .gap(gap)
            .child(
                div()
                    .id("cores-list-scroll")
                    .debug_selector(|| "cores-list-box".into())
                    .flex_none()
                    .w(px(list_w))
                    .max_h_full()
                    .overflow_y_scroll()
                    .child(list),
            )
            .child(
                div()
                    .id("core-card-scroll")
                    .debug_selector(|| "core-card-box".into())
                    .flex_1()
                    .min_w_0()
                    .max_h_full()
                    .overflow_y_scroll()
                    .children(card),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests;
