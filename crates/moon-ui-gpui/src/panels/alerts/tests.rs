//! Signature regressions for translated alert rows.

use super::{alerts_should_rebuild, locale_data_sig};
use crate::panels::common::GateStep;

/// Measure cold projection and sorting of 300 stored figures across 20 cores.
#[test]
#[ignore = "bench: S-smooth"]
fn bench_s_alerts_rebuild() {
    use super::model::{AlertsViewState, FigRow, sort_rows};
    use crate::test_support::{
        bench,
        fixtures::{EPOCH_MS, PanelFixture},
    };
    use moon_core::figures::{DrawStyle, Figure, FigureKind, FigureStore, tools::HLine};
    use moon_core::market::{MarketDataSource, MarketStore};
    use std::collections::HashMap;

    crate::test_locale::warm();
    let fixture = PanelFixture::new(20, 100, 0, 0, 300);
    let source = MarketDataSource::new(MarketStore::shared(0.0));
    let mut figures = FigureStore::default();
    for session in &fixture.sessions {
        for (index, detect) in fixture
            .store
            .core(session.id)
            .unwrap()
            .detects
            .iter()
            .enumerate()
        {
            figures.add(
                session.id,
                &detect.market,
                Figure::new(
                    FigureKind::HLine(HLine {
                        price: index as f64 + 10.0,
                    }),
                    DrawStyle::default(),
                    EPOCH_MS as f64 + index as f64,
                ),
            );
        }
    }
    let view = AlertsViewState::default();
    bench::measure("alerts_rebuild_cold_300_figure_rows", 24, || {
        let names: HashMap<_, _> = fixture.sessions.iter().map(|s| (s.id, &s.name)).collect();
        let mut by_core: HashMap<_, Vec<_>> = HashMap::new();
        for (core, market, fig) in figures.all().filter(|(core, _, fig)| {
            names.contains_key(core) && view.scope_accepts(fig.tool().def().alertable, fig.alert)
        }) {
            by_core
                .entry(core)
                .or_default()
                .push((market.to_owned(), fig.clone()));
        }
        let mut rows = Vec::new();
        for (core, figures) in by_core {
            let markets: Vec<_> = figures.iter().map(|(market, _)| market.as_str()).collect();
            let labels = source.market_labels(core, &markets);
            rows.extend(
                figures
                    .into_iter()
                    .zip(labels)
                    .map(|((market, fig), label)| {
                        FigRow::new(
                            core,
                            names[&core].clone(),
                            market,
                            label.display_coin().to_string(),
                            &fig,
                            super::NO_STRATEGY.to_owned(),
                            true,
                        )
                    }),
            );
        }
        rows.retain(|row| row.matches(""));
        sort_rows(&mut rows, view.sort, view.sort_asc);
        assert_eq!(rows.len(), 300);
        rows
    });
}

/// Omitting catalogue revisions freezes labels on coins resolved before a market-list update.
#[test]
fn catalogue_revision_moves_data_sig() {
    use super::catalogue_data_sig;

    let initial = catalogue_data_sig(17, Some((1, 2, 3)));
    assert_ne!(initial, catalogue_data_sig(17, Some((1, 2, 4))));
    assert_ne!(initial, catalogue_data_sig(17, Some((1, 3, 3))));
    assert_ne!(initial, catalogue_data_sig(17, Some((2, 2, 3))));
    assert_ne!(initial, catalogue_data_sig(17, None));
    assert_eq!(initial, catalogue_data_sig(17, Some((1, 2, 3))));
}

/// Rebuilding resolved rows each second wastes work; failing to retry fallback labels freezes them.
#[test]
fn repaint_rebuilds_only_with_unresolved_coin() {
    assert!(!alerts_should_rebuild(false, GateStep::Repaint, false));
    assert!(alerts_should_rebuild(false, GateStep::Repaint, true));
    assert!(!alerts_should_rebuild(false, GateStep::Idle, true));
    assert!(alerts_should_rebuild(true, GateStep::Idle, false));
    assert!(alerts_should_rebuild(false, GateStep::Rebuild, false));
}

/// Omitting the locale strands tool labels in the previous language on an idle account.
#[test]
fn locale_change_moves_data_sig() {
    let en = locale_data_sig(17, "en");
    assert_ne!(en, locale_data_sig(17, "ru"));
    assert_ne!(en, locale_data_sig(18, "en"));
    assert_eq!(en, locale_data_sig(17, "en"));
}
