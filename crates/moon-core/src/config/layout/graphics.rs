//! Chart graphics preferences and their defaults.

use super::*;

/// Per-axis enable switches for the core-warning engine, set from the Core Status gear popup.
///
/// Each field gates one warning axis end to end: while `false`, the backend engine opens no
/// episodes for that axis (so nothing is persisted and no tab/badge lights up) and the read paths
/// filter its persisted history out of the charts and the Warnings list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WarnAxesCfg {
    /// Sustained machine system-CPU warning (per server).
    #[serde(default = "def_true")]
    pub cpu: bool,
    /// Rising process-memory warning (per core).
    #[serde(default = "def_true")]
    pub mem: bool,
    /// Dropped-core connectivity warning (per server).
    #[serde(default = "def_true")]
    pub conn: bool,
    /// Sustained above-baseline client↔core ping/RTT warning (per core).
    #[serde(default = "def_true")]
    pub ping: bool,
    /// Sustained above-baseline core→exchange order-API latency warning (per core).
    #[serde(default = "def_true")]
    pub exch: bool,
    /// Expiring exchange API-key warning (per core).
    #[serde(default = "def_true")]
    pub api: bool,
    /// Exhausting exchange API request quota warning (per core).
    #[serde(default = "def_true")]
    pub api_quota: bool,
}

impl Default for WarnAxesCfg {
    /// Every axis on — the behaviour before the toggles existed, and for every config without the key.
    fn default() -> Self {
        Self {
            cpu: true,
            mem: true,
            conn: true,
            ping: true,
            exch: true,
            api: true,
            api_quota: true,
        }
    }
}

/// Where the horizontal volumes' cursor readout prints, and whether the zone's captions get their
/// backing plates — Moonbot's `Disp. vol`.
///
/// The zone itself always sits at the pane's LEFT edge with its rows growing from the plot side
/// outward, as the reference draws it; the side here is the side of the zone the VOLUME READOUT
/// under the crosshair prints at. The `Transparent` pair prints the zone's captions without
/// their backing plates — the theme's plain ink over the rows instead of light on dark.
/// How a tab draws the closed trades of its market: as the terminal's entry/exit arrows, or as
/// the order lines Moonbot itself draws — the archived buy/sell lines with their repricing paths
/// and stop markers, resolved through the local trace archive and the core.
///
/// One or the other PER TRADE, never both: the arrows and the lines describe the same trade at
/// the same place, and drawn together they cover each other. In the lines style a trade the
/// archive holds no lines for — older than the archive, or not answered yet — keeps its arrows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TradeHistoryStyle {
    /// Entry and exit arrows joined by a connector — the picture this terminal shipped with.
    #[default]
    Marks,
    /// Moonbot's order lines from the trace archive.
    MoonbotLines,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HvolSide {
    #[default]
    Right,
    Left,
    RightTransparent,
    LeftTransparent,
}

impl HvolSide {
    /// Whether the cursor readout prints at the zone's LEFT edge (else its right edge).
    pub fn is_left(self) -> bool {
        matches!(self, HvolSide::Left | HvolSide::LeftTransparent)
    }

    /// Whether the zone's captions print WITHOUT their backing plates.
    pub fn is_transparent(self) -> bool {
        matches!(self, HvolSide::RightTransparent | HvolSide::LeftTransparent)
    }
}

/// Chart drawing settings edited from the toolbar's palette popup.
///
/// Stored here as the GLOBAL DEFAULT: each chart tab may hold its own set in `charts.json`, and a
/// tab without one draws with this.
///
/// Deliberately separate from `OrdersStyle` in `orders.toml`: that file describes how each ORDER
/// LINE is painted (colour, dash, marker sizes), while these values decide how order-line repricing
/// and closed-trade history, the trade marks, and the bottom volume band are drawn, and which
/// closed TRADES appear at all. Mixing them would make two unrelated surfaces move together, which
/// is the same reason `trade_marks.rs` refused to read `orders.toml`.
///
/// The last six fields moved here from `ChartTheme` (`theme.toml`). They belong to a CHART TAB, not
/// to a colour scheme: two tabs on one theme routinely want different marker sizes and a different
/// volume band. One consequence is deliberate and worth knowing — `ChartTheme::apply_light_defaults`
/// used to give the light theme its own `candle_volume_alpha` and `candle_volume_scale`, and a
/// per-tab value cannot vary by theme mode, so that pair no longer switches with the mode.
/// `moonterminal::startup::graphics_migration` carries every existing user's values across.
///
/// The numeric fields are NOT clamped here, because `layout.toml` is hand-editable and the drawing
/// path and the hit-test path must clamp IDENTICALLY or the glyph and the region that responds to
/// it drift apart. Every clamp, and the `normalize_chart_graphics` that each storing or comparing
/// site applies, therefore live together in `moon_chart::trade_marks`.
///
/// Every field decodes LENIENTLY and independently, because the whole document is one
/// deserialization (see [`WindowLayout`]): a hand-typed `show_real_trades = "yes"` falls back to
/// that one field's default instead of resetting the others — or the file.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChartGraphicsCfg {
    /// Multiplier on the trade-history arrow's half-extents. One is the size the layer shipped with.
    #[serde(default = "def_trade_arrow_scale", deserialize_with = "de_arrow_scale")]
    pub trade_arrow_scale: f32,
    /// Thickness of the dashed entry-to-exit connector, in LOGICAL px.
    #[serde(
        default = "def_connector_thickness_px",
        deserialize_with = "de_connector_thickness"
    )]
    pub connector_thickness_px: f32,
    /// Whether closed trades made by a REAL (non-emulator) order draw their history marks.
    ///
    /// This pair once selected which ORDER LINES were drawn. It selects TRADES now: the order lines
    /// a core reports are few and every one of them is actionable, while closed-trade history is the
    /// crowded layer where telling a live result from an emulated one is what the user needs.
    #[serde(default = "def_true", deserialize_with = "de_lenient_true")]
    pub show_real_trades: bool,
    /// Whether closed trades made by an EMULATOR order draw their history marks.
    #[serde(default = "def_true", deserialize_with = "de_lenient_true")]
    pub show_emulator_trades: bool,
    /// Arrows or Moonbot's order lines for the closed trades the two switches above admit.
    ///
    /// ABSENT reads as arrows: the lines are opt-in, and a tab saved before this field existed
    /// keeps the picture it had.
    #[serde(default, deserialize_with = "de_trade_history_style")]
    pub trade_history_style: TradeHistoryStyle,
    /// Draw the closed trades of every Auto-Overview core on the chart core's own exchange,
    /// not only the chart's own core.
    ///
    /// OFF by default: an absent value keeps today's single-core picture, and ON would silently
    /// add other cores' arrows to every existing Overview chart. The stored flag alone never
    /// widens a read. The runtime gate (Auto + Overview, same exchange) is applied where the
    /// request is built, so a stored `true` read outside Auto Overview still draws one core.
    #[serde(default, deserialize_with = "de_lenient_false")]
    pub history_all_cores: bool,
    /// Whether a CLOSED order hides its sell-price line. Live orders always keep theirs.
    ///
    /// On by default: after an order closes, its blue sell line stays on the chart at
    /// `closed_alpha` and reads as a live price the terminal is still tracking.
    #[serde(default = "def_true", deserialize_with = "de_lenient_true")]
    pub hide_closed_sell_line: bool,
    /// Whether an order line hides its repricing history: the server-reported trace, the locally
    /// reconstructed staircase, and the knot marking each reprice. The server's `SetStopPrice`
    /// segment remains visible because it records where a stop sat, rather than a reprice.
    ///
    /// OFF by default, unlike its neighbour above: it removes information a user may rely on, so
    /// it is opt-in rather than opt-out.
    #[serde(default, deserialize_with = "de_lenient_false")]
    pub hide_order_move_history: bool,
    /// Whether the entry line ends in a plain cross at its fill instead of the fill arrow.
    ///
    /// RUNTIME ONLY, never stored: the chart sets it for the passes of the "Moonbot lines" style,
    /// where the exit line starting at that very point already says the entry filled and the arrow
    /// would be a second glyph on one event. A saved tab keeps the arrow it always had.
    #[serde(skip)]
    pub hide_entry_fill_arrow: bool,
    /// Whether a closed order that became a TRADE — its entry filled, at least in part — draws at
    /// the active opacity instead of `closed_alpha`. A cancelled order is untouched: it stays under
    /// the "closed/cancelled visibility" slider, so the eye still tells a trade from a leftover.
    ///
    /// RUNTIME ONLY, never stored: the chart sets it for the live pass of the "Moonbot lines"
    /// style, where Moonbot draws every closed trade in full colour beside the archived ones.
    #[serde(skip)]
    pub bright_closed_trades: bool,

    // --- Lines over the plot. The three price-line flags moved here from `CandleViewCfg`, and the
    // liquidations switch from the tab spec's own per-tab field, so that every setting the
    // "Chart graphics" popup shows travels with its ⧉ press; the carry-over for an existing profile
    // is `WindowLayout::carry_lines_into_graphics` and the startup pass beside it. ---
    /// Whether the orange `LastPrice` line from the core is drawn.
    #[serde(default = "def_true", deserialize_with = "de_lenient_true")]
    pub last_price_line: bool,
    /// Whether the blue `MarkPrice` line from the core is drawn. A market whose provider reports
    /// no mark price draws none regardless.
    #[serde(default = "def_true", deserialize_with = "de_lenient_true")]
    pub mark_price_line: bool,
    /// Whether a MoonShot order fills its corridor between the entry and the target.
    #[serde(default = "def_true", deserialize_with = "de_lenient_true")]
    pub moonshot_zone: bool,
    /// Whether liquidation trades draw their crosses beside the trade marks.
    #[serde(default = "def_true", deserialize_with = "de_lenient_true")]
    pub liquidations: bool,

    // --- Trade marks. Moved here from `ChartTheme` so they are per TAB rather than per theme. ---
    /// Multiplier on the trade-cross marker size. The device pixel ratio is applied separately and
    /// is not part of this number.
    ///
    /// It MULTIPLIES with [`Self::trade_arrow_scale`] and is a different knob: that one sizes the
    /// closed-trade HISTORY arrows, this one the live trade crosses.
    #[serde(default = "def_marker_scale", deserialize_with = "de_marker_scale")]
    pub marker_scale: f32,
    /// Opacity of the per-TRADE volume bars along the plot's bottom edge, 0..1. Distinct from
    /// [`Self::candle_volume_alpha`], which is the per-CANDLE band drawn beneath them.
    #[serde(
        default = "def_trade_volume_alpha",
        deserialize_with = "de_trade_volume_alpha"
    )]
    pub trade_volume_alpha: f32,

    // --- Bottom candle volumes, likewise moved off `ChartTheme`. ---
    /// Display style: `crate::market::candles::VOLUME_STYLE_OFF` or `_HILLS`. Together with
    /// [`Self::candle_volume_sides`] it is ONE switch — the volumes popup's checkbox — and the
    /// normaliser (`moon_chart::normalize_chart_graphics`) folds every pair an older build could
    /// write onto that switch: hills with the split on, or off. The two fields stay because both
    /// files carry them; neither is read on its own.
    #[serde(
        default = "def_candle_volume_style",
        deserialize_with = "de_candle_volume_style"
    )]
    pub candle_volume_style: u8,
    /// Band height as a fraction of the plot height, 0..1. Capped in physical pixels by the
    /// geometry module so the band cannot swallow a tall chart.
    #[serde(
        default = "def_candle_volume_height",
        deserialize_with = "de_candle_volume_height"
    )]
    pub candle_volume_height: f32,
    /// Bottom-volume opacity, 0..1. The band's colours come from the candle colours, which stay on
    /// the theme — only the opacity is per tab.
    #[serde(
        default = "def_candle_volume_alpha",
        deserialize_with = "de_candle_volume_alpha"
    )]
    pub candle_volume_alpha: f32,
    /// Colour of the volume scale's max and average reference lines, sRGB.
    #[serde(
        default = "def_candle_volume_scale",
        deserialize_with = "de_candle_volume_scale"
    )]
    pub candle_volume_scale: [u8; 3],
    /// Moonbot's `Vol` on top of the band: where the retained trade history reaches, the band
    /// shows BOUGHT and SOLD as rolling sums over `candle_volume_tf_s`; before that it keeps the
    /// candle turnover in the candle's own colours, scaled to the same interval so the two halves
    /// share one scale. On whenever the band is on — the other half of the one switch
    /// [`Self::candle_volume_style`] describes.
    ///
    /// `Default` is ON, so a fresh profile opens on the band. The serde default is still OFF,
    /// deliberately: a file without this key was written before the switch existed, and there
    /// the style alone said whether the band drew — the normaliser reads it that way, so a user
    /// who had the band off stays off. A file this build writes always carries the key.
    #[serde(default, deserialize_with = "de_lenient_false")]
    pub candle_volume_sides: bool,
    /// With [`Self::candle_volume_sides`]: draw SOLD on top of BOUGHT so a column's height is
    /// their sum, rather than overlaying the two from the floor so its height is the larger side.
    /// Moonbot's `Kind`: `Stacked graph` / `Smooth graph`.
    #[serde(default, deserialize_with = "de_lenient_false")]
    pub candle_volume_stacked: bool,
    /// With [`Self::candle_volume_sides`]: the rolling interval in seconds each side's turnover is
    /// summed over, one of `moon_chart::side_volume::SIDE_TF_CHOICES_S`; `0` picks one from the
    /// zoom so the hill stays readable. Not the candle timeframe: Moonbot's `TimeFrame` on its
    /// `Vol` popup.
    #[serde(default, deserialize_with = "de_candle_volume_tf_s")]
    pub candle_volume_tf_s: u32,
    /// Print the band's scale labels at the plot's RIGHT edge instead of its left. Moonbot's
    /// `Ind. Pos`; applies to every bottom-volume style.
    #[serde(default, deserialize_with = "de_lenient_false")]
    pub candle_volume_scale_right: bool,
    /// Let the captions of the plot's BOTTOM band print over the volume bars instead of above
    /// them. Off, every module in that band starts at the band's top edge, so a tall band pushes
    /// the captions up the plot; on, they start at the plot's floor and their plates back them
    /// against the bars. Applies to every bottom-volume style.
    #[serde(default, deserialize_with = "de_lenient_false")]
    pub candle_volume_labels_over: bool,

    // --- Horizontal volumes: Moonbot's `HVol`, turnover by price beside the plot. ---
    /// Whether the horizontal-volume zone is drawn at all. Moonbot's `HvShow`.
    #[serde(default, deserialize_with = "de_lenient_false")]
    pub hvol_enabled: bool,
    /// The trailing window the profile covers, in seconds: one of
    /// `moon_chart::hvol::HVOL_TF_CHOICES_S`, `0` for `Auto` (picked from the visible span), or
    /// [`HVOL_TF_MAX_S`] for everything retained. Moonbot's `TimeFrame` on its `HVol` popup.
    #[serde(default, deserialize_with = "de_hvol_tf_s")]
    pub hvol_tf_s: u32,
    /// The ROLLING window over price as a percentage of the price, `0.01..=5` — Moonbot's
    /// `PriceFrame`, the horizontal twin of the vertical band's interval: at every pixel of the
    /// zone's height the profile shows what traded within this much price around it. The chart
    /// floors the window at the market's tick, which is why Moonbot prints `PriceFrame: 0.40%`
    /// under a `0.12%` slider on a coin whose tick is that wide.
    #[serde(
        default = "def_hvol_price_frame_pct",
        deserialize_with = "de_hvol_price_frame_pct"
    )]
    pub hvol_price_frame_pct: f32,
    /// Zone width as a fraction of the pane width, `0.05..=0.5`.
    #[serde(default = "def_hvol_width", deserialize_with = "de_hvol_width")]
    pub hvol_width: f32,
    /// Which edge of the zone the cursor readout prints at, and whether its captions get backing
    /// plates. The rows' colours and opacity are the bottom band's ([`Self::candle_volume_alpha`]): the
    /// reference draws its two volume indicators alike, and one opacity serves both here.
    #[serde(default, deserialize_with = "de_hvol_side")]
    pub hvol_side: HvolSide,
    /// Moonbot's `Kind`: `Stacked graph` draws SOLD after BOUGHT so a row's length is their sum;
    /// off (`Smooth graph`) both grow from the same edge and the longer side shows past the other,
    /// exactly as [`Self::candle_volume_stacked`] draws the bottom band.
    #[serde(default, deserialize_with = "de_lenient_false")]
    pub hvol_stacked: bool,
    /// Lay the profile OVER the plot's left edge instead of in a zone of its own beside it — the
    /// way the bottom band sits inside the plot: the plot keeps its full width, the zone draws no
    /// backdrop or frame (the theme's `hvol_bg` goes unused), and the rows grow from the plot's
    /// left edge inward, mirrored, so they read as a profile anchored to the edge rather than
    /// stubs pointing at it. Not a Moonbot setting: the reference always carves the zone out.
    #[serde(default, deserialize_with = "de_lenient_false")]
    pub hvol_overlay: bool,
    /// Leave out the zone's two corner captions — the window (`Окно: 12h`) and the price window
    /// (`Окно цены: 0.10%`). The cursor readout and its plates are untouched; this hides only the
    /// standing captions, for a reader who knows what `Auto` picked and wants the zone bare.
    #[serde(default, deserialize_with = "de_lenient_false")]
    pub hvol_hide_captions: bool,
}

/// The `hvol_tf_s` value that means "everything retained" — Moonbot's `Max`.
pub const HVOL_TF_MAX_S: u32 = u32::MAX;

impl Default for ChartGraphicsCfg {
    /// The shipped sizes with every trade kind visible, the closed sell line hidden, and the
    /// order move-history trail shown.
    fn default() -> Self {
        Self {
            trade_arrow_scale: def_trade_arrow_scale(),
            connector_thickness_px: def_connector_thickness_px(),
            show_real_trades: true,
            show_emulator_trades: true,
            trade_history_style: TradeHistoryStyle::Marks,
            history_all_cores: false,
            hide_closed_sell_line: true,
            hide_order_move_history: false,
            hide_entry_fill_arrow: false,
            bright_closed_trades: false,
            last_price_line: true,
            mark_price_line: true,
            moonshot_zone: true,
            liquidations: true,
            marker_scale: def_marker_scale(),
            trade_volume_alpha: def_trade_volume_alpha(),
            candle_volume_style: def_candle_volume_style(),
            candle_volume_height: def_candle_volume_height(),
            candle_volume_alpha: def_candle_volume_alpha(),
            candle_volume_scale: def_candle_volume_scale(),
            candle_volume_sides: true,
            candle_volume_stacked: false,
            candle_volume_tf_s: 0,
            candle_volume_scale_right: false,
            candle_volume_labels_over: false,
            hvol_enabled: false,
            hvol_tf_s: 0,
            hvol_price_frame_pct: def_hvol_price_frame_pct(),
            hvol_width: def_hvol_width(),
            hvol_side: HvolSide::Right,
            hvol_stacked: false,
            hvol_overlay: false,
            hvol_hide_captions: false,
        }
    }
}
