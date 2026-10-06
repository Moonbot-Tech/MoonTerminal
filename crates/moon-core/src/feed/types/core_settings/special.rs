//! Engine, Telegram and automatic-buy settings.

/// Moonbot's "Специальные" page: the engine's own switches, its logging and its screenshot rules.
///
/// An area is a PAGE — see [`crate::feed::InterfaceSettings`]. This one is `trading` with its
/// `send_shots_config` and `orders_control` sub-records.
///
/// Moonbot's page has about twice this many controls, and the expert window draws all of them
/// disabled where they are not here. The reasons are stated on the page itself, row by row; the
/// ones that concern this block are: the Remote
/// block and the hang watchdog carry a bot token, a UDP password and a control VDS address, which
/// safe-share excludes outright; a few rows have no wire field at all; and the iceberg pair belongs
/// to [`crate::feed::GeneralSettings`], which the compact popup edits — one wire field belongs to one area.
#[derive(Debug, Clone)]
pub struct SpecialSettings {
    /// `trading.log_level` and `trading.auto_delete_logs`: how much is written, and for how long.
    pub log_level: i32,
    pub auto_delete_logs: i32,
    /// `trading.chart_clean_up_time`: minutes of inactivity after which a chart is dropped.
    pub chart_clean_up_time: i32,
    /// `trading.max_orders` and `trading.unlimited_orders`: the cap on open buys, and its removal.
    pub max_orders: i32,
    pub unlimited_orders: bool,
    /// `trading.random_price`: add a small random offset to an order's price.
    pub random_price: bool,
    /// `trading.correct_order_price`: snap an order price to the venue's tick.
    pub correct_order_price: bool,
    /// `trading.use_book_ticker`: take best bid/ask from the stream rather than by polling.
    pub use_book_ticker: bool,
    /// `trading.m_avg_use_vol_weight`: weight the moving average by volume.
    pub m_avg_use_vol_weight: bool,
    /// `trading.auto_buy_bnb`, `trading.auto_buy_bnb_level` and `trading.auto_buy_bnb_volume`: buy
    /// BNB for commissions when the balance falls below the level, and how much.
    pub auto_buy_bnb: bool,
    pub auto_buy_bnb_level: f64,
    pub auto_buy_bnb_volume: f64,
    /// `trading.auto_reduce_order`: shrink an order that exceeds the free balance.
    pub auto_reduce_order: bool,
    /// `trading.auto_close_zero_pos`: close a zero-quantity ghost position.
    pub auto_close_zero_pos: bool,
    /// `trading.auto_lower_lev`: drop the leverage when the venue refuses the level asked for.
    ///
    /// Moonbot calls the row "Auto Leverage"; this is the only unclaimed leverage flag in the
    /// section, the rest of them being the `auto_manage_lev` block [`crate::feed::LeverageSettings`] owns.
    pub auto_lower_lev: bool,
    /// `trading.use_websocket_api`: place orders over the socket rather than REST.
    pub use_websocket_api: bool,
    /// `trading.futures_rules`: Moonbot's "Quantitative Rules" — the futures position-mode checks.
    ///
    /// Its control is `cbFuturesRules`, which is the wire's own name for the field; caption and name
    /// disagree only in wording. Unlike `free_position_check`, which this page still draws dead, the
    /// caption carries no negation — ticked means on, and no direction is left to guess. That it can
    /// turn a live safety check off is what Moonbot's own checkbox does too, and mirroring that
    /// dialog is this window's whole contract.
    pub futures_rules: bool,
    /// `trading.iceberg_step`: the price step below which an order is placed as an iceberg, as a
    /// PER CENT. The wire's own default is 0.1, meaning a tenth of a per cent.
    ///
    /// The wire calls it "iceberg slice size as a fraction of total", and that is wrong. Moonbot
    /// formats the row itself as "Ставить Iceberg если шаг цены < %s%%" — a literal per cent sign
    /// after the value — so the number is a percentage of price, not a share of the order. The
    /// control's 0..1 range suits that reading as well, which is why it did not have to move.
    pub iceberg_step: f64,
    /// `trading.sell_x2_level`: the volume percentile above which the sell quantity doubles.
    pub sell_x2_level: i32,
    /// `trading.no_trades_markets_text`: tickers that generate no signals, one per line.
    pub no_trades_markets_text: String,
    /// `trading.orders_control.liq_control`: watch how near a position is to liquidation.
    pub liq_control: bool,
    /// `trading.orders_control.ignore_replacing_bug`: ignore the engine's "replacing" order state.
    pub ignore_replacing_bug: bool,
    /// `trading.orders_control.ignore_protection`: how far the order protection is bypassed.
    ///
    /// A LEVEL on the wire, where zero means the protection is on, but Moonbot draws one checkbox
    /// over it ("Turn Off Protection"). So this page reads a positive value as on and, when the box
    /// is ticked, supplies a level only if the core holds none — a level it already holds survives.
    /// Turning the box off does set zero, so the level is lost that way; Moonbot's own dialog can
    /// express no more than that either.
    pub ignore_protection: i32,
    /// `trading.orders_control.active`: watch this bot's ORDERS — Moonbot's "Следить за ордерами
    /// этого бота", the one switch in its worker-bot block.
    ///
    /// Its neighbour `orders_control.h_pos_control` ("hanging-position detection") is deliberately
    /// NOT here: no row on that page carries its caption, and binding one checkbox to two flags
    /// would turn a feature on and off that the trader never named.
    pub orders_control_active: bool,
    /// `trading.orders_control.h_pos_report` and `trading.orders_control.h_pos_auto_sell`: what the
    /// WATCHING bot does about a hanging position — report it, and sell it.
    pub h_pos_report: bool,
    pub h_pos_auto_sell: bool,
    /// `trading.h_pos_black_list_text`: coins the watcher leaves alone.
    ///
    /// One line, comma-separated. The wire names no separator for this field, unlike its siblings
    /// that say "one per line"; the evidence is Moonbot's own dialog, which draws it as a one-line
    /// box holding "BTC, ETH, BNB, …", and its documentation, which calls it a comma-separated
    /// blacklist. `trading.h_pos_black_list_add` beside it is a SECOND such list with no row on the
    /// page, so an empty box here does not mean the watcher skips nothing.
    pub h_pos_black_list_text: String,
    /// `trading.multi_commands`: accept batched commands.
    ///
    /// Moonbot's caption is "Мультистроковые команды" and its hint for that row says what it does:
    /// "Принимать несколько команд в одном сообщении в Телеграме". Several commands in ONE Telegram
    /// message — not the "batch order operations from a thin client" the wire's doc describes. The
    /// field is right, the wire's sentence is not.
    pub multi_commands: bool,
    /// `trading.send_shots_config.may_send` and the thresholds under it: when a trade's chart is
    /// posted to Telegram, and how that chart is scaled.
    ///
    /// Two of these carry a caption the wire words differently.
    ///
    /// `time_scale` is a PER CENT and the wire's "seconds of history" is wrong: Moonbot's own hint
    /// reads "Масштаб по оси времени от 100% до 400% означает, насколько график на скрине длиннее,
    /// чем заняла сама сделка" — a zoom factor, and one that states its own range. `price_scale`
    /// beside it is the same kind of number, and its hint confirms what the wire's default already
    /// implied: "Если 0 … применяется авто-масштабирование".
    ///
    /// `profit_session` is drawn as "или профит за час" while the wire calls it a session profit;
    /// the two coincide only when the session resets hourly. The binding is settled by POSITION
    /// rather than by either wording: Moonbot's group is three thresholds — "Если профит $ >",
    /// "или профит % >", "или профит за час $ >" — against this record's `profit_abs`,
    /// `profit_pers`, `profit_session`, in that order.
    pub send_shots: bool,
    pub profit_abs: i32,
    pub profit_pers: i32,
    pub profit_session: i32,
    pub send_negative: bool,
    pub send_public: bool,
    pub time_scale: i32,
    pub price_scale: i32,
}

/// Hand-written for the reason [`crate::feed::ManualSettings`]'s is: three of these come off the wire as `f64`,
/// and a core holding a non-finite one must still compare equal to itself, or
/// `feed::live::shared_config::edit_satisfied` is false for any mask naming this area, forever.
impl PartialEq for SpecialSettings {
    fn eq(&self, other: &Self) -> bool {
        // Destructured for the reason [`crate::feed::GeneralSettings`]'s is.
        let Self {
            log_level,
            auto_delete_logs,
            chart_clean_up_time,
            max_orders,
            unlimited_orders,
            random_price,
            correct_order_price,
            use_book_ticker,
            m_avg_use_vol_weight,
            auto_buy_bnb,
            auto_buy_bnb_level,
            auto_buy_bnb_volume,
            auto_reduce_order,
            auto_close_zero_pos,
            auto_lower_lev,
            use_websocket_api,
            futures_rules,
            iceberg_step,
            sell_x2_level,
            no_trades_markets_text,
            liq_control,
            ignore_replacing_bug,
            ignore_protection,
            orders_control_active,
            h_pos_report,
            h_pos_auto_sell,
            h_pos_black_list_text,
            multi_commands,
            send_shots,
            profit_abs,
            profit_pers,
            profit_session,
            send_negative,
            send_public,
            time_scale,
            price_scale,
        } = self;
        auto_buy_bnb_level
            .total_cmp(&other.auto_buy_bnb_level)
            .is_eq()
            && auto_buy_bnb_volume
                .total_cmp(&other.auto_buy_bnb_volume)
                .is_eq()
            && iceberg_step.total_cmp(&other.iceberg_step).is_eq()
            && *log_level == other.log_level
            && *auto_delete_logs == other.auto_delete_logs
            && *chart_clean_up_time == other.chart_clean_up_time
            && *max_orders == other.max_orders
            && *unlimited_orders == other.unlimited_orders
            && *random_price == other.random_price
            && *correct_order_price == other.correct_order_price
            && *use_book_ticker == other.use_book_ticker
            && *m_avg_use_vol_weight == other.m_avg_use_vol_weight
            && *auto_buy_bnb == other.auto_buy_bnb
            && *auto_reduce_order == other.auto_reduce_order
            && *auto_close_zero_pos == other.auto_close_zero_pos
            && *auto_lower_lev == other.auto_lower_lev
            && *use_websocket_api == other.use_websocket_api
            && *futures_rules == other.futures_rules
            && *sell_x2_level == other.sell_x2_level
            && *no_trades_markets_text == other.no_trades_markets_text
            && *liq_control == other.liq_control
            && *ignore_replacing_bug == other.ignore_replacing_bug
            && *ignore_protection == other.ignore_protection
            && *orders_control_active == other.orders_control_active
            && *h_pos_report == other.h_pos_report
            && *h_pos_auto_sell == other.h_pos_auto_sell
            && *h_pos_black_list_text == other.h_pos_black_list_text
            && *multi_commands == other.multi_commands
            && *send_shots == other.send_shots
            && *profit_abs == other.profit_abs
            && *profit_pers == other.profit_pers
            && *profit_session == other.profit_session
            && *send_negative == other.send_negative
            && *send_public == other.send_public
            && *time_scale == other.time_scale
            && *price_scale == other.price_scale
    }
}

/// Moonbot's "Телеграм" page: which channels a signal may come from, and the rules over them.
///
/// An area is a PAGE — see [`crate::feed::InterfaceSettings`]. This one is `signals` plus the one `trading` flag
/// Moonbot files under the same tab, and it does not overlap [`AutoBuySettings`]: that page owns
/// how a message is PARSED, this one owns where messages come from.
///
/// Moonbot's own dialog shows one channel box. The wire keeps a primary channel and a list of
/// additional ones, so this block keeps them apart and the page shows the primary first. Adding a
/// channel appends to [`Self::pump_channels`] and removing takes from it; the primary is shown but
/// not removable here, because which of the additional channels would take its place is a rule the
/// protocol does not state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramSettings {
    /// `signals.pump_channel`: the primary signal channel.
    pub pump_channel: String,
    /// `signals.pump_channels`: the additional channels used in multi-channel mode.
    pub pump_channels: Vec<String>,
    /// `signals.multi_channels`: accept signals from more than one channel at once.
    pub multi_channels: bool,
    /// `signals.more_then_1_channel`: buy only a token seen in two channels.
    pub more_then_1_channel: bool,
    /// `signals.listen_moon_channel`: listen to Moonbot's own signal channel.
    pub listen_moon_channel: bool,
    /// `trading.use_moon_bl`: use the Moonbot-curated cloud blacklist.
    pub use_moon_bl: bool,
}

/// Moonbot's "АвтоПокупка" page: where a buy signal may come from, and which messages count.
///
/// An area is a PAGE, not a wire section — see [`crate::feed::InterfaceSettings`] for why. This one reads from
/// `signals`, from its `signal_config` sub-record, and from two fields of `trading`, which is exactly
/// how Moonbot's own page is put together.
///
/// It deliberately does NOT overlap [`crate::feed::SignalsSettings`]: that block is the two price-approach alert
/// sounds, which the compact popup draws and this page does not. One wire field belongs to one
/// area, or a write from either surface would put the other's frozen copy back.
///
/// The three-button "search mode" is two wire flags per source, and the UI writes both together —
/// the shape [`crate::feed::LeverageSettings`] uses for isolated-versus-cross, and for the same reason: Moonbot's
/// own control is exclusive, so a packet carrying half the choice would leave the core in a state
/// that dialog cannot show. The wire's factory default sets both flags at once, which is a value
/// that dialog normalises rather than one its user can reach; the page therefore stages nothing on
/// a click that changes no mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoBuySettings {
    /// `signals.monitor_clipboard`: watch the clipboard for token names at all.
    ///
    /// Moonbot's "Захватывать буфер", and its own hint for that row says exactly this field's
    /// meaning: "бот будет искать монету в буфере, даже если не стоит автопокупка; в этом случае
    /// бот покажет монету но не купит". Which is the difference between this flag and
    /// [`Self::clipboard_auto_buy`] beside it.
    pub monitor_clipboard: bool,
    /// `signals.clipboard_auto_buy`: buy when the clipboard yields a token.
    pub clipboard_auto_buy: bool,
    /// `signals.lower_case_token_cbd` / `signals.look_full_link_cbd` /
    /// `signals.advanced_filter_clipboard`: the clipboard source's search mode.
    ///
    /// The last two are the mode pair described above: written together, never singly.
    pub lower_case_token_cbd: bool,
    pub look_full_link_cbd: bool,
    pub advanced_filter_clipboard: bool,
    /// `signals.telegram_auto_buy`: buy when a Telegram signal matches.
    pub telegram_auto_buy: bool,
    /// `signals.lower_case_token_tlg` / `signals.look_full_link_tlg` / `signals.advanced_filter`:
    /// the Telegram source's search mode, in the same shape.
    pub lower_case_token_tlg: bool,
    pub look_full_link_tlg: bool,
    pub advanced_filter: bool,
    /// `signals.dont_buy_reply`: ignore a signal that is a reply to another message.
    pub dont_buy_reply: bool,
    /// `trading.dont_buy_forward`: ignore a signal that was FORWARDED from another chat.
    ///
    /// The wire's own doc for this field says something else — "skip buying forward contracts /
    /// pre-market tokens" — and that doc is wrong. Moonbot carries its own log line for the flag,
    /// "Нашел монету в пересланном (forward) сообщении, не буду ее покупать!", which names the
    /// behaviour and the field in one sentence; its dialog draws the row beside "Не покупать
    /// ответное", which is [`Self::dont_buy_reply`]. Two message filters, one pair.
    ///
    /// The only oddity left is the wire's: this half lives in `trading` while its twin lives in
    /// `signals`.
    pub dont_buy_forward: bool,
    /// `signals.msg_keywords_long` and `signals.msg_keywords_short`: comma-separated words that
    /// mark a message as a long or a short signal.
    pub msg_keywords_long: String,
    pub msg_keywords_short: String,
    /// `signals.msg_black_words`: words whose presence cancels the signal.
    pub msg_black_words: String,
    /// `signals.msg_token_tags`: the tag prefixes a ticker is looked for behind, e.g. `#,$`.
    pub msg_token_tags: String,
    /// `signals.lower_price_words`: words that mean "wait for a lower price" rather than "buy now".
    pub lower_price_words: String,
    /// `signal_config.use_keywords` and `signal_config.buy_key_dist`: require a keyword, and how
    /// many words may stand between it and the token.
    pub use_keywords: bool,
    pub buy_key_dist: i32,
    /// `signal_config.use_black_words`.
    pub use_black_words: bool,
    /// `signal_config.use_words_count` and `signal_config.words_count`: cap the message length.
    pub use_words_count: bool,
    pub words_count: i32,
    /// `signal_config.use_lower_price_words` and `signal_config.x_lower_price`: the "wait for a
    /// dip" filter and the offset it buys at.
    pub use_lower_price_words: bool,
    pub x_lower_price: i32,
    /// `signal_config.x_found_price`: the offset applied to a price read out of the message.
    pub x_found_price: i32,
    /// `signal_config.buy_if_price_found`: buy only when the message carries a price.
    pub buy_if_price_found: bool,
    /// `signal_config.use_price` and `signal_config.use_stops`: take the buy price, and the stops
    /// and take-profit, from the message.
    pub use_price: bool,
    pub use_stops: bool,
    /// `signal_config.only_1_token`: buy only when the message names exactly one token.
    pub only_1_token: bool,
    /// `signal_config.use_token_tags`, `signal_config.tokens_no_tags`, `signal_config.token_links`
    /// and `signal_config.special_formats`: how a ticker may be recognised.
    pub use_token_tags: bool,
    pub tokens_no_tags: bool,
    pub token_links: bool,
    pub special_formats: bool,
    /// `trading.auto_cancel_lower_buy`: minutes after which a buy left below the market is
    /// cancelled. On Moonbot's page it sits under the dip filter, which is why it is here rather
    /// than with the other `trading` fields.
    pub auto_cancel_lower_buy: i32,
}
