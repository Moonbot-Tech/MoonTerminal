//! Strategies parameter list dialog implementation.

use super::*;

impl StrategiesView {
    /// Open a token-only editor without changing the ordinary input's replacement semantics.
    ///
    /// The dialog owns its input; typing here never stages replacements. Confirm resolves each
    /// captured strategy's current value and stages the operation, while Cancel changes nothing.
    pub(super) fn open_list_field_dialog(
        &mut self,
        keys: Vec<Key>,
        field: String,
        operation: ListEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if list_field_values(self, self.backend.read(cx).session.store(), &keys, &field).is_none() {
            return;
        }
        let target = Rc::new(ListEditTarget {
            workspace_generation: self.action_workspace_generation(cx),
            keys,
            field,
        });
        let input = cx.new(|cx| {
            MoonInputState::new(window, cx)
                .placeholder(t!("strat.list_tokens_placeholder").to_string())
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        let view = cx.entity();
        window.open_unique_moon_dialog("strat-list-edit", cx, move |dialog, _window, cx| {
            let (label, hint) = match operation {
                ListEdit::Append => (t!("strat.list_append"), t!("strat.list_append_hint")),
                ListEdit::Remove => (t!("strat.list_remove"), t!("strat.list_remove_hint")),
            };
            let p = MoonPalette::active(cx);
            let content_input = input.clone();
            let confirm_input = input.clone();
            let confirm_target = target.clone();
            let confirm_view = view.clone();
            let hint = hint.to_string();
            let selection = t!("strat.selected_count", n = target.keys.len()).to_string();
            dialog
                .w(design::font_w_px(cx, 420.0))
                .max_w(relative(0.9))
                .close_button(true)
                .overlay(true)
                .overlay_closable(true)
                .bg(moon(p.shell_high))
                .border_color(moon(p.border))
                .text_color(moon(p.text))
                .title(format!("{label}: {}", target.field))
                .content(move |content, _, cx| {
                    content.child(
                        v_flex()
                            .w_full()
                            .gap(design::ui_px(cx, 8.0))
                            .child(div().child(selection.clone()))
                            .child(div().text_color(moon(p.text_soft)).child(hint.clone()))
                            .child(
                                MoonInput::new("strat-list-tokens")
                                    .state(&content_input)
                                    .size(design::INPUT_SIZE),
                            ),
                    )
                })
                .footer(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap(design::ui_px(cx, 8.0))
                        .child(
                            MoonButton::new("strat-list-cancel")
                                .ghost()
                                .label(t!("dialogs.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx))
                                .render(),
                        )
                        .child(
                            MoonButton::new("strat-list-confirm")
                                .primary()
                                .label(label)
                                .on_click(move |_, window, cx| {
                                    let entered = confirm_input.read(cx).value().to_string();
                                    let staged = confirm_view.update(cx, |this, cx| {
                                        this.stage_list_field_value(
                                            &confirm_target,
                                            &entered,
                                            operation,
                                            cx,
                                        )
                                    });
                                    if staged {
                                        window.close_dialog(cx);
                                    } else {
                                        window.push_notification(
                                            moon_ui::MoonNotification::warning(t!(
                                                "strat.list_stale"
                                            )),
                                            cx,
                                        );
                                    }
                                })
                                .render(),
                        ),
                )
        });
    }

    /// A version's `valid_from` as the pane states it: bare `HH:MM` when the version is from
    /// today, `DD.MM HH:MM` otherwise.
    ///
    /// One helper for both banners so they can never end up rendering the same instant against
    /// different `now_ms` snapshots — which is the only way two dates for one version could ever
    /// disagree on this screen.
    pub(super) fn version_date(&self, vf: i64) -> String {
        moon_core::util::display_time::format_chart_clock(
            vf,
            self.display_zone,
            false,
            moon_core::util::now_unix_ms_i64(),
        )
    }
}
