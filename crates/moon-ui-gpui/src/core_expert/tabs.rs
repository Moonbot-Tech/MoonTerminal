//! Pages of the expert core-settings window, one per tab of Moonbot's own Settings dialog.
//!
//! The strip reproduces Moonbot's order deliberately, including the pages this terminal cannot fill
//! yet and the ones it can never fill: a trader who knows that dialog reaches for a page by
//! POSITION, and hiding one would silently renumber every tab after it. Every tab opens; what is
//! blocked is the control with no value behind it — see [`TabSource`].

use gpui::ElementId;
use rust_i18n::t;

/// How far one page's values actually reach this window — the answer to "why can I not edit this".
///
/// There are two separate limits, and conflating them would make the window lie about which one a
/// page is behind:
///
/// 1. The WIRE is the safe-share configuration (`moonproto::shared_config`): a deliberately safe
///    subset of Moonbot's settings, carrying no secrets and no machine-local state.
/// 2. The PROJECTION is `moon_core::feed::CoreConfig`, the part of that subset this terminal reads
///    into typed fields and — through `FieldMask::RENDERED_SECTIONS` — is allowed to write back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TabSource {
    /// Both limits cleared: the values are on the wire AND in the terminal's projection, so this
    /// page's controls can be seeded and sent as soon as they are drawn.
    Projected,
    /// On the wire, but not projected yet. Drawing this page needs `CoreConfig` and the field mask
    /// extended first — until then there is nothing to seed a control from and nothing OK could
    /// carry, however complete the layout looks.
    Wire,
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
    /// Moonbot's setup wizard.
    Help,
    /// Licence and PRO activation.
    Pro,
}

impl ExpertTab {
    pub(crate) const ALL: [ExpertTab; 10] = [
        Self::Login,
        Self::General,
        Self::Telegram,
        Self::AutoBuy,
        Self::Special,
        Self::Interface,
        Self::Hotkeys,
        Self::AutoStart,
        Self::Help,
        Self::Pro,
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
            Self::Help => "help",
            Self::Pro => "pro",
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
            Self::Help => t!("core_expert.tab_help"),
            Self::Pro => t!("core_expert.tab_pro"),
        }
        .to_string()
    }

    /// How far this page's values reach — see [`TabSource`].
    ///
    /// `Projected` is exactly the five sections `moon_core::feed::CoreConfig` carries and
    /// `FieldMask::RENDERED_SECTIONS` may write: the General page's exits and risk limits, the
    /// AutoStart page with its watchdogs and BTC blink, and the alert sounds that live on Moonbot's
    /// AutoBuy page. Everything else is on the wire but unprojected, and Login/Help/PRO are not on
    /// the wire at all.
    ///
    /// AutoBuy is deliberately NOT `Projected`: the terminal projects only that page's alert-sound
    /// block, and calling the whole page ready would promise nine tenths of it that cannot be
    /// seeded.
    pub(crate) fn source(self) -> TabSource {
        match self {
            // Login carries the API key and secret, the local password and the support identity —
            // none of which safe-share transports. Two of its lesser controls (the connection
            // variant, the log switches) DO travel, but not one field the page exists for.
            Self::Login | Self::Help | Self::Pro => TabSource::Absent,
            Self::General | Self::AutoStart => TabSource::Projected,
            Self::Telegram | Self::AutoBuy | Self::Special | Self::Interface | Self::Hotkeys => {
                TabSource::Wire
            }
        }
    }

    /// The page at one position in the strip, used by the tab strip's index-based click.
    pub(crate) fn at(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }
}

#[cfg(test)]
mod tests;
