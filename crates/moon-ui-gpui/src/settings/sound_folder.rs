//! The "your own sounds" block on the Trade sounds tab: where the folder is, what the last scan
//! took from it and refused, and the two buttons — open the folder, read it again.

use gpui::*;
use moon_ui::{MoonButton, MoonPalette, h_flex, rgba_from, v_flex};
use rust_i18n::t;

use super::{SettingsView, open_folder, section};
use crate::design;
use crate::media::sound::{FIRST_USER_ORDINAL, RejectReason, ScanStats};

impl SettingsView {
    /// The folder block, read straight off the installed catalog: the scan swaps the catalog in
    /// whole, so the stats shown here are always one consistent pass.
    pub(super) fn sound_folder_block(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = MoonPalette::active(cx);
        let muted = rgba_from(p.text_soft, 1.0);
        let dir = moon_core::config::paths::sounds_dir();
        let (scanned, stats, numbered) = crate::media::sound::with_catalog(|c| {
            let numbered: Vec<(i32, String)> = c
                .ordinals()
                .filter(|(n, _)| *n >= FIRST_USER_ORDINAL)
                .map(|(n, e)| (n, e.label.clone()))
                .collect();
            (c.scanned(), c.stats().clone(), numbered)
        });

        let tool_btn = |id: &'static str, label: String| {
            // Spaces around the label work around the fork's `MoonButton` `pad_x=0` bug, which
            // otherwise places text against the outline.
            MoonButton::new(id)
                .outline()
                .small()
                .label(format!("  {label}  "))
        };
        let open_dir = dir.clone();
        let mut block = v_flex()
            .w_full()
            .gap(design::ui_px(cx, 4.0))
            .child(section(&t!("sounds.folder_title"), p, cx))
            .child(
                div()
                    .font_family(design::ui_font())
                    .text_size(design::t_body(cx))
                    .text_color(rgba_from(p.text, 1.0))
                    .child(
                        t!(
                            "sounds.folder_help",
                            first = FIRST_USER_ORDINAL,
                            last = FIRST_USER_ORDINAL - 1
                        )
                        .to_string(),
                    ),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 10.0))
                    .items_center()
                    .child(
                        div()
                            .font_family(design::mono())
                            .text_color(muted)
                            .child(dir.display().to_string()),
                    )
                    .child(
                        tool_btn("sounds-open", t!("sounds.open_folder").to_string())
                            .on_click(cx.listener(move |_, _, _, _| {
                                // Created on demand so the file manager has something to show;
                                // the scan itself never creates it.
                                let _ = std::fs::create_dir_all(&open_dir);
                                open_folder(&open_dir);
                            }))
                            .render(),
                    )
                    .child(
                        tool_btn("sounds-rescan", t!("sounds.rescan").to_string())
                            .on_click(cx.listener(|_, _, _, cx| {
                                let view = cx.entity().downgrade();
                                crate::media::sound::rescan(cx, move |app| {
                                    let _ = view.update(app, |_, cx| cx.notify());
                                });
                            }))
                            .render(),
                    ),
            );
        block = block.child(summary_line(scanned, &stats, muted, cx));
        if !numbered.is_empty() {
            // The numbers the files claimed, so the user can see what the core's picker will call
            // them without opening that picker.
            let mut list = v_flex()
                .gap(design::ui_px(cx, 2.0))
                .font_family(design::mono())
                .text_size(design::t_caption(cx))
                .text_color(muted)
                .child(
                    div()
                        .font_family(design::ui_font())
                        .child(t!("sounds.numbered_title").to_string()),
                );
            for (n, label) in &numbered {
                list = list.child(format!("{n} — {label}"));
            }
            block = block.child(list);
        }
        if !stats.rejected.is_empty() {
            let mut list = v_flex()
                .gap(design::ui_px(cx, 2.0))
                .font_family(design::mono())
                .text_size(design::t_caption(cx))
                .text_color(muted)
                .child(
                    div()
                        .font_family(design::ui_font())
                        .child(t!("sounds.rejected_title").to_string()),
                );
            for rejected in &stats.rejected {
                list = list.child(format!(
                    "{} — {}",
                    rejected.name,
                    reject_text(&rejected.reason)
                ));
            }
            block = block.child(list);
        }
        block
    }
}

/// One line on what the scan took.
fn summary_line(
    scanned: bool,
    stats: &ScanStats,
    color: Hsla,
    cx: &App,
) -> impl IntoElement + use<> {
    let text = if !scanned {
        t!("sounds.scan_pending").to_string()
    } else {
        t!(
            "sounds.scan_summary",
            folder = stats.folder_files,
            numbered = stats.numbered,
            rejected = stats.rejected.len()
        )
        .to_string()
    };
    div()
        .font_family(design::ui_font())
        .text_size(design::t_caption(cx))
        .text_color(color)
        .child(text)
}

/// The reason a file was refused, in the user's language.
fn reject_text(reason: &RejectReason) -> String {
    match reason {
        RejectReason::NotPcmWav => t!("sounds.reject_not_pcm").to_string(),
        RejectReason::TooLarge(mb) => t!("sounds.reject_too_large", mb = mb).to_string(),
        RejectReason::OverTotal(mb) => t!("sounds.reject_over_total", mb = mb).to_string(),
        RejectReason::Io(detail) => t!("sounds.reject_io", detail = detail).to_string(),
        RejectReason::NumberReserved => t!(
            "sounds.reject_number_reserved",
            first = crate::media::sound::FIRST_USER_ORDINAL
        )
        .to_string(),
        RejectReason::NumberTaken(by) => t!("sounds.reject_number_taken", by = by).to_string(),
        RejectReason::NameTaken => t!("sounds.reject_name_taken").to_string(),
    }
}
