//! Pages of the expert core-settings window, one per tab of Moonbot's own Settings dialog.
//!
//! The strip reproduces Moonbot's order deliberately, including the pages this terminal cannot fill
//! yet: a trader who knows that dialog reaches for a page by POSITION. Every tab opens; what is
//! blocked is the control with no value behind it — see [`TabSource`].
//!
//! Two of Moonbot's tabs are deliberately NOT here. "Помощь с настройкой" is that program's setup
//! wizard and PRO is its licence purchase — both are actions inside Moonbot's own process, with no
//! setting behind them for a terminal to mirror, so reproducing them would only offer buttons that
//! can never do anything.

use gpui::ElementId;
use rust_i18n::t;

use moon_core::feed::FieldMask;

/// How far one page's values actually reach this window — the answer to "why can I not edit this".
///
/// There are two separate limits behind that question:
///
/// 1. The WIRE is the safe-share configuration (`moonproto::shared_config`): a deliberately safe
///    subset of Moonbot's settings, carrying no secrets and no machine-local state.
/// 2. The PROJECTION is `moon_core::feed::CoreConfig`, the part of that subset this terminal reads
///    into typed fields and — through the mask a surface builds with [`ExpertTab::add_sections`] —
///    is allowed to write back.
///
/// Only the first now separates one page from another. There used to be a third rating for a page
/// on the wire but not yet projected, and every such page has since been ported; a page added in
/// that state again needs the rating back, because without it the window would promise values it
/// cannot seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TabSource {
    /// Both limits cleared: the values are on the wire AND in the terminal's projection, so this
    /// page's controls can be seeded and sent as soon as they are drawn.
    Projected,
    /// Not on the wire at all: API keys, the local password and the licence state are exactly what
    /// safe-share excludes, and the setup wizard is a Moonbot action rather than a value.
    Absent,
}

/// Tabs of the expert window, in Moonbot's own strip order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ExpertTab {
    /// Exchange, API keys, password, support.
    Login,
    /// Moonbot's "Основные": exits, risk limits, order execution.
    ///
    /// The default page rather than Moonbot's own Login: Login is the one page nothing can ever
    /// arrive for, and opening on it would greet every trader with a dead tab.
    #[default]
    General,
    /// Telegram channels, message parsing and report messages.
    Telegram,
    /// Signal-driven autobuy and its filters.
    AutoBuy,
    /// Moonbot's "Специальные": order control, position guards, exchange money.
    Special,
    /// Moonbot's own chart and window appearance.
    Interface,
    /// The CORE's hotkeys, which fire inside MoonBot regardless of this terminal's own.
    Hotkeys,
    /// Autostart, watchdogs and session resets.
    AutoStart,
}

impl ExpertTab {
    pub(crate) const ALL: [ExpertTab; 8] = [
        Self::Login,
        Self::General,
        Self::Telegram,
        Self::AutoBuy,
        Self::Special,
        Self::Interface,
        Self::Hotkeys,
        Self::AutoStart,
    ];

    /// Stable untranslated identifier, used for element ids so switching the locale cannot rebuild
    /// the strip's identity mid-session.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::General => "general",
            Self::Telegram => "telegram",
            Self::AutoBuy => "autobuy",
            Self::Special => "special",
            Self::Interface => "interface",
            Self::Hotkeys => "hotkeys",
            Self::AutoStart => "autostart",
        }
    }

    /// Element id of this page's body, built from the static [`Self::id`] rather than formatted per
    /// frame.
    pub(crate) fn element_id(self) -> ElementId {
        ElementId::Name(self.id().into())
    }

    /// Localized tab label from the `core_expert.tab_*` namespace.
    pub(crate) fn title(self) -> String {
        match self {
            Self::Login => t!("core_expert.tab_login"),
            // The two pages the compact popup also draws borrow ITS labels: one Moonbot page must
            // not be named differently by the two faces of the same gear.
            Self::General => t!("core_settings.tab_general"),
            Self::Telegram => t!("core_expert.tab_telegram"),
            Self::AutoBuy => t!("core_expert.tab_autobuy"),
            Self::Special => t!("core_expert.tab_special"),
            Self::Interface => t!("core_expert.tab_interface"),
            Self::Hotkeys => t!("core_expert.tab_hotkeys"),
            Self::AutoStart => t!("core_settings.tab_autostart"),
        }
        .to_string()
    }

    /// Add the sections THIS page draws to a mask.
    ///
    /// The unit an OK is built from: a surface may write only what it drew, and this window draws a
    /// different set per tab. Folding it over the tabs the user actually edited is what keeps an OK
    /// pressed on General from writing the Interface block this window seeded when it opened —
    /// which would silently revert whatever the Moonbot user changed there meanwhile.
    ///
    /// A page with nothing projected behind it adds nothing, so a window where only dead rows were
    /// touched sends no write at all.
    pub(crate) fn add_sections(self, mask: FieldMask) -> FieldMask {
        match self {
            // Two areas for one Moonbot page: the compact popup draws only the first, so the
            // seven rows below it are their own — see `moon_core::feed::OrderRulesSettings`.
            Self::General => mask.with_general().with_order_rules(),
            // Moonbot's AutoStart page draws `visual.blink_config` beside `trading.auto_start`, so
            // the page owns both sections.
            Self::AutoStart => mask.with_auto_start().with_btc_blink(),
            // Moonbot puts its three alert sounds on the Interface page, so that page owns the
            // `signals` section the compact popup draws them from.
            Self::Interface => mask.with_interface().with_signals(),
            Self::AutoBuy => mask.with_auto_buy(),
            Self::Telegram => mask.with_telegram(),
            Self::Special => mask.with_special(),
            // Only the mouse-gesture block of the Hotkeys page: the rest of it mirrors the
            // manual block, which no mask may reach.
            Self::Hotkeys => mask.with_gestures(),
            Self::Login => mask,
        }
    }

    /// How far this page's values reach — see [`TabSource`].
    ///
    /// `Projected` is what `moon_core::feed::CoreConfig` carries AND this window draws: the General
    /// page's exits and risk limits, the AutoStart page with its watchdogs and BTC blink, the
    /// Interface page's appearance block together with the alert sounds Moonbot puts on it, the
    /// AutoBuy page's signal sources and message filter, the Telegram page's channels, the Special
    /// page's engine switches with its logging and its order watchdog, and the Hotkeys page's
    /// mouse gestures. That is every page but Login, which is not on the wire at all.
    ///
    /// A `Projected` page is not necessarily projected in FULL — every one of the seven draws rows
    /// the snapshot does not carry. The rating answers "can this page be filled and sent at all",
    /// which is what decides whether the window prints a note OVER it; a row that cannot be filled
    /// answers for itself, by being disabled.
    pub(crate) fn source(self) -> TabSource {
        match self {
            // Login carries the API key and secret, the local password and the support identity —
            // none of which safe-share transports. Two of its lesser controls (the connection
            // variant, the log switches) DO travel, but not one field the page exists for.
            Self::Login => TabSource::Absent,
            Self::General
            | Self::AutoStart
            | Self::Interface
            | Self::AutoBuy
            | Self::Telegram
            | Self::Hotkeys
            | Self::Special => TabSource::Projected,
        }
    }

    /// The page at one position in the strip, used by the tab strip's index-based click.
    pub(crate) fn at(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }
}

#[cfg(test)]
mod tests;
