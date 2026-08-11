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
        let (effective_source, _, workspace_owned) =
            self.effective_selection(self.backend.read(cx));
        let venues = self.backend.read(cx).session.core_venues();
        let cur = sources
            .iter()
            .find(|s| s.source == effective_source)
            .map(|s| s.display.clone())
            .unwrap_or_else(|| match &effective_source {
                LogSource::Core(core) => format!("#{core}"),
                // A live member captions the full venue — its DEX suffix included, and its own
                // spelling when the ordinal is one this build cannot name. The member is chosen by
                // lowest core id rather than by HashMap order, so two spellings of one unknown
                // ordinal cannot alternate between renders. With every member disconnected the
                // selection still stands, and the identity alone still names it.
                LogSource::Exchange(exchange) => venues
                    .iter()
                    .filter(|(_, venue)| venue.id == *exchange)
                    .min_by_key(|(core, _)| **core)
                    .map(|(_, venue)| crate::controls::venue_label(venue))
                    .unwrap_or_else(|| crate::controls::venue_id_label(*exchange)),
                LogSource::Aggregate | LogSource::Local => t!("log.source.local").to_string(),
            });
        let cores: Vec<(CoreId, String)> = sources
            .iter()
            .filter_map(|item| match &item.source {
                LogSource::Core(core) => Some((*core, item.display.clone())),
                LogSource::Aggregate | LogSource::Exchange(_) | LogSource::Local => None,
            })
            .collect();
        let sections = crate::controls::core_menu_sections(&cores, &venues);
        let view = cx.entity();
        let mut items = Vec::with_capacity(sources.len() + sections.len() + 1);
        for (index, item) in sources
            .iter()
            .filter(|item| !matches!(item.source, LogSource::Core(_)))
            .enumerate()
        {
            let source = item.source.clone();
            let selected = source == effective_source;
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
        for (section_index, (venue, members)) in sections.into_iter().enumerate() {
            let exchange_label = crate::controls::venue_section_label(venue);
            if let Some(venue) = venue {
                let exchange = venue.id;
                let selected = matches!(&effective_source, LogSource::Exchange(current) if *current == exchange);
                let item_view = view.clone();
                items.push(
                    MoonMenuItem::action_label(
                        format!("ls-exchange-{section_index}"),
                        exchange_label,
                    )
                    .selected(selected)
                    .on_click(move |_, _, app| {
                        item_view.update(app, |this, cx| {
                            this.set_source(LogSource::Exchange(exchange), cx);
                        });
                    }),
                );
            } else {
                items.push(MoonMenuItem::label(exchange_label));
            }
            for (core, name) in members {
                let selected = effective_source == LogSource::Core(core);
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
            .disabled(workspace_owned)
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            // Starts at the width the shared core selector uses everywhere else, and grows only
            // for a label that does not fit: unlike those, this trigger shows one source NAME
            // ("BinF3", an exchange, "Локальный"), not a "Ядер: 3" summary.
            .fit_trigger_width(crate::controls::CORE_COMBO_TRIGGER_W, 260.0)
            .fit_menu_width(180.0, 560.0)
            .menu_size(MoonMenuSize::Compact)
            .menu_max_height_ui(360.0)
            .items(items)
    }

    /// Builds the Live-and-history file dropdown used for non-aggregate sources.
    ///
    /// Args:
    ///     files: Available named log files for the effective source.
    ///     cx: Panel context used to resolve workspace ownership and wire callbacks.
    ///
    /// Returns:
    ///     File dropdown pinned to Live while Auto owns the panel.
    pub(super) fn file_combo(&self, files: &[String], cx: &Context<Self>) -> impl IntoElement {
        let (_, effective_file, workspace_owned) = self.effective_selection(self.backend.read(cx));
        let live = t!("log.live").to_string();
        let cur = match &effective_file {
            LogFile::Live => live.clone(),
            LogFile::Named(n) => n.clone(),
        };
        let view = cx.entity();
        let mut items = vec![
            MoonMenuItem::with_key("lf-live", live.clone())
                .selected(matches!(effective_file, LogFile::Live))
                .on_click({
                    let view = view.clone();
                    move |_, _, app| {
                        view.update(app, |t, c| t.set_file(LogFile::Live, c));
                    }
                }),
        ];
        for f in files {
            let selected = matches!(&effective_file, LogFile::Named(name) if name == f);
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
            .disabled(workspace_owned)
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .fit_trigger_width(180.0, 260.0)
            .fit_menu_width(220.0, 560.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    }
}
