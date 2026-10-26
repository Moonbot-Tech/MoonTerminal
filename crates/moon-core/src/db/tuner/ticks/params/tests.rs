//! The knobs and what writing one moves.

use super::*;

/// Omitting this model-only key would silently lose the switch on historical buys.
#[test]
fn price_down_allowed_drop_switch_is_fetched_and_parsed() {
    assert!(
        param_keys()
            .iter()
            .any(|key| key == "PriceDownToAllowedDrop")
    );
    for spelling in ["YES", "True", "1"] {
        let values = [("PriceDownToAllowedDrop".into(), spelling.into())].into();
        let p = exit_params(
            &StrategyValues {
                values: &values,
                defaults: &HashMap::new(),
            },
            ModelSettings::default(),
        );
        assert!(p.price_down_to_allowed_drop, "{spelling}");
    }
    let p = exit_params(
        &StrategyValues {
            values: &HashMap::new(),
            defaults: &HashMap::new(),
        },
        ModelSettings::default(),
    );
    assert!(!p.price_down_to_allowed_drop);
}

/// Neutral model fallbacks would disable PriceDown or replay an omitted sell field incorrectly.
#[test]
fn omitted_sell_fields_use_the_core_defaults_without_a_schema() {
    let values = [("PriceDownTimer".into(), "1".into())].into();
    let defaults = HashMap::new();
    let p = exit_params(
        &StrategyValues {
            values: &values,
            defaults: &defaults,
        },
        ModelSettings::default(),
    );
    assert!(p.price_down_on());
    assert_eq!(p.sell_price_pct, 0.5);
    assert_eq!(p.price_down_pct, 0.2);
    assert_eq!(p.price_down_delay_s, 10.0);
    assert_eq!(p.price_down_allowed_drop_pct, 0.1);
    assert_eq!(p.sell_level_time_s, 3600.0);
    assert_eq!(p.sell_level_adjust_pct, 0.2);
    assert_eq!(p.sell_level_count, 1);
    assert_eq!(p.sell_level_allowed_drop_pct, 0.1);
}

/// Hardcoded core defaults must not override schema defaults or explicitly stored sell fields.
#[test]
fn sell_fields_prefer_explicit_values_then_schema_defaults() {
    let fields = [
        "SellPrice",
        "PriceDownPercent",
        "PriceDownDelay",
        "PriceDownAllowedDrop",
        "SellLevelTime",
        "SellLevelAdjust",
        "SellLevelCount",
        "SellLevelAllowedDrop",
    ];
    let defaults = fields
        .iter()
        .map(|key| (key.to_ascii_lowercase(), 7.0))
        .collect();
    for explicit in [false, true] {
        let values = if explicit {
            fields
                .iter()
                .map(|key| ((*key).into(), "9".into()))
                .collect()
        } else {
            HashMap::new()
        };
        let p = exit_params(
            &StrategyValues {
                values: &values,
                defaults: &defaults,
            },
            ModelSettings::default(),
        );
        let expected = if explicit { 9.0 } else { 7.0 };
        assert_eq!(
            [
                p.sell_price_pct,
                p.price_down_pct,
                p.price_down_delay_s,
                p.price_down_allowed_drop_pct,
                p.sell_level_time_s,
                p.sell_level_adjust_pct,
                f64::from(p.sell_level_count),
                p.sell_level_allowed_drop_pct,
            ],
            [expected; 8]
        );
    }
}

/// Losing the KB default or minimum would keep canceled MoonShot entries fillable.
#[test]
fn moonshot_auto_cancel_buy_uses_default_minimum_and_disable() {
    let defaults = HashMap::new();
    for (value, expected) in [
        (None, 300.0),
        (Some("1"), 15.0),
        (Some("20"), 20.0),
        (Some("0"), 0.0),
    ] {
        let values = value
            .map(|v| ("AutoCancelBuy".into(), v.into()))
            .into_iter()
            .collect();
        let params = mshot_params(
            &StrategyValues {
                values: &values,
                defaults: &defaults,
            },
            ModelSettings::default(),
        );
        assert_eq!(params.auto_cancel_buy_s, expected);
    }
}

/// Treating a PostOnly rejection as a tape fill would invent an entry verdict.
#[test]
fn moonshot_with_post_only_leaves_entry_unjudged() {
    let defaults = HashMap::new();
    for (value, unjudged) in [("YES", true), ("NO", false)] {
        let values = [("UsePostOnly".into(), value.into())].into();
        let params = entry_params_for(
            KIND_MOONSHOT,
            &StrategyValues {
                values: &values,
                defaults: &defaults,
            },
            ModelSettings::default(),
        );
        assert_eq!(matches!(params, EntryParams::Fact), unjudged);
        assert_eq!(matches!(params, EntryParams::MoonShot(_)), !unjudged);
    }
}

/// Replaying an inter-exchange adjustment without its history would invent an entry verdict.
#[test]
fn moonshot_with_arbitrage_range_leaves_entry_unjudged() {
    let defaults = HashMap::new();
    for (value, unjudged) in [("0.1", true), ("0", false)] {
        let values = [("MShotAddArbRange".into(), value.into())].into();
        let params = entry_params_for(
            KIND_MOONSHOT,
            &StrategyValues {
                values: &values,
                defaults: &defaults,
            },
            ModelSettings::default(),
        );
        assert_eq!(matches!(params, EntryParams::Fact), unjudged);
        assert_eq!(matches!(params, EntryParams::MoonShot(_)), !unjudged);
    }
    let keys = param_keys();
    for key in ["AutoCancelBuy", "MShotAddArbRange", "UsePostOnly"] {
        assert_eq!(keys.iter().filter(|k| k.as_str() == key).count(), 1);
    }
}

/// Reading a falling BTC delta with its sign would pull the MoonShot corridor nearer.
#[test]
fn moonshot_reads_btc_delta_as_a_magnitude() {
    let values = [
        ("MShotAddBTCDelta".into(), "0.5".into()),
        ("MShotPriceMin".into(), "1".into()),
        ("MShotPrice".into(), "2".into()),
    ]
    .into();
    let defaults = HashMap::new();
    let params = mshot_params(
        &StrategyValues {
            values: &values,
            defaults: &defaults,
        },
        ModelSettings::default(),
    );
    assert_eq!(
        params
            .bounds_pct(&super::super::Deltas {
                btc1h: -2.0,
                ..Default::default()
            })
            .0,
        2.0
    );
}

/// Omitted dump fields use the absolute step, while an explicit relative step survives parsing.
#[test]
fn price_down_relative_uses_the_core_default_when_omitted() {
    let defaults = HashMap::new();
    for (raw, expected) in [
        (r#"{"StrategyName":"UNIT_01","Coins":"XALPHA"}"#, false),
        (
            r#"{"StrategyName":"UNIT_01","Coins":"XALPHA","PriceDownRelative":true}"#,
            true,
        ),
    ] {
        let json: HashMap<String, serde_json::Value> = serde_json::from_str(raw).unwrap();
        let values = json
            .into_iter()
            .map(|(key, value)| (key, value.to_string()))
            .collect();
        let strategy = StrategyValues {
            values: &values,
            defaults: &defaults,
        };
        assert_eq!(
            exit_params(&strategy, ModelSettings::default()).price_down_relative,
            expected,
            "{raw}"
        );
    }
}

/// A write's corridor warning must key on every field that moves a MoonShot's corridor: the
/// Entry group, and `MaxModifier` of the Exit group, which caps the `MShotAdd*` sum too — and on
/// no field the corridor does not read.
#[test]
fn max_modifier_moves_the_entry_like_the_entry_fields() {
    assert!(moves_entry("MaxModifier"));
    assert!(moves_entry("MShotPrice") && moves_entry("MShotAddHourlyDelta"));
    assert!(!moves_entry("SellModifier") && !moves_entry("Add1minDelta"));
    assert!(!moves_entry("SellPrice"));
    // The shared field is read by the entry builder: were it not, the warning would be noise.
    let values: HashMap<String, String> = [
        ("MaxModifier".to_string(), "2".to_string()),
        ("MShotAddHourlyDelta".to_string(), "1".to_string()),
    ]
    .into();
    let defaults = HashMap::new();
    let sv = StrategyValues {
        values: &values,
        defaults: &defaults,
    };
    assert_eq!(
        mshot_params(&sv, ModelSettings::default()).max_modifier,
        2.0
    );
}

/// `MStrikeSellLevel` is read into the exit parameters, as `HookSellLevel` is for a hook.
#[test]
fn mstrike_sell_level_maps_to_exit_params() {
    let values: HashMap<String, String> =
        [("MStrikeSellLevel".to_string(), "60".to_string())].into();
    let defaults = HashMap::new();
    let sv = StrategyValues {
        values: &values,
        defaults: &defaults,
    };
    assert_eq!(
        exit_params(&sv, ModelSettings::default()).strike_sell_level_pct,
        60.0
    );
}

/// A strike's grid offers its own level and not `SellPrice`, which it does not carry.
#[test]
fn strike_grid_shows_mstrike_sell_level_not_sell_price() {
    let strike: Vec<&str> = params_for(ParamGroup::Exit, super::super::strike::KIND_MOONSTRIKE)
        .map(|p| p.key)
        .collect();
    assert!(strike.contains(&"MStrikeSellLevel"));
    assert!(!strike.contains(&"SellPrice"));
    let pump: Vec<&str> = params_for(ParamGroup::Exit, "PumpsDetection")
        .map(|p| p.key)
        .collect();
    assert!(!pump.contains(&"MStrikeSellLevel"), "a strike-only field");
}

/// The panic sell's bounds are read with the strategy's values, defaults when absent.
#[test]
fn exit_params_reads_the_panic_sell_bounds() {
    let values: HashMap<String, String> = [
        ("AllowedDrop", "-12"),
        ("AllowedDrop3", "-20"),
        ("UseMarketOrder", "1"),
        ("StopLossSpread", "0.4"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let defaults = HashMap::new();
    let sv = StrategyValues {
        values: &values,
        defaults: &defaults,
    };
    let p = exit_params(&sv, ModelSettings::default());
    assert_eq!(p.allowed_drop_pct, -12.0);
    assert_eq!(p.allowed_drop3_pct, -20.0);
    assert!(p.use_market_order);
    assert_eq!(p.stop_loss_spread_pct, 0.4);
    let empty = HashMap::new();
    let p = exit_params(
        &StrategyValues {
            values: &empty,
            defaults: &defaults,
        },
        ModelSettings::default(),
    );
    let base = ExitParams::default();
    assert_eq!(p.allowed_drop_pct, base.allowed_drop_pct);
    assert_eq!(p.allowed_drop3_pct, base.allowed_drop3_pct);
    assert_eq!(p.use_market_order, base.use_market_order);
    assert_eq!(p.stop_loss_spread_pct, base.stop_loss_spread_pct);
}

/// Hash iteration must never select an alias over the canonical source during projection/save.
#[test]
fn formula_lookup_prefers_canonical_key() {
    for _ in 0..64 {
        let own = HashMap::from([
            ("sellbycustomema".into(), "EMA(5,1)<9".into()),
            ("SellByCustomEMA".into(), "EMA(2,1)>1".into()),
        ]);
        assert_eq!(formula_slot_values(&own)["SellByCustomEMA#1"], 1.0);
        assert_eq!(
            fold_formula_edits(&own, &[("SellByCustomEMA#1".into(), "2".into())]),
            [("SellByCustomEMA".into(), "EMA(2,1)>2".into())]
        );
        let edits = vec![
            ("SellByCustomEMA".into(), "EMA(3,1)>3".into()),
            ("sellbycustomema".into(), "EMA(5,1)<9".into()),
            ("SellByCustomEMA#1".into(), "4".into()),
        ];
        assert_eq!(
            fold_formula_edits(&own, &edits),
            [("SellByCustomEMA".into(), "EMA(3,1)>4".into())]
        );
        assert_eq!(
            formula_slot_values(&HashMap::from([(
                "sellbycustomema".into(),
                "EMA(5,1)>6".into()
            )]))["SellByCustomEMA#1"],
            6.0
        );
    }
}

/// Losing model-only strings or accepting unsupported syntax would silently disable the exit.
#[test]
fn exit_params_reads_custom_ema_and_delay() {
    use crate::db::tuner::ticks::custom_ema::Refusal;
    let defaults = HashMap::new();
    for (text, parsed, refusal) in [
        ("EMA(2s,1s) > 0.5", true, None),
        ("   ", false, None),
        ("Avg(2s,1s) > 0.5", false, Some(Refusal::Unsupported)),
        ("Max(2s,1s) > 0.5", false, Some(Refusal::Unparsed)),
        ("garbage", false, Some(Refusal::Unparsed)),
    ] {
        let values = HashMap::from([
            ("SellByCustomEMA".into(), text.into()),
            ("SellEMADelay".into(), "1.25".into()),
        ]);
        let p = exit_params(
            &StrategyValues {
                values: &values,
                defaults: &defaults,
            },
            ModelSettings::default(),
        );
        assert_eq!(p.custom_ema.is_some(), parsed);
        assert_eq!(p.custom_ema_refusal, refusal);
        assert_eq!(p.custom_ema_delay_s, 1.25);
        if let Some(formula) = p.custom_ema {
            assert_eq!(formula.terms[0].window_s, 2);
        }
    }
    let keys = param_keys();
    assert!(keys.iter().any(|key| key == "SellByCustomEMA"));
    assert!(keys.iter().any(|key| key == "SellEMADelay"));
}
