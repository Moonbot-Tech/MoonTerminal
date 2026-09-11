//! Resolves confirmed, core-local strategy overrides without changing the global order styles.

use std::collections::HashMap;

use moon_core::feed::{StrategyRow, StrategySchemaModel};
use moon_core::session::order_lines::{LineKind, RetainedOrder};

/// Optional valid fields of an enabled strategy; malformed fields retain the global setting.
#[derive(Default)]
pub(super) struct StrategyStyle {
    buy: Option<[f32; 4]>,
    sell: Option<[f32; 4]>,
    pub pattern: Option<f32>,
}

impl StrategyStyle {
    /// Applies the strategy's ARGB color while retaining the caller's lifecycle opacity.
    pub fn color(&self, kind: LineKind, fallback: [f32; 4]) -> [f32; 4] {
        let custom = match kind {
            LineKind::Buy => self.buy,
            LineKind::Sell => self.sell,
            _ => None,
        };
        custom.map_or(fallback, |mut color| {
            color[3] *= fallback[3];
            color
        })
    }
}

/// Indexed once per rebuild, including disabled strategies so an ID match stays authoritative.
pub(super) struct StrategyStyles {
    ids: HashMap<u64, StrategyStyle>,
    names: HashMap<String, Option<u64>>,
}

impl StrategyStyles {
    /// Parses the snapshot supplied by the same core that owns the order store.
    pub fn new(rows: &[StrategyRow], schema: Option<&StrategySchemaModel>) -> Self {
        let mut ids = HashMap::new();
        let mut names = HashMap::new();
        for row in rows {
            // Moonbot omits fields equal to their kind's schema defaults.
            let defaults =
                schema.and_then(|s| s.kinds.iter().find(|k| k.ordinal == row.kind_ordinal));
            let field = |name| {
                row.fields
                    .iter()
                    .find(|(k, _)| k == name)
                    .map(|(_, v)| v.trim())
                    .or_else(|| {
                        let field = defaults?
                            .sections
                            .iter()
                            .flat_map(|s| &s.fields)
                            .find(|f| f.name == name)?;
                        field
                            .default
                            .as_deref()
                            .map(str::trim)
                            // Without FLAG_DEFAULT_NZ, this numeric schema field defaults to zero.
                            .or_else(|| (name == "OrderLineKind").then_some("0"))
                    })
            };
            let enabled = field("UseCustomColors").is_some_and(|v| {
                v.eq_ignore_ascii_case("yes") || v.eq_ignore_ascii_case("true") || v == "1"
            });
            let style = if enabled {
                StrategyStyle {
                    buy: field("BuyOrderColor").and_then(parse_color),
                    sell: field("SellOrderColor").and_then(parse_color),
                    pattern: field("OrderLineKind")
                        .and_then(|v| v.parse::<u8>().ok())
                        .filter(|v| *v <= 4)
                        .map(f32::from),
                }
            } else {
                StrategyStyle::default()
            };
            ids.insert(row.id, style);
            names
                .entry(row.name.clone())
                .and_modify(|id| *id = None)
                .or_insert(Some(row.id));
        }
        Self { ids, names }
    }

    /// A supplied ID is authoritative; only ID-less orders may use a unique, nonempty name.
    pub fn get(&self, order: &RetainedOrder) -> Option<&StrategyStyle> {
        let id = if order.strat_id != 0 {
            order.strat_id
        } else if !order.strat_name.is_empty() {
            self.names.get(&order.strat_name).copied().flatten()?
        } else {
            return None;
        };
        self.ids.get(&id)
    }
}

/// Accepts RGB or Moonbot ARGB hex, rejecting malformed input instead of inventing a color.
fn parse_color(value: &str) -> Option<[f32; 4]> {
    let hex = value.strip_prefix('#').unwrap_or(value);
    if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(hex, 16).ok()?;
    Some([
        ((n >> 16) & 255) as f32 / 255.0,
        ((n >> 8) & 255) as f32 / 255.0,
        (n & 255) as f32 / 255.0,
        if hex.len() == 8 {
            (n >> 24) as f32 / 255.0
        } else {
            1.0
        },
    ])
}
