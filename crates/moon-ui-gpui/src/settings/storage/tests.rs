//! Storage page: the usage shares add up, and the page chrome fits the settings body.

use std::default::Default;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, InteractiveElement, IntoElement, ParentElement, Render, ScrollHandle,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use std::collections::HashSet;

use super::super::layout_probe::{ProbeSpec, Probed, sweep};
use super::core_data::{
    CoreCounts, CoreLine, CoreView, Group, Kinds, core_section_el, lines as core_lines,
};
use super::{StorageCtx, StorageInfo, StorageView, storage_body_el, usage_parts};

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
/// or listing the databases in an order other than reports, strategies, trades, klines.
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

    let expected = [400 + 12, 38, 200 + 31, 0];
    assert_eq!(
        parts.iter().map(|part| part.bytes).collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        parts.iter().map(|part| part.bytes).sum::<u64>(),
        400 + 12 + 38 + 200 + 31
    );
}

/// The production Storage page from synthetic data: two connected cores, one archived and one
/// syncing in the core table, two help lines open, a counted cleanup.
fn page(fields: &[gpui::Entity<moon_ui::MoonInputState>; 3], cx: &App) -> AnyElement {
    let open: HashSet<&'static str> = ["storage-trades-margin", "storage-core-hint"].into();
    let ctx = StorageCtx {
        weak: gpui::WeakEntity::new_invalid(),
        open: &open,
    };
    let core_line = |uid: u64, name: &str, group: Group, syncing: Option<&str>| CoreLine {
        counts: CoreCounts {
            uid,
            stored_name: name.into(),
            reports: 12_345,
            strategies: 48,
            versions: 960,
            traces: 7_777,
            warnings: 31,
        },
        name: name.into(),
        group,
        syncing: syncing.map(String::from),
    };
    let cores = [
        core_line(11, "Synthetic Binance Futures core", Group::Connected, None),
        core_line(12, "Synthetic Bybit core", Group::Offline, Some("42%")),
        core_line(
            13,
            "A retired synthetic core with a long name",
            Group::Archive,
            None,
        ),
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
    let info = StorageInfo {
        reports: Some((400 << 20, 12 << 20)),
        strategies: Some((38 << 20, 0)),
        klines: Some((200 << 20, 31 << 20)),
        trades: Some((1 << 30, 64 << 20)),
        report_rows: Some(Ok(1_234_567)),
        strat_live: 120,
        strat_deleted: 7,
        strat_versions: 4_321,
    };
    let [version_limit, max_mb, long_min] = fields.clone();
    storage_body_el(
        StorageView {
            info,
            busy: false,
            path: "C:/Users/synthetic-user-with-a-long-name/AppData/Roaming/MoonTerminal/data"
                .into(),
            cfg: Default::default(),
            version_limit,
            max_mb,
            long_min,
            cleanup: None,
            core,
        },
        &ctx,
        cx,
    )
}

/// The production Storage page — usage panel, the four translated database cards with their
/// actions, number fields, switches and cleanup, and the real core table — keeps every card, the
/// card actions, the core rows' footer and the open help inside the body, the cards apart, the
/// help uncut, in every language at every swept size.
#[gpui::test]
fn storage_page_fits_the_body(cx: &mut gpui::TestAppContext) {
    sweep(
        cx,
        |window, cx: &mut Context<Host>, (width, height)| {
            let fields = [
                super::super::common::number_state(100u32, window, cx),
                super::super::common::number_state(256u32, window, cx),
                super::super::common::number_state(5u32, window, cx),
            ];
            Host {
                width,
                height,
                scroll: ScrollHandle::new(),
                build: Rc::new(move |_, cx, _| page(&fields, cx)),
            }
        },
        |_| ProbeSpec {
            container: "probe-body",
            wraps: vec![
                (
                    "storage-trades-margin-more-text".into(),
                    rust_i18n::t!("storage.trades_margin_hint").to_string(),
                ),
                (
                    "storage-core-hint-more-text".into(),
                    rust_i18n::t!("storage.core_data_hint").to_string(),
                ),
            ],
            inside: [
                "storage-usage",
                "storage-card-reports",
                "storage-card-strategies",
                "storage-card-trades",
                "storage-card-klines",
                "storage-trades-margin-more-text",
                "storage-core",
                "storage-core-hint-more-text",
            ]
            .map(String::from)
            .to_vec(),
            no_overlap: vec![
                [
                    "storage-card-reports",
                    "storage-card-strategies",
                    "storage-card-trades",
                    "storage-card-klines",
                ]
                .map(String::from)
                .to_vec(),
            ],
            last: Some("storage-core".into()),
            ..ProbeSpec::default()
        },
    );
}
