//! Shared station and terminal message builder; edits use the existing bot draft transaction.

use super::{SettingsView, access::ChatsOf};
use crate::design;
use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::config::{
    CardField, CardLayout, GroupRowStyle, MessageLayout, ReportColumn, TotalPlace, TotalSeparation,
};
use moon_ui::{
    MoonButton, MoonCheckbox, MoonGroupBox, MoonPalette, MoonTabItem, MoonTabStrip, MoonTag,
    h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

mod preview;
#[cfg(test)]
mod tests;

/// Navigation and typed samples survive rerenders but are never serialized into bot settings.
#[derive(Default)]
pub(in crate::settings) struct MessageLayoutEd {
    report: bool,
    preview: preview::PreviewCache,
}

/// Card field dragged between lines and the hidden tray on one bot side.
#[derive(Clone)]
struct ChipDrag {
    field: CardField,
    side: ChatsOf,
}
/// Whole card line dragged by its grip.
#[derive(Clone)]
struct LineDrag {
    index: usize,
    side: ChatsOf,
}
/// Report column dragged between positions or into its hidden tray.
#[derive(Clone)]
struct ColumnDrag {
    col: ReportColumn,
    side: ChatsOf,
}
/// MoonUI tag shown under the drag cursor.
struct DragPreview {
    label: String,
}
impl Render for DragPreview {
    /// Preserve the chip's appearance while the pointer moves between drop targets.
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        MoonTag::new().mono(false).child(self.label.clone())
    }
}

/// One model operation shared by keyboard, pointer and drag handlers.
#[derive(Clone)]
enum LayoutAction {
    Field(CardField, usize, usize),
    HideField(CardField),
    Line(usize, usize),
    Column(ReportColumn, usize),
    HideColumn(ReportColumn),
    RestoreColumn(ReportColumn),
    Preset(bool),
    CoinHashtag(bool),
    CoreHashtag(bool),
    Total(TotalPlace),
    Separation(TotalSeparation),
    Group(GroupRowStyle),
    Reset,
}

/// Normalize the rendered view, apply an operation, and report whether the draft changed.
fn apply_action(layout: &mut MessageLayout, action: LayoutAction) -> bool {
    let before = layout.clone();
    *layout = editor_view(layout);
    match action {
        LayoutAction::Field(field, line, index) => layout.card.move_field(&field, line, index),
        LayoutAction::HideField(field) => {
            layout.card.hide(&field);
        }
        LayoutAction::Line(from, to) => layout.card.move_line(from, to),
        LayoutAction::Column(col, index) => layout.report.move_column(&col, index),
        LayoutAction::HideColumn(col) => {
            layout.report.hide_column(&col);
        }
        LayoutAction::RestoreColumn(col) => layout.report.restore_column(&col),
        LayoutAction::Preset(core) => {
            let lines = if core {
                CardLayout::moonbot_preset()
            } else {
                CardLayout::coin_first()
            }
            .lines;
            layout.card.lines = lines;
        }
        LayoutAction::CoinHashtag(value) => layout.card.coin_hashtag = value,
        LayoutAction::CoreHashtag(value) => layout.card.core_hashtag = value,
        LayoutAction::Total(value) => layout.report.total = value,
        LayoutAction::Separation(value) => layout.report.separation = value,
        LayoutAction::Group(value) => layout.report.group_row = value,
        LayoutAction::Reset => *layout = MessageLayout::default(),
    }
    *layout != before
}

/// Match the renderers' normalization before drawing or editing saved preferences.
fn editor_view(layout: &MessageLayout) -> MessageLayout {
    layout.sanitized()
}
/// Move across drawable neighbours while retaining unknown ids in the raw order.
fn neighbour_target(columns: &[ReportColumn], col: &ReportColumn, dir: i8) -> Option<usize> {
    let visible: Vec<_> = columns
        .iter()
        .enumerate()
        .filter(|(_, value)| ReportColumn::KNOWN.contains(value))
        .collect();
    let index = visible.iter().position(|(_, value)| *value == col)?;
    let neighbour = match dir {
        -1 => index.checked_sub(1)?,
        1 => index.checked_add(1)?,
        _ => return None,
    };
    visible.get(neighbour).map(|(raw, _)| *raw)
}
/// Restore hidden fields at the last existing line, creating one only when absent.
fn restore_target(card: &CardLayout) -> usize {
    card.lines.len().saturating_sub(1)
}
/// Apply the same drop affordance to every chip, line and tray.
fn drop_highlight(style: StyleRefinement, color: Hsla) -> StyleRefinement {
    style.bg(color)
}
/// Build the shared chip preview without duplicating drag rendering closures.
fn drag_ghost<T>(
    label: String,
) -> impl Fn(&T, Point<Pixels>, &mut Window, &mut App) -> Entity<DragPreview> {
    move |_, _, _, cx| {
        cx.new(|_| DragPreview {
            label: label.clone(),
        })
    }
}
/// The saved/read-back layout alone determines saved status; missing read-back stays dirty.
fn dirty(draft: &MessageLayout, base: Option<&MessageLayout>) -> bool {
    base != Some(draft)
}
/// Light a preset by its line order, independently of the hashtag switches.
fn segment_index(layout: &CardLayout) -> Option<usize> {
    if layout.is_preset(&CardLayout::coin_first()) {
        Some(0)
    } else if layout.is_preset(&CardLayout::moonbot_preset()) {
        Some(1)
    } else {
        None
    }
}
/// Resolve a chip drop before a field, or at the original line end/new-line row.
fn drop_target_index(card: &CardLayout, line: usize, before: Option<&CardField>) -> usize {
    card.lines.get(line).map_or(0, |fields| {
        before
            .and_then(|field| fields.iter().position(|shown| shown == field))
            .unwrap_or(fields.len())
    })
}
/// Resolve a report drop before a column, accounting for removal of the dragged column.
fn column_drop_index(
    columns: &[ReportColumn],
    col: &ReportColumn,
    before: Option<&ReportColumn>,
) -> usize {
    let remaining: Vec<_> = columns.iter().filter(|shown| *shown != col).collect();
    if before == Some(col) {
        return columns
            .iter()
            .position(|shown| shown == col)
            .unwrap_or(columns.len());
    }
    before
        .and_then(|target| remaining.iter().position(|shown| *shown == target))
        .unwrap_or(remaining.len())
}
/// Stable ids keep station and terminal controls independent in the same window.
fn control_id(side: ChatsOf, name: &str) -> SharedString {
    format!(
        "tgl-{}-{name}",
        if side == ChatsOf::Station { "s" } else { "t" }
    )
    .into()
}
/// Localized labels never derive from unknown saved ids.
fn field_label(field: &CardField) -> String {
    let key = format!("telegram.layout.field.{}", field.id());
    t!(&key).to_string()
}
/// Localized report column caption.
fn column_label(col: &ReportColumn) -> String {
    let key = format!("telegram.layout.col.{}", col.id());
    t!(&key).to_string()
}

impl SettingsView {
    /// The session cache owned by the selected bot's existing editor.
    fn layout_ed(&self, side: ChatsOf) -> &MessageLayoutEd {
        match side {
            ChatsOf::Terminal => &self.telegram.message_layout,
            ChatsOf::Station => &self.telegram.server.message_layout,
        }
    }
    /// Mutable session cache for the selected bot editor.
    fn layout_ed_mut(&mut self, side: ChatsOf) -> &mut MessageLayoutEd {
        match side {
            ChatsOf::Terminal => &mut self.telegram.message_layout,
            ChatsOf::Station => &mut self.telegram.server.message_layout,
        }
    }
    /// Navigate without changing the saved layout or bot signature.
    fn layout_tab(&mut self, side: ChatsOf, report: bool, cx: &mut Context<Self>) {
        let ed = self.layout_ed_mut(side);
        ed.report = report;
        cx.notify();
    }
    /// Route every operation through the same station/terminal bot transaction as the menu.
    fn layout_edit(&mut self, side: ChatsOf, action: LayoutAction, cx: &mut Context<Self>) {
        self.bot_settings_edit(side, cx, |bot| {
            apply_action(&mut bot.message_layout, action)
        });
    }
    /// A focusable MoonUI action with a localized tooltip, shared by chip and row controls.
    fn layout_button(
        &self,
        side: ChatsOf,
        name: &str,
        label: &str,
        key: &str,
        disabled: bool,
        action: LayoutAction,
        cx: &Context<Self>,
    ) -> AnyElement {
        MoonButton::new(control_id(side, name))
            .ghost()
            .label(label.to_string())
            .tooltip(t!(key).to_string())
            .disabled(disabled)
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.layout_edit(side, action.clone(), cx);
            }))
            .render()
            .into_any_element()
    }
    /// Wrap focusable option buttons onto additional rows when the settings column is narrow.
    fn layout_options(
        &self,
        side: ChatsOf,
        name: &str,
        keys: [&str; 2],
        selected: Option<usize>,
        actions: [LayoutAction; 2],
        cx: &Context<Self>,
    ) -> AnyElement {
        let mut row = h_flex().flex_wrap().gap(design::ui_px(cx, 6.0));
        for (index, (key, action)) in keys.into_iter().zip(actions).enumerate() {
            row = row.child(
                MoonButton::new(control_id(side, &format!("{name}-{index}")))
                    .when(selected == Some(index), |button| button.primary())
                    .when(selected != Some(index), |button| button.ghost())
                    .label(t!(key).to_string())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.layout_edit(side, action.clone(), cx);
                    }))
                    .render(),
            );
        }
        row.into_any_element()
    }
    /// Stack the editor and phone under the bot menu and reuse the existing save path.
    pub(in crate::settings) fn message_layout_box(
        &self,
        side: ChatsOf,
        cx: &Context<Self>,
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        let mut body = v_flex().w_full().min_w_0().gap(design::ui_px(cx, 12.0));
        let station_knows = self
            .telegram
            .server
            .access_base
            .as_ref()
            .is_some_and(|base| base.bot.is_some());
        let Some(layout) = self
            .chats(side, cx)
            .map(|telegram| telegram.bot.message_layout.as_ref())
        else {
            return body
                .when(side == ChatsOf::Station, |body| {
                    body.text_color(rgba_from(p.text_muted, 1.0))
                        .child(t!("telegram.menu_editor.station_too_old").to_string())
                })
                .into_any_element();
        };
        if side == ChatsOf::Station && !station_knows {
            return body
                .text_color(rgba_from(p.text_muted, 1.0))
                .child(t!("telegram.menu_editor.station_too_old").to_string())
                .into_any_element();
        }
        let report = self.layout_ed(side).report;
        let weak = cx.entity().downgrade();
        body = body.child(
            MoonTabStrip::new(control_id(side, "tabs"))
                .padding_left(0.0)
                .gap(4.0)
                .items([
                    MoonTabItem::new(t!("telegram.layout.tab_card").to_string()).selected(!report),
                    MoonTabItem::new(t!("telegram.layout.tab_report").to_string()).selected(report),
                ])
                .on_click(move |index, _, _, app| {
                    let _ = weak.update(app, |this, cx| this.layout_tab(side, index == 1, cx));
                }),
        );
        body = body.child(if report {
            self.layout_report_editor(side, layout, cx)
        } else {
            self.layout_card_editor(side, layout, cx)
        });
        let content = if report {
            preview::PreviewContent::Report(moon_tg::preview_report(&layout.report))
        } else {
            preview::PreviewContent::Card(moon_tg::preview_card(&layout.card))
        };
        body = body.child(preview::phone(
            content,
            self.layout_ed(side).preview.clone(),
        ));
        let base = match side {
            ChatsOf::Terminal => Some(
                self.backend
                    .read(cx)
                    .config
                    .telegram
                    .bot
                    .message_layout
                    .as_ref(),
            ),
            ChatsOf::Station => self
                .telegram
                .server
                .access_base
                .as_ref()
                .and_then(|base| base.bot.as_ref())
                .map(|bot| bot.message_layout.as_ref()),
        };
        let footer = v_flex()
            .w_full()
            .gap(design::ui_px(cx, 8.0))
            .border_t_1()
            .border_color(rgba_from(p.border, 1.0))
            .pt(design::ui_px(cx, 12.0))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 8.0))
                    .child(self.layout_button(
                        side,
                        "reset",
                        &t!("telegram.layout.reset_default"),
                        "telegram.layout.reset_default",
                        false,
                        LayoutAction::Reset,
                        cx,
                    ))
                    .child(
                        div().text_color(rgba_from(p.text_muted, 1.0)).child(
                            t!(if dirty(layout, base) {
                                "telegram.layout.dirty"
                            } else {
                                "telegram.layout.saved"
                            })
                            .to_string(),
                        ),
                    ),
            )
            .when(side == ChatsOf::Station, |footer| {
                footer.child(self.server_access_actions("tgl", cx))
            })
            .when(side == ChatsOf::Terminal, |footer| {
                footer.child(
                    div()
                        .text_color(rgba_from(p.text_muted, 1.0))
                        .child(t!("telegram.layout.terminal_save_hint").to_string()),
                )
            });
        MoonGroupBox::new(control_id(side, "box"))
            .title(t!("telegram.layout.title").to_string())
            .padding(14.0)
            .child(body.child(footer))
            .into_any_element()
    }

    /// Numbered draggable card lines, hidden-field tray, presets and independent hashtag switches.
    fn layout_card_editor(
        &self,
        side: ChatsOf,
        layout: &MessageLayout,
        cx: &Context<Self>,
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        let view = editor_view(layout);
        let card = &view.card;
        let mut editor = v_flex()
            .w_full()
            .min_w_0()
            .gap(design::ui_px(cx, 8.0))
            .child(
                div()
                    .text_color(rgba_from(p.text_muted, 1.0))
                    .child(t!("telegram.layout.start_with").to_string()),
            )
            .child(self.layout_options(
                side,
                "preset",
                ["telegram.layout.preset_coin", "telegram.layout.preset_core"],
                segment_index(card),
                [LayoutAction::Preset(false), LayoutAction::Preset(true)],
                cx,
            ));
        for (line_index, fields) in card.lines.iter().enumerate() {
            let grip_label = format!("{}", line_index + 1);
            let grip = div()
                .id(control_id(side, &format!("grip-{line_index}")))
                .cursor_grab()
                .child("⋮⋮")
                .on_drag(
                    LineDrag {
                        index: line_index,
                        side,
                    },
                    drag_ghost(grip_label),
                );
            let mut row = h_flex()
                .w_full()
                .min_w_0()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .child(
                    div()
                        .text_color(rgba_from(p.text_muted, 1.0))
                        .child((line_index + 1).to_string()),
                )
                .child(grip);
            let mut chips = h_flex()
                .flex_1()
                .min_w_0()
                .flex_wrap()
                .gap(design::ui_px(cx, 6.0));
            for field in fields
                .iter()
                .filter(|field| CardField::KNOWN.contains(field))
            {
                let field = field.clone();
                let target = field.clone();
                let label = field_label(&field);
                let preview_label = label.clone();
                let chip = div()
                    .id(control_id(side, &format!("field-{}", field.id())))
                    .cursor_grab()
                    .child(
                        MoonTag::new()
                            .mono(false)
                            .child(label)
                            .when(
                                (field == CardField::Coin && card.coin_hashtag)
                                    || (field == CardField::Core && card.core_hashtag),
                                |tag| tag.child("#"),
                            )
                            .child(self.layout_button(
                                side,
                                &format!("hide-{}", field.id()),
                                "×",
                                "telegram.layout.hide",
                                !card.can_hide(&field),
                                LayoutAction::HideField(field.clone()),
                                cx,
                            )),
                    )
                    .on_drag(ChipDrag { field, side }, drag_ghost(preview_label))
                    .drag_over::<ChipDrag>(move |style, _, _, _| {
                        drop_highlight(style, rgba_from(p.blue, 0.15))
                    })
                    .on_drop(cx.listener(move |this, drag: &ChipDrag, _, cx| {
                        cx.stop_propagation();
                        if drag.side != side {
                            return;
                        }
                        let Some(card) = this
                            .chats(side, cx)
                            .map(|t| t.bot.message_layout.card.sanitized())
                        else {
                            return;
                        };
                        let index = drop_target_index(&card, line_index, Some(&target));
                        this.layout_edit(
                            side,
                            LayoutAction::Field(drag.field.clone(), line_index, index),
                            cx,
                        );
                    }));
                chips = chips.child(chip);
            }
            if !fields.iter().any(|field| CardField::KNOWN.contains(field)) {
                chips = chips.child(t!("telegram.layout.line_empty").to_string());
            }
            row = row
                .child(chips)
                .child(self.layout_button(
                    side,
                    &format!("line-{line_index}-up"),
                    "↑",
                    "telegram.layout.move_up",
                    line_index == 0,
                    LayoutAction::Line(line_index, line_index.saturating_sub(1)),
                    cx,
                ))
                .child(self.layout_button(
                    side,
                    &format!("line-{line_index}-down"),
                    "↓",
                    "telegram.layout.move_down",
                    line_index + 1 == card.lines.len(),
                    LayoutAction::Line(line_index, line_index + 1),
                    cx,
                ));
            editor =
                editor.child(self.layout_line_target(side, line_index, row.into_any_element(), cx));
        }
        editor = editor.child(
            self.layout_line_target(
                side,
                card.lines.len(),
                div()
                    .text_color(rgba_from(p.text_muted, 1.0))
                    .child(t!("telegram.layout.line_new").to_string())
                    .into_any_element(),
                cx,
            ),
        );
        let mut tray = h_flex()
            .w_full()
            .flex_wrap()
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .child(t!("telegram.layout.tray").to_string());
        let hidden = card.tray();
        for field in &hidden {
            let label = field_label(field);
            let preview_label = label.clone();
            tray = tray.child(
                div()
                    .id(control_id(side, &format!("tray-{}", field.id())))
                    .child(self.layout_button(
                        side,
                        &format!("restore-{}", field.id()),
                        &label,
                        "telegram.layout.restore",
                        false,
                        LayoutAction::Field(field.clone(), restore_target(card), usize::MAX),
                        cx,
                    ))
                    .on_drag(
                        ChipDrag {
                            field: field.clone(),
                            side,
                        },
                        drag_ghost(preview_label),
                    ),
            );
        }
        if hidden.is_empty() {
            tray = tray.child(t!("telegram.layout.tray_empty_fields").to_string());
        }
        editor = editor.child(
            div()
                .id(control_id(side, "field-tray"))
                .border_1()
                .border_color(rgba_from(p.border, 1.0))
                .rounded(design::ui_px(cx, 6.0))
                .p(design::ui_px(cx, 8.0))
                .child(tray)
                .drag_over::<ChipDrag>(move |style, _, _, _| {
                    drop_highlight(style, rgba_from(p.blue, 0.15))
                })
                .on_drop(cx.listener(move |this, drag: &ChipDrag, _, cx| {
                    if drag.side == side {
                        this.layout_edit(side, LayoutAction::HideField(drag.field.clone()), cx);
                    }
                })),
        );
        editor
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 12.0))
                    .child(
                        MoonCheckbox::new(control_id(side, "coin-hashtag"))
                            .checked(card.coin_hashtag)
                            .label(t!("telegram.layout.hashtag_coin").to_string())
                            .on_change(cx.listener(move |this, value: &bool, _, cx| {
                                this.layout_edit(side, LayoutAction::CoinHashtag(*value), cx)
                            })),
                    )
                    .child(
                        MoonCheckbox::new(control_id(side, "core-hashtag"))
                            .checked(card.core_hashtag)
                            .label(t!("telegram.layout.hashtag_core").to_string())
                            .on_change(cx.listener(move |this, value: &bool, _, cx| {
                                this.layout_edit(side, LayoutAction::CoreHashtag(*value), cx)
                            })),
                    ),
            )
            .child(
                div()
                    .text_color(rgba_from(p.text_muted, 1.0))
                    .child(t!("telegram.layout.hashtag_hint").to_string()),
            )
            .into_any_element()
    }

    /// Highlight a line end/new row and accept field append or whole-line reordering.
    fn layout_line_target(
        &self,
        side: ChatsOf,
        line: usize,
        content: AnyElement,
        cx: &Context<Self>,
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        div()
            .id(control_id(side, &format!("line-{line}")))
            .w_full()
            .min_w_0()
            .border_1()
            .border_color(rgba_from(p.border, 1.0))
            .rounded(design::ui_px(cx, 6.0))
            .p(design::ui_px(cx, 6.0))
            .bg(rgba_from(p.shell_high, 1.0))
            .child(content)
            .drag_over::<ChipDrag>(move |style, _, _, _| {
                drop_highlight(style, rgba_from(p.blue, 0.15))
            })
            .drag_over::<LineDrag>(move |style, _, _, _| {
                drop_highlight(style, rgba_from(p.blue, 0.15))
            })
            .on_drop(cx.listener(move |this, drag: &ChipDrag, _, cx| {
                if drag.side != side {
                    return;
                }
                let Some(card) = this
                    .chats(side, cx)
                    .map(|t| t.bot.message_layout.card.sanitized())
                else {
                    return;
                };
                let index = drop_target_index(&card, line, None);
                this.layout_edit(
                    side,
                    LayoutAction::Field(drag.field.clone(), line, index),
                    cx,
                );
            }))
            .on_drop(cx.listener(move |this, drag: &LineDrag, _, cx| {
                if drag.side == side {
                    this.layout_edit(side, LayoutAction::Line(drag.index, line), cx);
                }
            }))
            .into_any_element()
    }

    /// Report columns with keyboard/drag ordering, the hidden tray and total/group choices.
    fn layout_report_editor(
        &self,
        side: ChatsOf,
        layout: &MessageLayout,
        cx: &Context<Self>,
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        let view = editor_view(layout);
        let report = &view.report;
        let mut columns = h_flex()
            .w_full()
            .min_w_0()
            .flex_wrap()
            .gap(design::ui_px(cx, 6.0))
            .child(
                MoonTag::new()
                    .mono(false)
                    .child(t!("telegram.layout.first_col").to_string()),
            );
        let known_count = report.drawable_columns().count();
        for col in report.drawable_columns() {
            let left = neighbour_target(&report.columns, col, -1);
            let right = neighbour_target(&report.columns, col, 1);
            let col = col.clone();
            let target = col.clone();
            let label = column_label(&col);
            let preview_label = label.clone();
            columns = columns.child(
                div()
                    .id(control_id(side, &format!("column-{}", col.id())))
                    .cursor_grab()
                    .child(
                        MoonTag::new()
                            .mono(false)
                            .child(label)
                            .child(self.layout_button(
                                side,
                                &format!("col-{}-left", col.id()),
                                "←",
                                "telegram.layout.move_left",
                                left.is_none(),
                                LayoutAction::Column(col.clone(), left.unwrap_or(0)),
                                cx,
                            ))
                            .child(self.layout_button(
                                side,
                                &format!("col-{}-right", col.id()),
                                "→",
                                "telegram.layout.move_right",
                                right.is_none(),
                                LayoutAction::Column(col.clone(), right.unwrap_or(0)),
                                cx,
                            ))
                            .child(self.layout_button(
                                side,
                                &format!("col-{}-hide", col.id()),
                                "×",
                                "telegram.layout.hide",
                                !report.can_hide_column(&col),
                                LayoutAction::HideColumn(col.clone()),
                                cx,
                            )),
                    )
                    .on_drag(ColumnDrag { col, side }, drag_ghost(preview_label))
                    .drag_over::<ColumnDrag>(move |style, _, _, _| {
                        drop_highlight(style, rgba_from(p.blue, 0.15))
                    })
                    .on_drop(cx.listener(move |this, drag: &ColumnDrag, _, cx| {
                        cx.stop_propagation();
                        if drag.side != side {
                            return;
                        }
                        let Some(cols) = this
                            .chats(side, cx)
                            .map(|t| t.bot.message_layout.report.sanitized().columns)
                        else {
                            return;
                        };
                        let index = column_drop_index(&cols, &drag.col, Some(&target));
                        this.layout_edit(side, LayoutAction::Column(drag.col.clone(), index), cx);
                    })),
            );
        }
        let columns = div()
            .id(control_id(side, "columns"))
            .child(columns)
            .drag_over::<ColumnDrag>(move |style, _, _, _| {
                drop_highlight(style, rgba_from(p.blue, 0.15))
            })
            .on_drop(cx.listener(move |this, drag: &ColumnDrag, _, cx| {
                if drag.side != side {
                    return;
                }
                let Some(cols) = this
                    .chats(side, cx)
                    .map(|t| t.bot.message_layout.report.sanitized().columns)
                else {
                    return;
                };
                let index = column_drop_index(&cols, &drag.col, None);
                this.layout_edit(side, LayoutAction::Column(drag.col.clone(), index), cx);
            }));
        let mut tray = h_flex()
            .flex_wrap()
            .gap(design::ui_px(cx, 6.0))
            .child(t!("telegram.layout.tray").to_string());
        let hidden = report.hidden_columns();
        for col in &hidden {
            let label = column_label(col);
            let preview_label = label.clone();
            tray = tray.child(
                div()
                    .id(control_id(side, &format!("tray-col-{}", col.id())))
                    .child(self.layout_button(
                        side,
                        &format!("restore-col-{}", col.id()),
                        &label,
                        "telegram.layout.restore",
                        false,
                        LayoutAction::RestoreColumn(col.clone()),
                        cx,
                    ))
                    .on_drag(
                        ColumnDrag {
                            col: col.clone(),
                            side,
                        },
                        drag_ghost(preview_label),
                    ),
            );
        }
        if hidden.is_empty() {
            tray = tray.child(t!("telegram.layout.tray_empty_cols").to_string());
        }
        let tray = div()
            .id(control_id(side, "column-tray"))
            .border_1()
            .border_color(rgba_from(p.border, 1.0))
            .rounded(design::ui_px(cx, 6.0))
            .p(design::ui_px(cx, 8.0))
            .child(tray)
            .drag_over::<ColumnDrag>(move |style, _, _, _| {
                drop_highlight(style, rgba_from(p.blue, 0.15))
            })
            .on_drop(cx.listener(move |this, drag: &ColumnDrag, _, cx| {
                if drag.side == side {
                    this.layout_edit(side, LayoutAction::HideColumn(drag.col.clone()), cx);
                }
            }));
        v_flex()
            .w_full()
            .min_w_0()
            .gap(design::ui_px(cx, 8.0))
            .child(t!("telegram.layout.columns").to_string())
            .child(columns)
            .child(tray)
            .when(known_count == 4, |editor| {
                editor.child(
                    div()
                        .text_color(rgba_from(p.text_muted, 1.0))
                        .child(t!("telegram.layout.col_warn_width").to_string()),
                )
            })
            .child(t!("telegram.layout.total").to_string())
            .child(self.layout_options(
                side,
                "total",
                ["telegram.layout.total_bottom", "telegram.layout.total_top"],
                Some(usize::from(report.total == TotalPlace::Top)),
                [
                    LayoutAction::Total(TotalPlace::Bottom),
                    LayoutAction::Total(TotalPlace::Top),
                ],
                cx,
            ))
            .child(t!("telegram.layout.separation").to_string())
            .child(self.layout_options(
                side,
                "separation",
                ["telegram.layout.sep_band", "telegram.layout.sep_gap_band"],
                Some(usize::from(report.separation == TotalSeparation::GapBand)),
                [
                    LayoutAction::Separation(TotalSeparation::Band),
                    LayoutAction::Separation(TotalSeparation::GapBand),
                ],
                cx,
            ))
            .child(
                div()
                    .text_color(rgba_from(p.text_muted, 1.0))
                    .child(t!("telegram.layout.sep_hint").to_string()),
            )
            .child(t!("telegram.layout.group_row").to_string())
            .child(self.layout_options(
                side,
                "group",
                [
                    "telegram.layout.group_band",
                    "telegram.layout.group_bold_left",
                ],
                Some(usize::from(report.group_row == GroupRowStyle::BoldLeft)),
                [
                    LayoutAction::Group(GroupRowStyle::Band),
                    LayoutAction::Group(GroupRowStyle::BoldLeft),
                ],
                cx,
            ))
            .into_any_element()
    }
}
