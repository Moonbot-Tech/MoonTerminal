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
/// Six fields of `trading` plus `signals.load_deep_history`, which is the one field of this area
/// outside `trading` and cannot collide with [`apply_signals`]: that applier owns six other fields
/// of the same section and none of them is this one.
fn apply_order_rules(cfg: &mut SharedConfig, r: &OrderRulesSettings) {
    cfg.signals.load_deep_history = r.analyze_on_start;
    let t = &mut cfg.trading;
    t.trailing_float = r.trailing_float;
    t.auto_sell_partial = r.auto_sell_partial;
    t.auto_cancel_buy_order = r.auto_cancel_buy_order;
    t.cancel_buy_on_sell_fill = r.cancel_buy_on_sell_fill;
    t.dont_buy_new_coins = r.dont_buy_new_coins;
    t.deltas_by_trades = r.deltas_by_trades;
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

/// Apply Moonbot's "Специальные" page to `trading` and its `send_shots_config` and
/// `orders_control` sub-records.
///
/// Thirty-six fields across `trading`, its `send_shots_config` and its `orders_control`:
/// everything else in that section — including its `unknown_tail`, the exits [`apply_general`] owns
/// and the leverage block [`apply_leverage`] owns — travels back untouched.
///
/// `orders_control.sign_orders` is NOT among them on purpose: the wire's own doc marks it a mirror
/// of `ClientSettingsCommand::sign_orders`, so it travels on the compact channel too, and writing it
/// from here would set two routes fighting over one field. Its neighbours `min_price`, `max_time`
/// and `h_pos_control` are absent for the plainer reason that no row of this page draws them — see
/// `core_expert::pages::special`, which states what that costs.
fn apply_special(cfg: &mut SharedConfig, s: &SpecialSettings) {
    let t = &mut cfg.trading;
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
    t.h_pos_black_list_text = s.h_pos_black_list_text.clone();
    let oc = &mut t.orders_control;
    oc.liq_control = s.liq_control;
    oc.ignore_replacing_bug = s.ignore_replacing_bug;
    oc.ignore_protection = s.ignore_protection;
    oc.active = s.orders_control_active;
    oc.h_pos_report = s.h_pos_report;
    oc.h_pos_auto_sell = s.h_pos_auto_sell;
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

/// Apply Moonbot's Telegram page to `signals` and the one `trading` flag beside it.
///
/// Six fields: everything else in both sections — including their `unknown_tail`s, the alert sounds
/// [`apply_signals`] owns and the message filter [`apply_auto_buy`] owns — travels back untouched.
fn apply_telegram(cfg: &mut SharedConfig, t: &TelegramSettings) {
    let sig = &mut cfg.signals;
    sig.pump_channel = t.pump_channel.clone();
    sig.pump_channels = t.pump_channels.clone();
    sig.multi_channels = t.multi_channels;
    sig.more_then_1_channel = t.more_then_1_channel;
    sig.listen_moon_channel = t.listen_moon_channel;
    cfg.trading.use_moon_bl = t.use_moon_bl;
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
