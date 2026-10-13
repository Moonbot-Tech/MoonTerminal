//! Trade sounds: which exchanges the table lists, and that the page chrome fits the body.

use std::default::Default;
use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext, Context, InteractiveElement, IntoElement, ParentElement, Render,
    ScrollHandle, Styled, Window, div, px,
};
use moon_core::feed::ExchangeId;

use super::super::layout_probe::{ProbeSpec, Probed, TextFace, sweep};
use super::super::render::{BODY_PAD, scrollable_tab_content};
use super::super::sound_folder::{FolderView, folder_el};
use super::{
    SoundRow, SoundsTab, listed_exchanges, sound_picker_el, sounds_tab_el, sounds_wide, volume_row,
};
use crate::media::sound::ScanStats;

/// A probe host laying the page out as `render.rs` places the Sounds page: inside the Settings
/// body's own vertical scroller ([`scrollable_tab_content`]), the content measured at its
/// natural height. `probe-body` is that content column, so it grows with a long exchange list.
struct Host {
    width: f32,
    height: f32,
    scroll: ScrollHandle,
    build: Rc<dyn Fn(&mut Window, &mut App, f32) -> AnyElement>,
}

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = (self.build)(window, cx, self.width);
        let pad = 2.0 * crate::design::ui_value(cx, BODY_PAD);
        div()
            .w(px(self.width + pad))
            .h(px(self.height))
            .flex()
            .flex_col()
            .child(scrollable_tab_content(
                moon_ui::v_flex()
                    .w_full()
                    .debug_selector(|| "probe-body".into())
                    .child(content),
                &self.scroll,
                cx,
            ))
    }
}

impl Probed for Host {
    fn scroll(&self) -> Option<ScrollHandle> {
        Some(self.scroll.clone())
    }
}

/// The default list is the distinct venues of the servers. Show-all adds every directory
/// exchange no server reports; a server's label wins over the directory's.
///
/// Expected rows are written out here. Labels decide the order, so a sort that used insertion
/// order, or a show-all list that let the directory's label replace the server's, fails.
///
/// Breaks on: `listed_exchanges` repeating an exchange that two servers share, dropping a DEX
/// that only a server reported, or listing a directory exchange no server is on while show-all
/// is off.
#[test]
fn the_default_list_is_the_servers_and_show_all_is_the_directory() {
    let binance = ExchangeId::new(3);
    let bybit = ExchangeId::new(2);
    let okx = ExchangeId::new(1);
    let dex = ExchangeId::with_dex(13, "xyz");
    let servers = vec![
        (binance, "A srv".to_string()),
        (binance, "A srv duplicate".to_string()),
        (bybit, "B srv".to_string()),
        (dex, "D dex".to_string()),
    ];
    let directory = vec![
        (okx, "C dir".to_string()),
        (binance, "A dir".to_string()),
        (bybit, "B dir".to_string()),
    ];

    assert_eq!(
        listed_exchanges(&servers, &directory, false),
        vec![
            (binance, "A srv".to_string()),
            (bybit, "B srv".to_string()),
            (dex, "D dex".to_string()),
        ]
    );
    assert_eq!(
        listed_exchanges(&servers, &directory, true),
        vec![
            (binance, "A srv".to_string()),
            (bybit, "B srv".to_string()),
            (okx, "C dir".to_string()),
            (dex, "D dex".to_string()),
        ]
    );
    assert!(listed_exchanges(&[], &directory, false).is_empty());
}

/// Synthetic exchanges with long labels; the live page lists what the cores report.
const VENUES: [&str; 3] = [
    "Binance Futures USDT-M",
    "Hyperliquid xyz (HIP-3 DEX)",
    "Bybit Spot",
];

/// Exchanges in the user's report: seven rows, as the shipped page listed them.
const SEVEN: usize = 7;
/// A "show every exchange" list: far more rows than any window shows at once.
const SHOW_ALL: usize = 40;

/// The production page: [`sounds_tab_el`] with the real volume row, `count` rows of real pickers
/// (an empty catalog: every trigger reads "off"; labels cycle through [`VENUES`]) and the real
/// folder block with numbered sounds and its help open — the tallest folder this test can build.
fn page(
    volume: &gpui::Entity<moon_ui::MoonSliderState>,
    width: f32,
    count: usize,
    show_all: bool,
    cx: &App,
) -> AnyElement {
    let body_width = width + crate::design::ui_value(cx, 36.0);
    let wide = sounds_wide(body_width, cx);
    let picker = |i: usize, open: bool| {
        sound_picker_el(
            format!("probe-sound-{i}-{open}"),
            format!("probe-preview-{i}-{open}"),
            open,
            !wide,
            rust_i18n::t!("trade_sounds.off").to_string().into(),
            Vec::new(),
            true,
            Box::new(|_, _, _| {}),
            cx,
        )
    };
    let rows = (0..count)
        .map(|i| SoundRow {
            label: format!("{} {i}", VENUES[i % VENUES.len()]),
            open: picker(i, true),
            close: picker(i, false),
        })
        .collect();
    let stats = ScanStats {
        folder_files: 4,
        numbered: 2,
        ..ScanStats::default()
    };
    let folder = folder_el(
        FolderView {
            dir:
                "C:/Users/synthetic-user-with-a-long-name/AppData/Roaming/MoonTerminal/user/sounds"
                    .into(),
            open: true,
            scanned: true,
            stats,
            numbered: vec![
                (19, "synthetic_alarm".into()),
                (20, "synthetic_bell".into()),
            ],
        },
        Rc::new(|_, _| {}),
        Box::new(|_, _, _| {}),
        Box::new(|_, _, _| {}),
        cx,
    );
    sounds_tab_el(
        SoundsTab {
            rows,
            wide,
            content_w: width,
            show_all,
            quiet: true,
            volume: volume_row(volume, cx),
            folder,
        },
        Box::new(|_, _, _| {}),
        cx,
    )
}

/// Build the probe host drawing [`page`] with `count` exchanges.
fn host(
    count: usize,
    show_all: bool,
) -> impl Fn(&mut Window, &mut Context<Host>, (f32, f32)) -> Host {
    move |_, cx: &mut Context<Host>, (width, height)| {
        let volume = cx.new(|_| {
            moon_ui::MoonSliderState::new()
                .max(100.0)
                .min(0.0)
                .step(1.0)
                .default_value(100.0)
        });
        Host {
            width,
            height,
            scroll: ScrollHandle::new(),
            build: Rc::new(move |_, cx, width| page(&volume, width, count, show_all, cx)),
        }
    }
}

/// The production Sounds page — volume, the exchange table with real pickers, the folder block
/// with its buttons and open help — keeps every region and the rows' last controls inside the
/// body and apart, with the help uncut and the folder card reachable by the body's scroll, in
/// every language at every swept size. Widen a picker or lengthen a translated control past the
/// body and a row's last cell leaves it.
#[gpui::test]
fn trade_sounds_page_fits_the_body(cx: &mut gpui::TestAppContext) {
    sweep(cx, host(VENUES.len(), false), |_| ProbeSpec {
        container: "probe-body",
        wraps: vec![(
            "trade-sounds-own-folder-more-text".into(),
            rust_i18n::t!("trade_sounds.own.more").to_string(),
        )],
        inside: [
            "trade-sounds-volume",
            "trade-sounds-ex",
            "trade-sounds-show-all",
            "trade-sounds-help",
            "probe-sound-0-true-field",
            "probe-preview-0-false-cell",
            "trade-sounds-own",
            "trade-sounds-own-controls",
            "trade-sounds-own-path",
            "trade-sounds-own-folder-more-text",
        ]
        .map(String::from)
        .to_vec(),
        no_overlap: vec![vec![
            "trade-sounds-volume".into(),
            "trade-sounds-ex".into(),
            "trade-sounds-own".into(),
        ]],
        last: Some("trade-sounds-own".into()),
        ..ProbeSpec::default()
    });
}

/// What every sounds-table layout must hold for `count` rows: the volume label on one line in
/// the shared label column, clear of its slider and of the exchanges heading; the table's LAST
/// row ending exactly one column gap (12) above the custom-sounds card; the table and the card
/// one width; the card reachable by the body's scroll.
fn table_then_card(count: usize) -> ProbeSpec {
    let last_row = format!("trade-sounds-row-{}", count - 1);
    ProbeSpec {
        container: "probe-body",
        fits: vec![(
            "trade-sounds-volume-row-label".into(),
            rust_i18n::t!("trade_sounds.volume").to_string(),
            TextFace::Ui,
        )],
        inside: vec![
            "trade-sounds-volume-row-control".into(),
            last_row.clone(),
            "trade-sounds-own".into(),
        ],
        no_overlap: vec![
            vec![
                "trade-sounds-volume-row-label".into(),
                "trade-sounds-volume-row-control".into(),
                "trade-sounds-ex-heading".into(),
            ],
            vec![last_row.clone(), "trade-sounds-own".into()],
        ],
        same_width: vec![vec!["trade-sounds-rows".into(), "trade-sounds-own".into()]],
        // The last row's bottom plus the table's 1 px border is the table's bottom; the column
        // gap (12) is all that separates it from the card — less is the card riding over rows.
        stacked: vec![
            ("trade-sounds-rows".into(), "trade-sounds-own".into(), 12.0),
            (last_row, "trade-sounds-own".into(), 13.0),
        ],
        last: Some("trade-sounds-own".into()),
        ..ProbeSpec::default()
    }
}

/// The Sounds page reads like every other Settings page, and the custom-sounds card follows the
/// exchange table in flow — with the seven exchanges of the user's report, laid out inside the
/// Settings body's own scroller exactly as the exe places it. Every language, every swept size.
///
/// Breaks on: the table getting a scroller of its own again (MoonUI's `overflow_y_scrollbar`
/// sizes it `size_full` + `flex_1`, so its column never learns the rows' height and the card is
/// laid one gap under the table's TOP, over its rows), the card losing the table's width, the
/// label going back into a `min_w_0` column beside a fixed control, or a hole opening between
/// the table and the card.
#[gpui::test]
fn sounds_volume_row_and_cards_line_up(cx: &mut gpui::TestAppContext) {
    sweep(cx, host(SEVEN, false), |_| table_then_card(SEVEN));
}

/// With "show every exchange" on, the long table still pushes the card down rather than
/// running under it: the card starts one gap under the last of the forty rows. Breaks on a
/// capped or self-scrolling table whose height its column misreads.
///
/// The body's scroll reaches the card's bottom too: a `max_w`-capped column measured the
/// card's wrapping text at the uncapped width, and in Russian the card painted 26 px past its
/// slot and out of the scroll extent.
#[gpui::test]
fn show_all_keeps_the_card_under_the_table(cx: &mut gpui::TestAppContext) {
    sweep(cx, host(SHOW_ALL, true), |_| table_then_card(SHOW_ALL));
}
