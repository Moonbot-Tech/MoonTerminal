use super::*;

use super::format::{caption_prefix, volume_key};

/// One caption of a preview line: what it prints and how it is styled.
///
/// The editor draws these itself — it is a dialog, not a chart — but it must not FORMAT them
/// itself: a preview built from its own spelling of "how a percentage looks" stops being a preview
/// the first time the real formatter changes.
pub(crate) struct PreviewCaption {
    /// Whether this line belongs to a COLUMN — an arbitrage venue — which stacks whatever the
    /// module's flow says. The preview has to know, or it shows a row of venues the chart will
    /// never draw.
    pub column: bool,
    /// The caption's prefix, drawn in the theme's colour beside the value — the same split the
    /// chart draws, so a preview cannot claim a caption prints something the chart does not.
    pub prefix: String,
    pub text: String,
    pub sign: Option<DeltaSign>,
    pub style: moon_core::config::ResolvedLabelStyle,
}

/// Format one row against SAMPLE values, for the editor's "how it will look" line.
///
/// Sample rather than live values, deliberately: the editor is where a caption is CHOSEN, and a
/// figure the current market happens not to have — no position, no funding on a spot market —
/// would print nothing and read as "this label is broken". Every field answers here.
pub(crate) fn preview_row(
    row: &moon_core::config::ChartLabelRow,
    chart_tf_ms: i64,
) -> Vec<PreviewCaption> {
    // A module switched off prints nothing, and the sample says so rather than showing what it
    // WOULD print: the editor's line answers "what will the chart show".
    if !row.is_drawn() {
        return Vec::new();
    }
    let mut inputs = sample_inputs();
    // The one input that is NOT a sample: a countdown set to `Авто` names the timeframe it will
    // actually follow. Previewing the sample's own would print a period the chart does not use, on
    // the single setting whose whole purpose is to track the chart.
    inputs.chart_tf_ms = chart_tf_ms;
    // One sample reading under every span this module asks for. Without it a caption set to "the
    // last 500 trades" would preview as blank — the editor would say the label is broken when it is
    // merely custom.
    // The sample answers as if the pointer were on the plot, so a measuring caption previews its
    // figure rather than its dash.
    inputs.cursor_ms = Some(0);
    let sample_keys: Vec<_> = row.parts[..row.used_parts()]
        .iter()
        .filter(|part| part.field.reads_volume())
        .filter_map(|part| volume_key(&inputs, part))
        .collect();
    for key in sample_keys {
        // Filed under the key this caption will actually LOOK UP — anchor included. Filing every
        // sample at the live edge left a measuring caption previewing as the dash it prints with no
        // pointer, which tells the reader nothing about what they just picked.
        if !inputs.volumes.iter().any(|(held, _)| *held == key) {
            inputs.volumes.push((key, sample_volume()));
            inputs.liquidations.push((key, sample_liquidations()));
        }
    }
    let preview_roster = preview_roster();
    let mut out = Vec::new();
    if row.show_name {
        let title = crate::controls::row_title(row);
        if let Some(title) = title {
            out.push(PreviewCaption {
                column: false,
                prefix: String::new(),
                text: title,
                sign: None,
                style: moon_core::config::ChartLabelRow::name_style(),
            });
        }
    }
    let mut column_drawn = false;
    for part in &row.parts[..row.used_parts()] {
        if !part.visible {
            continue;
        }
        // A column caption previews as the COLUMN it prints — same expansion the chart uses, same
        // sample data — because "what will this print" is a list of venues, not one line saying
        // "arbitrage". The first visible column owns the range; a second one is ignored, matching
        // LabelState::update.
        if part.field.is_column() {
            if column_drawn {
                continue;
            }
            column_drawn = true;
            let base = part.resolved_style();
            let mut lines = Vec::new();
            match part.field {
                ChartLabelField::StrategyFilters => {
                    push_filter_rows(&mut lines, 0, row, &inputs.filter_lines, 0)
                }
                _ => push_arb_rows(&mut lines, 0, &inputs, Some(&preview_roster), base, 0),
            }
            out.extend(lines.into_iter().map(|line| PreviewCaption {
                column: true,
                prefix: line.prefix,
                text: line.text,
                sign: line.sign,
                style: if line.part == FILTER_HEADER_PART {
                    moon_core::config::ChartLabelRow::name_style()
                } else {
                    moon_core::config::ResolvedLabelStyle {
                        color: match line.color {
                            Some(rgb) => moon_core::config::LabelColor::Fixed(rgb),
                            None => base.color,
                        },
                        ..base
                    }
                },
            }));
            continue;
        }
        if let Some((text, sign)) = resolve(part, &inputs) {
            let style = part.resolved_style();
            out.push(PreviewCaption {
                column: false,
                prefix: caption_prefix(part, style.caption, inputs.chart_tf_ms),
                text,
                sign,
                style,
            });
        }
    }
    out
}

/// The market the preview describes: one coin, in profit on the hour and down on the day, with two
/// orders open at a small loss.
///
/// Chosen so every figure has a value AND a sign — a preview where everything is positive hides
/// what the by-sign colour mode does.
/// The roster the preview arranges its sample column by: the shipped one.
fn preview_roster() -> ArbViewCfg {
    ArbViewCfg::default()
}

/// One period's traded amounts, for the editor's preview.
///
/// Lopsided on purpose: equal halves would draw two identical bars and hide what the bar is for.
fn sample_volume() -> VolumeSpanReadout {
    VolumeSpanReadout {
        buy_quote: 12_700.0,
        sell_quote: 3_500.0,
        buy_base: 0.24,
        sell_base: 0.07,
        trades: 418,
        complete: true,
        base_exact: true,
        total_quote_candles: None,
    }
}

/// One period's liquidations, for the editor's preview.
fn sample_liquidations() -> LiqSpanReadout {
    LiqSpanReadout {
        quote: 4_200.0,
        base: 0.08,
        count: 3,
        complete: true,
    }
}

fn sample_inputs() -> LabelInputs {
    let stats = BasisStats {
        open_orders: 2,
        pos_size: 0.35,
        spent: 100.0,
        pnl_quote: -4.11,
        exposure: 495.96,
        has_exposure: true,
        has_position: true,
    };
    LabelInputs {
        ticker: "BTC-USDT".to_string(),
        core_name: "Core-1".to_string(),
        venue: "Binance".to_string(),
        quote: "USDT".to_string(),
        strategy: "Alpha".to_string(),
        detect_strategy: "BTC Sniper".to_string(),
        detect_msg: "Delta 5m 3.4% · vol x7".to_string(),
        filter_lines: vec![
            "EMA_01 (EMA) : EMA filter not passed".to_string(),
            "HOOK_01 (MoonHook) : Daily vol. doesnt match".to_string(),
        ],
        // The editor previews every field on ONE sample, the trade captions included: a reader
        // configuring the trade window's module has to see what it will print, and a preview that
        // left them blank would read as a module that prints nothing.
        trade: Some(Rc::new(crate::chartdx::TradeLabels {
            strategy: "Hook Short".to_string(),
            detect: "Hook Short Depth: 2.5% R: 120% VolK: 19.5".to_string(),
            sell_reason: "Auto Price Down".to_string(),
        })),
        last_price: Some(51234.5),
        scale_badge: Some(12),
        time_scale_s: Some(6_720),
        compare_pct: Some(1.2),
        delta_1h: Some(3.8),
        delta_24h: Some(-2.1),
        // Both sample venues count as connected, so the preview shows the column the way a
        // configured terminal sees it rather than dimmed throughout.
        arb_reachable: vec![(4, String::new()), (9, String::new())],
        // The sample column: two venues, one above this market and one below it, so the preview
        // shows both directions the spread can take.
        arb: vec![
            moon_core::market::ArbQuote {
                venue: moon_core::market::ArbVenue::from_code(4),
                dex_name: String::new(),
                price: 51_290.0,
                my_price: 51_234.5,
                spread_pct: 0.11,
                deposit_blocked: false,
                withdraw_blocked: false,
            },
            moon_core::market::ArbQuote {
                venue: moon_core::market::ArbVenue::from_code(9),
                dex_name: String::new(),
                price: 51_180.0,
                my_price: 51_234.5,
                spread_pct: -0.11,
                deposit_blocked: true,
                withdraw_blocked: false,
            },
        ],
        // Every new figure answers here too: a preview that printed nothing for a field the user
        // just picked reads as a broken label rather than as an empty market.
        figures: Some(moon_core::market::MarketFiguresReadout {
            bid: Some(51_230.0),
            ask: Some(51_239.0),
            mark: Some(51_236.0),
            price_step: Some(0.5),
            vol_24h: Some(184_000_000.0),
            max_leverage: Some(50),
            max_order: moon_core::market::MaxOrder {
                value: 2_000_000.0,
                source: moon_core::market::MaxOrderSource::Stated,
            },
            tags: vec![
                moon_core::market::CoinTag::Seed,
                moon_core::market::CoinTag::Alpha,
            ],
            pos_size: Some(0.35),
            liq_price: Some(41_120.0),
            leverage_x: Some(10),
            isolated: Some(true),
            core_pnl: Some(-12.40),
            session: Some(48.15),
            coin_balance: Some(0.42),
        }),
        windows: Some(moon_core::market::MarketWindowsReadout {
            windows: [moon_core::market::WindowFigures {
                delta_pct: Some(0.58),
            }; moon_core::config::LABEL_WINDOW_COUNT],
        }),
        // Filled per ROW by `preview_row`: a span can be any number of minutes or trades, so the
        // sample cannot enumerate them — it answers whichever ones the module actually asks for.
        volumes: Vec::new(),
        liquidations: Vec::new(),
        // Filled by `preview_row` beside the volumes, for the same reason.
        cursor_ms: None,
        context: Some(moon_core::market::MarketContextReadout {
            exchange_1h_pct: 0.4,
            exchange_24h_pct: -1.1,
            btc_1h_pct: 0.9,
            btc_24h_pct: -0.6,
            btc_72h_pct: 4.75,
            funding_pct: Some(0.01),
            funding_at_ms: Some(5 * 3_600_000 + 33 * 60_000),
        }),
        // The countdowns are measured against this, so the funding pair prints a fixed `5ч 33м`.
        // A candle countdown lands on a bucket boundary here and previews its FULL period, which is
        // the widest string it can print — the preview is also how a reader judges whether the
        // caption fits where they are putting it.
        now_ms: 0,
        // The sample chart is on the five-minute timeframe, so an `Авто` caption previews `5м`.
        chart_tf_ms: 5 * 60_000,
        basis: [stats; 3],
        // The buttons preview LIVE and pressable: a reader placing one has to see what it will
        // carry, and a disabled sample would show them a state they will never configure. The ban
        // previews as RUNNING for the same reason every other optional figure previews with a
        // value — the lock shows closed, and the readout beside it counts `4ч 12м` down.
        actions: ActionInputs {
            live: true,
            allowed: true,
            panic_armed: false,
            ban_until_ms: Some(4 * 3_600_000 + 12 * 60_000),
            favorite: None,
        },
        column_scroll: Vec::new(),
    }
}
