//! Immediate, layout-persisted trade notifications; separate from core price-approach alerts.

use std::collections::HashMap;

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonMenuSize, MoonPalette,
    MoonScrollableElement, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::SettingsView;
use crate::design;
use crate::panels::common::SoundChoices;
use crate::panels::{RadioMark, radio_items};
use moon_core::config::trade_sounds::{TradeSounds, exchange_key};
use moon_core::feed::ExchangeId;

impl SettingsView {
    /// Align exchange and sound columns when they fit; narrow windows use labelled compact cards.
    ///
    /// The tab is BOUNDED, not scrolled whole (see `render.rs`): the help text sits on top, the
    /// exchange table takes whatever height is left and scrolls on its own, and the folder block
    /// stays pinned at the bottom — with a long exchange list the buttons would otherwise leave
    /// the window before the table did.
    pub(super) fn trade_sounds_tab(
        &self,
        window_width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let backend = self.backend.read(cx);
        let palette = MoonPalette::active(cx);
        let venue_width = design::font_w(cx, 190.0);
        let sound_width = design::font_w(cx, 178.0) + f32::from(design::ui_px(cx, 6.0));
        let gap = design::ui_px(cx, 16.0);
        let row_padding = design::ui_px(cx, 12.0);
        let table_width = venue_width
            + 2.0 * sound_width
            + 2.0 * f32::from(gap)
            + 2.0 * f32::from(row_padding)
            + 2.0;
        // The Settings host owns 18 logical pixels of body padding on each side.
        let wide = window_width - f32::from(design::ui_px(cx, 36.0)) >= table_width;
        let mut venues: HashMap<ExchangeId, String> = (0..=u8::MAX)
            .filter(|code| moon_core::venue::venue(*code).is_some())
            .map(|code| {
                let id = ExchangeId::new(code);
                (id, crate::controls::venue_id_label(id))
            })
            .collect();
        for venue in backend.session.core_venues().values() {
            venues.insert(venue.id, crate::controls::venue_section_label(Some(venue)));
        }
        let mut venues: Vec<_> = venues.into_iter().collect();
        venues.sort_by(|a, b| {
            a.1.cmp(&b.1)
                .then((a.0.code, a.0.dex).cmp(&(b.0.code, b.0.dex)))
        });
        let mut head = v_flex()
            .w_full()
            .max_w(px(table_width))
            .gap(design::ui_px(cx, 12.0))
            .child(
                div()
                    .font_family(design::ui_font())
                    .text_size(design::t_body(cx))
                    .text_color(rgba_from(palette.text, 1.0))
                    .child(t!("trade_sounds.help").to_string()),
            );
        if backend.quiet_sleeping {
            head = head.child(
                div()
                    .font_family(design::ui_font())
                    .text_color(rgba_from(palette.text, 1.0))
                    .child(t!("trade_sounds.quiet_note").to_string()),
            );
        }
        let mut rows = v_flex()
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
                            .child(t!("trade_sounds.exchange").to_string()),
                    )
                    .child(
                        div()
                            .w(px(sound_width))
                            .flex_none()
                            .child(t!("trade_sounds.open").to_string()),
                    )
                    .child(
                        div()
                            .w(px(sound_width))
                            .flex_none()
                            .child(t!("trade_sounds.close").to_string()),
                    ),
            );
        }
        for (id, label) in venues {
            let cfg = backend
                .layout
                .trade_sounds
                .get(&exchange_key(id))
                .cloned()
                .unwrap_or_default();
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
                    .child(self.trade_sound_picker(id, true, &cfg, false, cx))
                    .child(self.trade_sound_picker(id, false, &cfg, false, cx))
                    .into_any_element()
            } else {
                v_flex()
                    .w_full()
                    .gap(design::ui_px(cx, 10.0))
                    .child(name.font_weight(FontWeight::SEMIBOLD))
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap(gap)
                            .child(self.trade_sound_picker(id, true, &cfg, true, cx))
                            .child(self.trade_sound_picker(id, false, &cfg, true, cx)),
                    )
                    .into_any_element()
            };
            rows = rows.child(
                div()
                    .w_full()
                    .px(row_padding)
                    .py(design::ui_px(cx, 9.0))
                    .border_t_1()
                    .border_color(rgba_from(palette.border, 1.0))
                    .child(row),
            );
        }
        // The folder block is built after the table so `backend`, a read borrow of `cx`, is
        // released for the block's `cx.listener` calls.
        let folder = self.sound_folder_block(cx);
        // The outer `flex_1 + min_h(0)` takes the remaining height; the inner `size_full` owns
        // the scrollbar. Without both layers Scrollable breaks height layout (`import_preview.rs`
        // carries the same pair).
        let table = div().flex_1().min_h(px(0.0)).w_full().child(
            div()
                .id("trade-sounds-table")
                .size_full()
                .child(v_flex().w_full().max_w(px(table_width)).child(rows))
                .overflow_y_scrollbar(),
        );
        // `flex_1 + min_h(0)` inside the host's flex column, the one idiom every bounded tab and
        // `MoonVirtualList` site here uses to be handed a bounded height rather than an
        // unbounded one.
        v_flex()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .gap(design::ui_px(cx, 12.0))
            .child(head)
            .child(table)
            .child(folder)
    }

    /// Keep a fixed-size preview beside its selector; captions are needed only in card layout.
    fn trade_sound_picker(
        &self,
        id: ExchangeId,
        open: bool,
        cfg: &TradeSounds,
        labelled: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
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
        let mut field = v_flex().flex_none().gap(design::ui_px(cx, 4.0));
        if labelled {
            field = field.child(
                div()
                    .font_family(design::ui_font())
                    .text_size(design::t_caption(cx))
                    .text_color(rgba_from(MoonPalette::active(cx).text, 1.0))
                    .child(if open {
                        t!("trade_sounds.open").to_string()
                    } else {
                        t!("trade_sounds.close").to_string()
                    }),
            );
        }
        field.child(
            h_flex()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .child(
                    MoonDropdown::new(SharedString::from(prefix))
                        .label(trigger_label)
                        .trigger_caret(true)
                        .trigger_variant(MoonButtonVariant::Soft)
                        .trigger_size(MoonButtonSize::Action)
                        .trigger_width(design::font_w(cx, 150.0))
                        .menu_width_scaled(170.0)
                        .menu_size(MoonMenuSize::Compact)
                        .items(items),
                )
                .child(
                    MoonButton::new(SharedString::from(format!(
                        "trade-preview-{}-{}-{open}",
                        id.code, id.dex
                    )))
                    .label("▶")
                    .tooltip(t!("trade_sounds.preview").to_string())
                    .size(MoonButtonSize::Action)
                    .variant(MoonButtonVariant::Soft)
                    .width(design::font_w(cx, 28.0))
                    .disabled(preview_disabled)
                    .on_click(move |_, _, cx| {
                        if !preview_backend.read(cx).quiet_sleeping {
                            crate::media::sound::play(&preview);
                        }
                    }),
                ),
        )
    }
}
