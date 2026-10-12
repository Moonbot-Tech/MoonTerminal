//! The "your own sounds" block on the Trade sounds tab: where the folder is, what the last scan
//! took from it and refused, and the two buttons — open the folder, read it again.

use std::rc::Rc;

use gpui::*;
use moon_ui::{MoonButton, MoonPalette, h_flex, rgba_from, v_flex};
use rust_i18n::t;

use super::common::{Action, hint_row};
use super::telegram::plural_key;
use super::{SettingsView, open_folder};
use crate::design;
use crate::media::sound::{FIRST_USER_ORDINAL, RejectReason, ScanStats};

impl SettingsView {
    /// The folder block, read straight off the installed catalog: the scan swaps the catalog in
    /// whole, so the stats shown here are always one consistent pass.
    pub(super) fn sound_folder_block(&self, cx: &mut Context<Self>) -> AnyElement {
        let dir = moon_core::config::paths::sounds_dir();
        let (scanned, stats, numbered) = crate::media::sound::with_catalog(|c| {
            let numbered: Vec<(i32, String)> = c
                .ordinals()
                .filter(|(n, _)| *n >= FIRST_USER_ORDINAL)
                .map(|(n, e)| (n, e.label.clone()))
                .collect();
            (c.scanned(), c.stats().clone(), numbered)
        });
        let view = cx.entity().downgrade();
        let toggle: Action = Rc::new(move |_, app| {
            let _ = view.update(app, |this, cx| {
                if !this.more_open.remove("trade-sounds-own-folder") {
                    this.more_open.insert("trade-sounds-own-folder");
                }
                cx.notify();
            });
        });
        let open_dir = dir.clone();
        folder_el(
            FolderView {
                dir: dir.display().to_string(),
                open: self.more_open.contains("trade-sounds-own-folder"),
                scanned,
                stats,
                numbered,
            },
            toggle,
            Box::new(cx.listener(move |_, _, _, _| {
                // Created on demand so the file manager has something to show;
                // the scan itself never creates it.
                let _ = std::fs::create_dir_all(&open_dir);
                open_folder(&open_dir);
            })),
            Box::new(cx.listener(|_, _, _, cx| {
                let view = cx.entity().downgrade();
                crate::media::sound::rescan(cx, move |app| {
                    let _ = view.update(app, |_, cx| cx.notify());
                });
            })),
            cx,
        )
    }
}

/// What the folder block shows: the folder, whether its help is open and the last scan.
pub(super) struct FolderView {
    /// The folder's path as shown.
    pub(super) dir: String,
    /// The long help under the folder row is unfolded.
    pub(super) open: bool,
    /// A scan has finished in this process.
    pub(super) scanned: bool,
    /// The last scan's counts and refusals.
    pub(super) stats: ScanStats,
    /// Ordinals the files claimed, with their labels.
    pub(super) numbered: Vec<(i32, String)>,
}

/// A folder-block button's handler.
pub(super) type FolderClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// The folder block: title, the folder row with its two buttons and long help, the scan summary,
/// the numbered sounds and the refused files.
///
/// Args:
///     view: What to show.
///     toggle: Folds or unfolds the long help.
///     on_open: Opens the folder.
///     on_rescan: Reads the folder again.
///     cx: Application context for theme and scale.
pub(super) fn folder_el(
    view: FolderView,
    toggle: Action,
    on_open: FolderClick,
    on_rescan: FolderClick,
    cx: &App,
) -> AnyElement {
    let FolderView {
        dir,
        open,
        scanned,
        stats,
        numbered,
    } = view;
    let p = MoonPalette::active(cx);
    let muted = rgba_from(p.text_soft, 1.0);
    let tool_btn = |id: &'static str, label: String| {
        // Spaces around the label work around the fork's `MoonButton` `pad_x=0` bug, which
        // otherwise places text against the outline.
        MoonButton::new(id)
            .variant(moon_ui::MoonButtonVariant::Neutral)
            .label(format!("  {label}  "))
    };
    let controls = h_flex()
        .debug_selector(|| "trade-sounds-own-controls".into())
        .flex_wrap()
        .gap(design::ui_px(cx, 10.0))
        .items_center()
        .child(
            tool_btn("sounds-open", t!("sounds.open_folder").to_string())
                .on_click(on_open)
                .render(),
        )
        .child(
            tool_btn("sounds-rescan", t!("sounds.rescan").to_string())
                .on_click(on_rescan)
                .render(),
        )
        .into_any_element();
    let mut block = v_flex()
        .id("trade-sounds-own")
        .debug_selector(|| "trade-sounds-own".into())
        .w_full()
        .gap(design::ui_px(cx, 4.0))
        // The mockup's box: the folder block is one bordered panel, not loose lines.
        .p(design::ui_px(cx, 12.0))
        .border_1()
        .border_color(rgba_from(p.border, 1.0))
        .rounded(design::r_container(cx))
        .bg(rgba_from(p.panel, 1.0))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(t!("trade_sounds.own.title").to_string()),
        )
        .child(hint_row(
            "trade-sounds-own-folder",
            t!("trade_sounds.own.folder").to_string(),
            None,
            Some(t!("trade_sounds.own.more").to_string()),
            div().into_any_element(),
            open,
            toggle,
            cx,
        ))
        // The path and the buttons under the caption, not beside it: a real path is longer than
        // any body, so it gets the full width and truncates, its whole text in a tooltip.
        .child({
            let full = dir.clone();
            div()
                .id("trade-sounds-own-path")
                .debug_selector(|| "trade-sounds-own-path".into())
                .w_full()
                .min_w_0()
                .truncate()
                .font_family(design::mono())
                .text_color(muted)
                .tooltip(move |_window, cx| {
                    cx.new(|_| moon_ui::MoonTooltipView::new(full.clone()))
                        .into()
                })
                .child(dir)
        })
        .child(controls);
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
            .p(design::ui_px(cx, 8.0))
            .rounded(design::r_container(cx))
            .bg(rgba_from(p.panel_high, 1.0))
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
    block.into_any_element()
}

/// Found and rejected counts for the last scan, or the pending line before the first scan.
///
/// Args:
///     scanned: Whether a scan has finished in this process.
///     stats: The catalog's last pass. `folder_files` is the found count.
///     color: Caption color.
///     cx: Application context for scale.
///
/// Returns:
///     The caption row. The rejected count is omitted when nothing was refused.
fn summary_line(
    scanned: bool,
    stats: &ScanStats,
    color: Hsla,
    cx: &App,
) -> impl IntoElement + use<> {
    let found = if !scanned {
        t!("sounds.scan_pending").to_string()
    } else {
        let n = stats.folder_files as u64;
        t!(&plural_key("trade_sounds.own.found", n), n = n).to_string()
    };
    let mut line = h_flex()
        .flex_wrap()
        .gap(design::ui_px(cx, 10.0))
        .font_family(design::ui_font())
        .text_size(design::t_caption(cx))
        .text_color(color)
        .child(found);
    if scanned && !stats.rejected.is_empty() {
        let n = stats.rejected.len() as u64;
        line = line.child(t!(&plural_key("trade_sounds.own.rejected", n), n = n).to_string());
    }
    line
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
