//! Поля-списки источника и файла панели «Лог».

use super::*;
use rust_i18n::t;

impl LogPanel {
    /// Комбобокс источника.
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
        MoonDropdown::new("log-source")
            .label(format!("{cur} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(150.0)
            .menu_width(180.0)
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

    /// Комбобокс файла (Live + прошлые файлы) — только для одиночного источника.
    pub(super) fn file_combo(&self, files: &[String], cx: &Context<Self>) -> impl IntoElement {
        let cur = match &self.file {
            LogFile::Live => t!("log.live").to_string(),
            LogFile::Named(n) => n.clone(),
        };
        let view = cx.entity();
        let mut items = vec![
            MoonMenuItem::with_key("lf-live", t!("log.live").to_string())
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
        MoonDropdown::new("log-file")
            .label(format!("{cur} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(180.0)
            .menu_width(220.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    }
}
