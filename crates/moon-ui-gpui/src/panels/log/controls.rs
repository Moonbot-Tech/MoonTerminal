//! Source and file selector controls for the Log panel.

use super::*;
use rust_i18n::t;

impl LogPanel {
    /// Build the pseudo-source-first, exchange-grouped log-source dropdown.
    ///
    /// A removed selected core remains visible as its numeric id until the user chooses another
    /// source, rather than being mislabeled as Local. Known exchange headers select a live
    /// aggregate for that exchange; the unknown-exchange header remains passive.
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
            .unwrap_or_else(|| match &self.source {
                LogSource::Core(core) => format!("#{core}"),
                LogSource::Exchange(exchange) => exchange.clone(),
                LogSource::Aggregate | LogSource::Local => t!("log.source.local").to_string(),
            });
        let cores: Vec<(CoreId, String)> = sources
            .iter()
            .filter_map(|item| match &item.source {
                LogSource::Core(core) => Some((*core, item.display.clone())),
                LogSource::Aggregate | LogSource::Exchange(_) | LogSource::Local => None,
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
        for (section_index, (exchange, members)) in sections.into_iter().enumerate() {
            let exchange_label = exchange.unwrap_or(unknown_exchange.as_str());
            if exchange.is_some() {
                let selected = matches!(&self.source, LogSource::Exchange(current) if current == exchange_label);
                let source = exchange_label.to_string();
                let item_view = view.clone();
                items.push(
                    MoonMenuItem::action_label(
                        format!("ls-exchange-{section_index}"),
                        exchange_label,
                    )
                    .selected(selected)
                    .on_click(move |_, _, app| {
                        let source = source.clone();
                        item_view.update(app, |this, cx| {
                            this.set_source(LogSource::Exchange(source), cx);
                        });
                    }),
                );
            } else {
                items.push(MoonMenuItem::label(exchange_label));
            }
            for (core, name) in members {
                let selected = self.source == LogSource::Core(core);
                let item_view = view.clone();
                items.push(
                    MoonMenuItem::with_key(format!("ls-core-{core}"), name)
                        .selected(selected)
                        .on_click(move |_, _, app| {
                            item_view.update(app, |this, cx| {
                                this.set_source(LogSource::Core(core), cx);
                            });
                        }),
                );
            }
        }
        MoonDropdown::new("log-source")
            .label(cur)
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .fit_trigger_width(150.0, 260.0)
            .fit_menu_width(180.0, 560.0)
            .menu_size(MoonMenuSize::Compact)
            .menu_max_height_ui(360.0)
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
        MoonDropdown::new("log-file")
            .label(cur)
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .fit_trigger_width(180.0, 260.0)
            .fit_menu_width(220.0, 560.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    }
}
