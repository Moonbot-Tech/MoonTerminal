//! Explicit address-based station core comparison, updates and confirmed removals.

use gpui::*;
use moon_remote::ssh::Target;
use moon_ui::{
    MoonButton, MoonGroupBox, MoonPalette, MoonTag, MoonTone, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::super::SettingsView;
use crate::backend::station::{
    cores_sync::{self, Counts, Row, RowState},
    job::Job,
};
use crate::design;

/// Return the state's localization key and semantic tone independently of row actions.
fn state_label(state: RowState) -> (&'static str, MoonTone) {
    match state {
        RowState::Same => ("telegram.server.cores_state_same", MoonTone::Positive),
        RowState::OnlyHere => ("telegram.server.cores_state_only_here", MoonTone::Info),
        RowState::OnlyOnStation => ("telegram.server.cores_state_only_station", MoonTone::Danger),
        RowState::NameDiffers => ("telegram.server.cores_state_name", MoonTone::Warning),
        RowState::KeyDiffers => ("telegram.server.cores_state_key", MoonTone::Warning),
    }
}

/// Offer removal only for station-only entries when another station core remains.
fn removable(row: &Row, station_count: usize) -> bool {
    row.state == RowState::OnlyOnStation && row.station_uid.is_some() && station_count > 1
}

/// A vanished or no-longer-removable row cannot retain a destructive confirmation.
fn retained_arm(armed: Option<RemovalArm>, rows: &[Row]) -> Option<RemovalArm> {
    let count = Counts::from_rows(rows).on_station;
    armed.filter(|(uid, address, name)| {
        rows.iter().any(|row| {
            row.station_uid == Some(*uid)
                && &row.address == address
                && &row.name == name
                && removable(row, count)
        })
    })
}

/// Confirmation binds every visible part of the identity, not just a reused uid.
pub(super) type RemovalArm = (u64, Option<String>, String);

/// Comparison cache invalidates on the listing and the secret-free local config revision.
#[derive(Default)]
pub(super) struct CoreCache {
    seen: Option<Option<Vec<moon_core::station_api::ListedCore>>>,
    high_water: Option<u64>,
    local_revision: Vec<cores_sync::LocalCore>,
    pub(super) rows: Vec<Row>,
}
impl CoreCache {
    /// Reconcile only when inputs change; render uses the retained rows.
    fn update(
        &mut self,
        seen: &Option<Option<Vec<moon_core::station_api::ListedCore>>>,
        local: Vec<cores_sync::LocalCore>,
        high_water: Option<u64>,
    ) {
        if &self.seen != seen || self.local_revision != local || self.high_water != high_water {
            self.rows = seen
                .as_ref()
                .and_then(Option::as_ref)
                .map(|listing| cores_sync::reconcile(&local, listing, high_water))
                .unwrap_or_default();
            self.seen = seen.clone();
            self.high_water = high_water;
            self.local_revision = local;
        }
    }
}

/// Choose a table only when its measured controls and readable text columns all fit.
fn table_fits(width: f32, action: f32, state: f32, name: f32, address: f32, gaps: f32) -> bool {
    width >= action + state + name + address + gaps
}

impl SettingsView {
    /// Clear stale confirmations after a listing change or any backend job starts.
    pub(super) fn station_cores_sync(&mut self, cx: &App) {
        let backend = self.backend.read(cx);
        self.telegram.server.cores_cache.update(
            &backend.station.cores_seen,
            cores_sync::local_cores(&backend.config),
            backend.station.core_uid_high_water,
        );
        self.telegram.server.cores_armed = if backend.station.busy() {
            None
        } else {
            retained_arm(
                self.telegram.server.cores_armed.take(),
                &self.telegram.server.cores_cache.rows,
            )
        };
    }

    /// Show the last listing without requiring a station bot; old stations get an update notice.
    pub(super) fn station_cores_block(
        &self,
        target: &Target,
        width: f32,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let backend = self.backend.read(cx);
        let seen = backend.station.cores_seen.as_ref()?;
        let palette = MoonPalette::active(cx);
        let muted = rgba_from(palette.text_muted, 1.0);
        let mut section = MoonGroupBox::new("station-cores")
            .title(t!("telegram.server.cores_title").to_string())
            .padding(12.0)
            .gap(10.0);
        let Some(listing) = seen else {
            return Some(
                section
                    .child(
                        div()
                            .text_color(muted)
                            .child(t!("telegram.server.cores_unsupported").to_string()),
                    )
                    .into_any_element(),
            );
        };
        let rows = &self.telegram.server.cores_cache.rows;
        let counts = Counts::from_rows(rows);
        let busy = backend.station.busy();
        let metrics = design::CONTROL_TIER.control_metrics();
        let text_width = |key: &str| {
            design::ui_text_width_zoomed(cx, t!(key).as_ref(), metrics.font_size, 400.0, false)
        };
        let button_padding = 2.0 * design::ui_value(cx, metrics.pad_x + 1.0);
        let cancel = text_width("dialogs.cancel") + button_padding;
        let action_width = [
            "telegram.server.cores_add",
            "telegram.server.cores_name",
            "telegram.server.cores_key",
        ]
        .into_iter()
        .map(|key| text_width(key) + button_padding)
        .chain(std::iter::once(
            text_width("telegram.server.cores_remove_confirm")
                + button_padding
                + cancel
                + design::ui_value(cx, 6.0),
        ))
        .fold(0.0_f32, f32::max);
        let state_width = [
            RowState::Same,
            RowState::OnlyHere,
            RowState::OnlyOnStation,
            RowState::NameDiffers,
            RowState::KeyDiffers,
        ]
        .into_iter()
        .map(|state| text_width(state_label(state).0) + button_padding)
        .fold(0.0_f32, f32::max);
        let wide = table_fits(
            width - design::ui_value(cx, 24.0),
            action_width,
            state_width,
            design::ui_value(cx, 100.0),
            design::ui_value(cx, 150.0),
            design::ui_value(cx, 24.0),
        );
        section = MoonGroupBox::new("station-cores")
            .padding(12.0)
            .gap(10.0)
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .flex_wrap()
                    .justify_between()
                    .gap(design::ui_px(cx, 8.0))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("telegram.server.cores_title").to_string()),
                    )
                    .child(
                        div().font_family(design::mono()).text_color(muted).child(
                            t!(
                                "telegram.server.cores_count",
                                station = counts.on_station,
                                here = counts.here
                            )
                            .to_string(),
                        ),
                    ),
            );
        let mut summary = h_flex().flex_wrap().gap(design::ui_px(cx, 6.0));
        if counts.only_here + counts.only_station + counts.differs == 0 {
            summary = summary.child(t!("telegram.server.cores_all_same").to_string());
        }
        for (n, key, tone) in [
            (
                counts.same,
                "telegram.server.cores_sum_same",
                MoonTone::Positive,
            ),
            (
                counts.only_here,
                "telegram.server.cores_sum_only_here",
                MoonTone::Info,
            ),
            (
                counts.differs,
                "telegram.server.cores_sum_differ",
                MoonTone::Warning,
            ),
            (
                counts.only_station,
                "telegram.server.cores_sum_only_station",
                MoonTone::Danger,
            ),
        ] {
            if n != 0 {
                summary = summary.child(
                    MoonTag::new()
                        .tone(tone)
                        .mono(false)
                        .child(t!(key, n = n).to_string()),
                );
            }
        }
        section = section.child(summary).child(
            div()
                .text_color(muted)
                .child(t!("telegram.server.cores_sync_hint").to_string()),
        );
        if wide {
            section = section.child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap(design::ui_px(cx, 8.0))
                    .text_color(muted)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(t!("telegram.server.cores_col_core").to_string()),
                    )
                    .child(
                        div()
                            .w(design::ui_px(cx, 150.0))
                            .min_w_0()
                            .truncate()
                            .child(t!("telegram.server.cores_col_address").to_string()),
                    )
                    .child(
                        div()
                            .w(px(state_width))
                            .flex_shrink_0()
                            .child(t!("telegram.server.cores_col_state").to_string()),
                    )
                    .child(div().w(px(action_width)).flex_shrink_0()),
            );
        }
        if listing.is_empty() {
            section = section.child(
                div()
                    .text_color(muted)
                    .child(t!("telegram.server.cores_empty").to_string()),
            );
        }
        for row in rows {
            let mut name = v_flex()
                .min_w_0()
                .child(div().truncate().child(row.name.clone()));
            if row.state == RowState::NameDiffers {
                name = name.child(
                    div()
                        .text_color(muted)
                        .font_family(design::mono())
                        .truncate()
                        .child(
                            t!(
                                "telegram.server.cores_was",
                                name = row.station_name.as_deref().unwrap_or_default()
                            )
                            .to_string(),
                        ),
                );
            } else if row.state == RowState::OnlyOnStation {
                name = name.child(
                    div()
                        .text_color(muted)
                        .child(t!("telegram.server.cores_missing_here").to_string()),
                );
            }
            let address = div()
                .min_w_0()
                .font_family(design::mono())
                .text_color(muted)
                .truncate()
                .child(row.address.clone().unwrap_or_else(|| "\u{2014}".into()));
            let (key, tone) = state_label(row.state);
            let state = MoonTag::new()
                .tone(tone)
                .mono(false)
                .child(t!(key).to_string());
            let actions = self.station_core_actions(row, counts.on_station, target, busy, cx);
            let content = if wide {
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .child(div().flex_1().min_w_0().child(name))
                    .child(div().w(design::ui_px(cx, 150.0)).min_w_0().child(address))
                    .child(div().w(px(state_width)).flex_shrink_0().child(state))
                    .child(div().w(px(action_width)).flex_shrink_0().child(actions))
                    .into_any_element()
            } else {
                v_flex()
                    .w_full()
                    .min_w_0()
                    .gap(design::ui_px(cx, 6.0))
                    .child(name)
                    .child(address)
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .flex_wrap()
                            .items_center()
                            .gap(design::ui_px(cx, 8.0))
                            .child(state)
                            .child(div().flex_1().min_w_0().child(actions)),
                    )
                    .into_any_element()
            };
            section = section.child(
                div()
                    .w_full()
                    .min_w_0()
                    .py(design::ui_px(cx, 6.0))
                    .border_b_1()
                    .border_color(rgba_from(palette.border, 1.0))
                    .child(content),
            );
        }
        let upsert = cores_sync::bulk(rows);
        let target = target.clone();
        let eligible: Vec<_> = self
            .telegram
            .server
            .cores_cache
            .local_revision
            .iter()
            .map(|core| core.uid)
            .collect();
        section = section.child(
            MoonButton::new("station-cores-send-all")
                .primary()
                .size(design::CONTROL_TIER)
                .label(if counts.pushable == 0 {
                    t!("telegram.server.cores_nothing").to_string()
                } else {
                    t!("telegram.server.cores_send_all", n = counts.pushable).to_string()
                })
                .disabled(busy || upsert.is_empty())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.server_bot_run(
                        Ok(Job::Cores {
                            target: target.clone(),
                            upsert: upsert.clone(),
                            eligible: eligible.clone(),
                        }),
                        cx,
                    );
                }))
                .render(),
        );
        if counts.only_station != 0 {
            section = section.child(
                div()
                    .text_color(muted)
                    .child(t!("telegram.server.cores_send_all_hint").to_string()),
            );
        }
        section = section.child(
            div()
                .w_full()
                .min_w_0()
                .font_family(design::mono())
                .text_color(muted)
                .min_h(design::ui_px(cx, 28.0))
                .p(design::ui_px(cx, 6.0))
                .rounded(design::ui_px(cx, 5.0))
                .bg(rgba_from(palette.shell_high, 1.0))
                .truncate()
                .child(backend.station.cores_result.clone().unwrap_or_default()),
        );
        Some(section.into_any_element())
    }

    /// Bind row actions to station identities; removal needs an explicit second click.
    fn station_core_actions(
        &self,
        row: &Row,
        station_count: usize,
        target: &Target,
        busy: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let mut actions = h_flex()
            .w_full()
            .min_w_0()
            .flex_wrap()
            .gap(design::ui_px(cx, 6.0));
        let id = row.station_uid.unwrap_or_default();
        let changes = cores_sync::bulk(std::slice::from_ref(row));
        let eligible: Vec<_> = self
            .telegram
            .server
            .cores_cache
            .local_revision
            .iter()
            .map(|core| core.uid)
            .collect();
        let action = match row.state {
            RowState::OnlyHere => Some("telegram.server.cores_add"),
            RowState::NameDiffers => Some("telegram.server.cores_name"),
            RowState::KeyDiffers => Some("telegram.server.cores_key"),
            _ => None,
        };
        if let Some(key) = action {
            let target = target.clone();
            actions = actions.child(
                MoonButton::new(SharedString::from(format!("station-core-send-{id}")))
                    .size(design::CONTROL_TIER)
                    .label(t!(key).to_string())
                    .disabled(busy || changes.is_empty())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.server_bot_run(
                            Ok(Job::Cores {
                                target: target.clone(),
                                upsert: changes.clone(),
                                eligible: eligible.clone(),
                            }),
                            cx,
                        );
                    }))
                    .render(),
            );
        } else if removable(row, station_count) {
            let arm = (id, row.address.clone(), row.name.clone());
            let armed = self.telegram.server.cores_armed.as_ref() == Some(&arm);
            let target = target.clone();
            let name = row.name.clone();
            actions = actions.child(
                MoonButton::new(SharedString::from(format!("station-core-remove-{id}")))
                    .danger()
                    .size(design::CONTROL_TIER)
                    .label(
                        t!(if armed {
                            "telegram.server.cores_remove_confirm"
                        } else {
                            "telegram.server.cores_remove"
                        })
                        .to_string(),
                    )
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.telegram.server.cores_armed.as_ref() == Some(&arm) {
                            this.server_bot_run(
                                Ok(Job::CoresRemove {
                                    target: target.clone(),
                                    uids: vec![id],
                                    names: vec![name.clone()],
                                    addresses: vec![arm.1.clone()],
                                    eligible: eligible.clone(),
                                }),
                                cx,
                            );
                        } else {
                            this.telegram.server.cores_armed = Some(arm.clone());
                            cx.notify();
                        }
                    }))
                    .render(),
            );
            if armed {
                actions = actions.child(
                    MoonButton::new(SharedString::from(format!("station-core-cancel-{id}")))
                        .size(design::CONTROL_TIER)
                        .label(t!("dialogs.cancel").to_string())
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.telegram.server.cores_armed = None;
                            cx.notify();
                        }))
                        .render(),
                );
            }
        }
        actions.into_any_element()
    }
}

#[cfg(test)]
mod tests;
