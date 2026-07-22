//! Source and file selector controls for the Log panel.

use super::*;
use rust_i18n::t;

use crate::design;

impl LogPanel {
    /// Builds the log-source dropdown.
    pub(super) fn source_combo(
        &self,
        sources: &[LogSourceItem],
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let cur = sources
            .iter()
            .find(|s| s.source == self.source)
            .map(|s| s.display.clone())
            .unwrap_or_else(|| t!("log.source.local").to_string());
        let view = cx.entity();
        let items: Vec<(LogSource, String)> = sources
            .iter()
            .map(|s| (s.source.clone(), s.display.clone()))
            .collect();
        // Derive widths from content because `LogSource::Core` items include variable-length core
        // names. The trigger and menu use separate 150- and 180-pixel floors; the shared helper
        // applies scaled ceilings and ellipsizes the trigger label when it reaches its cap.
        let (trigger_label, trigger_w, menu_w) = design::dropdown_content_widths(
            cx,
            &cur,
            sources.iter().map(|s| s.display.as_str()),
            150.0,
            180.0,
        );
        MoonDropdown::new("log-source")
            .label(trigger_label)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(trigger_w)
            .menu_width(menu_w)
            .menu_size(MoonMenuSize::Compact)
            .items(items.into_iter().enumerate().map(move |(i, (src, disp))| {
                let selected = src == self.source;
                let view = view.clone();
                MoonMenuItem::with_key(format!("ls-{i}"), disp)
                    .selected(selected)
                    .on_click(move |_, _, app| {
                        let src = src.clone();
                        view.update(app, |t, c| t.set_source(src, c));
                    })
            }))
    }

    /// Builds the Live-and-history file dropdown used for non-aggregate sources.
    pub(super) fn file_combo(&self, files: &[String], cx: &Context<Self>) -> impl IntoElement {
        let live = t!("log.live").to_string();
        let cur = match &self.file {
            LogFile::Live => live.clone(),
            LogFile::Named(n) => n.clone(),
        };
        let view = cx.entity();
        let mut items = vec![
            MoonMenuItem::with_key("lf-live", live.clone())
                .selected(matches!(self.file, LogFile::Live))
                .on_click({
                    let view = view.clone();
                    move |_, _, app| {
                        view.update(app, |t, c| t.set_file(LogFile::Live, c));
                    }
                }),
        ];
        for f in files {
            let selected = matches!(&self.file, LogFile::Named(name) if name == f);
            let view = view.clone();
            let file = f.clone();
            items.push(
                MoonMenuItem::with_key(SharedString::from(format!("lf-{f}")), f.clone())
                    .selected(selected)
                    .on_click(move |_, _, app| {
                        let file = file.clone();
                        view.update(app, |t, c| t.set_file(LogFile::Named(file), c));
                    }),
            );
        }
        // Derive widths from potentially long log-file names. The trigger and menu use separate
        // 180- and 220-pixel floors; the shared helper applies scaled ceilings and ellipsizes the
        // trigger label when it reaches its cap.
        let (trigger_label, trigger_w, menu_w) = design::dropdown_content_widths(
            cx,
            &cur,
            std::iter::once(live.as_str()).chain(files.iter().map(String::as_str)),
            180.0,
            220.0,
        );
        MoonDropdown::new("log-file")
            .label(trigger_label)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(trigger_w)
            .menu_width(menu_w)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    }
}
