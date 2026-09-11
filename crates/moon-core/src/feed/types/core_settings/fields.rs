//! Field-level view of [`CoreConfig`]: one [`CoreField`] per leaf of the ten areas a settings
//! surface may write, so a change can be counted, compared across cores and carried from one core's
//! page onto another's.
//!
//! The projection is a plain struct tree with no reflection, and the two questions the expert
//! window has to answer are field-shaped rather than area-shaped: "how many parameters will this OK
//! change" and "which parameters differ between these four cores". An area comparison
//! (`CoreConfigRejection::Areas`) says only that SOMETHING in `general` moved. This table answers
//! per field, with three function pointers a macro writes once per leaf — compare, copy, show — so
//! no type has to implement a trait and no value is boxed.
//!
//! Each entry also carries the [`FieldMask`] a write of it must name, spelled once per SECTION in
//! the table rather than derived from the area at run time: the mask is a fact about the section,
//! and a section without a builder cannot be listed at all.
//!
//! What the table covers IS the boundary of what a multi-core apply may write: `leverage`, the
//! manual block and `fav_markets` are deliberately absent. None of the expert window's pages stages
//! into them, and a field outside the table can neither be counted as a change nor copied onto
//! another core — so leaving them out is what keeps a bulk OK from ever reaching the manual
//! block, the same guarantee [`FieldMask`] gives the single-core send.
//!
//! Completeness is enforced by the sibling test, which reads the struct definitions out of the
//! source and demands one entry per `pub` field of each covered section. A field added to a section
//! without an entry here fails that test rather than silently escaping the diff.

use crate::feed::FieldMask;

use super::{CoreConfig, CoreConfigArea};

/// One leaf of the projection, addressed by its `section.field` path.
#[derive(Debug)]
pub struct CoreField {
    /// `section.field`, spelled as the projection spells it — which follows the wire's own names,
    /// so a trader who knows Moonbot's ini recognises them.
    pub key: &'static str,
    /// Area the field belongs to.
    pub area: CoreConfigArea,
    /// The mask a write of this field must carry.
    pub mask: FieldMask,
    differs: fn(&CoreConfig, &CoreConfig) -> bool,
    copy: fn(&CoreConfig, &mut CoreConfig),
    show: fn(&CoreConfig) -> FieldValue,
    perturb: fn(&mut CoreConfig),
    /// When this field is a COPY of another rather than a value of its own — see
    /// [`CoreField::is_derived`]. `None` for the ordinary leaf.
    derived: Option<fn(&CoreConfig) -> bool>,
}

impl CoreField {
    /// Whether the two projections disagree on this one field.
    pub fn differs(&self, a: &CoreConfig, b: &CoreConfig) -> bool {
        (self.differs)(a, b)
    }

    /// Copy this one field from `from` into `into`, leaving every other field of `into` alone.
    pub fn copy_into(&self, from: &CoreConfig, into: &mut CoreConfig) {
        (self.copy)(from, into)
    }

    /// The field's value in `cfg`, for display.
    pub fn show(&self, cfg: &CoreConfig) -> FieldValue {
        (self.show)(cfg)
    }

    /// Move this field to SOME other value — any other — leaving the rest of `cfg` alone.
    ///
    /// For [`perturbed`], whose one job is a configuration that disagrees with a given one on every
    /// field: the second base a probe of a control needs, so that a setter which happens to write
    /// the value one base already holds still shows which field it writes on the other.
    pub fn perturb(&self, cfg: &mut CoreConfig) {
        (self.perturb)(cfg)
    }

    /// Whether, in `cfg`, this field is derived from another rather than set on its own.
    ///
    /// The projection keeps one cross-field rule: with `same_hotkeys_for_move` on, each short
    /// move gesture mirrors its long twin (`GestureSettings::set_move_gesture`). While that holds,
    /// the four shorts are copies — they move with every long edit — and a change set must not
    /// stage them as the user's own values, or a bulk write would stamp one core's copies over a
    /// target that keeps its shorts independent. `overlay` re-derives them on any target whose
    /// flag is on, so nothing is lost by leaving them out.
    pub fn is_derived(&self, cfg: &CoreConfig) -> bool {
        self.derived.is_some_and(|derived| derived(cfg))
    }
}

/// A field's value with its type kept, so the UI can caption a flag in its own language and format
/// a number its own way rather than receiving a string this crate chose.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    Bool(bool),
    Int(i64),
    /// An `f32` leaf, kept at its own width: widened to `f64` it would print as
    /// `0.30000001192092896` where the core holds `0.3`.
    Float32(f32),
    Float(f64),
    Text(String),
    List(Vec<String>),
}

/// What every leaf type of the table has to be able to do: tell two of its values apart, move
/// to some other value, and show itself.
///
/// One trait rather than three, implemented for exactly the types the covered sections use; a
/// new leaf type is a compile error here, which is the right place to notice it.
trait Leaf: Clone {
    /// Whether the two are the same value. `PartialEq` for everything but the floats, which are
    /// compared by their bits: a `NaN` the wire sent would otherwise differ from itself, and a
    /// field that always differs is a field every diff reports and every mixed mark shows for
    /// good.
    fn same(&self, other: &Self) -> bool;
    /// Move to SOME other value — any other; see [`CoreField::perturb`].
    fn perturb(&mut self);
    /// The value, for display.
    fn value(&self) -> FieldValue;
}

impl Leaf for bool {
    fn same(&self, other: &Self) -> bool {
        self == other
    }
    fn perturb(&mut self) {
        *self = !*self;
    }
    fn value(&self) -> FieldValue {
        FieldValue::Bool(*self)
    }
}

macro_rules! int_leaf {
    ($($t:ty),*) => {
        $(impl Leaf for $t {
            fn same(&self, other: &Self) -> bool {
                self == other
            }
            fn perturb(&mut self) {
                *self = self.wrapping_add(1);
            }
            fn value(&self) -> FieldValue {
                FieldValue::Int(i64::from(*self))
            }
        })*
    };
}
int_leaf!(i32, u8, u16);

impl Leaf for f32 {
    fn same(&self, other: &Self) -> bool {
        self.to_bits() == other.to_bits()
    }
    fn perturb(&mut self) {
        // A non-finite value stays non-finite under `+ 1.0` and so would not move; zero always
        // differs from it.
        *self = if self.is_finite() { *self + 1.0 } else { 0.0 };
    }
    fn value(&self) -> FieldValue {
        FieldValue::Float32(*self)
    }
}

impl Leaf for f64 {
    fn same(&self, other: &Self) -> bool {
        self.to_bits() == other.to_bits()
    }
    fn perturb(&mut self) {
        *self = if self.is_finite() { *self + 1.0 } else { 0.0 };
    }
    fn value(&self) -> FieldValue {
        FieldValue::Float(*self)
    }
}

impl Leaf for String {
    fn same(&self, other: &Self) -> bool {
        self == other
    }
    fn perturb(&mut self) {
        self.push('x');
    }
    fn value(&self) -> FieldValue {
        FieldValue::Text(self.clone())
    }
}

impl Leaf for Vec<String> {
    fn same(&self, other: &Self) -> bool {
        self == other
    }
    fn perturb(&mut self) {
        self.push("x".to_string());
    }
    fn value(&self) -> FieldValue {
        FieldValue::List(self.clone())
    }
}

/// The whole table from a list of sections, each `Area / with_mask / section: [fields]`.
///
/// The area and the mask are written once per section, so a leaf cannot claim the wrong one; the
/// key is spelled from the same tokens the accessors use, so a typo in a field name fails to
/// compile rather than naming a field that is not the one compared. A leaf marked
/// `=> derived(pred)` is a copy of another field whenever `pred` holds — see
/// [`CoreField::is_derived`].
macro_rules! core_fields {
    (@derived) => { None };
    (@derived $pred:expr) => { Some($pred as fn(&CoreConfig) -> bool) };
    ($( $area:ident / $with:ident / $sec:ident : [ $( $f:ident $(=> derived($pred:expr))? ),* $(,)? ] ),* $(,)?) => {
        &[ $( $(
            CoreField {
                key: concat!(stringify!($sec), ".", stringify!($f)),
                area: CoreConfigArea::$area,
                mask: FieldMask::EMPTY.$with(),
                differs: |a, b| !Leaf::same(&a.$sec.$f, &b.$sec.$f),
                // UFCS rather than `.clone()`: most leaves are `Copy`, and a method-call clone on
                // one is a clippy error under `-D warnings`.
                copy: |from, into| into.$sec.$f = Clone::clone(&from.$sec.$f),
                show: |cfg| Leaf::value(&cfg.$sec.$f),
                perturb: |cfg| Leaf::perturb(&mut cfg.$sec.$f),
                derived: core_fields!(@derived $($pred)?),
            }
        ),* ),* ]
    };
}

/// Every leaf of the ten writable areas, in the order the projection declares them.
///
/// Indices into this slice are stable for the life of a build, which is what lets a change set
/// hold `usize`s rather than keys.
pub static CORE_FIELDS: &[CoreField] = core_fields! {
    Signals / with_signals / signals: [
        play_sell_alert,
        sell_alert_level,
        signal_sound_2,
        play_buy_alert,
        buy_alert_level,
        buy_signal_sound,
    ],
    AutoStart / with_auto_start / auto_start: [
        auto_start,
        auto_detect_on,
        strategies_on,
        remember_state,
        auto_update,
        dont_wait_sells,
        work_time,
        work_time_from_min,
        work_time_to_min,
        auto_stop_if_loss,
        auto_stop_loss,
        stop_trades,
        sell_if_loss,
        auto_stop_if_loss_hours,
        auto_stop_hours_val,
        stop_hours,
        stop_hours_trades,
        ignore_emulator,
        reset_session,
        rs_hours,
        max_session_cap,
        panic_btc,
        panic_btc_delta,
        panic_btc_delta_up,
        panic_market,
        panic_market_delta,
        restart_on_market,
        btc_higher_then,
        btc_lower_then,
        market_higher_then,
        auto_stop_on_errors,
        errors_level,
        sell_all_on_errors,
        restart_after_err,
        restart_err_time,
        auto_stop_on_ping,
        ping_level,
        sell_all_on_ping,
        restart_after_ping,
        restart_ping_time,
    ],
    BtcBlink / with_btc_blink / btc_blink: [
        blink_btc,
        blink_btc_delta,
        blink_btc_delta_up,
        alarm_btc,
        alarm_type,
    ],
    General / with_general / general: [
        take_profit_on,
        take_profit_pct,
        trailing_on,
        trailing_pct,
        vstop_on,
        vol_drop_level,
        buy_iceberg,
        sell_iceberg,
        blacklist_on,
        blacklist_text,
        exclude_blacklisted_from_deltas,
    ],
    OrderRules / with_order_rules / order_rules: [
        trailing_float,
        auto_sell_partial,
        auto_cancel_buy_order,
        cancel_buy_on_sell_fill,
        dont_buy_new_coins,
        deltas_by_trades,
        analyze_on_start,
    ],
    Gestures / with_gestures / gestures: [
        buy_set_click,
        short_set_click,
        pending_order_set_click,
        pending_short_set_click,
        same_hotkeys_for_move,
        buy_move_click,
        short_buy_move_click => derived(|c| c.gestures.same_hotkeys_for_move),
        replace_buy_kind,
        sell_move_click,
        short_sell_move_click => derived(|c| c.gestures.same_hotkeys_for_move),
        replace_sell_kind,
        buy_move_click_2,
        short_buy_move_click_2 => derived(|c| c.gestures.same_hotkeys_for_move),
        replace_buy_kind_2,
        sell_move_click_2,
        short_sell_move_click_2 => derived(|c| c.gestures.same_hotkeys_for_move),
        replace_sell_kind_2,
    ],
    Special / with_special / special: [
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
    ],
    Telegram / with_telegram / telegram: [
        pump_channel,
        pump_channels,
        multi_channels,
        more_then_1_channel,
        listen_moon_channel,
        use_moon_bl,
    ],
    AutoBuy / with_auto_buy / auto_buy: [
        monitor_clipboard,
        clipboard_auto_buy,
        lower_case_token_cbd,
        look_full_link_cbd,
        advanced_filter_clipboard,
        telegram_auto_buy,
        lower_case_token_tlg,
        look_full_link_tlg,
        advanced_filter,
        dont_buy_reply,
        dont_buy_forward,
        msg_keywords_long,
        msg_keywords_short,
        msg_black_words,
        msg_token_tags,
        lower_price_words,
        use_keywords,
        buy_key_dist,
        use_black_words,
        use_words_count,
        words_count,
        use_lower_price_words,
        x_lower_price,
        x_found_price,
        buy_if_price_found,
        use_price,
        use_stops,
        only_1_token,
        use_token_tags,
        tokens_no_tags,
        token_links,
        special_formats,
        auto_cancel_lower_buy,
    ],
    Interface / with_interface / interface: [
        buy_on_enter,
        dbl_click_panic_sell,
        chart_split_zones,
        draw_stop,
        pending_orders_spread,
        pending_orders_spread_h_delta,
        hide_forum_label,
        scrolling_charts,
        startup_load_charts,
        hide_right_chart_panel,
        left_chart_info,
        show_iceberg,
        show_orders_captions,
        orders_captions_lower,
        hide_pnl,
        hide_buy_button,
        hide_cashback_button,
        remember_chart_buttons,
        scale_tool,
        icon_selection,
        price_line_width,
        panic_sell_opacity,
        glass_opacity,
        book_cumulative_opacity,
        book_orders_opacity,
        book_orders_width,
        play_signal_sound,
        confirm_close,
        hide_demo_button,
        auto_show_on_signal,
        show_market_captions,
        show_usd_on_charts,
        show_detects_tool,
        auto_request_charts,
        new_markets_max_scale,
        new_markets_on_top,
        use_last_detect_caption,
        full_screen_prevent_signals,
        pending_buy_price,
        hide_cashback_info,
    ],
};

/// A configuration that disagrees with `cfg` on every table field.
///
/// The second base a control probe needs — see [`CoreField::perturb`]. Only the table's fields
/// move; the areas the table does not cover are copied as they are.
pub fn perturbed(cfg: &CoreConfig) -> CoreConfig {
    let mut other = cfg.clone();
    for field in CORE_FIELDS {
        field.perturb(&mut other);
    }
    other
}

/// Two configurations that disagree on every table field, for finding out which fields a setter
/// writes without being told.
///
/// A settings control carries only the function that stages its value (`fn(&mut CoreConfig,
/// bool)` and the like). Run against ONE configuration, a setter that happens to write the value
/// already held moves nothing and hides its field; run against two that disagree everywhere, it
/// moves at least one of them. That is the whole trick, and it is what lets a surface say "this
/// control's parameter differs between the selected cores" without naming a field per control.
pub struct ProbeBases(Box<(CoreConfig, CoreConfig)>);

impl ProbeBases {
    /// Build the pair from any configuration — which one does not matter, only that the two
    /// disagree.
    pub fn new(cfg: &CoreConfig) -> Self {
        Self(Box::new((cfg.clone(), perturbed(cfg))))
    }

    /// The table fields `setter` writes as VALUES, ascending, or empty for a setter that writes
    /// nothing — including one whose text the parser behind it rejected.
    ///
    /// A field the setter moved only as a copy of another — a short gesture the long one's setter
    /// mirrored because that base's mirror was on — is not counted: the two bases disagree on the
    /// mirror flag as on everything else, so a long gesture's setter would otherwise read as
    /// writing the short too, and a decision on the long would stage a short the user never
    /// touched.
    pub fn fields_written_by(&self, setter: &dyn Fn(&mut CoreConfig)) -> Vec<usize> {
        let (a, b) = &*self.0;
        let mut fields = Vec::new();
        for base in [a, b] {
            let mut after = base.clone();
            setter(&mut after);
            fields.extend(
                changed_fields(base, &after)
                    .into_iter()
                    .filter(|&index| !CORE_FIELDS[index].is_derived(&after)),
            );
        }
        fields.sort_unstable();
        fields.dedup();
        fields
    }
}

/// Index of the field with this key, for callers that address one field by name.
pub fn index_of(key: &str) -> Option<usize> {
    CORE_FIELDS.iter().position(|field| field.key == key)
}

/// Indices of every field on which `a` and `b` disagree.
pub fn changed_fields(a: &CoreConfig, b: &CoreConfig) -> Vec<usize> {
    differing_fields(&[a, b])
}

/// Indices of every field on which at least one of `cfgs` disagrees with the first.
///
/// Empty for fewer than two configurations: one core differs from nothing.
pub fn differing_fields(cfgs: &[&CoreConfig]) -> Vec<usize> {
    let Some((first, rest)) = cfgs.split_first() else {
        return Vec::new();
    };
    CORE_FIELDS
        .iter()
        .enumerate()
        .filter(|(_, field)| rest.iter().any(|other| field.differs(first, other)))
        .map(|(index, _)| index)
        .collect()
}

/// The mask a write of exactly these fields must carry: the union of their sections' masks.
///
/// Args:
///     indices: Indices into [`CORE_FIELDS`]; an index outside the table is a programming error,
///         since only this module hands them out.
pub fn mask_for_fields(indices: &[usize]) -> FieldMask {
    indices.iter().fold(FieldMask::EMPTY, |mask, &index| {
        mask.union(CORE_FIELDS[index].mask)
    })
}

#[cfg(test)]
mod tests;
