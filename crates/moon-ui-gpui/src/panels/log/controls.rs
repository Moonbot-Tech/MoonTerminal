//! Source and file selector controls for the Log panel.

use super::*;
use rust_i18n::t;

use crate::design;

impl LogPanel {
    /// Build the pseudo-source-first, exchange-grouped log-source dropdown.
    ///
    /// A removed selected core remains visible as its numeric id until the user chooses another
    /// source, rather than being mislabeled as Local.
    ///
    /// Args:
    ///     sources: Available aggregate, local, and core log sources.
    ///     cx: Panel context used to read exchanges and wire source callbacks.
    ///
    /// Returns:
    ///     The configured source dropdown.
    pub(super) fn source_combo(
        &self,
        sources: &[LogSourceItem],
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let cur = sources
            .iter()
            .find(|s| s.source == self.source)
            .map(|s| s.display.clone())
            .unwrap_or_else(|| match self.source {
                LogSource::Core(core) => format!("#{core}"),
                LogSource::Aggregate | LogSource::Local => t!("log.source.local").to_string(),
            });
        let cores: Vec<(CoreId, String)> = sources
            .iter()
            .filter_map(|item| match item.source {
                LogSource::Core(core) => Some((core, item.display.clone())),
                LogSource::Aggregate | LogSource::Local => None,
            })
            .collect();
        let exchange_names = self
            .backend
            .read(cx)
            .session
            .market_source()
            .core_exchange_names();
        let unknown_exchange = t!("common.exchange_unknown").to_string();
        let sections = crate::controls::core_menu_sections(&cores, &exchange_names);
        let view = cx.entity();
        let mut items = Vec::with_capacity(sources.len() + sections.len() + 1);
        for (index, item) in sources
            .iter()
            .filter(|item| !matches!(item.source, LogSource::Core(_)))
            .enumerate()
        {
            let source = item.source.clone();
            let selected = source == self.source;
            let item_view = view.clone();
            items.push(
                MoonMenuItem::with_key(format!("ls-pseudo-{index}"), item.display.clone())
                    .selected(selected)
                    .on_click(move |_, _, app| {
                        let source = source.clone();
                        item_view.update(app, |this, cx| this.set_source(source, cx));
                    }),
            );
        }
        if !sections.is_empty() {
            items.push(MoonMenuItem::separator());
        }
        for (exchange, members) in &sections {
            items.push(MoonMenuItem::label(
                exchange.unwrap_or(unknown_exchange.as_str()),
            ));
            for (core, name) in members {
                let core = *core;
                let selected = self.source == LogSource::Core(core);
                let item_view = view.clone();
                items.push(
                    MoonMenuItem::with_key(format!("ls-core-{core}"), *name)
                        .selected(selected)
                        .on_click(move |_, _, app| {
                            item_view.update(app, |this, cx| {
                                this.set_source(LogSource::Core(core), cx);
                            });
                        }),
                );
            }
        }
        // Derive widths from content because `LogSource::Core` items include variable-length core
        // names. Exchange headers participate in the menu width, while the trigger and menu retain
        // separate 150- and 180-pixel floors.
        let (trigger_label, trigger_w, menu_w) = design::dropdown_content_widths(
            cx,
            &cur,
            sources.iter().map(|item| item.display.as_str()).chain(
                sections
                    .iter()
                    .map(|(exchange, _)| exchange.unwrap_or(unknown_exchange.as_str())),
            ),
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
            .menu_max_height(design::ui_value(cx, 360.0))
            .items(items)
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
