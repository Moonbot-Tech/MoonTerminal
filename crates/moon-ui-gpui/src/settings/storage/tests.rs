//! Storage page: the usage shares add up, and the three cards fit the settings body in every
//! state, language and theme.

use std::default::Default;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, InteractiveElement, IntoElement, ParentElement, Render, ScrollHandle,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use moon_core::config::UiThemeMode;
use moon_ui::MoonTheme;
use std::collections::HashSet;

use super::super::layout_probe::{ProbeSpec, Probed, SIZES, TextFace, check, leak};
use super::core_data::{
    CoreCounts, CoreLine, CoreView, Group, Kinds, core_section_el, lines as core_lines,
};
use super::trades_cleanup::TapeCount;
use super::{StorageCtx, StorageInfo, StorageView, StratCounts, storage_body_el, usage_parts};

/// A late sizes stage must not replace a newer snapshot or clear its in-flight flag.
#[test]
fn older_generation_storage_results_are_ignored() {
    let mut ed = super::build();
    let old = ed.generation;
    ed.reset_info();
    ed.inflight = true;
    assert!(!ed.receive_info(old, StorageInfo::default()));
    assert!(ed.info.is_none());
    assert!(ed.inflight);
    assert!(ed.receive_info(ed.generation, StorageInfo::default()));
    assert!(!ed.inflight);
}

/// Saving an unrelated choice from an old page must retain the reconciled shared window.
#[test]
fn cached_storage_save_retains_reconciled_window() {
    let _window = crate::test_support::SharedWindowGuard::take();
    let mut ed = super::StorageEd {
        cfg: moon_core::config::storage::StorageCfg::default(),
        info: None,
        inflight: false,
        generation: 0,
        tape_inflight: false,
        busy: false,
        core_data: super::core_data::CoreDataEd::default(),
    };
    ed.cfg.trade_replay.margin_s = 180;
    ed.cfg.trade_replay.long_position_min = 5;
    moon_core::market::trade_replay::set_margin_s(600);
    moon_core::market::trade_replay::set_long_position_min(10);
    ed.sync_shared_window();
    let saved = serde_json::to_value(&ed.cfg).expect("serialize cached config");
    assert_eq!(saved["trade_replay"]["margin_s"], 600);
    assert_eq!(saved["trade_replay"]["long_position_min"], 10);
    assert_eq!(moon_core::market::trade_replay::margin_ms(), 600_000);
}

/// Observe the same input Change guard used by the Storage subscription.
struct InputProbe {
    ui: super::StorageUi,
    edits: usize,
}

/// An adopted threshold must update the seeded input without writing its Change back.
#[gpui::test]
fn adopted_long_threshold_updates_input_without_an_edit(cx: &mut gpui::TestAppContext) {
    let _window = crate::test_support::SharedWindowGuard::take();
    let _locale = crate::test_locale::force("en");
    cx.update(moon_ui::init);
    let (mut visual, _, _) = draw_page(cx, (860.0, 900.0), 5, Variant::Normal);
    let probe = visual.update(|window, cx| {
        let mut cfg = moon_core::config::storage::StorageCfg::default();
        cfg.trade_replay.long_position_min = 5;
        gpui::AppContext::new(cx, |cx| {
            let ui = super::StorageUi::init(&cfg, window, cx);
            cx.subscribe(
                &ui.long_min,
                |this: &mut InputProbe, state, event: &moon_ui::MoonInputEvent, cx| {
                    if matches!(event, moon_ui::MoonInputEvent::Change)
                        && let Some(value) = moon_ui::MoonNumberInput::read_value(&state, cx)
                        && this.ui.long_edit(value.round().max(0.0) as u32)
                    {
                        this.edits += 1;
                    }
                },
            )
            .detach();
            InputProbe { ui, edits: 0 }
        })
    });
    moon_core::market::trade_replay::set_long_position_min(10);
    visual.update(|window, cx| {
        probe.update(cx, |this, cx| {
            let value = moon_core::market::trade_replay::window().long_position_min;
            this.ui.sync_long_input(value, window, cx);
            // The pinned input suppresses programmatic events; inject one to prove the guard.
            this.ui
                .long_min
                .update(cx, |_, cx| cx.emit(moon_ui::MoonInputEvent::Change));
        })
    });
    visual.run_until_parked();
    visual.update(|_, cx| {
        assert_eq!(
            probe.read(cx).ui.long_min.read(cx).value().to_string(),
            "10"
        );
        assert_eq!(probe.read(cx).edits, 0);
    });
    visual.update(|window, cx| {
        probe.update(cx, |this, cx| {
            this.ui.long_min.update(cx, |state, cx| {
                state.set_value("11", window, cx);
                cx.emit(moon_ui::MoonInputEvent::Change);
            });
        })
    });
    visual.run_until_parked();
    visual.update(|_, cx| assert_eq!(probe.read(cx).edits, 1));
}

/// The fill estimate waits for a second sample with deals done, then divides what is left by the
/// rate seen; a count that grows starts the sampling over.
///
/// Breaks on: an estimate from one sample, a rate taken from the wrong end, or a grown count
/// keeping the stale first sample.
#[gpui::test]
fn the_fill_estimate_needs_two_samples(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        let cfg = moon_core::config::storage::StorageCfg::default();
        let mut ui = super::StorageUi::init(&cfg, window, cx);
        let t0 = std::time::Instant::now();
        assert_eq!(ui.fill_eta(300, t0), None);
        assert_eq!(
            ui.fill_eta(300, t0 + std::time::Duration::from_secs(5)),
            None
        );
        // 100 deals in 10 s: 200 left take 20 s.
        assert_eq!(
            ui.fill_eta(200, t0 + std::time::Duration::from_secs(10)),
            Some(20)
        );
        assert_eq!(
            ui.fill_eta(400, t0 + std::time::Duration::from_secs(11)),
            None
        );
        assert_eq!(
            ui.fill_eta(300, t0 + std::time::Duration::from_secs(21)),
            Some(30)
        );
    });
}

/// A probe host drawing whatever `build` returns inside the body box.
struct Host {
    width: f32,
    height: f32,
    scroll: ScrollHandle,
    build: Rc<dyn Fn(&mut Window, &mut App, f32) -> AnyElement>,
}

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = (self.build)(window, cx, self.width);
        div()
            .w(px(self.width))
            .h(px(self.height))
            .debug_selector(|| "probe-body".into())
            .child(
                div()
                    .id("probe-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(div().w_full().flex_none().child(content)),
            )
    }
}

impl Probed for Host {
    fn scroll(&self) -> Option<ScrollHandle> {
        Some(self.scroll.clone())
    }
}

/// The four database shares add up to the bytes on disk, including each WAL, and a missing file
/// contributes zero.
///
/// The expected total is added here from the snapshot literals. It does not call the share helper
/// a second time.
///
/// Breaks on: `usage_parts` dropping a WAL, counting a missing klines file as anything but zero,
/// or listing the databases in an order other than the mockup's reports, strategies, klines,
/// trades.
#[test]
fn usage_shares_sum_to_the_total() {
    let _locale = crate::test_locale::force("en");
    let info = StorageInfo {
        reports: Some((400, 12)),
        strategies: Some((38, 0)),
        klines: None,
        trades: Some((200, 31)),
        ..StorageInfo::default()
    };

    let parts = usage_parts(&info);

    let expected = [400 + 12, 38, 0, 200 + 31];
    assert_eq!(
        parts.iter().map(|part| part.bytes).collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        parts.iter().map(|part| part.bytes).sum::<u64>(),
        400 + 12 + 38 + 200 + 31
    );
}

/// The mockup's five states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Variant {
    Normal,
    /// No snapshot yet: rows dimmed under the counting line.
    Loading,
    /// The tape count failed: one red line.
    Error,
    /// The autoload is filling tape.
    Filling,
    /// No station set up.
    NoStation,
}

const VARIANTS: [Variant; 5] = [
    Variant::Normal,
    Variant::Loading,
    Variant::Error,
    Variant::Filling,
    Variant::NoStation,
];

/// The production Storage page from synthetic data: three cores in the core table (CORE_A
/// connected, CORE_B syncing, CORE_C archived), the core help open, in one of the [`Variant`]s.
fn page(
    long_min: &gpui::Entity<moon_ui::MoonInputState>,
    variant: Variant,
    width: f32,
    cx: &App,
) -> AnyElement {
    let open: HashSet<&'static str> = ["storage-core-hint"].into();
    let ctx = StorageCtx {
        weak: gpui::WeakEntity::new_invalid(),
        open: &open,
    };
    let core_line = |uid: u64, name: &str, group: Group, syncing: Option<&str>| CoreLine {
        counts: CoreCounts {
            uid,
            stored_name: name.into(),
            reports: 118_204,
            strategies: 412,
            versions: 960,
            traces: 9_812,
            warnings: 31,
        },
        name: name.into(),
        group,
        syncing: syncing.map(String::from),
    };
    let cores = [
        core_line(11, "CORE_A", Group::Connected, None),
        core_line(12, "CORE_B", Group::Offline, Some("42%")),
        core_line(13, "CORE_C", Group::Archive, None),
    ]
    .into_iter()
    .map(|line| (line.counts.uid, line))
    .collect();
    let lines = core_lines(
        &[],
        cores,
        &rust_i18n::t!("storage.core_data_group_archive"),
    );
    let core = core_section_el(
        &CoreView {
            lines: Some(Ok(Rc::new(lines))),
            selected: [11, 13].into(),
            kinds: Kinds {
                reports: true,
                strategies: true,
                traces: true,
                warnings: true,
            },
            busy: false,
            icons: Default::default(),
        },
        &ctx,
        cx,
    );
    let mut info = StorageInfo {
        reports: Some((1_300 << 20, 12 << 20)),
        strategies: Some((11 << 20, 0)),
        klines: Some((1_700 << 20, 31 << 20)),
        trades: Some((11 << 30, 64 << 20)),
        report_rows: Some(Ok(412_388)),
        strat: Some(Ok(StratCounts {
            live: 1_669,
            deleted: 163,
            versions: 2_720,
        })),
        tape: Some(Ok(TapeCount {
            with_tape: 48_120,
            long: 3_904,
            incomplete_markets: 0,
        })),
        last_backup_ms: Some(1_790_000_000_000),
    };
    if variant == Variant::Error {
        info.tape = Some(Err("the file is held by another process (synthetic)".into()));
    }
    storage_body_el(
        StorageView {
            loading: variant == Variant::Loading,
            info: if variant == Variant::Loading {
                StorageInfo::default()
            } else {
                info
            },
            cfg: moon_core::config::storage::StorageCfg::default(),
            long_min: long_min.clone(),
            station: variant != Variant::NoStation,
            fill: (variant == Variant::Filling).then_some((214, Some(180))),
            core,
        },
        &ctx,
        width,
        cx,
    )
}

/// Open the production Storage page ([`page`]) in a window of `size`, its number field seeded
/// with `seed`, and draw it until the layout settles.
///
/// Returns:
///     The drawn window, the field state and the body's scroll handle.
fn draw_page(
    cx: &mut gpui::TestAppContext,
    size: (f32, f32),
    seed: u32,
    variant: Variant,
) -> (
    gpui::VisualTestContext,
    gpui::Entity<moon_ui::MoonInputState>,
    ScrollHandle,
) {
    let mut state = None;
    let scroll = ScrollHandle::new();
    let handle = scroll.clone();
    let window = cx.open_window(gpui::size(px(size.0), px(size.1)), |window, cx| {
        let (width, height) = super::super::layout_probe::body_box(size, cx);
        let made = super::super::common::number_state(seed, window, cx);
        state = Some(made.clone());
        // A `Root` above the page, as the Settings window has: the number input's `+` reads it.
        let host = gpui::AppContext::new(cx, |_| Host {
            width,
            height,
            scroll: handle,
            build: Rc::new(move |_, cx, width| page(&made, variant, width, cx)),
        });
        moon_ui::Root::new(gpui::AnyView::from(host), window, cx)
    });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    for _ in 0..3 {
        visual.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
    (visual, state.expect("window built"), scroll)
}

/// Install the light or the dark theme, as the app presents it.
fn theme(cx: &mut gpui::TestAppContext, mode: UiThemeMode) {
    cx.update(|cx| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(mode, 1.0),
            cx,
        )
    });
}

const DBS: [&str; 4] = ["reports", "strategies", "klines", "trades"];

/// Every caption line `variant` draws, by selector, with its text: the hints, the auto lines and
/// the status line.
fn captions(variant: Variant) -> Vec<(String, String)> {
    let t = |k: &str| rust_i18n::t!(k).to_string();
    let mut out = vec![
        (
            "storage-trades-margin-hint".to_string(),
            t("storage.trades_margin_hint"),
        ),
        (
            "storage-trades-long-hint".to_string(),
            t("storage.trades_long_position_hint"),
        ),
        (
            "storage-tape-auto-text".to_string(),
            t(if variant == Variant::NoStation {
                "storage.tape.auto_local"
            } else {
                "storage.tape.auto"
            }),
        ),
        (
            "storage-auto-text".to_string(),
            // A loading page has no snapshot, so no backup stamp either.
            super::maint_text((variant != Variant::Loading).then_some(1_790_000_000_000)),
        ),
    ];
    match variant {
        Variant::Loading => out.push(("storage-loading".into(), t("storage.counting"))),
        Variant::Error => out.push((
            "storage-error".into(),
            rust_i18n::t!(
                "storage.count_failed",
                db = t("storage.db.trades.title"),
                err = "the file is held by another process (synthetic)"
            )
            .to_string(),
        )),
        Variant::Filling => out.push((
            "storage-tape-fill".into(),
            rust_i18n::t!(
                "storage.tape.fill_eta",
                n = 214,
                eta = crate::display_text::fmt_duration_short(180.0)
            )
            .to_string(),
        )),
        Variant::Normal | Variant::NoStation => {}
    }
    out
}

/// Shape the actual caption face independently of the production layout-width estimate.
fn shaped_caption_width(visual: &mut gpui::VisualTestContext, text: &str, mono: bool) -> f32 {
    visual.update(|window, cx| {
        let runs = [gpui::TextRun {
            len: text.len(),
            font: gpui::font(if mono {
                crate::design::mono()
            } else {
                crate::design::ui_font()
            }),
            ..gpui::TextRun::default()
        }];
        f32::from(
            window
                .text_system()
                .shape_line(
                    text.to_owned().into(),
                    crate::design::t_caption(cx),
                    &runs,
                    None,
                )
                .width,
        )
    })
}

/// A caption-size text in `sel` is tall enough for every line it needs at the box's width: the
/// box is measured in the face the page draws it in, here the UI face or the mono one.
fn assert_wraps(visual: &mut gpui::VisualTestContext, sel: &str, text: &str, mono: bool, at: &str) {
    let b = visual
        .debug_bounds(leak(sel.to_string()))
        .unwrap_or_else(|| panic!("{sel} was not laid out ({at})"));
    let width = shaped_caption_width(visual, text, mono);
    let line = visual.update(|_, cx| f32::from(crate::design::line_px(cx, 12.0)));
    let lines = ((width - 0.5) / f32::from(b.size.width).max(1.0))
        .ceil()
        .max(1.0);
    assert!(
        f32::from(b.size.height) + 0.5 >= lines * line,
        "{sel} cuts {text:?} off ({} x {} for {lines} lines, {at})",
        f32::from(b.size.width),
        f32::from(b.size.height)
    );
}

/// What the generic probe checks on every drawn page: the three cards and every row and line in
/// the body, the cards and the database rows apart with the design gaps between them, each row's
/// parts inside it, and the core table last.
fn page_spec(variant: Variant) -> ProbeSpec {
    let mut inside: Vec<String> = ["storage-total", "storage-tape", "storage-core-card"]
        .map(String::from)
        .to_vec();
    let mut within = Vec::new();
    for db in DBS {
        let row = format!("storage-db-{db}");
        inside.push(row.clone());
        for part in ["-name", "-facts", "-size"] {
            within.push((format!("{row}{part}"), row.clone()));
            inside.push(format!("{row}{part}"));
        }
    }
    for (sel, _) in captions(variant) {
        inside.push(sel.clone());
        within.push((sel, "probe-body".into()));
    }
    for card in ["storage-total", "storage-tape"] {
        within.push((format!("{card}-title"), card.into()));
    }
    let rows: Vec<String> = DBS.iter().map(|db| format!("storage-db-{db}")).collect();
    let stacked = rows
        .windows(2)
        .map(|pair| (pair[0].clone(), pair[1].clone(), 0.0))
        .chain([
            ("storage-total".into(), "storage-tape".into(), 12.0),
            ("storage-tape".into(), "storage-core-card".into(), 12.0),
        ])
        .collect();
    ProbeSpec {
        container: "probe-body",
        inside,
        within,
        no_overlap: vec![
            ["storage-total", "storage-tape", "storage-core-card"]
                .map(String::from)
                .to_vec(),
            rows,
        ],
        stacked,
        last: Some("storage-core".into()),
        ..ProbeSpec::default()
    }
}

/// The production Storage page — the disk card with its four database rows and status lines,
/// the tape card, and the real core table in its card — keeps every card, row and line inside
/// the body, the cards and the rows apart with equal gaps (12 between cards, none between rows),
/// each row's name, facts and size inside the row, every caption uncut and the core table last;
/// in both themes, every language, every swept size and every state.
///
/// Breaks on: a card or row escaping the body, a hole where a removed row was, a facts line
/// pushed out of its row, a hint or status caption cut off, a gap that differs between cards.
#[gpui::test]
fn storage_page_fits_the_body(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    for mode in [UiThemeMode::Light, UiThemeMode::Dark] {
        theme(cx, mode);
        for language in moon_core::config::Language::ALL {
            let _locale = crate::test_locale::force(language.code());
            for size in SIZES {
                for variant in VARIANTS {
                    let at = format!(
                        "{mode:?} {} at {}x{} {variant:?}",
                        language.code(),
                        size.0,
                        size.1
                    );
                    let (mut visual, _, scroll) = draw_page(cx, size, 30, variant);
                    if let Err(broken) = check(&mut visual, Some(&scroll), &page_spec(variant)) {
                        panic!("{broken} ({at})");
                    }
                    for (sel, text) in captions(variant) {
                        assert_wraps(&mut visual, &sel, &text, false, &at);
                    }
                }
            }
        }
    }
}

/// At the narrowest swept size the database facts sit beside the name only where they fit whole,
/// and otherwise wrap under it, uncut, the size kept on the name's line; at the widest they fit
/// beside the name. Both themes, every language.
///
/// Breaks on: the facts truncated to an ellipsis, the size pushed under the facts, or a wide row
/// too narrow for its figures.
#[gpui::test]
fn storage_db_facts_wrap_when_narrow_and_fit_when_wide(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    let narrow = SIZES
        .iter()
        .copied()
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .expect("sizes");
    let wide = SIZES
        .iter()
        .copied()
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .expect("sizes");
    for mode in [UiThemeMode::Light, UiThemeMode::Dark] {
        theme(cx, mode);
        for language in moon_core::config::Language::ALL {
            let _locale = crate::test_locale::force(language.code());
            let facts = [
                super::plural("storage.facts.reports", 412_388),
                [
                    super::plural("storage.facts.strategies", 1_669),
                    super::plural("storage.facts.deleted", 163),
                    super::plural("storage.facts.versions", 2_720),
                ]
                .join(" \u{b7} "),
                rust_i18n::t!("storage.facts.klines").to_string(),
                format!(
                    "{} \u{b7} {}",
                    super::plural("storage.facts.trades", 48_120),
                    rust_i18n::t!("storage.facts.trades_long", n = 3_904)
                ),
            ];
            let at = format!("{mode:?} {}", language.code());
            let (mut visual, _, _) = draw_page(cx, narrow, 30, Variant::Normal);
            for (db, text) in DBS.iter().zip(&facts) {
                let sel = format!("storage-db-{db}-facts");
                assert_wraps(&mut visual, &sel, text, true, &at);
                let name = visual
                    .debug_bounds(leak(format!("storage-db-{db}-name")))
                    .expect("name laid out");
                let size = visual
                    .debug_bounds(leak(format!("storage-db-{db}-size")))
                    .expect("size laid out");
                let facts = visual.debug_bounds(leak(sel)).expect("facts laid out");
                if f32::from(facts.origin.y) < f32::from(name.bottom_left().y) {
                    // Beside the name: then the whole line fits, nothing is lost to an ellipsis.
                    let need = shaped_caption_width(&mut visual, text, true);
                    assert!(
                        f32::from(facts.size.width) + 0.5 >= need,
                        "{db}'s facts {text:?} are cut beside the name at the narrow size ({at})"
                    );
                }
                assert!(
                    f32::from(size.origin.y) < f32::from(name.bottom_left().y),
                    "{db}'s size left the name's line at the narrow size ({at})"
                );
            }
            let (mut visual, _, _) = draw_page(cx, wide, 30, Variant::Normal);
            for (db, text) in DBS.iter().zip(&facts) {
                let b = visual
                    .debug_bounds(leak(format!("storage-db-{db}-facts")))
                    .expect("facts laid out");
                let need = shaped_caption_width(&mut visual, text, true);
                // The size is rendered in the same face at the same size and cannot wrap.
                let single_line = visual
                    .debug_bounds(leak(format!("storage-db-{db}-size")))
                    .expect("size laid out");
                assert!(
                    (f32::from(b.size.height) - f32::from(single_line.size.height)).abs() < 0.5,
                    "{db}'s wide facts are not one line tall ({at})"
                );
                assert!(
                    f32::from(b.size.width) + 0.5 >= need,
                    "{db}'s facts {text:?} are cut on a wide page ({} < {need}, {at})",
                    f32::from(b.size.width)
                );
            }
        }
    }
}

/// In a wide window the row labels, card titles and database names sit on one line, every
/// language.
#[gpui::test]
fn storage_labels_fit_one_line_in_a_wide_window(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    for language in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        let t = |k: &str| rust_i18n::t!(k).to_string();
        let mut fits = vec![
            (
                "storage-trades-margin-label".into(),
                t("storage.trades_margin"),
            ),
            (
                "storage-trades-long-label".into(),
                t("storage.trades_long_position"),
            ),
            ("storage-total-title".into(), t("storage.usage.total")),
            ("storage-tape-title".into(), t("storage.db.trades.title")),
        ];
        for db in DBS {
            fits.push((
                format!("storage-db-{db}-name"),
                t(&format!("storage.db.{db}.title")),
            ));
        }
        let spec = ProbeSpec {
            container: "probe-body",
            fits: fits
                .into_iter()
                .map(|(sel, text)| (sel, text, TextFace::Ui))
                .collect(),
            ..ProbeSpec::default()
        };
        let (mut visual, _, _) = draw_page(cx, (1900.0, 1000.0), 30, Variant::Normal);
        if let Err(broken) = check(&mut visual, None, &spec) {
            panic!("{broken} ({} at 1900x1000)", language.code());
        }
    }
}

/// The tape card's five states at the narrowest size, both themes: its tag, both hints, the fill
/// line and the auto line stay inside the card, uncut.
///
/// Breaks on: the shared-or-local tag pushed past the card, or a state's caption cut off.
#[gpui::test]
fn storage_tape_states_fit(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    let narrow = SIZES
        .iter()
        .copied()
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .expect("sizes");
    for mode in [UiThemeMode::Light, UiThemeMode::Dark] {
        theme(cx, mode);
        for language in moon_core::config::Language::ALL {
            let _locale = crate::test_locale::force(language.code());
            for variant in VARIANTS {
                let at = format!("{mode:?} {} {variant:?}", language.code());
                let (mut visual, _, _) = draw_page(cx, narrow, 30, variant);
                let tag = if variant == Variant::NoStation {
                    "storage.tape.local_only"
                } else {
                    "storage.tape.shared"
                };
                assert_wraps(
                    &mut visual,
                    "storage-tape-tag",
                    &rust_i18n::t!(tag),
                    false,
                    &at,
                );
                let mut within = vec![("storage-tape-tag".to_string(), "storage-tape".to_string())];
                for (sel, _) in captions(variant)
                    .into_iter()
                    .filter(|(sel, _)| sel.starts_with("storage-t"))
                {
                    within.push((sel, "storage-tape".into()));
                }
                let spec = ProbeSpec {
                    container: "storage-tape",
                    within,
                    ..ProbeSpec::default()
                };
                if let Err(broken) = check(&mut visual, None, &spec) {
                    panic!("{broken} ({at})");
                }
            }
        }
    }
}

/// The long-position field on the production Storage page holds its seeded digits after drawing,
/// and its box fits both step buttons plus the widest value it accepts plus its unit and stays
/// inside the tape card, in every language at every swept size.
///
/// The oracle is built here from the control's own metrics: two square buttons (one control
/// height each) plus the mono width of the range's widest end and of the unit. It does not call
/// the field helper.
///
/// Breaks on: a field seeded empty or only on blur, a field drawn over a state other than the
/// page's, or a row/card that squeezes the field so the value has no room between `-` and `+`,
/// or a field drawn without its unit.
#[gpui::test]
fn storage_number_fields_show_their_digits(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    let (sel, seeded, widest) = ("storage-trades-long-n", 5u32, "120");
    for language in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        for size in SIZES.into_iter().chain([(1940.0, 1180.0)]) {
            let at = format!("{} at {}x{}", language.code(), size.0, size.1);
            let (mut visual, state, _) = draw_page(cx, size, seeded, Variant::Normal);
            let text = visual.update(|_, cx| state.read(cx).value().to_string());
            assert_eq!(text, seeded.to_string(), "{sel} lost its value ({at})");
            let b = visual
                .debug_bounds(sel)
                .unwrap_or_else(|| panic!("{sel} was not laid out"));
            let unit = rust_i18n::t!("storage.unit_min").to_string();
            let need = visual.update(|_, cx| {
                let m = crate::design::CONTROL_TIER.control_metrics();
                2.0 * crate::design::ui_value(cx, m.height)
                    + crate::design::ui_text_width_zoomed(cx, widest, m.font_size, 500.0, true)
                    + crate::design::ui_text_width_zoomed(cx, &unit, m.font_size - 2.0, 400.0, true)
            });
            let card = visual
                .debug_bounds("storage-tape")
                .expect("the tape card laid out");
            assert!(
                f32::from(b.right()) <= f32::from(card.right()) + 0.5,
                "{sel} with its unit {unit:?} paints past the tape card ({at})"
            );
            assert!(
                f32::from(b.size.width) + 0.5 >= need,
                "{sel} is {} wide, needs {need} for {widest:?} and {unit:?} ({at})",
                f32::from(b.size.width)
            );
            // A click just inside the wrapper's right edge must land on `+` and step THIS state,
            // where the field is on screen without scrolling.
            let body = visual
                .debug_bounds("probe-body")
                .expect("probe body laid out");
            if b.bottom_left().y < body.bottom_left().y {
                let plus = gpui::point(b.right() - gpui::px(4.0), b.origin.y + b.size.height / 2.0);
                visual.simulate_click(plus, gpui::Modifiers::none());
                visual.run_until_parked();
                let stepped = visual.update(|_, cx| state.read(cx).value().to_string());
                assert_eq!(
                    stepped,
                    (seeded + 1).to_string(),
                    "{sel}'s + is not at its right edge or not bound to its state ({at})"
                );
            } else {
                assert!(size.0 < 1900.0, "{sel} is off screen in the wide window");
            }
        }
    }
}
