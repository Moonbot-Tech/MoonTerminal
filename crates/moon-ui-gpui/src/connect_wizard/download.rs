//! "Download Moonbot for Windows" on the start screen: the user picks a folder, the wizard
//! downloads and unpacks Moonbot there, starts `MoonBot.exe` and closes the terminal — the rest of
//! the setup happens in Moonbot's own windows.

use std::path::PathBuf;

use gpui::*;
use moon_ui::{MoonButton, MoonPalette, MoonSpinner, h_flex, rgba_from, v_flex};
use rust_i18n::t;

use super::ConnectWizardView;
use super::render::note;
use crate::design;

/// Where the download stands.
#[derive(Default)]
pub(super) enum Download {
    #[default]
    Idle,
    /// Downloading and unpacking into this folder.
    Working(PathBuf),
    /// Stopped on this error, already in the user's language.
    Failed(String),
}

impl ConnectWizardView {
    /// The block under the three ways, set apart by "or".
    pub(super) fn download_block(
        &self,
        first_run: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let status: Option<AnyElement> = match &self.download {
            Download::Idle => None,
            Download::Working(dir) => Some(
                h_flex()
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .child(MoonSpinner::new())
                    .child(
                        t!(
                            "connect_wizard.download.working",
                            dir = dir.display().to_string()
                        )
                        .to_string(),
                    )
                    .into_any_element(),
            ),
            Download::Failed(error) => Some(
                div()
                    .text_color(rgba_from(p.red_text, 1.0))
                    .child(error.clone())
                    .into_any_element(),
            ),
        };
        let busy = matches!(self.download, Download::Working(_));
        v_flex()
            .gap(design::ui_px(cx, 6.0))
            .pt(design::ui_px(cx, 6.0))
            .child(note(t!("connect_wizard.download.or"), cx))
            .child(
                h_flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(design::ui_px(cx, 10.0))
                    .p(design::ui_px(cx, 10.0))
                    .border_1()
                    .border_color(rgba_from(p.border, 1.0))
                    .rounded(design::r_container(cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.0))
                            .gap(design::ui_px(cx, 2.0))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(t!("connect_wizard.download.title").to_string()),
                            )
                            .child(note(t!("connect_wizard.download.desc"), cx)),
                    )
                    .child(
                        MoonButton::new("cw-download")
                            .size(design::CONTROL_TIER)
                            .disabled(busy)
                            .label(if first_run {
                                t!("connect_wizard.download.button_first").to_string()
                            } else {
                                t!("connect_wizard.download.button").to_string()
                            })
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.download_moonbot(window, cx)
                                }),
                            )
                            .render(),
                    ),
            )
            .children(status)
    }

    /// Ask for the folder, then download, unpack, start Moonbot and close the terminal.
    fn download_moonbot(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.download, Download::Working(_)) {
            return;
        }
        let selection = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t!("connect_wizard.download.pick").to_string().into()),
        });
        cx.spawn_in(window, async move |view, cx| {
            let chosen = match selection.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Err(error)) => {
                    log::warn!("Moonbot folder dialog failed: {error}");
                    None
                }
                _ => None,
            };
            let Some(chosen) = chosen else {
                return;
            };
            let dir = match moon_core::moonbot_windows::target_dir(&chosen) {
                Ok(dir) => dir,
                Err(e) => {
                    let _ = view.update(cx, |this, cx| {
                        this.download = Download::Failed(format!("{e:#}"));
                        cx.notify();
                    });
                    return;
                }
            };
            let _ = view.update(cx, |this, cx| {
                this.download = Download::Working(dir.clone());
                cx.notify();
            });
            let at = dir.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let exe = moon_core::moonbot_windows::install(
                        moon_core::moonbot_windows::RELEASE_URL,
                        &at,
                    )?;
                    let folder = exe.parent().map(PathBuf::from).unwrap_or(at);
                    std::process::Command::new(&exe)
                        .current_dir(folder)
                        .spawn()
                        .map_err(|e| anyhow::anyhow!("start {}: {e}", exe.display()))?;
                    anyhow::Ok(exe)
                })
                .await;
            match result {
                Ok(exe) => {
                    log::info!(
                        "Moonbot started from {}; closing the terminal",
                        exe.display()
                    );
                    let _ = cx.update(|_, app| app.quit());
                }
                Err(e) => {
                    log::warn!("Moonbot download failed: {e:#}");
                    let _ = view.update(cx, |this, cx| {
                        this.download = Download::Failed(
                            t!("connect_wizard.download.failed", reason = format!("{e:#}"))
                                .to_string(),
                        );
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }
}
