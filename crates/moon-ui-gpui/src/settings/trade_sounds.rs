//! Immediate, layout-persisted trade notifications; separate from core price-approach alerts.

use std::collections::HashSet;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonDropdown, MoonPalette,
    MoonScrollableElement, MoonSliderEvent, MoonSliderState, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::SettingsView;
use super::common::{LabelCol, hint_row_in, label_col, slider_field};
use super::telegram::plural_key;
use crate::design;
use crate::panels::common::SoundChoices;
use crate::panels::{RadioMark, radio_items};
use moon_core::config::trade_sounds::{TradeSounds, exchange_key};
use moon_core::feed::ExchangeId;

/// One exchange row of the sounds table: its label and its two pickers, built by the page.
pub(super) struct SoundRow {
    pub(super) label: String,
    pub(super) open: AnyElement,
    pub(super) close: AnyElement,
}

/// What the sounds tab draws, gathered by [`SettingsView::trade_sounds_tab`].
pub(super) struct SoundsTab {
    /// The exchange rows, already listed and labelled.
    pub(super) rows: Vec<SoundRow>,
    /// Columns side by side (`true`) or labelled cards; see [`sounds_wide`].
    pub(super) wide: bool,
    /// "Show every exchange" is on.
    pub(super) show_all: bool,
    /// Sleep mode silences sounds: the volume block says so.
    pub(super) quiet: bool,
    /// The volume row; see [`volume_row`].
    pub(super) volume: AnyElement,
    /// The own-sounds folder block, right under the table.
    pub(super) folder: AnyElement,
}

/// The table's width: the exchange column, two sound columns, gaps, padding and border.
pub(super) fn sounds_table_w(cx: &App) -> f32 {
    let venue_width = design::font_w(cx, 190.0);
    let sound_width = design::font_w(cx, 178.0) + f32::from(design::ui_px(cx, 6.0));
    let gap = f32::from(design::ui_px(cx, 16.0));
    let row_padding = f32::from(design::ui_px(cx, 12.0));
    venue_width + 2.0 * sound_width + 2.0 * gap + 2.0 * row_padding + 2.0
}

/// Whether the table's columns fit the body; the Settings host owns 18 logical pixels of body
/// padding on each side.
pub(super) fn sounds_wide(window_width: f32, cx: &App) -> bool {
    window_width - f32::from(design::ui_px(cx, 36.0)) >= sounds_table_w(cx)
}

/// The volume row: the label on one line in the shared label column of every Settings page, the
/// slider and its value right beside it, or under it when the body is too narrow for both.
///
/// Args:
///     st: The retained volume slider.
///     cx: Application context for theme and scale.
///
/// Returns:
///     The row (selector `trade-sounds-volume-row`, its label `trade-sounds-volume-row-label`).
pub(super) fn volume_row(st: &Entity<MoonSliderState>, cx: &App) -> AnyElement {
    let label = t!("trade_sounds.volume").to_string();
    // The shared column width, held whole: in a language whose label is long, a narrow body
    // drops the slider under the label rather than wrapping the label beside it.
    let col = match label_col(std::slice::from_ref(&label), cx) {
        LabelCol::Fit(w) => LabelCol::Line(w),
        other => other,
    };
    hint_row_in(
        col,
        "trade-sounds-volume-row",
        label,
        None,
        None,
        slider_field(st, 0.0..=100.0, |value| format!("{value:.0}%"), cx),
        false,
        std::rc::Rc::new(|_, _| {}),
        cx,
    )
}

/// The sounds tab: volume on top, then the exchange table, and the folder block right under it,
/// the table and the block one content width.
///
/// The tab is BOUNDED, not scrolled whole (see `render.rs`): the table takes its rows' height
/// and, when the window is too short for all of them, shrinks and scrolls on its own, so the
/// folder's buttons never leave the window before the table does.
///
/// Args:
///     tab: What to draw.
///     on_show_all: The "show every exchange" switch's handler.
///     cx: Application context for theme and scale.
pub(super) fn sounds_tab_el(
    tab: SoundsTab,
    on_show_all: Box<dyn Fn(&bool, &mut Window, &mut App) + 'static>,
    cx: &App,
) -> AnyElement {
    let SoundsTab {
        rows: venues,
        wide,
        show_all,
        quiet,
        volume,
        folder,
    } = tab;
    let palette = MoonPalette::active(cx);
    let venue_width = design::font_w(cx, 190.0);
    let sound_width = design::font_w(cx, 178.0) + f32::from(design::ui_px(cx, 6.0));
    let gap = design::ui_px(cx, 16.0);
    let row_padding = design::ui_px(cx, 12.0);
    let table_width = sounds_table_w(cx);
    let count = venues.len() as u64;
    let heading = h_flex()
        .debug_selector(|| "trade-sounds-ex-heading".into())
        .w_full()
        .flex_wrap()
        .items_center()
        .gap(design::ui_px(cx, 12.0))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(t!("trade_sounds.ex.title").to_string()),
        )
        .child(
            div()
                .text_color(rgba_from(palette.text_muted, 1.0))
                .child(t!(&plural_key("trade_sounds.ex.count", count), n = count).to_string()),
        )
        .child(
            div()
                .debug_selector(|| "trade-sounds-show-all".into())
                .child(
                    MoonCheckbox::new("trade-sounds-show-all")
                        .checked(show_all)
                        .label(t!("trade_sounds.ex.show_all").to_string())
                        .on_change(on_show_all),
                ),
        );
    let mut head = v_flex()
        .id("trade-sounds-volume")
        .debug_selector(|| "trade-sounds-volume".into())
        .w_full()
        .max_w(px(table_width))
        .flex_none()
        .gap(design::ui_px(cx, 12.0))
        .child(volume);
    if quiet {
        head = head.child(
            div()
                .font_family(design::ui_font())
                .text_color(rgba_from(palette.text, 1.0))
                .child(t!("trade_sounds.quiet_note").to_string()),
        );
    }
    let mut rows = v_flex()
        .debug_selector(|| "trade-sounds-rows".into())
        .w_full()
        .border_1()
        .border_color(rgba_from(palette.border, 1.0))
        .rounded(design::r_container(cx))
        .overflow_hidden()
        .bg(rgba_from(palette.table_body, 1.0));
    if wide {
        rows = rows.child(
            h_flex()
                .px(row_padding)
                .py(design::ui_px(cx, 8.0))
                .gap(gap)
                .font_family(design::ui_font())
                .text_size(design::t_caption(cx))
                .text_color(rgba_from(palette.text, 1.0))
                .bg(rgba_from(palette.shell_high, 1.0))
                .child(
                    div()
                        .w(px(venue_width))
                        .flex_none()
                        .child(t!("trade_sounds.col.exchange").to_string()),
                )
                .child(
                    div()
                        .w(px(sound_width))
                        .flex_none()
                        .child(t!("trade_sounds.col.open").to_string()),
                )
                .child(
                    div()
                        .w(px(sound_width))
                        .flex_none()
                        .child(t!("trade_sounds.col.close").to_string()),
                ),
        );
    }
    let empty = venues.is_empty();
    for (index, SoundRow { label, open, close }) in venues.into_iter().enumerate() {
        let name = div()
            .font_family(design::mono())
            .text_color(rgba_from(palette.text, 1.0))
            .child(label);
        let row = if wide {
            h_flex()
                .w_full()
                .items_center()
                .gap(gap)
                .child(name.w(px(venue_width)).flex_none())
                .child(open)
                .child(close)
                .into_any_element()
        } else {
            v_flex()
                .w_full()
                .gap(design::ui_px(cx, 10.0))
                .child(name.font_weight(FontWeight::SEMIBOLD))
                .child(h_flex().flex_wrap().gap(gap).child(open).child(close))
                .into_any_element()
        };
        rows = rows.child(
            div()
                .debug_selector(move || format!("trade-sounds-row-{index}"))
                .w_full()
                .px(row_padding)
                .py(design::ui_px(cx, 9.0))
                .border_t_1()
                .border_color(rgba_from(palette.border, 1.0))
                .child(row),
        );
    }
    // The outer `flex_1 + min_h(0)` takes what the exchanges block holds: its rows' height, since
    // a 0% basis in a column of indefinite height counts as the content, or less once the block
    // shrinks on a short page; the inner `size_full` owns the scrollbar. Without both layers Scrollable breaks height layout (`import_preview.rs`
    // carries the same pair). Volume and the folder stay outside this scroller: the page is
    // bounded (`render.rs` treats Sounds like Cores), so a whole-page scroll is not available.
    let table = div()
        .debug_selector(|| "trade-sounds-table".into())
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        .child(
            div()
                .id("trade-sounds-table")
                .size_full()
                .child(v_flex().w_full().max_w(px(table_width)).child(rows))
                .overflow_y_scrollbar(),
        );
    let exchanges = v_flex()
        .id("trade-sounds-ex")
        .debug_selector(|| "trade-sounds-ex".into())
        .flex_shrink(1.0)
        .min_h(px(0.0))
        .w_full()
        .max_w(px(table_width))
        .gap(design::ui_px(cx, 8.0))
        .child(heading)
        .child(
            div()
                .debug_selector(|| "trade-sounds-help".into())
                .w_full()
                .text_color(rgba_from(palette.text_muted, 1.0))
                .child(t!("trade_sounds.help").to_string()),
        )
        // No core reported an exchange yet: say so instead of an empty frame.
        .when(empty, |ex| {
            ex.child(
                div()
                    .debug_selector(|| "trade-sounds-empty".into())
                    .w_full()
                    .text_color(rgba_from(palette.text_muted, 1.0))
                    .child(t!("trade_sounds.ex.empty").to_string()),
            )
        })
        .when(!empty, |ex| ex.child(table));
    // `flex_1 + min_h(0)` inside the host's flex column, the one idiom every bounded tab and
    // `MoonVirtualList` site here uses to be handed a bounded height rather than an
    // unbounded one.
    v_flex()
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        .gap(design::ui_px(cx, 12.0))
        .child(head)
        .child(exchanges)
        .child(
            div()
                .flex_none()
                .w_full()
                .max_w(px(table_width))
                .child(folder),
        )
        .into_any_element()
}

/// One sound picker: the choice dropdown and its preview button, captioned in card layout.
///
/// Args:
///     prefix: Element id prefix, unique per exchange and side.
///     preview_id: The preview button's id.
///     open: The open-sound side (else close), for the caption.
///     labelled: Card layout: the caption sits above the controls.
///     trigger: The current choice's label.
///     items: The dropdown's choices.
///     preview_disabled: Nothing playable is chosen, or sleep mode is on.
///     on_preview: Plays the current choice.
///     cx: Application context for theme and scale.
pub(super) fn sound_picker_el(
    prefix: String,
    preview_id: String,
    open: bool,
    labelled: bool,
    trigger: SharedString,
    items: Vec<moon_ui::MoonMenuItem>,
    preview_disabled: bool,
    on_preview: Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>,
    cx: &App,
) -> AnyElement {
    let mut field = v_flex()
        .debug_selector({
            let sel = format!("{prefix}-field");
            move || sel.clone()
        })
        .flex_none()
        .gap(design::ui_px(cx, 4.0));
    if labelled {
        field = field.child(
            div()
                .font_family(design::ui_font())
                .text_size(design::t_caption(cx))
                .text_color(rgba_from(MoonPalette::active(cx).text, 1.0))
                .child(if open {
                    t!("trade_sounds.col.open").to_string()
                } else {
                    t!("trade_sounds.col.close").to_string()
                }),
        );
    }
    let selector = format!("{preview_id}-cell");
    field
        .child(
            h_flex()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .child(
                    MoonDropdown::new(SharedString::from(prefix))
                        .label(trigger)
                        .trigger_caret(true)
                        .trigger_variant(MoonButtonVariant::Soft)
                        .trigger_size(MoonButtonSize::density(cx))
                        .trigger_width(design::font_w(cx, 150.0))
                        .menu_width_scaled(170.0)
                        .items(items),
                )
                .child(
                    div().debug_selector(move || selector.clone()).child(
                        MoonButton::new(SharedString::from(preview_id))
                            .label("▶")
                            .tooltip(t!("trade_sounds.preview").to_string())
                            .variant(MoonButtonVariant::Soft)
                            .width(design::font_w(cx, 28.0))
                            .disabled(preview_disabled)
                            .on_click(on_preview)
                            .render(),
                    ),
                ),
        )
        .into_any_element()
}

impl SettingsView {
    /// Align exchange and sound columns when they fit; narrow windows use labelled compact cards.
    /// The layout is [`sounds_tab_el`]'s; this gathers what it draws.
    pub(super) fn trade_sounds_tab(&self, window_width: f32, cx: &mut Context<Self>) -> AnyElement {
        let show_all = self.sounds_ui.show_all;
        let (venues, quiet) = {
            let backend = self.backend.read(cx);
            let directory: Vec<(ExchangeId, String)> = (0..=u8::MAX)
                .filter(|code| moon_core::venue::venue(*code).is_some())
                .map(|code| {
                    let id = ExchangeId::new(code);
                    (id, crate::controls::venue_id_label(id))
                })
                .collect();
            let reported = backend.session.core_venues();
            let mut of_servers = Vec::new();
            for server in &backend.config.servers {
                let venue = reported
                    .get(&server.id)
                    .or_else(|| reported.get(&server.uid))
                    .filter(|venue| venue.is_nameable());
                if let Some(venue) = venue {
                    of_servers.push((venue.id, crate::controls::venue_section_label(Some(venue))));
                }
            }
            let quiet = backend.quiet_sleeping;
            (listed_exchanges(&of_servers, &directory, show_all), quiet)
        };
        let wide = sounds_wide(window_width, cx);
        let rows = venues
            .into_iter()
            .map(|(id, label)| {
                let cfg = self
                    .backend
                    .read(cx)
                    .layout
                    .trade_sounds
                    .get(&exchange_key(id))
                    .cloned()
                    .unwrap_or_default();
                SoundRow {
                    label,
                    open: self.trade_sound_picker(id, true, &cfg, !wide, cx),
                    close: self.trade_sound_picker(id, false, &cfg, !wide, cx),
                }
            })
            .collect();
        let tab = SoundsTab {
            rows,
            wide,
            show_all,
            quiet,
            volume: volume_row(&self.trade_volume, cx),
            folder: self.sound_folder_block(cx),
        };
        sounds_tab_el(
            tab,
            Box::new(cx.listener(|this, on: &bool, _, cx| {
                this.sounds_ui.show_all = *on;
                cx.notify();
            })),
            cx,
        )
    }

    /// One exchange side's picker over the catalog; the layout is [`sound_picker_el`]'s.
    fn trade_sound_picker(
        &self,
        id: ExchangeId,
        open: bool,
        cfg: &TradeSounds,
        labelled: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let current = if open { &cfg.open } else { &cfg.close };
        // Row 0 is the mute; the catalog rows follow, shifted by one. A stored name no file
        // answers to is listed and marked rather than dropped — see `SoundChoices`.
        let choices = SoundChoices::for_current(current);
        let prefix = format!("trade-sound-{}-{}-{open}", id.code, id.dex);
        let mut options = vec![(
            0usize,
            SharedString::from(format!("{prefix}-mute")),
            SharedString::from(t!("trade_sounds.off").to_string()),
        )];
        options.extend(
            choices
                .rows(&prefix)
                .into_iter()
                .map(|(i, key, label)| (i + 1, key, label)),
        );
        let selected = choices.selected.map_or(0, |i| i + 1);
        let trigger_label = choices
            .selected_label()
            .unwrap_or_else(|| SharedString::from(t!("trade_sounds.off").to_string()));
        let stems = std::rc::Rc::new(choices.stems);
        let backend = self.backend.clone();
        let view = cx.entity().downgrade();
        let preview = current.clone();
        let preview_backend = self.backend.clone();
        let preview_disabled =
            !crate::media::sound::is_playable(current) || self.backend.read(cx).quiet_sleeping;
        let items = radio_items(
            options,
            selected,
            RadioMark::Check,
            move |app, row: usize| {
                let name = match row.checked_sub(1) {
                    Some(i) => stems.get(i).cloned().unwrap_or_default(),
                    None => String::new(),
                };
                backend.update(app, |b, cx| {
                    let cfg = b.layout.trade_sounds.entry(exchange_key(id)).or_default();
                    let target = if open { &mut cfg.open } else { &mut cfg.close };
                    if *target != name {
                        *target = name;
                        b.layout_dirty = true;
                    }
                    cx.notify();
                });
                let _ = view.update(app, |_, cx| cx.notify());
            },
        );
        sound_picker_el(
            prefix,
            format!("trade-preview-{}-{}-{open}", id.code, id.dex),
            open,
            labelled,
            trigger_label,
            items,
            preview_disabled,
            Box::new(move |_, _, cx| {
                if !preview_backend.read(cx).quiet_sleeping {
                    crate::media::sound::play_trade_preview(
                        &preview,
                        preview_backend.read(cx).layout.trade_sound_volume_percent(),
                    );
                }
            }),
            cx,
        )
    }
}

/// Build a retained slider that applies trade loudness immediately, like the sound pickers.
pub(super) fn build_volume(
    backend: &Entity<crate::Backend>,
    cx: &mut Context<SettingsView>,
) -> Entity<MoonSliderState> {
    let volume = backend.read(cx).layout.trade_sound_volume_percent();
    let state = cx.new(|_| {
        MoonSliderState::new()
            .max(100.0)
            .min(0.0)
            .step(1.0)
            .default_value(f32::from(volume))
    });
    cx.subscribe(&state, |this, _, event: &MoonSliderEvent, cx| {
        let MoonSliderEvent::Change(value) = event else {
            return;
        };
        let volume = value.end().round().clamp(0.0, 100.0) as u32;
        this.backend.update(cx, |backend, bcx| {
            if backend.layout.trade_sound_volume != Some(volume) {
                backend.layout.trade_sound_volume = Some(volume);
                backend.layout_dirty = true;
                bcx.notify();
            }
        });
        cx.notify();
    })
    .detach();
    state
}

impl SettingsView {
    /// The Trade sounds page: its body unchanged.
    pub(super) fn sounds_page(
        &self,
        width: f32,
        cx: &mut Context<Self>,
    ) -> super::frame::PageParts {
        let body = self.trade_sounds_tab(width, cx).into_any_element();
        self.page_parts(super::Tab::Sounds, body, Vec::new(), cx)
    }
}

/// The exchanges the sounds table lists.
///
/// With `show_all` off, the rows are the distinct nameable venues of `of_servers` (one row per
/// exchange, first label kept). With it on, `directory` adds every exchange no server reports;
/// a server's label wins over the directory's. Both results are sorted by label, then by
/// `(code, dex)`.
///
/// Args:
///     of_servers: Venues of configured servers, in server order, already labelled. Duplicates
///         are the same exchange reported by more than one server.
///     directory: Every exchange this build can name, labelled by the directory.
///     show_all: Session-only "show every exchange". Not persisted.
///
/// Returns:
///     The rows to draw, distinct by [`ExchangeId`].
pub(super) fn listed_exchanges(
    of_servers: &[(ExchangeId, String)],
    directory: &[(ExchangeId, String)],
    show_all: bool,
) -> Vec<(ExchangeId, String)> {
    // Server venues first, so a server's own label wins over the directory's.
    let extra = if show_all { directory } else { &[] };
    let mut known = HashSet::new();
    let mut rows: Vec<(ExchangeId, String)> = of_servers
        .iter()
        .chain(extra)
        .filter(|(id, _)| known.insert(*id))
        .cloned()
        .collect();
    rows.sort_by(|a, b| {
        a.1.cmp(&b.1)
            .then((a.0.code, a.0.dex).cmp(&(b.0.code, b.0.dex)))
    });
    rows
}

/// UI state of the Trade sounds page that the draft does not hold. Branch-owned: fields are added
/// here, never on `SettingsView`.
///
/// `show_all` lasts for this window only. The volume and the per-exchange sounds stay on the layout.
#[derive(Default)]
pub(super) struct SoundsUi {
    /// List every directory exchange, not only the venues of configured servers.
    pub(super) show_all: bool,
}

#[cfg(test)]
mod tests;
