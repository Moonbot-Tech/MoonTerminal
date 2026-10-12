//! Trade sounds: which exchanges the table lists, and that the page chrome fits the body.

use std::default::Default;
use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext, Context, InteractiveElement, IntoElement, ParentElement, Render,
    ScrollHandle, Styled, Window, div, px,
};
use moon_core::feed::ExchangeId;

use super::super::common::slider_row;
use super::super::layout_probe::{ProbeSpec, Probed, sweep};
use super::super::sound_folder::{FolderView, folder_el};
use super::{SoundRow, SoundsTab, listed_exchanges, sound_picker_el, sounds_tab_el, sounds_wide};
use crate::media::sound::ScanStats;

/// A probe host laying the page out BOUNDED, as `render.rs` places the Sounds page: the body
/// gets the window's height and nothing scrolls but the exchange table.
struct Host {
    width: f32,
    height: f32,
    build: Rc<dyn Fn(&mut Window, &mut App, f32) -> AnyElement>,
}

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = (self.build)(window, cx, self.width);
        div()
            .w(px(self.width))
            .h(px(self.height))
            .debug_selector(|| "probe-body".into())
            .flex()
            .flex_col()
            .child(content)
    }
}

impl Probed for Host {
    fn scroll(&self) -> Option<ScrollHandle> {
        None
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

/// The production page: [`sounds_tab_el`] with the real volume row, real pickers (an empty
/// catalog: every trigger reads "off") and the real folder block with numbered sounds and its help
/// open — the tallest folder this test can build.
fn page(volume: &gpui::Entity<moon_ui::MoonSliderState>, width: f32, cx: &App) -> AnyElement {
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
    let rows = VENUES
        .iter()
        .enumerate()
        .map(|(i, label)| SoundRow {
            label: (*label).to_string(),
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
            show_all: false,
            quiet: true,
            volume: slider_row(
                &rust_i18n::t!("trade_sounds.volume"),
                volume,
                0.0..=100.0,
                |value| format!("{value:.0}%"),
                cx,
            )
            .into_any_element(),
            folder,
        },
        Box::new(|_, _, _| {}),
        cx,
    )
}

/// The production Sounds page — volume, the exchange table with real pickers, the folder block
/// with its buttons and open help — keeps every region and the rows' last controls inside the
/// bounded body and apart, with the help uncut, in every language at every swept size. Widen
/// a picker or lengthen a translated control past the body and a row's last cell leaves it.
#[gpui::test]
fn trade_sounds_page_fits_the_body(cx: &mut gpui::TestAppContext) {
    sweep(
        cx,
        |_, cx: &mut Context<Host>, (width, height)| {
            let volume = cx.new(|_| {
                moon_ui::MoonSliderState::new()
                    .max(100.0)
                    .min(0.0)
                    .step(1.0)
                    .default_value(80.0)
            });
            Host {
                width,
                height,
                build: Rc::new(move |_, cx, width| page(&volume, width, cx)),
            }
        },
        |_| ProbeSpec {
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
            ..ProbeSpec::default()
        },
    );
}
