//! Masked writes into the complete safe-share configuration snapshot.

use super::*;

/// Write only the areas `touched` names into a full safe-share snapshot.
///
/// Every other section — including each section's `unknown_tail`, which carries settings written
/// by a newer core than this build knows — travels back untouched, and so does every area this
/// call was not told to touch: two edits queued before either's echo arrives can no longer restore
/// each other's fields, and a mask that never names the manual block cannot reach it at all. A
/// field added to [`CoreConfig`] is still picked up by both the read and the write, so a tab can
/// never send a value the projection cannot show, nor show one it cannot send — the mask narrows
/// WHEN a named field is written, never WHETHER an unnamed one could be.
pub(in crate::feed::live) fn apply_core_config(
    cfg: &mut SharedConfig,
    wanted: &CoreConfig,
    touched: FieldMask,
) {
    // Destructured for the reason `rejection_within_mask` is, and it matters MORE here: a bit with
    // a comparison arm but no applier arm would send every edit naming it, never apply it, and burn
    // the retry budget into a `GaveUp` — the two functions have to grow together.
    let FieldMask {
        auto_buy,
        auto_start,
        btc_blink,
        general,
        gestures,
        interface,
        order_rules,
        leverage,
        signals,
        special,
        telegram,
        ignore_strat_sell_price,
        view,
    } = touched;
    if auto_buy {
        apply_auto_buy(cfg, &wanted.auto_buy);
    }
    if auto_start {
        apply_auto_start(cfg, &wanted.auto_start);
    }
    if btc_blink {
        apply_btc_blink(cfg, &wanted.btc_blink);
    }
    if general {
        apply_general(cfg, &wanted.general);
    }
    if gestures {
        apply_gestures(cfg, &wanted.gestures);
    }
    if interface {
        apply_interface(cfg, &wanted.interface);
    }
    if order_rules {
        apply_order_rules(cfg, &wanted.order_rules);
    }
    if leverage {
        apply_leverage(cfg, &wanted.leverage);
    }
    if signals {
        apply_signals(cfg, &wanted.signals);
    }
    if special {
        apply_special(cfg, &wanted.special);
    }
    if telegram {
        apply_telegram(cfg, &wanted.telegram);
    }
    if ignore_strat_sell_price {
        cfg.trading.ignore_strat_sell_price = wanted.manual.ignore_strat_sell_price;
    }
    if view {
        apply_view(cfg, &wanted.view);
    }
}

/// Apply the two price-approach alerts to the `signals` section.
///
/// Six fields of a section with about a hundred: everything else in it — including its
/// `unknown_tail` — travels back untouched, exactly as `apply_general` leaves the rest of
/// `trading` alone. The connectivity alert that neighbours them on the wire is NOT here: it belongs
/// to [`apply_interface`], the page that draws it.
fn apply_signals(cfg: &mut SharedConfig, s: &SignalsSettings) {
    let sig = &mut cfg.signals;
    sig.play_sell_alert = s.play_sell_alert;
    sig.sell_alert_level = s.sell_alert_level;
    sig.signal_sound_2 = s.signal_sound_2;
    sig.play_buy_alert = s.play_buy_alert;
    sig.buy_alert_level = s.buy_alert_level;
    sig.buy_signal_sound = s.buy_signal_sound;
}

/// Apply the Hotkeys page's mouse-gesture block to `trading.multi_orders`.
///
/// Sixteen of the record's twenty-four fields, plus `trading.pending_order_set_click` beside it.
/// The eight left out are: `join_sell_kind`, which the wire marks a mirror of
/// `ClientSettingsCommand::join_sell_kind`, so it travels on the compact channel too and writing it
/// from here would set two routes fighting over one field; `use_multi_orders`, `split_sells`,
/// `show_orders_num`, `kir_style`, `fix_pos` and `done_opacity`, which are in the record but not on
/// this PAGE — Moonbot draws them with its chart, not with its gestures; and `ver`, which is the
/// wire's own version byte, not a setting.
fn apply_gestures(cfg: &mut SharedConfig, g: &GestureSettings) {
    cfg.trading.pending_order_set_click = g.pending_order_set_click;
    let mo = &mut cfg.trading.multi_orders;
    mo.buy_set_click = g.buy_set_click;
    mo.short_set_click = g.short_set_click;
    mo.pending_short_set_click = g.pending_short_set_click;
    mo.same_hotkeys_for_move = g.same_hotkeys_for_move;
    mo.buy_move_click = g.buy_move_click;
    mo.short_buy_move_click = g.short_buy_move_click;
    mo.replace_buy_kind = g.replace_buy_kind;
    mo.sell_move_click = g.sell_move_click;
    mo.short_sell_move_click = g.short_sell_move_click;
    mo.replace_sell_kind = g.replace_sell_kind;
    mo.buy_move_click_2 = g.buy_move_click_2;
    mo.short_buy_move_click_2 = g.short_buy_move_click_2;
    mo.replace_buy_kind_2 = g.replace_buy_kind_2;
    mo.sell_move_click_2 = g.sell_move_click_2;
    mo.short_sell_move_click_2 = g.short_sell_move_click_2;
    mo.replace_sell_kind_2 = g.replace_sell_kind_2;
}

/// Apply the rest of Moonbot's General page — the rows the compact popup does not draw.
///
/// Fourteen fields of `trading` plus `signals.load_deep_history`, which is the one field of this
/// area outside `trading` and cannot collide with [`apply_signals`]: that applier owns six other
/// fields of the same section and none of them is this one.
fn apply_order_rules(cfg: &mut SharedConfig, r: &OrderRulesSettings) {
    cfg.signals.load_deep_history = r.analyze_on_start;
    let t = &mut cfg.trading;
    t.trailing_float = r.trailing_float;
    t.auto_sell_partial = r.auto_sell_partial;
    t.auto_cancel_buy_order = r.auto_cancel_buy_order;
    t.cancel_buy_on_sell_fill = r.cancel_buy_on_sell_fill;
    t.dont_buy_new_coins = r.dont_buy_new_coins;
    t.deltas_by_trades = r.deltas_by_trades;
    t.sell_auto_move = r.sell_auto_move;
    t.dont_buy_delisted = r.dont_buy_delisted;
    t.manual_mode = r.manual_mode;
    t.use_manual_balance = r.use_manual_balance;
    t.fixed_trade_balance = r.fixed_trade_balance;
    t.play_with = r.play_with;
    t.x_ask = r.x_ask;
    t.use_current_ask = r.use_current_ask;
    t.panic_if_price_drop = r.stop_loss_on;
    t.price_drop_level = r.stop_loss_pct;
}

/// Apply the General tab to the exit rules, iceberg flags and the blacklist's delta filter in
/// `trading`.
///
/// NOT the blacklist itself (`use_coins_black_list` / `coins_black_list_text`), although the
/// projection carries it and the tab draws it. Measured on 2026-09-13
/// (`docs-internal/proto_global_blacklist_shared_config.md`): the core stores what this packet
/// says about the list but never rebuilds its per-market flag from it, so the field is written
/// through the compact channel instead (`shell::core_settings::draft::send_core_config_to`). It
/// also must not be written HERE as well: a page is frozen once its user touches anything, and the
/// list is one string — an OK on the leverage row would carry the frozen list out and paint the
/// core's echo over with a copy that lacks the coin someone else added meanwhile. Left alone, the
/// packet carries what the core holds at build time, compact overlay included.
fn apply_general(cfg: &mut SharedConfig, g: &GeneralSettings) {
    let t = &mut cfg.trading;
    t.use_g_take_profit = g.take_profit_on;
    t.g_take_profit = g.take_profit_pct;
    t.trailing_stop = g.trailing_on;
    t.trailing_drop = g.trailing_pct;
    t.panic_if_vol_drop = g.vstop_on;
    t.vol_drop_level = g.vol_drop_level;
    t.buy_iceberg = g.buy_iceberg;
    t.sell_iceberg = g.sell_iceberg;
    t.exclude_black_list_delta = g.exclude_blacklisted_from_deltas;
}

/// Apply Moonbot's "Специальные" page to `trading`, its `send_shots_config`, `orders_control` and
/// `moonbot_config` sub-records, and `auto_start_2.show_old_listing`.
///
/// Thirty-eight fields across those records: everything else in that section — including its
/// `unknown_tail`, the exits [`apply_general`] owns, the leverage block [`apply_leverage`] owns and
/// the rest of `auto_start_2`, which [`apply_auto_start`] writes field by field — travels back
/// untouched.
///
/// Nothing of the hang watchdog is written here (#1173). `orders_control.sign_orders`,
/// `h_pos_report`, `h_pos_auto_sell` and `trading.h_pos_black_list_text` are the settings the
/// core's observer API carries as `sign_orders` / `report` / `auto_sell` / `blacklist`, and the
/// expert window edits them through that API (`client.watchdogs()`), so a second writer here would
/// let an OK on any Special row put back a value the observer block just changed. Left alone, the
/// packet carries what the core holds at build time — the rule [`apply_general`] follows for the
/// coin blacklist. `active`, `min_price`, `max_time` and `h_pos_control` are legacy mirrors of an
/// observer row and are not written either.
fn apply_special(cfg: &mut SharedConfig, s: &SpecialSettings) {
    cfg.trading.auto_start_2.show_old_listing = s.show_old_listing;
    let mb = &mut cfg.trading.moonbot_config;
    mb.auto_margin_borrow = s.auto_margin_borrow;
    mb.auto_margin_transfer = s.auto_margin_transfer;
    let t = &mut cfg.trading;
    t.free_position_check = s.free_position_check;
    t.binance_connection = s.binance_connection;
    t.use_hl_fast_ioc = s.use_hl_fast_ioc;
    t.log_level = s.log_level;
    t.auto_delete_logs = s.auto_delete_logs;
    t.chart_clean_up_time = s.chart_clean_up_time;
    t.max_orders = s.max_orders;
    t.unlimited_orders = s.unlimited_orders;
    t.random_price = s.random_price;
    t.correct_order_price = s.correct_order_price;
    t.use_book_ticker = s.use_book_ticker;
    t.m_avg_use_vol_weight = s.m_avg_use_vol_weight;
    t.auto_buy_bnb = s.auto_buy_bnb;
    t.auto_buy_bnb_level = s.auto_buy_bnb_level;
    t.auto_buy_bnb_volume = s.auto_buy_bnb_volume;
    t.auto_reduce_order = s.auto_reduce_order;
    t.auto_close_zero_pos = s.auto_close_zero_pos;
    t.auto_lower_lev = s.auto_lower_lev;
    t.use_websocket_api = s.use_websocket_api;
    t.futures_rules = s.futures_rules;
    t.iceberg_step = s.iceberg_step;
    t.sell_x2_level = s.sell_x2_level;
    t.no_trades_markets_text = s.no_trades_markets_text.clone();
    t.multi_commands = s.multi_commands;
    let oc = &mut t.orders_control;
    oc.liq_control = s.liq_control;
    oc.ignore_replacing_bug = s.ignore_replacing_bug;
    oc.ignore_protection = s.ignore_protection;
    let shots = &mut t.send_shots_config;
    shots.may_send = s.send_shots;
    shots.profit_abs = s.profit_abs;
    shots.profit_pers = s.profit_pers;
    shots.profit_session = s.profit_session;
    shots.send_negative = s.send_negative;
    shots.send_public = s.send_public;
    shots.time_scale = s.time_scale;
    shots.price_scale = s.price_scale;
}

/// Apply Moonbot's Telegram page to `signals`, the `trading` fields beside it and two of
/// `trading.moonbot_config`.
///
/// Fifteen fields: everything else in those sections — including their `unknown_tail`s, the alert
/// sounds [`apply_signals`] owns, the message filter [`apply_auto_buy`] owns and the margin pair
/// [`apply_special`] owns in the same `moonbot_config` — travels back untouched.
fn apply_telegram(cfg: &mut SharedConfig, t: &TelegramSettings) {
    let sig = &mut cfg.signals;
    sig.pump_channel = t.pump_channel.clone();
    sig.pump_channels = t.pump_channels.clone();
    sig.multi_channels = t.multi_channels;
    sig.more_then_1_channel = t.more_then_1_channel;
    sig.listen_moon_channel = t.listen_moon_channel;
    let mb = &mut cfg.trading.moonbot_config;
    mb.silent = t.silent;
    mb.group_delay = t.group_delay;
    let tr = &mut cfg.trading;
    tr.use_moon_bl = t.use_moon_bl;
    tr.tm_send_only_share = t.tm_send_only_share;
    tr.tm_panic_sell_closed = t.tm_panic_sell_closed;
    tr.tm_cancel_closed = t.tm_cancel_closed;
    tr.tm_cancel_buy_on_fill = t.tm_cancel_buy_on_fill;
    tr.tm_send_first_sell = t.tm_send_first_sell;
    tr.tm_sell_if_master_not_filled = t.tm_sell_if_master_not_filled;
    tr.tm_max_order = t.tm_max_order;
}

/// Apply Moonbot's autobuy page to `signals`, its `signal_config` sub-record and two `trading`
/// fields.
///
/// Thirty-three fields: everything else in each section — including their `unknown_tail`s and the two
/// price-approach alerts [`apply_signals`] owns — travels back untouched.
fn apply_auto_buy(cfg: &mut SharedConfig, b: &AutoBuySettings) {
    let sig = &mut cfg.signals;
    sig.monitor_clipboard = b.monitor_clipboard;
    sig.clipboard_auto_buy = b.clipboard_auto_buy;
    sig.lower_case_token_cbd = b.lower_case_token_cbd;
    sig.look_full_link_cbd = b.look_full_link_cbd;
    sig.advanced_filter_clipboard = b.advanced_filter_clipboard;
    sig.telegram_auto_buy = b.telegram_auto_buy;
    sig.lower_case_token_tlg = b.lower_case_token_tlg;
    sig.look_full_link_tlg = b.look_full_link_tlg;
    sig.advanced_filter = b.advanced_filter;
    sig.dont_buy_reply = b.dont_buy_reply;
    cfg.trading.dont_buy_forward = b.dont_buy_forward;
    sig.msg_keywords_long = b.msg_keywords_long.clone();
    sig.msg_keywords_short = b.msg_keywords_short.clone();
    sig.msg_black_words = b.msg_black_words.clone();
    sig.msg_token_tags = b.msg_token_tags.clone();
    sig.lower_price_words = b.lower_price_words.clone();
    let c = &mut sig.signal_config;
    c.use_keywords = b.use_keywords;
    c.buy_key_dist = b.buy_key_dist;
    c.use_black_words = b.use_black_words;
    c.use_words_count = b.use_words_count;
    c.words_count = b.words_count;
    c.use_lower_price_words = b.use_lower_price_words;
    c.x_lower_price = b.x_lower_price;
    c.x_found_price = b.x_found_price;
    c.buy_if_price_found = b.buy_if_price_found;
    c.use_price = b.use_price;
    c.use_stops = b.use_stops;
    c.only_1_token = b.only_1_token;
    c.use_token_tags = b.use_token_tags;
    c.tokens_no_tags = b.tokens_no_tags;
    c.token_links = b.token_links;
    c.special_formats = b.special_formats;
    cfg.trading.auto_cancel_lower_buy = b.auto_cancel_lower_buy;
}

/// Apply Moonbot's interface page across the four sections it lives in.
///
/// Forty fields of the several hundred those sections hold: everything else in each of them —
/// including all four `unknown_tail`s — travels back untouched, exactly as `apply_general` leaves
/// the rest of `trading` alone.
///
/// Three of Moonbot's rows on that page are deliberately NOT here, and the page draws them
/// disabled. `trading.use_lev_for_take` and `trading.ignore_strat_sell_price` already belong to
/// [`crate::feed::ManualSettings`], and projecting one wire field into two areas would leave the
/// second stale after a write and make `edit_satisfied` false for any mask naming that area.
/// `visual.manual_charts_full_screen` sits behind that section's tail gate, so a core older than
/// the field reads it back as `false` however it was written — an edit that could never echo, and
/// would burn all three attempts.
fn apply_interface(cfg: &mut SharedConfig, i: &InterfaceSettings) {
    let t = &mut cfg.trading;
    t.buy_on_enter = i.buy_on_enter;
    t.dbl_click_panic_sell = i.dbl_click_panic_sell;
    t.chart_split_zones = i.chart_split_zones;
    t.draw_stop = i.draw_stop;
    t.pending_orders_spread = i.pending_orders_spread;
    t.pending_orders_spread_h_delta = i.pending_orders_spread_h_delta;
    t.pending_buy_price = i.pending_buy_price;
    t.cashback_settings.hide_info = i.hide_cashback_info;
    let v = &mut cfg.visual;
    v.hide_forum_label = i.hide_forum_label;
    v.scrolling_charts = i.scrolling_charts;
    v.startup_load_charts = i.startup_load_charts;
    v.hide_right_chart_panel = i.hide_right_chart_panel;
    v.left_chart_info = i.left_chart_info;
    v.show_iceberg = i.show_iceberg;
    v.show_orders_captions = i.show_orders_captions;
    v.orders_captions_lower = i.orders_captions_lower;
    v.hide_pnl = i.hide_pnl;
    v.hide_buy_button = i.hide_buy_button;
    v.hide_cashback_button = i.hide_cashback_button;
    v.remember_chart_buttons = i.remember_chart_buttons;
    v.show_filters.scale_tool = i.scale_tool;
    v.show_market_captions = i.show_market_captions;
    v.show_usd_on_charts = i.show_usd_on_charts;
    v.show_detects_tool = i.show_detects_tool;
    v.auto_request_charts = i.auto_request_charts;
    v.new_markets_max_scale = i.new_markets_max_scale;
    v.icon_selection = i.icon_selection;
    v.colors.price_line_width = i.price_line_width;
    v.panic_sell_opacity = i.panic_sell_opacity;
    v.glass_opacity = i.glass_opacity;
    v.book_cumulative_opacity = i.book_cumulative_opacity;
    v.book_orders_opacity = i.book_orders_opacity;
    v.book_orders_width = i.book_orders_width;
    let s = &mut cfg.signals;
    s.play_signal_sound = i.play_signal_sound;
    s.signal_sound = i.signal_sound;
    s.auto_show_on_signal = i.auto_show_on_signal;
    s.use_last_detect_caption = i.use_last_detect_caption;
    s.full_screen_prevent_signals = i.full_screen_prevent_signals;
    let u = &mut cfg.ui;
    u.confirm_close = i.confirm_close;
    u.hide_demo_button = i.hide_demo_button;
    u.new_markets_on_top = i.new_markets_on_top;
}

/// Apply the leverage-management block.
fn apply_leverage(cfg: &mut SharedConfig, l: &LeverageSettings) {
    let m = &mut cfg.trading.auto_manage_lev;
    m.auto_max_order = l.auto_max_order;
    m.auto_lev_up = l.auto_lev_up;
    // Isolated and cross are mutually exclusive in Moonbot; the caller owns which one is set, and
    // both are written so turning one on turns the other off in the same packet.
    m.auto_isolated = l.auto_isolated;
    m.auto_cross = l.auto_cross;
    m.tlg_report = l.tlg_report;
    m.auto_fix_lev = l.auto_fix_lev;
    m.fix_lev = l.fix_lev;
    cfg.trading.auto_lev_control = l.lev_control.clone();
}

/// Apply the AutoStart tab to `trading.auto_start` and `trading.auto_start_2`.
fn apply_auto_start(cfg: &mut SharedConfig, s: &AutoStartSettings) {
    let a = &mut cfg.trading.auto_start;
    a.auto_start = s.auto_start;
    a.auto_detect_on = s.auto_detect_on;
    a.strategies_on = s.strategies_on;
    a.remember_state = s.remember_state;
    a.auto_update = s.auto_update;
    a.dont_wait_sells = s.dont_wait_sells;
    a.work_time = s.work_time;
    // The wire fraction holds more precision than one minute, so rewriting it from an unchanged
    // minute value would drift the core's own boundary (0.9999 -> 0.99930...) on every OK press.
    if day_fraction_to_minutes(a.work_time_from) != s.work_time_from_min {
        a.work_time_from = minutes_to_day_fraction(s.work_time_from_min);
    }
    if day_fraction_to_minutes(a.work_time_to) != s.work_time_to_min {
        a.work_time_to = minutes_to_day_fraction(s.work_time_to_min);
    }
    a.auto_stop_if_loss = s.auto_stop_if_loss;
    a.auto_stop_loss = s.auto_stop_loss;
    a.stop_trades = s.stop_trades;
    a.sell_if_loss = s.sell_if_loss;
    a.auto_stop_if_loss_hours = s.auto_stop_if_loss_hours;
    a.auto_stop_hours_val = s.auto_stop_hours_val;
    a.stop_hours = s.stop_hours;
    a.stop_hours_trades = s.stop_hours_trades;
    a.ignore_emulator = s.ignore_emulator;
    a.panic_btc = s.panic_btc;
    a.panic_btc_delta = s.panic_btc_delta;
    a.panic_btc_delta_up = s.panic_btc_delta_up;
    a.panic_market = s.panic_market;
    a.panic_market_delta = s.panic_market_delta;
    a.auto_stop_on_errors = s.auto_stop_on_errors;
    a.errors_level = s.errors_level;
    a.sell_all_on_errors = s.sell_all_on_errors;
    a.restart_after_err = s.restart_after_err;
    a.restart_err_time = s.restart_err_time;
    a.auto_stop_on_ping = s.auto_stop_on_ping;
    a.ping_level = s.ping_level;
    a.sell_all_on_ping = s.sell_all_on_ping;
    a.restart_after_ping = s.restart_after_ping;
    a.restart_ping_time = s.restart_ping_time;

    let a2 = &mut cfg.trading.auto_start_2;
    a2.reset_session = s.reset_session;
    a2.rs_hours = s.rs_hours;
    a2.max_session_cap = s.max_session_cap;
    a2.restart_on_market = s.restart_on_market;
    a2.btc_higher_then = s.btc_higher_then;
    a2.btc_lower_then = s.btc_lower_then;
    a2.market_higher_then = s.market_higher_then;
}

/// Apply the BTC blink and alarm controls drawn at the bottom of the AutoStart tab to
/// `visual.blink_config`. A separate function from [`apply_auto_start`] because [`FieldMask`]
/// tracks the two as separate [`CoreConfigArea`] areas, matching [`CoreConfig::btc_blink`] being
/// its own projected section.
fn apply_btc_blink(cfg: &mut SharedConfig, blink: &BtcBlinkSettings) {
    let b = &mut cfg.visual.blink_config;
    b.blink_btc = blink.blink_btc;
    b.blink_btc_delta = blink.blink_btc_delta;
    b.blink_btc_delta_up = blink.blink_btc_delta_up;
    b.alarm_btc = blink.alarm_btc;
    b.alarm_type = blink.alarm_type;
}

/// Apply Moonbot's own appearance — `feed::ViewSettings` — across `signals`, `trading` and its
/// display sub-records, `visual` and its sub-records, `theme` and `ui`.
///
/// Only the scalar fields the projection names: the candle palettes, the drawing tools, the
/// report's column arrays, the market table's column arrays, the INI sections and every
/// `unknown_tail` travel back untouched, as do the fields [`apply_interface`] owns in the same
/// sections — `visual.show_filters.scale_tool` among them, beside this area's six other
/// `show_filters` flags.
fn apply_view(cfg: &mut SharedConfig, v: &ViewSettings) {
    cfg.signals.show_cross = v.show_cross;
    cfg.signals.base_vol_ind_type = v.base_vol_ind_type;
    cfg.signals.base_vol_n = v.base_vol_n;
    cfg.trading.price_range = v.price_range;
    cfg.trading.x9_mode = v.x9_mode;
    cfg.trading.draw_all_buy = v.draw_all_buy;
    cfg.trading.show_order_comment = v.show_order_comment;
    cfg.trading.multi_orders.show_orders_num = v.show_orders_num;
    cfg.trading.multi_orders.done_opacity = v.done_opacity;
    cfg.trading.alert_config.keep_time = v.alert_keep_time;
    cfg.trading.alert_config.sound_kind = v.alert_sound;
    cfg.trading.alert_config.repeat_count = v.alert_repeat;
    cfg.trading.report_config.active_type = v.report_active_type;
    cfg.trading.report_config.rep_emulator_orders = v.report_emulator;
    cfg.trading.report_config.stretch = v.report_stretch;
    cfg.trading.report_config.only_2_orders = v.report_only_2_orders;
    cfg.trading.report_config.use_leverage = v.report_use_leverage;
    cfg.trading.report_config.max_lines = v.report_max_lines;
    cfg.trading.report_config.pos_direction = v.report_direction;
    cfg.trading.report_config.store_liq = v.report_store_liq;
    cfg.trading.report_config.store_fund = v.report_store_fund;
    cfg.trading.report_config.by_close_date = v.report_by_close_date;
    cfg.trading.spot_config.show_trades = v.spot_show_trades;
    cfg.trading.spot_config.show_book = v.spot_show_book;
    cfg.trading.spot_config.book_len = v.spot_book_len;
    cfg.trading.spot_config.show_market_avg = v.show_market_avg;
    cfg.trading.spot_config.show_min_max = v.show_min_max;
    cfg.trading.spot_config.show_avg_price = v.show_avg_price;
    cfg.trading.spot_config.spot_btc = v.spot_btc;
    cfg.trading.spot_config.shift_spot = v.shift_spot;
    cfg.trading.spot_config.show_mark_price = v.show_mark_price;
    cfg.trading.spot_config.show_liq = v.show_liq;
    cfg.trading.spot_config.huge_liq = v.huge_liq;
    cfg.trading.spot_config.show_open_int = v.show_open_int;
    cfg.trading.spot_config.show_avg_price_line = v.show_avg_price_line;
    cfg.trading.cashback_settings.sep = v.cashback_sep;
    cfg.trading.cashback_settings.max_days = v.cashback_period;
    cfg.trading.cashback_settings.instant_bnb = v.cashback_instant_bnb;
    cfg.trading.transfer_config.show_total = v.transfer_show_total;
    cfg.trading.transfer_config.hide_zero = v.transfer_hide_zero;
    cfg.trading.arb_view_config.show_absolute = v.arb_show_absolute;
    cfg.trading.arb_view_config.show_numbers = v.arb_show_numbers;
    cfg.trading.arb_view_config.show_lines = v.arb_show_lines;
    cfg.trading.arb_view_config.show_percent = v.arb_show_percent;
    cfg.trading.arb_view_config.show_right = v.arb_show_right;
    cfg.visual.show_red_green_lines = v.show_red_green_lines;
    cfg.visual.show_red_green_dots = v.show_red_green_dots;
    cfg.visual.auto_close_charts = v.auto_close_charts;
    cfg.visual.chart_watch_multi_frames = v.chart_watch_multi_frames;
    cfg.visual.chart_history_len = v.chart_history_len;
    cfg.visual.show_vert_volume = v.show_vert_volume;
    cfg.visual.vert_vol_time_frame = v.vert_vol_time_frame;
    cfg.visual.vert_vol_opacity = v.vert_vol_opacity;
    cfg.visual.vert_vol_height = v.vert_vol_height;
    cfg.visual.vert_vol_kind = v.vert_vol_kind;
    cfg.visual.vert_vol_ind_pos = v.vert_vol_ind_pos;
    cfg.visual.v_tool_show = v.v_tool_show;
    cfg.visual.v_tool_time = v.v_tool_time;
    cfg.visual.v_tool_time_index = v.v_tool_time_index;
    cfg.visual.v_tool_opacity = v.v_tool_opacity;
    cfg.visual.hv_show = v.hv_show;
    cfg.visual.hv_width = v.hv_width;
    cfg.visual.hv_time_frame_ind = v.hv_time_frame_ind;
    cfg.visual.hv_price_frame = v.hv_price_frame;
    cfg.visual.hv_opacity = v.hv_opacity;
    cfg.visual.hv_disp_vol = v.hv_disp_vol;
    cfg.visual.hv_kind = v.hv_kind;
    cfg.visual.charts_auto_shift = v.charts_auto_shift;
    cfg.visual.draw_cur_price = v.draw_cur_price;
    cfg.visual.scale_plus_index = v.scale_plus_index;
    cfg.visual.vo_sort_kind = v.vo_sort_kind;
    cfg.visual.vo_older_first = v.vo_older_first;
    cfg.visual.vo_sort_by_market = v.vo_sort_by_market;
    cfg.visual.show_strat_numbers = v.show_strat_numbers;
    cfg.visual.hide_ny_elka = v.hide_ny_elka;
    cfg.visual.spy_hide_mode = v.spy_hide_mode;
    cfg.visual.chart_candles_style = v.chart_candles_style;
    cfg.visual.chart_candles_tick_opacity = v.chart_candles_tick_opacity;
    cfg.visual.chart_candles_neutral_ticks = v.chart_candles_neutral_ticks;
    cfg.visual.chart_candles_outline_width = v.chart_candles_outline_width;
    cfg.visual.chart_candles_tick_wicks = v.chart_candles_tick_wicks;
    cfg.visual.ai_card_provider = v.ai_card_provider;
    cfg.visual.ai_card_model = v.ai_card_model.clone();
    cfg.visual.ai_card_prompt = v.ai_card_prompt.clone();
    cfg.visual.custom_draw_config.f_show = v.draw_show;
    cfg.visual.custom_draw_config.f_opacity = v.draw_opacity;
    cfg.visual.show_filters.need_show = v.filters_show;
    cfg.visual.show_filters.filters = v.filters_strategies;
    cfg.visual.show_filters.sessions = v.filters_sessions;
    cfg.visual.show_filters.hide_zero = v.filters_hide_zero;
    cfg.visual.show_filters.m_shot_area = v.filters_moonshot;
    cfg.visual.show_filters.scroll_filters = v.filters_scroll;
    cfg.visual.show_filters.show_detects = v.filters_detects;
    cfg.visual.charts_settings.auto_arrange = v.monitor_auto_arrange;
    cfg.visual.charts_settings.visible = v.monitor_visible;
    cfg.visual.charts_settings.wide = v.monitor_wide;
    cfg.visual.charts_settings.stay_on_top = v.charts_stay_on_top;
    cfg.visual.charts_settings.max_charts = v.monitor_max_charts;
    cfg.visual.charts_settings.refresh = v.monitor_refresh;
    cfg.visual.heatmap_config.show = v.heatmap_show;
    cfg.visual.heatmap_config.use_q = v.heatmap_use_q;
    cfg.visual.heatmap_config.height = v.heatmap_height;
    cfg.visual.heatmap_config.cpu = v.heatmap_cpu;
    cfg.visual.heatmap_config.trades = v.heatmap_trades;
    cfg.visual.heatmap_config.app_latency = v.heatmap_app_latency;
    cfg.visual.heatmap_config.draw_latency = v.heatmap_draw_latency;
    cfg.visual.news_form_config.stay_on_top = v.news_stay_on_top;
    cfg.visual.news_form_config.exact_time = v.news_exact_time;
    cfg.visual.news_form_config.font_size = v.news_font_size;
    cfg.visual.news_form_config.update_orig = v.news_translate;
    cfg.visual.news_form_config.sound = v.news_sound;
    cfg.visual.news_form_config.theme = v.news_theme;
    cfg.visual.news_form_config.coin_card_font_size = v.news_coin_card_font;
    cfg.visual.news_form_config.full_tags = v.news_full_tags;
    cfg.visual.news_form_config.feed_mode = v.news_feed_mode;
    cfg.theme.current_style = v.current_style;
    cfg.ui.coins_sort_order = v.coins_sort_order;
    cfg.ui.markets_table_config.sort_col = v.markets_sort_col;
    cfg.ui.main_button_index_1 = v.main_button;
}
