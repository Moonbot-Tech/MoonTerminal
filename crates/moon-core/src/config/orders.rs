//! Chart order-line styles in a separate portable `orders.toml` file in the config
//! directory (like `theme.toml`). Each line kind (buy/sell/stop/trailing/tp/vstop/
//! cond/liq) specifies its color, thickness, start/end markers (45° cross), repricing
//! knots, and their sizes. Global settings control active/closed line opacity and the
//! number of newest closed orders drawn per market. Colors are in sRGB (shaders
//! calculate linear values).

use serde::{Deserialize, Serialize};

use super::paths;
use crate::palette;

/// Style for one line kind. With `#[serde(default)]`, fields missing from the current
/// table use the generic default (see `Default`); colors are best specified explicitly.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LineStyle {
    /// Line and marker color (sRGB).
    pub color: [u8; 3],
    /// Line thickness, in pixels.
    pub thickness: f32,
    /// Whether to draw a cross at the start point (placement).
    pub start_marker: bool,
    /// Whether to draw a cross at the end point (close/cancel).
    pub end_marker: bool,
    /// Half-length of a cross arm, in pixels (tick ≈ 3.5 → ×3 ≈ 10).
    pub marker_size: f32,
    /// Cross-line thickness, in pixels.
    pub marker_thickness: f32,
    /// Whether to draw a point knot at every price move.
    pub knots: bool,
    /// Knot half-size, in pixels.
    pub knot_size: f32,
    /// Base dashed-line setting (for example, a pending condition).
    pub dashed: bool,
    /// Opacity of lines for a PLACED but not yet filled order (fill=0), 0..1.
    /// Applied at the order level through its entry line: `buy` (long) or `buy_short`
    /// (short). After the fill, lines use `active_alpha`. Other lines ignore this field
    /// (it is meaningful only for `buy`/`buy_short`).
    pub pending_alpha: f32,
    /// Entry-line color for a PLACED but not yet filled order (fill=0).
    /// `None` = use the primary `color` (that is, placed = filled, only dimmer).
    /// Meaningful only for `buy`/`buy_short`; after the fill, `color` is used.
    #[serde(default)]
    pub pending_color: Option<[u8; 3]>,
}

impl Default for LineStyle {
    fn default() -> Self {
        Self {
            color: palette::TEXT_2,
            thickness: 1.0,
            start_marker: true,
            end_marker: true,
            marker_size: 4.0,
            marker_thickness: 1.5,
            knots: true,
            knot_size: 3.0,
            dashed: false,
            pending_alpha: 0.65,
            pending_color: None,
        }
    }
}

impl LineStyle {
    fn with(color: [u8; 3]) -> Self {
        Self {
            color,
            ..Self::default()
        }
    }
    /// The same style without start/end crosses (for trailing/tp/vstop).
    fn no_markers(mut self) -> Self {
        self.start_marker = false;
        self.end_marker = false;
        self
    }
    /// Line-thickness builder.
    fn t(mut self, thickness: f32) -> Self {
        self.thickness = thickness;
        self
    }
    /// Unfilled-order color (placed, not filled) builder.
    fn pending(mut self, color: [u8; 3]) -> Self {
        self.pending_color = Some(color);
        self
    }
}

/// Path (trail) style — the actual line movement through its repricing history.
/// Separate from the primary (straight) line, with its own visibility toggle, color, thickness, and dash setting.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PathStyle {
    /// Whether to show the path through historical positions.
    pub show: bool,
    /// Path color (sRGB).
    pub color: [u8; 3],
    /// Path thickness, in pixels.
    pub thickness: f32,
    /// Whether the path is dashed.
    pub dashed: bool,
}

impl Default for PathStyle {
    fn default() -> Self {
        Self {
            show: true,
            color: palette::TEXT_3,
            thickness: 1.0,
            dashed: true,
        }
    }
}

/// Complete order-line style (orders.toml).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct OrdersStyle {
    /// LONG-order entry line (buy_price). Orange by default.
    pub buy: LineStyle,
    /// SHORT-order entry line, with a separate color/style from long (as in Moonbot long/short).
    /// Applied to the entry line, cross, and size label when the order is short.
    pub buy_short: LineStyle,
    /// LONG-order sell-price line (sell_price). Blue by default.
    pub sell: LineStyle,
    /// SHORT-order sell-price line, with a separate color/style from long (as in Moonbot:
    /// BuyShort/SellShort). Applied to the sell line when the order is short.
    pub sell_short: LineStyle,
    /// Stop loss. Red.
    pub stop: LineStyle,
    /// Trailing stop. Light blue.
    pub trailing: LineStyle,
    /// Take profit. Green.
    pub take_profit: LineStyle,
    /// VStop. Purple.
    pub vstop: LineStyle,
    /// Pending condition (BuyCondPrice). Gray, dashed.
    pub pending_cond: LineStyle,
    /// Liquidation line. Red, WITHOUT start/end markers (continuous).
    pub liq: LineStyle,
    /// Path (trail) — line movement through the repricing history (optional).
    pub path: PathStyle,
    /// Whether to draw MoonShot/MoonHook corridor fill zones.
    pub moonshot_corridor: bool,
    /// Opacity of the server-provided order trace (`CO_OrderLine.Thikness2` in Moonbot).
    pub trace_alpha: f32,

    /// Active-line opacity, 0..1.
    pub active_alpha: f32,
    /// Closed-line (canceled/filled) opacity, 0..1.
    pub closed_alpha: f32,
    /// Whether to draw a pending order's entry line as dashed.
    pub pending_dashed: bool,
    /// Number of newest closed orders selected for chart drawing per market.
    /// Closed-order storage is independently capped at 5000 entries per core.
    pub max_closed_orders: u32,
}

impl Default for OrdersStyle {
    /// Default = Moonbot dark set.
    fn default() -> Self {
        let liq = LineStyle {
            start_marker: false,
            end_marker: false,
            knots: false,
            thickness: 2.0,
            ..LineStyle::with([255, 69, 0])
        };
        let pending_cond = LineStyle {
            dashed: true,
            ..LineStyle::with([255, 106, 0]).t(2.0)
        };
        let path = PathStyle {
            color: [211, 211, 211],
            ..PathStyle::default()
        };
        Self {
            buy: LineStyle::with([210, 105, 30])
                .t(2.0)
                .pending([255, 106, 0]),
            buy_short: LineStyle::with([176, 196, 222])
                .t(2.0)
                .pending([255, 106, 0]),
            sell: LineStyle::with([127, 201, 255]).t(2.0),
            sell_short: LineStyle::with([211, 211, 211]).t(2.0),
            stop: LineStyle::with([255, 127, 80]).t(2.0),
            trailing: LineStyle::with([100, 149, 237]).t(2.0).no_markers(),
            take_profit: LineStyle::with([47, 168, 92]).no_markers(),
            vstop: LineStyle::with([180, 120, 255]).no_markers(),
            pending_cond,
            liq,
            path,
            moonshot_corridor: true,
            trace_alpha: 1.0,
            active_alpha: 1.0,
            closed_alpha: 0.35,
            pending_dashed: true,
            max_closed_orders: 500,
        }
    }
}

impl OrdersStyle {
    /// Default Moonbot light set.
    fn default_light() -> Self {
        let liq = LineStyle {
            start_marker: false,
            end_marker: false,
            knots: false,
            thickness: 2.0,
            ..LineStyle::with([255, 0, 0])
        };
        let pending_cond = LineStyle {
            dashed: true,
            ..LineStyle::with([176, 0, 0]).t(2.0)
        };
        let path = PathStyle {
            color: [0, 0, 0],
            ..PathStyle::default()
        };
        Self {
            buy: LineStyle::with([0, 0, 0]).t(2.0).pending([176, 0, 0]),
            buy_short: LineStyle::with([139, 0, 139]).t(2.0).pending([176, 0, 0]),
            sell: LineStyle::with([0, 0, 255]).t(2.0),
            sell_short: LineStyle::with([128, 0, 0]).t(2.0),
            stop: LineStyle::with([255, 0, 0]).t(2.0),
            trailing: LineStyle::with([0, 0, 139]).t(2.0).no_markers(),
            take_profit: LineStyle::with([47, 168, 92]).no_markers(),
            vstop: LineStyle::with([180, 120, 255]).no_markers(),
            pending_cond,
            liq,
            path,
            moonshot_corridor: true,
            trace_alpha: 1.0,
            active_alpha: 1.0,
            closed_alpha: 0.35,
            pending_dashed: true,
            max_closed_orders: 500,
        }
    }
}

impl OrdersStyle {
    /// Reads `orders.toml` from the config directory. A missing file yields the default and saves it
    /// (so the user immediately has a complete file with the correct colors to edit).
    /// A corrupt file yields the default without failing.
    pub fn load() -> Self {
        let path = paths::orders_path();
        if !path.exists() {
            let def = Self::default();
            let _ = def.save();
            return def;
        }
        super::toml_io::load_or_default(&path, "orders.toml", |_| {})
    }

    /// Writes orders.toml (open, human-readable TOML that can be shared).
    pub fn save(&self) -> anyhow::Result<()> {
        super::toml_io::save(&paths::orders_path(), self, "orders.toml")
    }
}

/// Order-line styles SEPARATELY for dark and light themes (per theme). Stored in one `orders.toml`
/// with two tables, `[dark]` / `[light]`. The current application theme selects the active set.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct OrdersStyleSet {
    pub dark: OrdersStyle,
    pub light: OrdersStyle,
}

impl Default for OrdersStyleSet {
    fn default() -> Self {
        // Defaults = the user's current sets: dark plus its OWN light set (baked from orders.toml).
        Self {
            dark: OrdersStyle::default(),
            light: OrdersStyle::default_light(),
        }
    }
}

impl OrdersStyleSet {
    /// Set for the active theme: `light=true` → light, otherwise dark.
    pub fn get(&self, light: bool) -> &OrdersStyle {
        if light {
            &self.light
        } else {
            &self.dark
        }
    }
    pub fn get_mut(&mut self, light: bool) -> &mut OrdersStyle {
        if light {
            &mut self.light
        } else {
            &mut self.dark
        }
    }

    /// Reads `orders.toml`. The new format uses `[dark]`/`[light]` tables. An OLD flat
    /// `OrdersStyle` (without them) becomes `dark`, while `light` uses its own default;
    /// the result is immediately saved again.
    /// A missing file yields and saves the default; a corrupt file yields the default without failing.
    pub fn load() -> Self {
        let path = paths::orders_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            let def = Self::default();
            let _ = def.save();
            return def;
        };
        // New format: a theme table is present. Early development builds migrated old flat
        // orders.toml files identically into `[dark]` and `[light]`, making lines almost invisible
        // in the light theme. When such a clone is found, keep dark as the user's set and reset
        // light to the light default.
        if text.contains("[dark") || text.contains("[light") {
            let mut set: Self = toml::from_str(&text).unwrap_or_else(|e| {
                log::warn!("orders.toml повреждён ({e}); беру дефолт");
                Self::default()
            });
            if set.light == set.dark {
                set.light = OrdersStyle::default_light();
                let _ = set.save();
            }
            return set;
        }
        // Old flat file → take the dark set from the file and the light set from its own default.
        let flat: OrdersStyle = toml::from_str(&text).unwrap_or_default();
        let set = Self {
            light: OrdersStyle::default_light(),
            dark: flat,
        };
        let _ = set.save();
        set
    }

    pub fn save(&self) -> anyhow::Result<()> {
        super::toml_io::save(&paths::orders_path(), self, "orders.toml")
    }

    /// Text in orders.toml format for "Copy" in Settings (= file contents).
    pub fn to_share_string(&self) -> Option<String> {
        toml::to_string_pretty(self).ok()
    }

    /// Parses orders.toml text (clipboard paste / file contents). Validates using distinctive
    /// lines; serde ignores unknown fields and would silently produce the default for a foreign file.
    /// An old flat `OrdersStyle` replaces `dark` in `current` (as in the load migration).
    /// `None` means the text is not an order-line style configuration.
    pub fn parse_share(text: &str, current: &Self) -> Option<Self> {
        const KEYS: [&str; 4] = ["buy", "sell", "stop", "take_profit"];
        let v: toml::Value = toml::from_str(text).ok()?;
        let table_has = |name: &str| {
            v.get(name)
                .and_then(|x| x.as_table())
                .is_some_and(|t| KEYS.iter().any(|k| t.contains_key(*k)))
        };
        if table_has("dark") || table_has("light") {
            return toml::from_str(text).ok();
        }
        if v.as_table()
            .is_some_and(|t| KEYS.iter().any(|k| t.contains_key(*k)))
        {
            let flat: OrdersStyle = toml::from_str(text).ok()?;
            return Some(Self {
                dark: flat,
                light: current.light.clone(),
            });
        }
        None
    }
}
