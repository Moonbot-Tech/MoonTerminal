//! Strategies parameter labels implementation.

/// Return the locale keys attached to an exact raw strategy field name.
///
/// ONE registry rather than a help table beside a label table: both answer the same question about
/// the same identity, and two of them would have to be corrected in step forever. The tuple is
/// `(help key, label key)`, and the label is optional because a field whose meaning the repository
/// cannot state honestly gets none -- 22 of the 216 today. A wrong label on a trading field is
/// worse than no label, so those keep the raw identifier and nothing else changes for them.
///
/// This is an INDEX of what the repository has evidence for, never a catalogue of what can arrive:
/// the schema streams at runtime, so an unknown name yields `None` and the row renders exactly as
/// it did before this table existed.
///
/// Args:
///     raw_name: Case-sensitive strategy field name supplied by the runtime schema.
///
/// Returns:
///     `(optional help key, optional label key)` when the field is known, else `None`. A field
///     whose only evidence is the strategy replica carries a label and no help.
pub(super) fn field_keys(raw_name: &str) -> Option<(Option<&'static str>, Option<&'static str>)> {
    match raw_name {
        "AddToChart" => Some((
            Some("strat.field.AddToChart"),
            Some("strat.label.AddToChart"),
        )),
        "AllowedDrop" => Some((
            Some("strat.field.AllowedDrop"),
            Some("strat.label.AllowedDrop"),
        )),
        "AllowedDrop3" => Some((
            Some("strat.field.AllowedDrop3"),
            Some("strat.label.AllowedDrop3"),
        )),
        "AutoBuy" => Some((Some("strat.field.AutoBuy"), Some("strat.label.AutoBuy"))),
        "AutoCancelBuy" => Some((
            Some("strat.field.AutoCancelBuy"),
            Some("strat.label.AutoCancelBuy"),
        )),
        "AutoCancelLowerBuy" => Some((
            Some("strat.field.AutoCancelLowerBuy"),
            Some("strat.label.AutoCancelLowerBuy"),
        )),
        "AutoSell" => Some((Some("strat.field.AutoSell"), Some("strat.label.AutoSell"))),
        "BinancePriceBug" => Some((
            Some("strat.field.BinancePriceBug"),
            Some("strat.label.BinancePriceBug"),
        )),
        "BinancePriceBugMin" => Some((
            Some("strat.field.BinancePriceBugMin"),
            Some("strat.label.BinancePriceBugMin"),
        )),
        "BinanceTokenTags" => Some((
            Some("strat.field.BinanceTokenTags"),
            Some("strat.label.BinanceTokenTags"),
        )),
        "BuyDelay" => Some((Some("strat.field.BuyDelay"), Some("strat.label.BuyDelay"))),
        "BuyOrderColor" => Some((
            Some("strat.field.BuyOrderColor"),
            Some("strat.label.BuyOrderColor"),
        )),
        "buyPrice" => Some((Some("strat.field.buyPrice"), Some("strat.label.buyPrice"))),
        "buyPriceAbsolute" => Some((
            Some("strat.field.buyPriceAbsolute"),
            Some("strat.label.buyPriceAbsolute"),
        )),
        "BuyPriceStep" => Some((
            Some("strat.field.BuyPriceStep"),
            Some("strat.label.BuyPriceStep"),
        )),
        "BuyStepKind" => Some((
            Some("strat.field.BuyStepKind"),
            Some("strat.label.BuyStepKind"),
        )),
        "BuyType" => Some((Some("strat.field.BuyType"), Some("strat.label.BuyType"))),
        "BV_SV_FilterRatio" => Some((
            Some("strat.field.BV_SV_FilterRatio"),
            Some("strat.label.BV_SV_FilterRatio"),
        )),
        "BV_SV_FilterRatioMax" => Some((
            Some("strat.field.BV_SV_FilterRatioMax"),
            Some("strat.label.BV_SV_FilterRatioMax"),
        )),
        "BV_SV_Kind" => Some((
            Some("strat.field.BV_SV_Kind"),
            Some("strat.label.BV_SV_Kind"),
        )),
        "BV_SV_Ratio" => Some((
            Some("strat.field.BV_SV_Ratio"),
            Some("strat.label.BV_SV_Ratio"),
        )),
        "BV_SV_Reverse" => Some((
            Some("strat.field.BV_SV_Reverse"),
            Some("strat.label.BV_SV_Reverse"),
        )),
        "BV_SV_TakeProfit" => Some((
            Some("strat.field.BV_SV_TakeProfit"),
            Some("strat.label.BV_SV_TakeProfit"),
        )),
        "BV_SV_TradesN" => Some((
            Some("strat.field.BV_SV_TradesN"),
            Some("strat.label.BV_SV_TradesN"),
        )),
        "CancelBuyAfterSell" => Some((
            Some("strat.field.CancelBuyAfterSell"),
            Some("strat.label.CancelBuyAfterSell"),
        )),
        "CancelBuyStep" => Some((
            Some("strat.field.CancelBuyStep"),
            Some("strat.label.CancelBuyStep"),
        )),
        "CheckFreeBalance" => Some((
            Some("strat.field.CheckFreeBalance"),
            Some("strat.label.CheckFreeBalance"),
        )),
        "Comment" => Some((Some("strat.field.Comment"), Some("strat.label.Comment"))),
        "CustomEMA" => Some((Some("strat.field.CustomEMA"), Some("strat.label.CustomEMA"))),
        "Delta_24h_Max" => Some((
            Some("strat.field.Delta_24h_Max"),
            Some("strat.label.Delta_24h_Max"),
        )),
        "Delta_24h_Min" => Some((
            Some("strat.field.Delta_24h_Min"),
            Some("strat.label.Delta_24h_Min"),
        )),
        "Delta_3h_Max" => Some((
            Some("strat.field.Delta_3h_Max"),
            Some("strat.label.Delta_3h_Max"),
        )),
        "Delta_3h_Min" => Some((
            Some("strat.field.Delta_3h_Min"),
            Some("strat.label.Delta_3h_Min"),
        )),
        "Delta_BTC_1m_Max" => Some((
            Some("strat.field.Delta_BTC_1m_Max"),
            Some("strat.label.Delta_BTC_1m_Max"),
        )),
        "Delta_BTC_1m_Min" => Some((
            Some("strat.field.Delta_BTC_1m_Min"),
            Some("strat.label.Delta_BTC_1m_Min"),
        )),
        "Delta_BTC_24_Max" => Some((
            Some("strat.field.Delta_BTC_24_Max"),
            Some("strat.label.Delta_BTC_24_Max"),
        )),
        "Delta_BTC_24_Min" => Some((
            Some("strat.field.Delta_BTC_24_Min"),
            Some("strat.label.Delta_BTC_24_Min"),
        )),
        "Delta_BTC_5m_Max" => Some((
            Some("strat.field.Delta_BTC_5m_Max"),
            Some("strat.label.Delta_BTC_5m_Max"),
        )),
        "Delta_BTC_5m_Min" => Some((
            Some("strat.field.Delta_BTC_5m_Min"),
            Some("strat.label.Delta_BTC_5m_Min"),
        )),
        "Delta_BTC_Max" => Some((
            Some("strat.field.Delta_BTC_Max"),
            Some("strat.label.Delta_BTC_Max"),
        )),
        "Delta_BTC_Min" => Some((
            Some("strat.field.Delta_BTC_Min"),
            Some("strat.label.Delta_BTC_Min"),
        )),
        "Delta_Market_24_Max" => Some((
            Some("strat.field.Delta_Market_24_Max"),
            Some("strat.label.Delta_Market_24_Max"),
        )),
        "Delta_Market_24_Min" => Some((
            Some("strat.field.Delta_Market_24_Min"),
            Some("strat.label.Delta_Market_24_Min"),
        )),
        "Delta_Market_Max" => Some((
            Some("strat.field.Delta_Market_Max"),
            Some("strat.label.Delta_Market_Max"),
        )),
        "Delta_Market_Min" => Some((
            Some("strat.field.Delta_Market_Min"),
            Some("strat.label.Delta_Market_Min"),
        )),
        "Delta2_Max" => Some((
            Some("strat.field.Delta2_Max"),
            Some("strat.label.Delta2_Max"),
        )),
        "Delta2_Min" => Some((
            Some("strat.field.Delta2_Min"),
            Some("strat.label.Delta2_Min"),
        )),
        "Delta2_Type" => Some((
            Some("strat.field.Delta2_Type"),
            Some("strat.label.Delta2_Type"),
        )),
        "Delta3_Max" => Some((
            Some("strat.field.Delta3_Max"),
            Some("strat.label.Delta3_Max"),
        )),
        "Delta3_Min" => Some((
            Some("strat.field.Delta3_Min"),
            Some("strat.label.Delta3_Min"),
        )),
        "Delta3_Type" => Some((
            Some("strat.field.Delta3_Type"),
            Some("strat.label.Delta3_Type"),
        )),
        "DeltaSwitch" => Some((
            Some("strat.field.DeltaSwitch"),
            Some("strat.label.DeltaSwitch"),
        )),
        "DontKeepOrdersOnChart" => Some((
            Some("strat.field.DontKeepOrdersOnChart"),
            Some("strat.label.DontKeepOrdersOnChart"),
        )),
        "DontSellBelowLiq" => Some((
            Some("strat.field.DontSellBelowLiq"),
            Some("strat.label.DontSellBelowLiq"),
        )),
        "DontWriteLog" => Some((
            Some("strat.field.DontWriteLog"),
            Some("strat.label.DontWriteLog"),
        )),
        "EmulatorMode" => Some((
            Some("strat.field.EmulatorMode"),
            Some("strat.label.EmulatorMode"),
        )),
        "FastStopLoss" => Some((
            Some("strat.field.FastStopLoss"),
            Some("strat.label.FastStopLoss"),
        )),
        "FilterBy" => Some((Some("strat.field.FilterBy"), Some("strat.label.FilterBy"))),
        "FilterMax" => Some((Some("strat.field.FilterMax"), Some("strat.label.FilterMax"))),
        "FilterMin" => Some((Some("strat.field.FilterMin"), Some("strat.label.FilterMin"))),
        "FundingAfter" => Some((
            Some("strat.field.FundingAfter"),
            Some("strat.label.FundingAfter"),
        )),
        "FundingBefore" => Some((
            Some("strat.field.FundingBefore"),
            Some("strat.label.FundingBefore"),
        )),
        "GlobalDetectPenalty" => Some((
            Some("strat.field.GlobalDetectPenalty"),
            Some("strat.label.GlobalDetectPenalty"),
        )),
        "GlobalFilterPenalty" => Some((
            Some("strat.field.GlobalFilterPenalty"),
            Some("strat.label.GlobalFilterPenalty"),
        )),
        "HFT" => Some((Some("strat.field.HFT"), Some("strat.label.HFT"))),
        "HODLmode" => Some((Some("strat.field.HODLmode"), Some("strat.label.HODLmode"))),
        "IgnoreBase" => Some((
            Some("strat.field.IgnoreBase"),
            Some("strat.label.IgnoreBase"),
        )),
        "IgnoreCancelBuy" => Some((
            Some("strat.field.IgnoreCancelBuy"),
            Some("strat.label.IgnoreCancelBuy"),
        )),
        "IgnoreDelta" => Some((
            Some("strat.field.IgnoreDelta"),
            Some("strat.label.IgnoreDelta"),
        )),
        "IgnoreFilters" => Some((
            Some("strat.field.IgnoreFilters"),
            Some("strat.label.IgnoreFilters"),
        )),
        "IgnorePing" => Some((
            Some("strat.field.IgnorePing"),
            Some("strat.label.IgnorePing"),
        )),
        "IgnorePrice" => Some((
            Some("strat.field.IgnorePrice"),
            Some("strat.label.IgnorePrice"),
        )),
        "IgnoreSellShot" => Some((
            Some("strat.field.IgnoreSellShot"),
            Some("strat.label.IgnoreSellShot"),
        )),
        "IgnoreSellSpread" => Some((
            Some("strat.field.IgnoreSellSpread"),
            Some("strat.label.IgnoreSellSpread"),
        )),
        "IgnoreSession" => Some((
            Some("strat.field.IgnoreSession"),
            Some("strat.label.IgnoreSession"),
        )),
        "IgnoreTime" => Some((
            Some("strat.field.IgnoreTime"),
            Some("strat.label.IgnoreTime"),
        )),
        "IgnoreVolume" => Some((
            Some("strat.field.IgnoreVolume"),
            Some("strat.label.IgnoreVolume"),
        )),
        "JoinPriceFixed" => Some((
            Some("strat.field.JoinPriceFixed"),
            Some("strat.label.JoinPriceFixed"),
        )),
        "JoinSellKey" => Some((
            Some("strat.field.JoinSellKey"),
            Some("strat.label.JoinSellKey"),
        )),
        "KeepAlert" => Some((Some("strat.field.KeepAlert"), Some("strat.label.KeepAlert"))),
        "KeepInChart" => Some((
            Some("strat.field.KeepInChart"),
            Some("strat.label.KeepInChart"),
        )),
        "LastEditDate" => Some((
            Some("strat.field.LastEditDate"),
            Some("strat.label.LastEditDate"),
        )),
        "MarketStopLevel" => Some((
            Some("strat.field.MarketStopLevel"),
            Some("strat.label.MarketStopLevel"),
        )),
        "MarkPriceMax" => Some((
            Some("strat.field.MarkPriceMax"),
            Some("strat.label.MarkPriceMax"),
        )),
        "MarkPriceMin" => Some((
            Some("strat.field.MarkPriceMin"),
            Some("strat.label.MarkPriceMin"),
        )),
        "MaxActiveOrders" => Some((
            Some("strat.field.MaxActiveOrders"),
            Some("strat.label.MaxActiveOrders"),
        )),
        "MaxBalance" => Some((
            Some("strat.field.MaxBalance"),
            Some("strat.label.MaxBalance"),
        )),
        "MaxHourlyVolFast" => Some((
            Some("strat.field.MaxHourlyVolFast"),
            Some("strat.label.MaxHourlyVolFast"),
        )),
        "MaxHourlyVolume" => Some((
            Some("strat.field.MaxHourlyVolume"),
            Some("strat.label.MaxHourlyVolume"),
        )),
        "MaxLatency" => Some((
            Some("strat.field.MaxLatency"),
            Some("strat.label.MaxLatency"),
        )),
        "MaxLeverage" => Some((
            Some("strat.field.MaxLeverage"),
            Some("strat.label.MaxLeverage"),
        )),
        "MaxMarkets" => Some((
            Some("strat.field.MaxMarkets"),
            Some("strat.label.MaxMarkets"),
        )),
        "MaxOrdersPerMarket" => Some((
            Some("strat.field.MaxOrdersPerMarket"),
            Some("strat.label.MaxOrdersPerMarket"),
        )),
        "MaxPing" => Some((Some("strat.field.MaxPing"), Some("strat.label.MaxPing"))),
        "MaxPosition" => Some((
            Some("strat.field.MaxPosition"),
            Some("strat.label.MaxPosition"),
        )),
        "MaxVolume" => Some((Some("strat.field.MaxVolume"), Some("strat.label.MaxVolume"))),
        "MinFreeBalance" => Some((
            Some("strat.field.MinFreeBalance"),
            Some("strat.label.MinFreeBalance"),
        )),
        "MinHourlyVolFast" => Some((
            Some("strat.field.MinHourlyVolFast"),
            Some("strat.label.MinHourlyVolFast"),
        )),
        "MinHourlyVolume" => Some((
            Some("strat.field.MinHourlyVolume"),
            Some("strat.label.MinHourlyVolume"),
        )),
        "MinLeverage" => Some((
            Some("strat.field.MinLeverage"),
            Some("strat.label.MinLeverage"),
        )),
        "MinPing" => Some((Some("strat.field.MinPing"), Some("strat.label.MinPing"))),
        "MinuteVolDeltaMax" => Some((
            Some("strat.field.MinuteVolDeltaMax"),
            Some("strat.label.MinuteVolDeltaMax"),
        )),
        "MinuteVolDeltaMin" => Some((
            Some("strat.field.MinuteVolDeltaMin"),
            Some("strat.label.MinuteVolDeltaMin"),
        )),
        "MinVolume" => Some((Some("strat.field.MinVolume"), Some("strat.label.MinVolume"))),
        "MoonIntRiskLevel" => Some((
            Some("strat.field.MoonIntRiskLevel"),
            Some("strat.label.MoonIntRiskLevel"),
        )),
        "MoonIntStopLevel" => Some((
            Some("strat.field.MoonIntStopLevel"),
            Some("strat.label.MoonIntStopLevel"),
        )),
        "OrderLineKind" => Some((
            Some("strat.field.OrderLineKind"),
            Some("strat.label.OrderLineKind"),
        )),
        "OrdersCount" => Some((
            Some("strat.field.OrdersCount"),
            Some("strat.label.OrdersCount"),
        )),
        "OrderSize" => Some((Some("strat.field.OrderSize"), Some("strat.label.OrderSize"))),
        "OrderSizeKind" => Some((
            Some("strat.field.OrderSizeKind"),
            Some("strat.label.OrderSizeKind"),
        )),
        "OrderSizeStep" => Some((
            Some("strat.field.OrderSizeStep"),
            Some("strat.label.OrderSizeStep"),
        )),
        "PenaltyTime" => Some((
            Some("strat.field.PenaltyTime"),
            Some("strat.label.PenaltyTime"),
        )),
        "PriceDownAllowedDrop" => Some((
            Some("strat.field.PriceDownAllowedDrop"),
            Some("strat.label.PriceDownAllowedDrop"),
        )),
        "PriceDownDelay" => Some((
            Some("strat.field.PriceDownDelay"),
            Some("strat.label.PriceDownDelay"),
        )),
        "PriceDownPercent" => Some((
            Some("strat.field.PriceDownPercent"),
            Some("strat.label.PriceDownPercent"),
        )),
        "PriceDownRelative" => Some((
            Some("strat.field.PriceDownRelative"),
            Some("strat.label.PriceDownRelative"),
        )),
        "PriceDownTimer" => Some((
            Some("strat.field.PriceDownTimer"),
            Some("strat.label.PriceDownTimer"),
        )),
        "PriceStepMax" => Some((
            Some("strat.field.PriceStepMax"),
            Some("strat.label.PriceStepMax"),
        )),
        "PriceStepMin" => Some((
            Some("strat.field.PriceStepMin"),
            Some("strat.label.PriceStepMin"),
        )),
        "PriceToSwitch2Stop" => Some((
            Some("strat.field.PriceToSwitch2Stop"),
            Some("strat.label.PriceToSwitch2Stop"),
        )),
        "PriceToSwitchStop3" => Some((
            Some("strat.field.PriceToSwitchStop3"),
            Some("strat.label.PriceToSwitchStop3"),
        )),
        "SamePosition" => Some((
            Some("strat.field.SamePosition"),
            Some("strat.label.SamePosition"),
        )),
        "SecondStopLoss" => Some((
            Some("strat.field.SecondStopLoss"),
            Some("strat.label.SecondStopLoss"),
        )),
        "SellByCustomEMA" => Some((
            Some("strat.field.SellByCustomEMA"),
            Some("strat.label.SellByCustomEMA"),
        )),
        "SellByFilters" => Some((
            Some("strat.field.SellByFilters"),
            Some("strat.label.SellByFilters"),
        )),
        "SellDelay" => Some((Some("strat.field.SellDelay"), Some("strat.label.SellDelay"))),
        "SellEMACheckEnter" => Some((
            Some("strat.field.SellEMACheckEnter"),
            Some("strat.label.SellEMACheckEnter"),
        )),
        "SellEMADelay" => Some((
            Some("strat.field.SellEMADelay"),
            Some("strat.label.SellEMADelay"),
        )),
        "SellFromAssets" => Some((
            Some("strat.field.SellFromAssets"),
            Some("strat.label.SellFromAssets"),
        )),
        "SellLevelAdjust" => Some((
            Some("strat.field.SellLevelAdjust"),
            Some("strat.label.SellLevelAdjust"),
        )),
        "SellLevelAllowedDrop" => Some((
            Some("strat.field.SellLevelAllowedDrop"),
            Some("strat.label.SellLevelAllowedDrop"),
        )),
        "SellLevelCount" => Some((
            Some("strat.field.SellLevelCount"),
            Some("strat.label.SellLevelCount"),
        )),
        "SellLevelDelay" => Some((
            Some("strat.field.SellLevelDelay"),
            Some("strat.label.SellLevelDelay"),
        )),
        "SellLevelDelayNext" => Some((
            Some("strat.field.SellLevelDelayNext"),
            Some("strat.label.SellLevelDelayNext"),
        )),
        "SellLevelRelative" => Some((
            Some("strat.field.SellLevelRelative"),
            Some("strat.label.SellLevelRelative"),
        )),
        "SellLevelTime" => Some((
            Some("strat.field.SellLevelTime"),
            Some("strat.label.SellLevelTime"),
        )),
        "SellLevelWorkTime" => Some((
            Some("strat.field.SellLevelWorkTime"),
            Some("strat.label.SellLevelWorkTime"),
        )),
        "SellOrderColor" => Some((
            Some("strat.field.SellOrderColor"),
            Some("strat.label.SellOrderColor"),
        )),
        "SellPrice" => Some((Some("strat.field.SellPrice"), Some("strat.label.SellPrice"))),
        "SellPriceAbsolute" => Some((
            Some("strat.field.SellPriceAbsolute"),
            Some("strat.label.SellPriceAbsolute"),
        )),
        "SellQuantity" => Some((
            Some("strat.field.SellQuantity"),
            Some("strat.label.SellQuantity"),
        )),
        "SellShotAllowedDown" => Some((
            Some("strat.field.SellShotAllowedDown"),
            Some("strat.label.SellShotAllowedDown"),
        )),
        "SellShotAllowedUp" => Some((
            Some("strat.field.SellShotAllowedUp"),
            Some("strat.label.SellShotAllowedUp"),
        )),
        "SellShotCalcInterval" => Some((
            Some("strat.field.SellShotCalcInterval"),
            Some("strat.label.SellShotCalcInterval"),
        )),
        "SellShotCorridor" => Some((
            Some("strat.field.SellShotCorridor"),
            Some("strat.label.SellShotCorridor"),
        )),
        "SellShotDelay" => Some((
            Some("strat.field.SellShotDelay"),
            Some("strat.label.SellShotDelay"),
        )),
        "SellShotDistance" => Some((
            Some("strat.field.SellShotDistance"),
            Some("strat.label.SellShotDistance"),
        )),
        "SellShotPriceDown" => Some((
            Some("strat.field.SellShotPriceDown"),
            Some("strat.label.SellShotPriceDown"),
        )),
        "SellShotPriceDownDelay" => Some((
            Some("strat.field.SellShotPriceDownDelay"),
            Some("strat.label.SellShotPriceDownDelay"),
        )),
        "SellShotRaiseWait" => Some((
            Some("strat.field.SellShotRaiseWait"),
            Some("strat.label.SellShotRaiseWait"),
        )),
        "SellShotReplaceDelay" => Some((
            Some("strat.field.SellShotReplaceDelay"),
            Some("strat.label.SellShotReplaceDelay"),
        )),
        "SellSpreadAllowedDrop" => Some((
            Some("strat.field.SellSpreadAllowedDrop"),
            Some("strat.label.SellSpreadAllowedDrop"),
        )),
        "SellSpreadCalcInterval" => Some((
            Some("strat.field.SellSpreadCalcInterval"),
            Some("strat.label.SellSpreadCalcInterval"),
        )),
        "SellSpreadDelay" => Some((
            Some("strat.field.SellSpreadDelay"),
            Some("strat.label.SellSpreadDelay"),
        )),
        "SellSpreadDistance" => Some((
            Some("strat.field.SellSpreadDistance"),
            Some("strat.label.SellSpreadDistance"),
        )),
        "SellSpreadMinSpread" => Some((
            Some("strat.field.SellSpreadMinSpread"),
            Some("strat.label.SellSpreadMinSpread"),
        )),
        "SellSpreadReplaceCount" => Some((
            Some("strat.field.SellSpreadReplaceCount"),
            Some("strat.label.SellSpreadReplaceCount"),
        )),
        "SessionIncreaseOrder" => Some((
            Some("strat.field.SessionIncreaseOrder"),
            Some("strat.label.SessionIncreaseOrder"),
        )),
        "SessionIncreaseOrderMax" => Some((
            Some("strat.field.SessionIncreaseOrderMax"),
            Some("strat.label.SessionIncreaseOrderMax"),
        )),
        "SessionLevelsUSDT" => Some((
            Some("strat.field.SessionLevelsUSDT"),
            Some("strat.label.SessionLevelsUSDT"),
        )),
        "SessionMinusCount" => Some((
            Some("strat.field.SessionMinusCount"),
            Some("strat.label.SessionMinusCount"),
        )),
        "SessionPenaltyTime" => Some((
            Some("strat.field.SessionPenaltyTime"),
            Some("strat.label.SessionPenaltyTime"),
        )),
        "SessionPlusCount" => Some((
            Some("strat.field.SessionPlusCount"),
            Some("strat.label.SessionPlusCount"),
        )),
        "SessionProfitMax" => Some((
            Some("strat.field.SessionProfitMax"),
            Some("strat.label.SessionProfitMax"),
        )),
        "SessionProfitMin" => Some((
            Some("strat.field.SessionProfitMin"),
            Some("strat.label.SessionProfitMin"),
        )),
        "SessionReduceOrder" => Some((
            Some("strat.field.SessionReduceOrder"),
            Some("strat.label.SessionReduceOrder"),
        )),
        "SessionReduceOrderMin" => Some((
            Some("strat.field.SessionReduceOrderMin"),
            Some("strat.label.SessionReduceOrderMin"),
        )),
        "SessionResetOnMinus" => Some((
            Some("strat.field.SessionResetOnMinus"),
            Some("strat.label.SessionResetOnMinus"),
        )),
        "SessionResetTime" => Some((
            Some("strat.field.SessionResetTime"),
            Some("strat.label.SessionResetTime"),
        )),
        "SessionStratIncreaseMax" => Some((
            Some("strat.field.SessionStratIncreaseMax"),
            Some("strat.label.SessionStratIncreaseMax"),
        )),
        "SessionStratMax" => Some((
            Some("strat.field.SessionStratMax"),
            Some("strat.label.SessionStratMax"),
        )),
        "SessionStratMin" => Some((
            Some("strat.field.SessionStratMin"),
            Some("strat.label.SessionStratMin"),
        )),
        "SessionStratReduceMin" => Some((
            Some("strat.field.SessionStratReduceMin"),
            Some("strat.label.SessionStratReduceMin"),
        )),
        "Short" => Some((Some("strat.field.Short"), Some("strat.label.Short"))),
        "SignalType" => Some((
            Some("strat.field.SignalType"),
            Some("strat.label.SignalType"),
        )),
        "SoundAlert" => Some((
            Some("strat.field.SoundAlert"),
            Some("strat.label.SoundAlert"),
        )),
        "SoundKind" => Some((Some("strat.field.SoundKind"), Some("strat.label.SoundKind"))),
        "SplitPiece" => Some((
            Some("strat.field.SplitPiece"),
            Some("strat.label.SplitPiece"),
        )),
        "StopAboveLiq" => Some((
            Some("strat.field.StopAboveLiq"),
            Some("strat.label.StopAboveLiq"),
        )),
        "StopLoss" => Some((Some("strat.field.StopLoss"), Some("strat.label.StopLoss"))),
        "StopLoss3" => Some((Some("strat.field.StopLoss3"), Some("strat.label.StopLoss3"))),
        "StopLossDelay" => Some((
            Some("strat.field.StopLossDelay"),
            Some("strat.label.StopLossDelay"),
        )),
        "StopLossEMA" => Some((
            Some("strat.field.StopLossEMA"),
            Some("strat.label.StopLossEMA"),
        )),
        "StopLossFixed" => Some((
            Some("strat.field.StopLossFixed"),
            Some("strat.label.StopLossFixed"),
        )),
        "StopLossModifier" => Some((
            Some("strat.field.StopLossModifier"),
            Some("strat.label.StopLossModifier"),
        )),
        "StopLossSpread" => Some((
            Some("strat.field.StopLossSpread"),
            Some("strat.label.StopLossSpread"),
        )),
        "StopSpreadAdd1mDelta" => Some((
            Some("strat.field.StopSpreadAdd1mDelta"),
            Some("strat.label.StopSpreadAdd1mDelta"),
        )),
        "StrategyName" => Some((
            Some("strat.field.StrategyName"),
            Some("strat.label.StrategyName"),
        )),
        "TakeProfit" => Some((
            Some("strat.field.TakeProfit"),
            Some("strat.label.TakeProfit"),
        )),
        "TimeToSwitch2Stop" => Some((
            Some("strat.field.TimeToSwitch2Stop"),
            Some("strat.label.TimeToSwitch2Stop"),
        )),
        "TimeToSwitchStop3" => Some((
            Some("strat.field.TimeToSwitchStop3"),
            Some("strat.label.TimeToSwitchStop3"),
        )),
        "TlgBuyDipPrice" => Some((
            Some("strat.field.TlgBuyDipPrice"),
            Some("strat.label.TlgBuyDipPrice"),
        )),
        "TlgUseBuyDipWords" => Some((
            Some("strat.field.TlgUseBuyDipWords"),
            Some("strat.label.TlgUseBuyDipWords"),
        )),
        "TotalLoss" => Some((Some("strat.field.TotalLoss"), Some("strat.label.TotalLoss"))),
        "TradePenaltyTime" => Some((
            Some("strat.field.TradePenaltyTime"),
            Some("strat.label.TradePenaltyTime"),
        )),
        "TrailingEMA" => Some((
            Some("strat.field.TrailingEMA"),
            Some("strat.label.TrailingEMA"),
        )),
        "TrailingPercent" => Some((
            Some("strat.field.TrailingPercent"),
            Some("strat.label.TrailingPercent"),
        )),
        "TrailingSpread" => Some((
            Some("strat.field.TrailingSpread"),
            Some("strat.label.TrailingSpread"),
        )),
        "Use30SecOldASK" => Some((
            Some("strat.field.Use30SecOldASK"),
            Some("strat.label.Use30SecOldASK"),
        )),
        "UseBTCPriceStep" => Some((
            Some("strat.field.UseBTCPriceStep"),
            Some("strat.label.UseBTCPriceStep"),
        )),
        "UseBV_SV_Filter" => Some((
            Some("strat.field.UseBV_SV_Filter"),
            Some("strat.label.UseBV_SV_Filter"),
        )),
        "UseBV_SV_Stop" => Some((
            Some("strat.field.UseBV_SV_Stop"),
            Some("strat.label.UseBV_SV_Stop"),
        )),
        "UseCustomColors" => Some((
            Some("strat.field.UseCustomColors"),
            Some("strat.label.UseCustomColors"),
        )),
        "UseMarketStop" => Some((
            Some("strat.field.UseMarketStop"),
            Some("strat.label.UseMarketStop"),
        )),
        "UseOldPrice" => Some((
            Some("strat.field.UseOldPrice"),
            Some("strat.label.UseOldPrice"),
        )),
        "UsePostOnly" => Some((
            Some("strat.field.UsePostOnly"),
            Some("strat.label.UsePostOnly"),
        )),
        "UseScalpingMode" => Some((
            Some("strat.field.UseScalpingMode"),
            Some("strat.label.UseScalpingMode"),
        )),
        "UseSecondStop" => Some((
            Some("strat.field.UseSecondStop"),
            Some("strat.label.UseSecondStop"),
        )),
        "UseStopLoss" => Some((
            Some("strat.field.UseStopLoss"),
            Some("strat.label.UseStopLoss"),
        )),
        "UseStopLoss3" => Some((
            Some("strat.field.UseStopLoss3"),
            Some("strat.label.UseStopLoss3"),
        )),
        "UseTakeProfit" => Some((
            Some("strat.field.UseTakeProfit"),
            Some("strat.label.UseTakeProfit"),
        )),
        "UseTrailing" => Some((
            Some("strat.field.UseTrailing"),
            Some("strat.label.UseTrailing"),
        )),
        "WorkingPriceMax" => Some((
            Some("strat.field.WorkingPriceMax"),
            Some("strat.label.WorkingPriceMax"),
        )),
        "WorkingPriceMin" => Some((
            Some("strat.field.WorkingPriceMin"),
            Some("strat.label.WorkingPriceMin"),
        )),
        "WorkingTime" => Some((
            Some("strat.field.WorkingTime"),
            Some("strat.label.WorkingTime"),
        )),
        "WorkingWeekTime" => Some((
            Some("strat.field.WorkingWeekTime"),
            Some("strat.label.WorkingWeekTime"),
        )),
        "ActiveTrigger" => Some((None, Some("strat.label.ActiveTrigger"))),
        "Add15minDelta" => Some((None, Some("strat.label.Add15minDelta"))),
        "Add1minDelta" => Some((None, Some("strat.label.Add1minDelta"))),
        "Add3hDelta" => Some((None, Some("strat.label.Add3hDelta"))),
        "Add5minDelta" => Some((None, Some("strat.label.Add5minDelta"))),
        "AddBTC1mDelta" => Some((None, Some("strat.label.AddBTC1mDelta"))),
        "AddBTC5mDelta" => Some((None, Some("strat.label.AddBTC5mDelta"))),
        "AddBTCDelta" => Some((None, Some("strat.label.AddBTCDelta"))),
        "AddDump1h" => Some((None, Some("strat.label.AddDump1h"))),
        "AddHourlyDelta" => Some((None, Some("strat.label.AddHourlyDelta"))),
        "AddMarketDelta" => Some((None, Some("strat.label.AddMarketDelta"))),
        "AddPriceBug" => Some((None, Some("strat.label.AddPriceBug"))),
        "AddPump1h" => Some((None, Some("strat.label.AddPump1h"))),
        "BuyModifier" => Some((None, Some("strat.label.BuyModifier"))),
        "BuyOrderReduce" => Some((None, Some("strat.label.BuyOrderReduce"))),
        "BuyPriceInSpread" => Some((None, Some("strat.label.BuyPriceInSpread"))),
        "CheckAfterBuy" => Some((None, Some("strat.label.CheckAfterBuy"))),
        "CoinsBlackList" => Some((None, Some("strat.label.CoinsBlackList"))),
        "CoinsWhiteList" => Some((None, Some("strat.label.CoinsWhiteList"))),
        "DeltaInterval" => Some((None, Some("strat.label.DeltaInterval"))),
        "DeltaLastPrice" => Some((None, Some("strat.label.DeltaLastPrice"))),
        "DeltaMin" => Some((None, Some("strat.label.DeltaMin"))),
        "DeltaPrice" => Some((None, Some("strat.label.DeltaPrice"))),
        "DeltaShortInterval" => Some((None, Some("strat.label.DeltaShortInterval"))),
        "DeltaVol" => Some((None, Some("strat.label.DeltaVol"))),
        "DeltaVolRaise" => Some((None, Some("strat.label.DeltaVolRaise"))),
        "DeltaVolSec" => Some((None, Some("strat.label.DeltaVolSec"))),
        "DetectModifier" => Some((None, Some("strat.label.DetectModifier"))),
        "DontTradeListing" => Some((None, Some("strat.label.DontTradeListing"))),
        "DropsLastPriceMA" => Some((None, Some("strat.label.DropsLastPriceMA"))),
        "DropsMaxTime" => Some((None, Some("strat.label.DropsMaxTime"))),
        "DropsPriceDelta" => Some((None, Some("strat.label.DropsPriceDelta"))),
        "DropsPriceIsLow" => Some((None, Some("strat.label.DropsPriceIsLow"))),
        "DropsPriceMA" => Some((None, Some("strat.label.DropsPriceMA"))),
        "DropsUseLastPrice" => Some((None, Some("strat.label.DropsUseLastPrice"))),
        "DynBL_SortBy" => Some((None, Some("strat.label.DynBL_SortBy"))),
        "DynBL_SortDesc" => Some((None, Some("strat.label.DynBL_SortDesc"))),
        "DynWL_Count" => Some((None, Some("strat.label.DynWL_Count"))),
        "DynWL_SortBy" => Some((None, Some("strat.label.DynWL_SortBy"))),
        "DynWL_SortDesc" => Some((None, Some("strat.label.DynWL_SortDesc"))),
        "Dyn_Refresh" => Some((None, Some("strat.label.Dyn_Refresh"))),
        "FastShotAlgo" => Some((None, Some("strat.label.FastShotAlgo"))),
        "HookAntiPump" => Some((None, Some("strat.label.HookAntiPump"))),
        "HookDetectDepth" => Some((None, Some("strat.label.HookDetectDepth"))),
        "HookDetectDepthMax" => Some((None, Some("strat.label.HookDetectDepthMax"))),
        "HookDetectMinVolume" => Some((None, Some("strat.label.HookDetectMinVolume"))),
        "HookDirection" => Some((None, Some("strat.label.HookDirection"))),
        "HookDropMax" => Some((None, Some("strat.label.HookDropMax"))),
        "HookDropMin" => Some((None, Some("strat.label.HookDropMin"))),
        "HookInitialPrice" => Some((None, Some("strat.label.HookInitialPrice"))),
        "HookInterpolate" => Some((None, Some("strat.label.HookInterpolate"))),
        "HookOppositeOrder" => Some((None, Some("strat.label.HookOppositeOrder"))),
        "HookPartFilledDelay" => Some((None, Some("strat.label.HookPartFilledDelay"))),
        "HookPriceDistance" => Some((None, Some("strat.label.HookPriceDistance"))),
        "HookPriceRollBack" => Some((None, Some("strat.label.HookPriceRollBack"))),
        "HookPriceRollBackMax" => Some((None, Some("strat.label.HookPriceRollBackMax"))),
        "HookRaiseWait" => Some((None, Some("strat.label.HookRaiseWait"))),
        "HookRepeatAfterSell" => Some((None, Some("strat.label.HookRepeatAfterSell"))),
        "HookRepeatIfProfit" => Some((None, Some("strat.label.HookRepeatIfProfit"))),
        "HookReplaceDelay" => Some((None, Some("strat.label.HookReplaceDelay"))),
        "HookRollBackWait" => Some((None, Some("strat.label.HookRollBackWait"))),
        "HookSellFixed" => Some((None, Some("strat.label.HookSellFixed"))),
        "HookSellLevel" => Some((None, Some("strat.label.HookSellLevel"))),
        "HookTimeFrame" => Some((None, Some("strat.label.HookTimeFrame"))),
        "IndependentSignals" => Some((None, Some("strat.label.IndependentSignals"))),
        "IntervalsForBuySpread" => Some((None, Some("strat.label.IntervalsForBuySpread"))),
        "LiqCount" => Some((None, Some("strat.label.LiqCount"))),
        "LiqDirection" => Some((None, Some("strat.label.LiqDirection"))),
        "LiqSameDirection" => Some((None, Some("strat.label.LiqSameDirection"))),
        "LiqTime" => Some((None, Some("strat.label.LiqTime"))),
        "LiqVolumeMax" => Some((None, Some("strat.label.LiqVolumeMax"))),
        "LiqVolumeMin" => Some((None, Some("strat.label.LiqVolumeMin"))),
        "LiqWaitTime" => Some((None, Some("strat.label.LiqWaitTime"))),
        "LiqWithinTime" => Some((None, Some("strat.label.LiqWithinTime"))),
        "Liq_BV_SV_Filter" => Some((None, Some("strat.label.Liq_BV_SV_Filter"))),
        "Liq_BV_SV_Time" => Some((None, Some("strat.label.Liq_BV_SV_Time"))),
        "ListedType" => Some((None, Some("strat.label.ListedType"))),
        "MShotAdd15minDelta" => Some((None, Some("strat.label.MShotAdd15minDelta"))),
        "MShotAdd1minDelta" => Some((None, Some("strat.label.MShotAdd1minDelta"))),
        "MShotAdd24hDelta" => Some((None, Some("strat.label.MShotAdd24hDelta"))),
        "MShotAdd3hDelta" => Some((None, Some("strat.label.MShotAdd3hDelta"))),
        "MShotAdd5minDelta" => Some((None, Some("strat.label.MShotAdd5minDelta"))),
        "MShotAddBTC5mDelta" => Some((None, Some("strat.label.MShotAddBTC5mDelta"))),
        "MShotAddBTCDelta" => Some((None, Some("strat.label.MShotAddBTCDelta"))),
        "MShotAddDistance" => Some((None, Some("strat.label.MShotAddDistance"))),
        "MShotAddHourlyDelta" => Some((None, Some("strat.label.MShotAddHourlyDelta"))),
        "MShotAddMarkDelta" => Some((None, Some("strat.label.MShotAddMarkDelta"))),
        "MShotAddMarketDelta" => Some((None, Some("strat.label.MShotAddMarketDelta"))),
        "MShotAddPriceBug" => Some((None, Some("strat.label.MShotAddPriceBug"))),
        "MShotMinusSatoshi" => Some((None, Some("strat.label.MShotMinusSatoshi"))),
        "MShotPrice" => Some((None, Some("strat.label.MShotPrice"))),
        "MShotPriceMin" => Some((None, Some("strat.label.MShotPriceMin"))),
        "MShotRaiseWait" => Some((None, Some("strat.label.MShotRaiseWait"))),
        "MShotRepeatAfterBuy" => Some((None, Some("strat.label.MShotRepeatAfterBuy"))),
        "MShotRepeatIfProfit" => Some((None, Some("strat.label.MShotRepeatIfProfit"))),
        "MShotRepeatWait" => Some((None, Some("strat.label.MShotRepeatWait"))),
        "MShotReplaceDelay" => Some((None, Some("strat.label.MShotReplaceDelay"))),
        "MShotSellAtLastPrice" => Some((None, Some("strat.label.MShotSellAtLastPrice"))),
        "MShotSellPriceAdjust" => Some((None, Some("strat.label.MShotSellPriceAdjust"))),
        "MShotSortBy" => Some((None, Some("strat.label.MShotSortBy"))),
        "MShotSortDesc" => Some((None, Some("strat.label.MShotSortDesc"))),
        "MShotUsePrice" => Some((None, Some("strat.label.MShotUsePrice"))),
        "MStrikeAdd15minDelta" => Some((None, Some("strat.label.MStrikeAdd15minDelta"))),
        "MStrikeAddBTCDelta" => Some((None, Some("strat.label.MStrikeAddBTCDelta"))),
        "MStrikeAddHourlyDelta" => Some((None, Some("strat.label.MStrikeAddHourlyDelta"))),
        "MStrikeAddMarketDelta" => Some((None, Some("strat.label.MStrikeAddMarketDelta"))),
        "MStrikeBuyDelay" => Some((None, Some("strat.label.MStrikeBuyDelay"))),
        "MStrikeBuyLevel" => Some((None, Some("strat.label.MStrikeBuyLevel"))),
        "MStrikeBuyRelative" => Some((None, Some("strat.label.MStrikeBuyRelative"))),
        "MStrikeDepth" => Some((None, Some("strat.label.MStrikeDepth"))),
        "MStrikeDirection" => Some((None, Some("strat.label.MStrikeDirection"))),
        "MStrikeSellAdjust" => Some((None, Some("strat.label.MStrikeSellAdjust"))),
        "MStrikeSellLevel" => Some((None, Some("strat.label.MStrikeSellLevel"))),
        "MStrikeVolume" => Some((None, Some("strat.label.MStrikeVolume"))),
        "MStrikeWaitDip" => Some((None, Some("strat.label.MStrikeWaitDip"))),
        "MaxModifier" => Some((None, Some("strat.label.MaxModifier"))),
        "MinReducedSize" => Some((None, Some("strat.label.MinReducedSize"))),
        "NextDetectPenalty" => Some((None, Some("strat.label.NextDetectPenalty"))),
        "PendingOrderSpread" => Some((None, Some("strat.label.PendingOrderSpread"))),
        "PriceIntervalShift" => Some((None, Some("strat.label.PriceIntervalShift"))),
        "PriceIntervals" => Some((None, Some("strat.label.PriceIntervals"))),
        "PriceSpread" => Some((None, Some("strat.label.PriceSpread"))),
        "PriceSpreadMax" => Some((None, Some("strat.label.PriceSpreadMax"))),
        "ReportToTelegram" => Some((None, Some("strat.label.ReportToTelegram"))),
        "ReportTradesToTelegram" => Some((None, Some("strat.label.ReportTradesToTelegram"))),
        "SellModifier" => Some((None, Some("strat.label.SellModifier"))),
        "SellPriceInSpread" => Some((None, Some("strat.label.SellPriceInSpread"))),
        "SilentNoCharts" => Some((None, Some("strat.label.SilentNoCharts"))),
        "SpreadFlat" => Some((None, Some("strat.label.SpreadFlat"))),
        "SpreadPolarityMax" => Some((None, Some("strat.label.SpreadPolarityMax"))),
        "SpreadPolarityMin" => Some((None, Some("strat.label.SpreadPolarityMin"))),
        "SpreadRepeatIfProfit" => Some((None, Some("strat.label.SpreadRepeatIfProfit"))),
        "Spread_BV_SV_Max" => Some((None, Some("strat.label.Spread_BV_SV_Max"))),
        "Spread_BV_SV_Min" => Some((None, Some("strat.label.Spread_BV_SV_Min"))),
        "Spread_BV_SV_Time" => Some((None, Some("strat.label.Spread_BV_SV_Time"))),
        "StrategyPenalty" => Some((None, Some("strat.label.StrategyPenalty"))),
        "TMSameDirection" => Some((None, Some("strat.label.TMSameDirection"))),
        "TimeInterval" => Some((None, Some("strat.label.TimeInterval"))),
        "TradesCountMin" => Some((None, Some("strat.label.TradesCountMin"))),
        "TradesDensity" => Some((None, Some("strat.label.TradesDensity"))),
        "TradesDensityPrev" => Some((None, Some("strat.label.TradesDensityPrev"))),
        "TriggerAllMarkets" => Some((None, Some("strat.label.TriggerAllMarkets"))),
        "TriggerByKey" => Some((None, Some("strat.label.TriggerByKey"))),
        "TriggerKey" => Some((None, Some("strat.label.TriggerKey"))),
        "TriggerKeyBuy" => Some((None, Some("strat.label.TriggerKeyBuy"))),
        "TriggerKeysBL" => Some((None, Some("strat.label.TriggerKeysBL"))),
        "TriggerSeconds" => Some((None, Some("strat.label.TriggerSeconds"))),
        "TriggerSecondsBL" => Some((None, Some("strat.label.TriggerSecondsBL"))),
        "VLiteDelta0" => Some((None, Some("strat.label.VLiteDelta0"))),
        "VLiteMaxP" => Some((None, Some("strat.label.VLiteMaxP"))),
        "VLiteMaxSpike" => Some((None, Some("strat.label.VLiteMaxSpike"))),
        "VLiteP1" => Some((None, Some("strat.label.VLiteP1"))),
        "VLiteP2" => Some((None, Some("strat.label.VLiteP2"))),
        "VLiteP3" => Some((None, Some("strat.label.VLiteP3"))),
        "VLitePDelta2" => Some((None, Some("strat.label.VLitePDelta2"))),
        "VLiteReducedVolumes" => Some((None, Some("strat.label.VLiteReducedVolumes"))),
        "VLiteT0" => Some((None, Some("strat.label.VLiteT0"))),
        "VLiteT1" => Some((None, Some("strat.label.VLiteT1"))),
        "VLiteT2" => Some((None, Some("strat.label.VLiteT2"))),
        "VLiteT3" => Some((None, Some("strat.label.VLiteT3"))),
        "VLiteV1" => Some((None, Some("strat.label.VLiteV1"))),
        "VLiteV2" => Some((None, Some("strat.label.VLiteV2"))),
        "VLiteV3" => Some((None, Some("strat.label.VLiteV3"))),
        "VLiteWeightedAvg" => Some((None, Some("strat.label.VLiteWeightedAvg"))),
        "VolAtMaxP" => Some((None, Some("strat.label.VolAtMaxP"))),
        "VolAtMinP" => Some((None, Some("strat.label.VolAtMinP"))),
        "VolBvLongToDailyMax" => Some((None, Some("strat.label.VolBvLongToDailyMax"))),
        "VolBvLongToDailyMin" => Some((None, Some("strat.label.VolBvLongToDailyMin"))),
        "VolBvLongToHourlyMax" => Some((None, Some("strat.label.VolBvLongToHourlyMax"))),
        "VolBvLongToHourlyMin" => Some((None, Some("strat.label.VolBvLongToHourlyMin"))),
        "VolBvShort" => Some((None, Some("strat.label.VolBvShort"))),
        "VolBvShortToLong" => Some((None, Some("strat.label.VolBvShortToLong"))),
        "VolBvToSvShort" => Some((None, Some("strat.label.VolBvToSvShort"))),
        "VolDeltaAtMaxP" => Some((None, Some("strat.label.VolDeltaAtMaxP"))),
        "VolDeltaAtMinP" => Some((None, Some("strat.label.VolDeltaAtMinP"))),
        "VolLongInterval" => Some((None, Some("strat.label.VolLongInterval"))),
        "VolShortInterval" => Some((None, Some("strat.label.VolShortInterval"))),
        "VolShortPriseRaise" => Some((None, Some("strat.label.VolShortPriseRaise"))),
        "VolSvLong" => Some((None, Some("strat.label.VolSvLong"))),
        "VolTakeLongMaxP" => Some((None, Some("strat.label.VolTakeLongMaxP"))),
        "WavesDelta0" => Some((None, Some("strat.label.WavesDelta0"))),
        "WavesMaxSpike" => Some((None, Some("strat.label.WavesMaxSpike"))),
        "WavesP1" => Some((None, Some("strat.label.WavesP1"))),
        "WavesP2" => Some((None, Some("strat.label.WavesP2"))),
        "WavesP3" => Some((None, Some("strat.label.WavesP3"))),
        "WavesReducedVolumes" => Some((None, Some("strat.label.WavesReducedVolumes"))),
        "WavesT0" => Some((None, Some("strat.label.WavesT0"))),
        "WavesT1" => Some((None, Some("strat.label.WavesT1"))),
        "WavesT2" => Some((None, Some("strat.label.WavesT2"))),
        "WavesT3" => Some((None, Some("strat.label.WavesT3"))),
        "WavesV1" => Some((None, Some("strat.label.WavesV1"))),
        "WavesV2" => Some((None, Some("strat.label.WavesV2"))),
        "WavesV3" => Some((None, Some("strat.label.WavesV3"))),
        "WavesWeightedAvg" => Some((None, Some("strat.label.WavesWeightedAvg"))),
        "volAsksDeep" => Some((None, Some("strat.label.volAsksDeep"))),
        "volBids" => Some((None, Some("strat.label.volBids"))),
        "volBidsDeep" => Some((None, Some("strat.label.volBidsDeep"))),
        "volBidsToAsks" => Some((None, Some("strat.label.volBidsToAsks"))),
        _ => None,
    }
}
