//! Settings window, ported from egui's `src/settings/*` and `window/settings_window.rs`.
//! The separate OS window gives draft-backed tabs a live `Backend.preview`; closing the window
//! discards unsaved changes, while the Storage tab applies its separately persisted settings
//! immediately.
//! Save writes to disk through `AppConfig::save`; the shared daily scheduler independently owns
//! recovery copies in `backups/settings/`.
//!
//! The window is split into pages like the egui original. Six strip buttons reach them:
//! Connections, Telegram, Station, Hotkeys, Interface, and General. Interface and General each
//! open an inner switch over the pages that used to be their own buttons. This module owns the
//! `SettingsView` state and `open`; tab state and `impl SettingsView` blocks live in submodules.
//! [`render`] owns the tab bar, header, body, and Save footer, [`apply`] owns persistence and
//! activation, and [`common`] provides the shared UI and draft-binding helpers re-exported below.

mod apply;
mod badges;
mod common;
mod connections;
mod frame;
mod general;
mod hotkeys;
mod import_preview;
mod interface;
#[cfg(test)]
mod layout_probe;
mod lines;
mod nav;
mod render;
mod security;
mod share;
mod sound_folder;
mod storage;
pub(crate) use storage::trades_cleanup_startup;
/// Open for the core move window, which takes the station form's address parser and progress
/// log; the tab's own items keep their narrower visibility.
pub(crate) mod telegram;
mod trade_sounds;

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use gpui::*;
use moon_ui::{
    IndexPath, MoonBackgroundPolicy, MoonCheckbox, MoonInputEvent, MoonInputState, MoonSelectEvent,
    MoonSelectItem, MoonSelectState, MoonSliderState, Root,
};
use rust_i18n::t;

use crate::Backend;
use crate::media::icons::IconSet;
use moon_core::config::{AppConfig, CoreSortMode, GroupConfig, Language, LiveScope, UiThemeMode};
use moon_core::db::valuation::ValuationMode;
use moon_core::market::MarketDataMode;
use moon_core::session::CoreId;

use badges::BadgesEd;
use common::{color_row, draft_color, draft_slider, open_folder};
use connections::ConnRow;
use interface::Iface;
use lines::Lines;

const SETTINGS_HEADER_H: f32 = 30.0;

/// One Settings page, in navigation order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Tab {
    Cores,
    TgBot,
    TgNotify,
    TgMessages,
    TgCore,
    StOverview,
    StCores,
    StTape,
    StMaint,
    Appearance,
    Lines,
    Badges,
    Hotkeys,
    General,
    Security,
    Storage,
    Sounds,
}

impl Tab {
    /// Every page in navigation order.
    pub(crate) const ALL: [Tab; 17] = [
        Tab::Cores,
        Tab::TgBot,
        Tab::TgNotify,
        Tab::TgMessages,
        Tab::TgCore,
        Tab::StOverview,
        Tab::StCores,
        Tab::StTape,
        Tab::StMaint,
        Tab::Appearance,
        Tab::Lines,
        Tab::Badges,
        Tab::Hotkeys,
        Tab::General,
        Tab::Security,
        Tab::Storage,
        Tab::Sounds,
    ];

    /// Stable, untranslated page id used for element ids.
    fn id(self) -> &'static str {
        match self {
            Tab::Cores => "cores",
            Tab::TgBot => "tg-bot",
            Tab::TgNotify => "tg-notify",
            Tab::TgMessages => "tg-msg",
            Tab::TgCore => "tg-core",
            Tab::StOverview => "st-over",
            Tab::StCores => "st-cores",
            Tab::StTape => "st-tape",
            Tab::StMaint => "st-maint",
            Tab::Appearance => "appearance",
            Tab::Lines => "lines",
            Tab::Badges => "badges",
            Tab::Hotkeys => "hotkeys",
            Tab::General => "general",
            Tab::Security => "security",
            Tab::Storage => "storage",
            Tab::Sounds => "sounds",
        }
    }

    /// Localized page name.
    fn title(self) -> String {
        match self {
            Tab::Cores => t!("settings.page.cores.title"),
            Tab::TgBot => t!("settings.page.tg_bot.title"),
            Tab::TgNotify => t!("settings.page.tg_notify.title"),
            Tab::TgMessages => t!("settings.page.tg_msg.title"),
            Tab::TgCore => t!("settings.page.tg_core.title"),
            Tab::StOverview => t!("settings.page.st_over.title"),
            Tab::StCores => t!("settings.page.st_cores.title"),
            Tab::StTape => t!("settings.page.st_tape.title"),
            Tab::StMaint => t!("settings.page.st_maint.title"),
            Tab::Appearance => t!("settings.page.appearance.title"),
            Tab::Lines => t!("settings.page.lines.title"),
            Tab::Badges => t!("settings.page.badges.title"),
            Tab::Hotkeys => t!("settings.page.hotkeys.title"),
            Tab::General => t!("settings.page.general.title"),
            Tab::Security => t!("settings.page.security.title"),
            Tab::Storage => t!("settings.page.storage.title"),
            Tab::Sounds => t!("settings.page.sounds.title"),
        }
        .to_string()
    }

    /// Localized one-line page description.
    fn desc(self) -> String {
        match self {
            Tab::Cores => t!("settings.page.cores.desc"),
            Tab::TgBot => t!("settings.page.tg_bot.desc"),
            Tab::TgNotify => t!("settings.page.tg_notify.desc"),
            Tab::TgMessages => t!("settings.page.tg_msg.desc"),
            Tab::TgCore => t!("settings.page.tg_core.desc"),
            Tab::StOverview => t!("settings.page.st_over.desc"),
            Tab::StCores => t!("settings.page.st_cores.desc"),
            Tab::StTape => t!("settings.page.st_tape.desc"),
            Tab::StMaint => t!("settings.page.st_maint.desc"),
            Tab::Appearance => t!("settings.page.appearance.desc"),
            Tab::Lines => t!("settings.page.lines.desc"),
            Tab::Badges => t!("settings.page.badges.desc"),
            Tab::Hotkeys => t!("settings.page.hotkeys.desc"),
            Tab::General => t!("settings.page.general.desc"),
            Tab::Security => t!("settings.page.security.desc"),
            Tab::Storage => t!("settings.page.storage.desc"),
            Tab::Sounds => t!("settings.page.sounds.desc"),
        }
        .to_string()
    }

    /// Localized search keywords, lowercase and space-separated.
    fn keywords(self) -> String {
        match self {
            Tab::Cores => t!("settings.page.cores.keywords"),
            Tab::TgBot => t!("settings.page.tg_bot.keywords"),
            Tab::TgNotify => t!("settings.page.tg_notify.keywords"),
            Tab::TgMessages => t!("settings.page.tg_msg.keywords"),
            Tab::TgCore => t!("settings.page.tg_core.keywords"),
            Tab::StOverview => t!("settings.page.st_over.keywords"),
            Tab::StCores => t!("settings.page.st_cores.keywords"),
            Tab::StTape => t!("settings.page.st_tape.keywords"),
            Tab::StMaint => t!("settings.page.st_maint.keywords"),
            Tab::Appearance => t!("settings.page.appearance.keywords"),
            Tab::Lines => t!("settings.page.lines.keywords"),
            Tab::Badges => t!("settings.page.badges.keywords"),
            Tab::Hotkeys => t!("settings.page.hotkeys.keywords"),
            Tab::General => t!("settings.page.general.keywords"),
            Tab::Security => t!("settings.page.security.keywords"),
            Tab::Storage => t!("settings.page.storage.keywords"),
            Tab::Sounds => t!("settings.page.sounds.keywords"),
        }
        .to_string()
    }

    /// Navigation group the page is listed under.
    fn group(self) -> NavGroup {
        match self {
            Tab::Cores => NavGroup::Connections,
            Tab::TgBot | Tab::TgNotify | Tab::TgMessages | Tab::TgCore => NavGroup::Telegram,
            Tab::StOverview | Tab::StCores | Tab::StTape | Tab::StMaint => NavGroup::Station,
            Tab::Appearance | Tab::Lines | Tab::Badges => NavGroup::Appearance,
            Tab::Hotkeys => NavGroup::Control,
            Tab::General | Tab::Security | Tab::Storage | Tab::Sounds => NavGroup::Common,
        }
    }

    /// How an edit on this page takes effect.
    ///
    /// Args:
    ///     station: Whether the Telegram bot runs on a known station.
    pub(crate) fn save_mode(self, station: bool) -> SaveMode {
        match self {
            // TgCore's settings live in the core itself, not in the terminal's config.
            Tab::StOverview | Tab::StCores | Tab::StTape | Tab::StMaint | Tab::TgCore => {
                SaveMode::Remote
            }
            Tab::TgBot | Tab::TgMessages | Tab::TgNotify if station => SaveMode::Remote,
            Tab::Storage | Tab::Sounds | Tab::TgNotify => SaveMode::Live,
            Tab::Cores
            | Tab::Appearance
            | Tab::Lines
            | Tab::Badges
            | Tab::Hotkeys
            | Tab::General
            | Tab::Security
            | Tab::TgBot
            | Tab::TgMessages => SaveMode::Draft,
        }
    }
}

/// Heading a run of pages in the navigation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NavGroup {
    Connections,
    Telegram,
    Station,
    Appearance,
    Control,
    Common,
}

impl NavGroup {
    /// Every group in navigation order.
    pub(crate) const ALL: [NavGroup; 6] = [
        NavGroup::Connections,
        NavGroup::Telegram,
        NavGroup::Station,
        NavGroup::Appearance,
        NavGroup::Control,
        NavGroup::Common,
    ];

    /// Localized group heading.
    fn title(self) -> String {
        match self {
            NavGroup::Connections => t!("settings.nav.group.connections"),
            NavGroup::Telegram => t!("settings.nav.group.telegram"),
            NavGroup::Station => t!("settings.nav.group.station"),
            NavGroup::Appearance => t!("settings.nav.group.appearance"),
            NavGroup::Control => t!("settings.nav.group.control"),
            NavGroup::Common => t!("settings.nav.group.common"),
        }
        .to_string()
    }
}

/// How an edit on a page takes effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SaveMode {
    /// Kept in the draft until Save.
    Draft,
    /// Written as soon as it is made.
    Live,
    /// Sent to the station by the page's own action.
    Remote,
}

/// A set of pages, one bit per [`Tab`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TabSet(u32);

impl TabSet {
    fn bit(tab: Tab) -> u32 {
        1 << Tab::ALL.iter().position(|t| *t == tab).unwrap_or(0)
    }

    pub(crate) fn insert(&mut self, tab: Tab) {
        self.0 |= Self::bit(tab);
    }

    pub(crate) fn contains(self, tab: Tab) -> bool {
        self.0 & Self::bit(tab) != 0
    }

    pub(crate) fn iter(self) -> impl Iterator<Item = Tab> {
        Tab::ALL.into_iter().filter(move |t| self.contains(*t))
    }

    pub(crate) fn len(self) -> usize {
        self.0.count_ones() as usize
    }
}

/// Data-source modes for the Connections tab, paired with stable i18n keys.
/// Labels are localized at the call site through `conn.market_dedup` and `conn.market_percore`.
const MODE_LABELS: [(&str, MarketDataMode); 2] = [
    ("conn.market_dedup", MarketDataMode::Dedup),
    ("conn.market_percore", MarketDataMode::PerCore),
];

/// Labels and values for the quote-money conversion selector, historical first: it is the default
/// and the one almost every user should keep.
const VALUATION_LABELS: [(&str, ValuationMode); 2] = [
    (
        "general.valuation_mode.historical",
        ValuationMode::Historical,
    ),
    ("general.valuation_mode.current", ValuationMode::Current),
];

/// Labels and values for the Live-scope selector (#688), the application-wide default first.
const LIVE_SCOPE_LABELS: [(&str, LiveScope); 2] = [
    ("general.live_scope.all", LiveScope::AllCharts),
    ("general.live_scope.each", LiveScope::EachChart),
];

/// Labels and values for the interface-theme selector, ordered light to dark so the list reads
/// as a brightness ramp rather than as the order the themes happened to be written in. The
/// experimental colour-role modes follow the reviewed three, as the same light-to-dark pair.
const THEME_MODE_LABELS: [(&str, UiThemeMode); 5] = [
    ("iface.light_theme", UiThemeMode::Light),
    ("iface.graphite_theme", UiThemeMode::Graphite),
    ("iface.dark_theme", UiThemeMode::Dark),
    (
        "iface.light_experimental_theme",
        UiThemeMode::LightExperimental,
    ),
    (
        "iface.dark_experimental_theme",
        UiThemeMode::DarkExperimental,
    ),
];

/// Labels and values for the global core-order selector.
const CORE_SORT_LABELS: [(&str, CoreSortMode); 3] = [
    ("conn.core_sort.name", CoreSortMode::Name),
    ("conn.core_sort.added", CoreSortMode::AddedOldest),
    ("conn.core_sort.added_newest", CoreSortMode::AddedNewest),
];

/// Footer status represented by an i18n key or a ready-to-display message.
///
/// Keys are resolved during rendering so changing languages cannot leave a cached status in the
/// previous locale. Ready-to-display messages are used for non-localized I/O errors.
pub(crate) enum StatusMsg {
    Key(&'static str),
    Text(String),
}

/// Settings editor whose draft is previewed in the backend and committed only on save.
pub struct SettingsView {
    backend: Entity<Backend>,
    active: Tab,
    /// Navigation search box; its text filters the page list.
    nav_query: Entity<MoonInputState>,
    /// Narrow-window page picker; its items follow [`Self::nav_pick_labels`].
    nav_pick: Entity<MoonSelectState<Tab>>,
    /// Captions the picker was last filled with, so it is refilled only when one changes.
    nav_pick_labels: Vec<String>,
    /// Numbers beside page names, recomputed only when [`Self::last_sig`] moves.
    nav_counts: nav::NavCounts,
    /// Pages whose config fields differ from the saved config; empty while the draft is clean.
    dirty_pages: TabSet,
    /// Per-page signatures of the SAVED config, refreshed only when a Save moves it.
    saved_page_sigs: SavedPageSigs,
    /// Keys of the `hint_row`s whose long help is unfolded. Not persisted.
    more_open: HashSet<&'static str>,
    /// The user chose to close without saving; the next close request goes through.
    close_confirmed: bool,
    // Per-page UI state for the pages rebuilt in phases 3-5. Each page's branch adds fields
    // inside its own struct; these lines stay as they are.
    appearance_ui: interface::AppearanceUi,
    lines_ui: lines::LinesUi,
    badges_ui: badges::BadgesUi,
    general_ui: general::GeneralUi,
    telegram_ui: telegram::TelegramUi,
    storage_ui: storage::StorageUi,
    sounds_ui: trade_sounds::SoundsUi,
    /// "Save and close" sent the station draft and waits for the station's answer to close.
    close_after_station: bool,
    /// Save status as `(message, is_error)`.
    status: Option<(StatusMsg, bool)>,
    iface: Iface,
    lines: Lines,
    /// Detect-type badge editor for the Badges tab; rebuilt after additions and removals.
    badges: BadgesEd,
    /// When the first-run hint on the Connections tab was armed, if it was.
    ///
    /// The Settings gear got the user here; this points at the next thing to do. It targets ONE
    /// control at a time and which one depends on the draft: with no rows yet there is no key field
    /// to point at, so the add button carries it.
    conn_hint_at: Option<std::time::Instant>,
    /// Whether the hint's repaint chain is already running, so arming twice cannot stack timers.
    conn_hint_armed: bool,
    /// Per-server editor states for the Connections tab; rebuilt after additions and removals.
    conn: Vec<ConnRow>,
    /// UI-zoom slider for the personal geometry scale in settings.toml.
    ui_zoom: Entity<MoonSliderState>,
    /// Immediate, automatically persisted trade-only loudness slider.
    trade_volume: Entity<MoonSliderState>,
    /// Interface-theme selector for the General tab.
    theme_mode: Entity<MoonSelectState<UiThemeMode>>,
    /// Language selector for the General tab.
    lang: Entity<MoonSelectState<Language>>,
    /// Quote-money conversion selector for the General tab.
    valuation: Entity<MoonSelectState<ValuationMode>>,
    /// Live-scope selector for the General tab's charts section (#688).
    live_scope: Entity<MoonSelectState<LiveScope>>,
    /// Data-source selector for the Connections tab.
    mode: Entity<MoonSelectState<MarketDataMode>>,
    /// Order selector shared by every core list on the Connections tab.
    core_sort: Entity<MoonSelectState<CoreSortMode>>,
    /// Active Hotkeys category, matching Moonbot's hotkey pages.
    hotkeys_group: hotkeys::HotkeyGroup,
    /// Hotkeys search box: a command name, a MoonBot name or a key; searches every category.
    hotkeys_query: Entity<MoonInputState>,
    /// The Hotkeys table shows only the rows with a conflict or a bare-key warning.
    hotkeys_conflicts_only: bool,
    /// Hotkeys rows (by row id) whose gesture editor "+ жест" revealed while still unset.
    hotkeys_gesture_open: HashSet<String>,
    /// The built-in combos block on the Hotkeys page is unfolded.
    hotkeys_builtin_open: bool,
    /// Core and `core_config_recv_rev` baseline of an in-flight "pull hotkey layout from core"
    /// request, or `None` when none is pending. The baseline is the revision seen when the pull
    /// was requested; the request is settled once the store's revision has moved past it.
    core_pull: Option<(CoreId, u64)>,
    /// Storage-tab state: `storage.toml` configuration plus a background size/count snapshot.
    storage: storage::StorageEd,
    /// Group-icon cache for the Connections tab.
    icons: IconSet,
    /// Group whose icon picker is open, or `None` when closed; ported from egui's `picking`.
    picking: Option<String>,
    /// Row key of the core whose card the Cores page shows. Re-validated by every rebuild of the
    /// core rows (`rebuild_conn`) and on every frame (`valid_selection`).
    conn_selected: Option<u64>,
    /// The backend's count of core moves that switched a row, as the Connections fields last
    /// showed it (`Backend::core_moves_switched`).
    core_moves_seen: u64,
    /// Signature of data consumed by Settings: draft/configuration fields plus session statuses.
    last_sig: u64,
    /// Whether the CONFIG draft differed from the saved config as of the last backend
    /// notification.
    ///
    /// Kept beside [`Self::last_sig`] because it is the SECOND repaint trigger: `settings_sig`
    /// deliberately ignores `transport` and `chart_bundle`, so a keystroke in the Charts field or
    /// a pick in the Proto dropdown moves this flag while leaving that signature untouched, and
    /// the footer would never learn about it.
    ///
    /// It carries the CONFIG term ONLY, and the observer below is its ONLY writer. The footer
    /// combines it with the pending-password term locally and writes nothing back: a field
    /// written under one definition and compared under another masks exactly the transitions it
    /// exists to catch. Reading it in `render` rather than recomputing also keeps `draft_sig`
    /// off the render path, where 56 servers would be serialized twice per frame.
    draft_dirty: bool,
    /// Last valid Main auto-close timeout retained for this Settings session.
    ///
    /// Disabling the checkbox writes zero to the draft; re-enabling restores this value instead
    /// of the 120-second default. Zero means no value has been retained, so `IDLE_DEFAULT_SECS` is
    /// used. `Cell` lets `general_tab(&self)` update it while rendering.
    idle_last_secs: std::cell::Cell<u32>,
    /// Open MoonBot settings import preview, or `None` when closed; see [`import_preview`].
    import: Option<import_preview::ImportState>,
    /// Launch and `servers.enc` password editors on the General tab; see [`security`].
    security: security::SecurityEd,
    /// Telegram tab editor; see [`telegram`].
    telegram: telegram::TelegramEd,
}

impl SettingsView {
    /// Builds the shared draft-backed checkbox used by the Lines, Connections, and General tabs.
    ///
    /// The supplied value initializes the control. On change, `apply` updates `Backend.preview`;
    /// the backend and view are notified only when the setter reports a change. The caller adds
    /// the label and size to the returned base `MoonCheckbox`.
    pub(super) fn draft_checkbox(
        &self,
        cx: &Context<Self>,
        id: impl Into<SharedString>,
        init: bool,
        apply: impl Fn(&mut AppConfig, bool) -> bool + 'static,
    ) -> MoonCheckbox {
        MoonCheckbox::new(id.into())
            .checked(init)
            .on_change(cx.listener(move |this, ch: &bool, _w, cx| {
                let v = *ch;
                let changed = this.backend.update(cx, |b, bcx| {
                    let mut changed = false;
                    if let Some(p) = b.preview.as_mut()
                        && apply(p, v)
                    {
                        bcx.notify();
                        changed = true;
                    }
                    changed
                });
                if changed {
                    cx.notify();
                }
            }))
    }

    /// Build Settings window state and synchronized selectors from the current draft.
    ///
    /// Args:
    ///     backend: Application backend that owns the settings draft and live session state.
    ///     window: Newly opened Settings window.
    ///     cx: Application context used to create controls and subscriptions.
    ///
    /// Returns:
    ///     Fully initialized Settings state with a retained Connections virtual-list scroll handle.
    fn new(
        backend: Entity<Backend>,
        tab: Tab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let iface = interface::build(&backend, window, cx);
        let lines = lines::build(&backend, window, cx);
        let badges = badges::build(&backend, window, cx);
        let conn = connections::build_conn(&backend, window, cx);
        let security_ed = security::build(&backend, window, cx);
        let telegram_ed = telegram::build(&backend, window, cx);

        // The zoom slider reinstalls the draft theme only on release; dragging keeps its percentage caption live.
        let ui_zoom = general::build_zoom(&backend, cx);
        let draft = {
            let b = backend.read(cx);
            b.preview.as_ref().unwrap_or(&b.config).clone()
        };
        let appearance_ui = interface::AppearanceUi::init();
        let lines_ui = lines::LinesUi::init(&draft, window, cx);
        let general_ui = general::GeneralUi::init(&draft, window, cx);
        let storage_ed = storage::build();
        let storage_ui = storage::StorageUi::init(&storage_ed.cfg, window, cx);
        let trade_volume = trade_sounds::build_volume(&backend, cx);

        // Persist the Settings window position and size in layout so it reopens in the same place.
        // The debounced persistence loop drains `layout_dirty`, as it does for Strategies/Assets.
        cx.observe_window_bounds(window, |this, window, cx| {
            let geom = crate::window::windowing::window_geom_rect(window, cx);
            this.backend.update(cx, |b, _| {
                let geom = geom.keeping_display_of(b.layout.settings_window);
                if b.layout.settings_window != Some(geom) {
                    b.layout.settings_window = Some(geom);
                    b.layout_dirty = true;
                }
            });
        })
        .detach();

        // Initialize the language dropdown, ported from egui's ComboBox, from the current draft.
        let (cur_lang, cur_mode, cur_core_sort, cur_valuation, cur_theme_mode, cur_live_scope) = {
            let b = backend.read(cx);
            let d = b.preview.as_ref().unwrap_or(&b.config);
            (
                d.language,
                d.market_mode,
                d.core_sort,
                d.report_valuation_mode,
                d.ui_theme_mode,
                d.live_scope,
            )
        };

        // Interface theme. Unlike every other selector here it is consumed LIVE from the draft:
        // the palette is installed on change and rolls back with the draft if Settings closes
        // unsaved, which is what makes picking a theme feel like previewing one.
        let theme_mode_items = THEME_MODE_LABELS
            .iter()
            .map(|(key, mode)| MoonSelectItem::new(*mode, t!(*key).to_string()))
            .collect::<Vec<_>>();
        let theme_mode_idx = THEME_MODE_LABELS
            .iter()
            .position(|(_, m)| *m == cur_theme_mode)
            .unwrap_or(0);
        let theme_mode = cx.new(|cx| {
            MoonSelectState::new(
                theme_mode_items,
                Some(IndexPath::new(theme_mode_idx)),
                window,
                cx,
            )
        });
        // `subscribe_in` rather than `subscribe`: the per-mode colour editors below need a
        // `&mut Window` to be rebuilt, and a plain subscription does not carry one.
        cx.subscribe_in(
            &theme_mode,
            window,
            |this, _e, ev: &MoonSelectEvent<UiThemeMode>, window, cx| {
                let MoonSelectEvent::Confirm(Some(mode)) = ev else {
                    return;
                };
                let mode = *mode;
                let changed = this.backend.update(cx, |b, bcx| {
                    let Some(p) = b.preview.as_mut() else {
                        return false;
                    };
                    if p.ui_theme_mode == mode {
                        return false;
                    }
                    p.ui_theme_mode = mode;
                    crate::install_moon_theme_for_config(p, bcx);
                    bcx.notify();
                    true
                });
                if changed {
                    // The per-mode editors hold a SNAPSHOT of the variant that was live when
                    // their widgets were built: a slider or colour picker keeps its own state
                    // entity and does not re-read the draft on render. The write side resolves
                    // the mode live, so leaving those snapshots in place would show one
                    // variant's numbers while the next edit wrote them into the other. Rebuild
                    // them here, exactly as paste and import already do.
                    this.iface = interface::build(&this.backend, window, cx);
                    this.lines = lines::build(&this.backend, window, cx);
                    this.badges = badges::build(&this.backend, window, cx);
                    cx.notify();
                }
            },
        )
        .detach();
        let lang_items = Language::ALL
            .iter()
            .map(|l| MoonSelectItem::new(*l, l.label()))
            .collect::<Vec<_>>();
        let lang_idx = Language::ALL
            .iter()
            .position(|l| *l == cur_lang)
            .unwrap_or(0);
        let lang = cx
            .new(|cx| MoonSelectState::new(lang_items, Some(IndexPath::new(lang_idx)), window, cx));
        cx.subscribe(&lang, |this, _e, ev: &MoonSelectEvent<Language>, cx| {
            if let MoonSelectEvent::Confirm(Some(language)) = ev {
                let language = *language;
                this.backend.update(cx, |b, bcx| {
                    if let Some(p) = b.preview.as_mut() {
                        p.language = language;
                        bcx.notify();
                    }
                });
            }
        })
        .detach();

        // Quote-money conversion. It lives here rather than on the Report or Analytics toolbars
        // because it is an expert setting almost nobody should touch: the default answers "what was
        // this trade worth when it closed", which is the right question for nearly every user.
        let valuation_items = VALUATION_LABELS
            .iter()
            .map(|(key, mode)| MoonSelectItem::new(*mode, t!(*key).to_string()))
            .collect::<Vec<_>>();
        let valuation_idx = VALUATION_LABELS
            .iter()
            .position(|(_, m)| *m == cur_valuation)
            .unwrap_or(0);
        let valuation = cx.new(|cx| {
            MoonSelectState::new(
                valuation_items,
                Some(IndexPath::new(valuation_idx)),
                window,
                cx,
            )
        });
        cx.subscribe(
            &valuation,
            |this, _e, ev: &MoonSelectEvent<ValuationMode>, cx| {
                if let MoonSelectEvent::Confirm(Some(mode)) = ev {
                    let mode = *mode;
                    this.backend.update(cx, |b, bcx| {
                        if let Some(p) = b.preview.as_mut() {
                            p.report_valuation_mode = mode;
                            bcx.notify();
                        }
                    });
                }
            },
        )
        .detach();

        // Live scope (#688). Consumed live from the draft by every chart, like the book-zone pan,
        // and rolled back with the draft if Settings closes unsaved.
        let live_scope_items = LIVE_SCOPE_LABELS
            .iter()
            .map(|(key, scope)| MoonSelectItem::new(*scope, t!(*key).to_string()))
            .collect::<Vec<_>>();
        let live_scope_idx = LIVE_SCOPE_LABELS
            .iter()
            .position(|(_, s)| *s == cur_live_scope)
            .unwrap_or(0);
        let live_scope = cx.new(|cx| {
            MoonSelectState::new(
                live_scope_items,
                Some(IndexPath::new(live_scope_idx)),
                window,
                cx,
            )
        });
        cx.subscribe(
            &live_scope,
            |this, _e, ev: &MoonSelectEvent<LiveScope>, cx| {
                if let MoonSelectEvent::Confirm(Some(scope)) = ev {
                    let scope = *scope;
                    this.backend.update(cx, |b, bcx| {
                        if let Some(p) = b.preview.as_mut() {
                            p.live_scope = scope;
                            bcx.notify();
                        }
                    });
                }
            },
        )
        .detach();

        // Build the data-source dropdown, ported from egui's ComboBox.
        let mode_items = MODE_LABELS
            .iter()
            .map(|(key, mode)| MoonSelectItem::new(*mode, t!(*key).to_string()))
            .collect::<Vec<_>>();
        let mode_idx = MODE_LABELS
            .iter()
            .position(|(_, m)| *m == cur_mode)
            .unwrap_or(0);
        let mode = cx
            .new(|cx| MoonSelectState::new(mode_items, Some(IndexPath::new(mode_idx)), window, cx));
        cx.subscribe(
            &mode,
            |this, _e, ev: &MoonSelectEvent<MarketDataMode>, cx| {
                if let MoonSelectEvent::Confirm(Some(mode)) = ev {
                    let mode = *mode;
                    this.backend.update(cx, |b, bcx| {
                        if let Some(p) = b.preview.as_mut() {
                            p.market_mode = mode;
                            bcx.notify();
                        }
                    });
                }
            },
        )
        .detach();

        // One order mode controls every core list.
        let core_sort_items = CORE_SORT_LABELS
            .iter()
            .map(|(key, mode)| MoonSelectItem::new(*mode, t!(*key).to_string()))
            .collect::<Vec<_>>();
        let core_sort_idx = CORE_SORT_LABELS
            .iter()
            .position(|(_, m)| *m == cur_core_sort)
            .unwrap_or(0);
        let core_sort = cx.new(|cx| {
            MoonSelectState::new(
                core_sort_items,
                Some(IndexPath::new(core_sort_idx)),
                window,
                cx,
            )
        });
        cx.subscribe(
            &core_sort,
            |this, _e, ev: &MoonSelectEvent<CoreSortMode>, cx| {
                if let MoonSelectEvent::Confirm(Some(mode)) = ev {
                    let mode = *mode;
                    this.backend.update(cx, |b, bcx| {
                        if let Some(p) = b.preview.as_mut() {
                            p.core_sort = mode;
                            bcx.notify();
                        }
                    });
                }
            },
        )
        .detach();

        let nav_query = cx.new(|cx| {
            MoonInputState::new(window, cx)
                .placeholder(t!("settings.search.placeholder").to_string())
        });
        cx.subscribe(&nav_query, |_, _, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        let hotkeys_query = cx.new(|cx| {
            MoonInputState::new(window, cx)
                .placeholder(t!("hotkeys.search.placeholder").to_string())
        });
        cx.subscribe(&hotkeys_query, |_, _, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        let nav_pick = cx.new(|cx| {
            let mut state = MoonSelectState::new(Vec::new(), None, window, cx);
            state.set_selected_value(&tab);
            state
        });
        cx.subscribe(&nav_pick, |this, _, ev: &MoonSelectEvent<Tab>, cx| {
            if let MoonSelectEvent::Confirm(Some(tab)) = ev {
                this.show_page(*tab, cx);
            }
        })
        .detach();

        let initial_sig = settings_sig(backend.read(cx));
        let core_moves_seen = backend.read(cx).core_moves_switched;
        let initial_counts = nav_counts(backend.read(cx));
        let initial_dirty = backend_dirty(backend.read(cx));
        let initial_saved = SavedPageSigs::of(&backend.read(cx).config);
        let initial_pages = backend_dirty_pages(backend.read(cx), initial_dirty, &initial_saved);
        cx.observe(&backend, |this, backend, cx| {
            let b = backend.read(cx);
            let sig = settings_sig(b);
            // The dirty flag is a SECOND repaint trigger, not a consequence of the first:
            // `settings_sig` skips `transport` and `chart_bundle`, so an edit to either moves
            // only this one and the footer would otherwise keep painting the stale caption.
            // It flips at most once per clean-to-dirty transition, so this costs one repaint,
            // not one per keystroke.
            let dirty = backend_dirty(b);
            let mut changed = false;
            if sig != this.last_sig || dirty != this.draft_dirty {
                if sig != this.last_sig {
                    let diverging = this.nav_counts.station_diverging;
                    this.nav_counts = nav_counts(b);
                    this.nav_counts.station_diverging = diverging;
                }
                this.last_sig = sig;
                this.draft_dirty = dirty;
                changed = true;
            }
            // The station's count moves with the station's answers, not with the draft.
            let diverging = this.station_diverging_count(cx);
            if diverging != this.nav_counts.station_diverging {
                this.nav_counts.station_diverging = diverging;
                changed = true;
            }
            // Per-page dirt is hashed only while the draft is dirty at all, and then on every
            // notify: an edit to a field `settings_sig` skips moves no other trigger.
            // The saved side moves only on Save, so its page signatures are cached and every
            // notify hashes the draft alone.
            this.saved_page_sigs.refresh(&b.config);
            let pages = backend_dirty_pages(b, dirty, &this.saved_page_sigs);
            if pages != this.dirty_pages {
                this.dirty_pages = pages;
                changed = true;
            }
            // A moved core's new address reaches the draft from outside; the render that sees
            // the count move rebuilds the Connections fields (it has the window they need).
            changed |= b.core_moves_switched != this.core_moves_seen;
            if changed {
                cx.notify();
            }
        })
        .detach();

        // Closing with unsaved work asks first; see `SettingsView::should_close`.
        let view = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, app| {
            view.update(app, |this, cx| this.should_close(window, cx))
                .unwrap_or(true)
        });

        // Dropping the window view discards the draft and restores the saved theme, cancelling
        // unsaved live-preview changes as in the egui implementation.
        cx.on_release(|this, app| {
            this.backend.update(app, |b, cx| {
                crate::install_moon_theme_for_config(&b.config, cx);
                b.preview = None;
                b.settings_window = None;
                cx.notify();
            });
        })
        .detach();
        Self {
            // Armed at construction, never from `render`. Read from the SAVED config: a draft row
            // the user is halfway through typing is not a configured core.
            conn_hint_at: (!backend.read(cx).config.core_ever_configured())
                .then(std::time::Instant::now),
            conn_hint_armed: false,
            backend,
            active: tab,
            nav_query,
            nav_pick,
            nav_pick_labels: Vec::new(),
            nav_counts: initial_counts,
            status: None,
            iface,
            lines,
            badges,
            conn,
            ui_zoom,
            trade_volume,
            theme_mode,
            lang,
            valuation,
            live_scope,
            mode,
            core_sort,
            hotkeys_group: hotkeys::HotkeyGroup::Presets,
            hotkeys_query,
            hotkeys_conflicts_only: false,
            hotkeys_gesture_open: HashSet::new(),
            hotkeys_builtin_open: false,
            core_pull: None,
            storage: storage_ed,
            icons: IconSet::discover(),
            picking: None,
            conn_selected: None,
            core_moves_seen,
            last_sig: initial_sig,
            draft_dirty: initial_dirty,
            dirty_pages: initial_pages,
            saved_page_sigs: initial_saved,
            more_open: HashSet::new(),
            close_confirmed: false,
            appearance_ui,
            lines_ui,
            badges_ui: Default::default(),
            general_ui,
            telegram_ui: Default::default(),
            storage_ui,
            sounds_ui: Default::default(),
            close_after_station: false,
            idle_last_secs: std::cell::Cell::new(0),
            import: None,
            security: security_ed,
            telegram: telegram_ed,
        }
    }
}

/// Hash settings whose changes require refreshing the open window's editor state.
///
/// Args:
///     b: Backend containing the active draft or saved configuration and live session state.
///
/// Returns:
///     A signature for configuration and status inputs that affect Settings rendering.
fn settings_sig(b: &Backend) -> u64 {
    let cfg = b.preview.as_ref().unwrap_or(&b.config);
    let mut h = DefaultHasher::new();

    b.quiet_sleeping.hash(&mut h);
    cfg.language.code().hash(&mut h);
    cfg.market_mode.code().hash(&mut h);
    cfg.core_sort.hash(&mut h);
    cfg.report_valuation_mode.hash(&mut h);
    cfg.live_scope.hash(&mut h);
    cfg.charts_split_by_core.hash(&mut h);
    cfg.charts_stack_scroll.hash(&mut h);
    cfg.charts_stack_compress.hash(&mut h);
    cfg.chart_stack_height.hash(&mut h);
    cfg.log_to_file.hash(&mut h);
    cfg.log_retention_days.hash(&mut h);
    cfg.ui_theme_mode.hash(&mut h);
    cfg.ui_scale.to_bits().hash(&mut h);
    cfg.hotkeys.hash(&mut h);
    format!("{:?}", cfg.theme).hash(&mut h);
    format!("{:?}", cfg.orders).hash(&mut h);
    format!("{:?}", cfg.badges).hash(&mut h);

    cfg.servers.len().hash(&mut h);
    for s in &cfg.servers {
        s.id.hash(&mut h);
        s.uid.hash(&mut h);
        s.name.hash(&mut h);
        s.active.hash(&mut h);
        s.feed.orders.hash(&mut h);
        s.feed.detects.hash(&mut h);
        s.feed.reports.hash(&mut h);
        s.feed.balance.hash(&mut h);
        s.feed.strategies.hash(&mut h);
        s.feed.log.hash(&mut h);
        s.feed.alerts.hash(&mut h);
        s.feed.arb.hash(&mut h);
        s.feed.log_delivery.hash(&mut h);
        // Key input and Paste callbacks explicitly refresh draft endpoint cells.
        // Keep secret contents out of this broader settings signature.
        s.key.is_empty().hash(&mut h);
        s.group.hash(&mut h);
        s.market.hash(&mut h);
        s.color.hash(&mut h);
        s.synthetic.hash(&mut h);
    }

    // Token plaintext is never hashed; emptiness plus the non-secret preference fields
    // are enough to repaint the Telegram tab.
    b.telegram.revision.hash(&mut h);
    // The station job's progress lives in the backend (the Station tab).
    b.station.revision.hash(&mut h);
    cfg.telegram.token.is_empty().hash(&mut h);
    cfg.telegram.authorized_chat_ids.hash(&mut h);
    cfg.telegram.owner_chat_id.hash(&mut h);
    cfg.telegram.chat_access.hash(&mut h);
    cfg.telegram.mini_app_enabled.hash(&mut h);
    cfg.telegram.bot.hash(&mut h);

    cfg.groups.len().hash(&mut h);
    for g in &cfg.groups {
        g.name.hash(&mut h);
        g.active.hash(&mut h);
        g.icon.hash(&mut h);
    }

    // The Connections tab renders a connection VERDICT, not just a status dot, so its repaint
    // signature has to cover every input that verdict reads. Hashing the coarse status alone left
    // the tooltip frozen at an earlier init step for as long as `Stage(..)` held the same value —
    // which is most of a slow startup, exactly when someone is watching it.
    let faults = b.session.fault_map();
    let startups = b.session.startup_map();
    let mut statuses = b.session.status_map().into_iter().collect::<Vec<_>>();
    statuses.sort_by_key(|(id, _)| *id);
    for (id, status) in statuses {
        id.hash(&mut h);
        format!("{status:?}").hash(&mut h);
        // The fault is hashed by the few fields the tooltip turns into words, not by Debug-
        // serialising the whole record: a `ConnFault` carries an entire frozen startup snapshot,
        // and formatting that per core per notify is a string built only to be thrown away.
        if let Some(f) = faults.get(&id) {
            std::mem::discriminant(&f.kind).hash(&mut h);
            f.identity.hash_into(&mut h);
        }
        // Only the fields the tooltip actually shows: hashing the whole snapshot would churn the
        // signature on byte counters that move every poll and change nothing a reader can see.
        if let Some(s) = startups.get(&id) {
            format!("{:?}", s.state).hash(&mut h);
            format!("{:?}", s.current_step).hash(&mut h);
            s.completed_mask.hash(&mut h);
            (s.elapsed_ms / 1000).hash(&mut h);
        }
        if let Some(data) = b.session.store().core(id) {
            data.telegram_rev.hash(&mut h);
            data.telegram_fresh.hash(&mut h);
        }
    }

    h.finish()
}

/// Feed a serde stream straight into a hasher, so no serialized copy is ever allocated.
///
/// `ServerConfig::key` is a `Secret`, which is `#[serde(transparent)]` over the PLAINTEXT core
/// key (`moon_core::config::secrets`). `serde_json::to_string`/`to_vec` would therefore build a
/// `String` holding every one of the user's Moonbot keys -- unzeroized, once per check. Streaming
/// into the hasher allocates nothing and leaves no plaintext copy behind.
struct HashSink<'a>(&'a mut DefaultHasher);

impl std::io::Write for HashSink<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        Hasher::write(self.0, buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Fold one serializable config aggregate into the running signature.
///
/// Serde is what makes this EXHAUSTIVE per struct: a field added to `ServerConfig` or
/// `GroupConfig` joins the signature without anyone remembering to list it here.
///
/// INVARIANT for whoever adds the next field: every type hashed here must have an INFALLIBLE
/// `Serialize`. Today all of them do, so the error branch below is unreachable. It is not
/// merely a fallback if that ever changes: `HashSink` forwards each buffer to the hasher before
/// any error is known, so a mid-stream failure leaves the bytes written so far mixed in, and two
/// configs differing only AFTER the common failure point would hash equal. A fallible field
/// needs a fixed sentinel or a buffered path, not this branch. The one shape that introduces
/// one is a map with non-string or non-finite-float keys.
///
/// KNOWN LIMIT, deliberately not chased: `serde_json` writes a non-finite `f32`/`f64` as `null`
/// rather than failing, so `NaN` and `+Infinity` in the same nested float hash equal. Only a
/// hand-edited TOML can produce one, the effect is confined to this caption, and Save is never
/// gated on the flag -- the scalar floats this function hashes directly go through `to_bits()`
/// and are unaffected.
///
/// Args:
///     h: Running signature sink that receives the serialized bytes.
///     value: Config aggregate to serialize into the signature.
///
/// Returns:
///     Nothing; appends the aggregate's JSON representation to `h`.
fn hash_json<T: serde::Serialize>(h: &mut DefaultHasher, value: &T) {
    if let Err(err) = serde_json::to_writer(HashSink(h), value) {
        err.to_string().hash(h);
    }
}

/// Reduce a config's group list to exactly what a Save would persist.
///
/// `AppConfig::save_impl` runs `ensure_server_group_configs` (add a row for every server group
/// that lacks one) and then `prune_orphan_groups` (drop every row no server references). The net
/// persisted set is therefore one row per DISTINCT server group name, and this restates that in
/// one rule -- restated rather than called because `prune_orphan_groups` is private to
/// `moon_core::config`.
///
/// Doing it on BOTH sides of the comparison is what makes the dirty check answer "would saving
/// change anything?" instead of "do these two structs differ?". Two live failures depend on it:
/// the window seeds its draft through `sync_groups_from_servers`, so an un-normalized compare
/// reads dirty the moment Settings opens; and a group rename typed and then typed back leaves the
/// intermediate row behind in the draft (`connections/tests.rs` proves it survives until save),
/// so a reverted edit would report unsaved changes forever.
///
/// Args:
///     cfg: Saved configuration or live draft to normalize.
///
/// Returns:
///     The group rows a Save would persist for the configuration's current server groups.
fn canonical_groups(cfg: &AppConfig) -> Vec<GroupConfig> {
    let mut names: Vec<&str> = cfg.servers.iter().map(|s| s.group.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    names
        .into_iter()
        .map(|name| {
            cfg.groups
                .iter()
                .find(|g| g.name == name)
                .cloned()
                .unwrap_or_else(|| GroupConfig::new(name))
        })
        .collect()
}

/// Signature of everything a Settings Save would write, for the draft-vs-saved comparison.
///
/// Distinct from [`settings_sig`] beside it, which is a REPAINT gate and is deliberately lossy --
/// it hashes only `key.is_empty()` and skips `transport`, `chart_bundle` and
/// `workspace_membership` entirely, so reusing it here would call a changed core key or a changed
/// transport mode "no changes". Distinct from `AppConfig::structural_sig`, which neutralizes
/// exactly the presentation fields the Connections tab edits.
///
/// Three `AppConfig` fields are deliberately EXCLUDED, named here so the `theme_contract`
/// exhaustiveness test can see them and so a reader knows the omission was a decision:
/// `next_uid` moves only inside `save`, which then writes the same candidate into both the draft
/// and the saved config; `settings_unreadable` and `chart_core_remap_needed` are runtime flags no
/// tab edits. Every other field is covered.
///
/// Args:
///     cfg: Saved config or live draft; both sides go through this identically.
///
/// Returns:
///     A signature equal for two configs that would persist the same bytes.
fn draft_sig(cfg: &AppConfig) -> u64 {
    let mut h = DefaultHasher::new();

    hash_json(&mut h, &cfg.servers);
    hash_json(&mut h, &canonical_groups(cfg));
    hash_json(&mut h, &cfg.core_groups);
    hash_json(&mut h, &cfg.hotkeys);
    hash_json(&mut h, &cfg.theme);
    hash_json(&mut h, &cfg.orders);
    hash_json(&mut h, &cfg.badges);
    // Streams the Telegram aggregate, including the token, into the hasher without allocating
    // a plaintext buffer. See [`hash_json`].
    hash_json(&mut h, &cfg.telegram);

    // `MarketDataMode` and `ValuationMode` are not `Serialize`, so they take the same
    // stable-code path `settings_sig` uses; the rest are plain scalars.
    cfg.language.code().hash(&mut h);
    cfg.market_mode.code().hash(&mut h);
    cfg.core_sort.hash(&mut h);
    cfg.report_valuation_mode.hash(&mut h);
    cfg.live_scope.hash(&mut h);
    cfg.ui_theme_mode.hash(&mut h);
    cfg.charts_split_by_core.hash(&mut h);
    cfg.charts_stack_scroll.hash(&mut h);
    cfg.charts_stack_compress.hash(&mut h);
    cfg.chart_stack_height.hash(&mut h);
    cfg.separate_control_zones.hash(&mut h);
    cfg.main_idle_close_secs.hash(&mut h);
    cfg.log_to_file.hash(&mut h);
    cfg.log_retention_days.hash(&mut h);
    cfg.chart_memory_percent.hash(&mut h);
    cfg.ui_scale.to_bits().hash(&mut h);

    h.finish()
}

/// Whether saving the draft would change anything on disk.
///
/// Args:
///     saved: The config as it currently sits on disk.
///     draft: The Settings window's live draft.
///
/// Returns:
///     `true` when a Save would persist something different.
pub(super) fn draft_dirty(saved: &AppConfig, draft: &AppConfig) -> bool {
    draft_sig(saved) != draft_sig(draft)
}

/// Whether the backend currently holds a draft that differs from the saved config.
///
/// Args:
///     b: Settings backend whose saved configuration and optional draft are compared.
///
/// Returns:
///     `true` when an open draft would change the saved configuration; `false` without a draft.
fn backend_dirty(b: &Backend) -> bool {
    b.preview
        .as_ref()
        .is_some_and(|draft| draft_dirty(&b.config, draft))
}

/// `draft_sig` fields no page owns: they have no control in the window, so a change to one
/// dirties the draft without putting a dot on any page. Read as source by `theme_contract`.
#[allow(dead_code)]
const NO_PAGE_FIELDS: [&str; 4] = [
    "charts_stack_scroll",
    "charts_stack_compress",
    "chart_stack_height",
    "chart_memory_percent",
];

/// Signature of the config fields one page edits, or `None` for a page that edits none.
///
/// Every field `draft_sig` hashes belongs to exactly one page here or to [`NO_PAGE_FIELDS`];
/// `theme_contract` holds the two lists together. Secrets stream through [`hash_json`] like in
/// `draft_sig`, so no plaintext copy is built.
///
/// Args:
///     cfg: Saved config or live draft; both sides go through this identically.
///     tab: Page whose fields are hashed.
///
/// Returns:
///     A signature equal for two configs that agree on the page's fields.
fn page_sig(cfg: &AppConfig, tab: Tab) -> Option<u64> {
    let mut h = DefaultHasher::new();
    match tab {
        Tab::Cores => {
            hash_json(&mut h, &cfg.servers);
            hash_json(&mut h, &canonical_groups(cfg));
            hash_json(&mut h, &cfg.core_groups);
            cfg.market_mode.code().hash(&mut h);
            cfg.core_sort.hash(&mut h);
        }
        Tab::Hotkeys => hash_json(&mut h, &cfg.hotkeys),
        Tab::Appearance => {
            hash_json(&mut h, &cfg.theme);
            cfg.ui_theme_mode.hash(&mut h);
            cfg.ui_scale.to_bits().hash(&mut h);
        }
        Tab::Lines => hash_json(&mut h, &cfg.orders),
        Tab::Badges => hash_json(&mut h, &cfg.badges),
        Tab::General => {
            cfg.language.code().hash(&mut h);
            cfg.report_valuation_mode.hash(&mut h);
            cfg.live_scope.hash(&mut h);
            cfg.charts_split_by_core.hash(&mut h);
            cfg.separate_control_zones.hash(&mut h);
            cfg.main_idle_close_secs.hash(&mut h);
            cfg.log_to_file.hash(&mut h);
            cfg.log_retention_days.hash(&mut h);
        }
        Tab::TgBot => {
            hash_json(&mut h, &cfg.telegram.token);
            hash_json(&mut h, &cfg.telegram.authorized_chat_ids);
            hash_json(&mut h, &cfg.telegram.owner_chat_id);
            hash_json(&mut h, &cfg.telegram.chat_access);
            hash_json(&mut h, &cfg.telegram.mini_app_enabled);
            hash_json(&mut h, &cfg.telegram.bot.report_view);
            hash_json(&mut h, &cfg.telegram.bot.period_basis);
            hash_json(&mut h, &cfg.telegram.bot.extensions);
        }
        Tab::TgMessages => {
            hash_json(&mut h, &cfg.telegram.bot.menu);
            hash_json(&mut h, &cfg.telegram.bot.message_layout);
        }
        Tab::TgNotify
        | Tab::TgCore
        | Tab::StOverview
        | Tab::StCores
        | Tab::StTape
        | Tab::StMaint
        | Tab::Security
        | Tab::Storage
        | Tab::Sounds => return None,
    }
    Some(h.finish())
}

/// Pages whose config fields differ between `saved` and `draft`.
///
/// Args:
///     saved: The config as it sits on disk.
///     draft: The Settings window's live draft.
///
/// Returns:
///     Every page whose [`page_sig`] differs.
#[cfg(test)]
fn dirty_pages(saved: &AppConfig, draft: &AppConfig) -> TabSet {
    SavedPageSigs::of(saved).dirty(draft)
}

/// [`page_sig`] of every page of the saved config, keyed by the saved config's [`draft_sig`].
#[derive(Clone, Debug)]
struct SavedPageSigs {
    sig: u64,
    pages: [Option<u64>; 17],
}

impl SavedPageSigs {
    /// Signatures of `saved`.
    fn of(saved: &AppConfig) -> Self {
        Self {
            sig: draft_sig(saved),
            pages: Tab::ALL.map(|tab| page_sig(saved, tab)),
        }
    }

    /// Re-hash when `saved` is no longer the config these signatures were taken from.
    fn refresh(&mut self, saved: &AppConfig) {
        if draft_sig(saved) != self.sig {
            *self = Self::of(saved);
        }
    }

    /// Pages whose fields differ in `draft`.
    fn dirty(&self, draft: &AppConfig) -> TabSet {
        let mut set = TabSet::default();
        for (tab, saved) in Tab::ALL.into_iter().zip(self.pages) {
            if let Some(sig) = page_sig(draft, tab)
                && saved != Some(sig)
            {
                set.insert(tab);
            }
        }
        set
    }
}

/// [`dirty_pages`] of the backend's draft, skipped while the draft is clean.
///
/// Args:
///     b: Backend holding the draft.
///     dirty: Whether the draft differs from the saved config at all.
///     saved: Cached page signatures of the saved config.
///
/// Returns:
///     The dirty pages, empty when `dirty` is false or no draft exists.
fn backend_dirty_pages(b: &Backend, dirty: bool, saved: &SavedPageSigs) -> TabSet {
    match b.preview.as_ref() {
        Some(draft) if dirty => saved.dirty(draft),
        _ => TabSet::default(),
    }
}

/// Numbers drawn beside page names, read from the draft.
///
/// Args:
///     b: Backend holding the draft or the saved config.
///
/// Returns:
///     Core count and hotkey conflicts; the station count stays unknown for now.
fn nav_counts(b: &Backend) -> nav::NavCounts {
    let cfg = b.preview.as_ref().unwrap_or(&b.config);
    nav::NavCounts {
        cores: u32::try_from(cfg.servers.len()).unwrap_or(u32::MAX),
        hotkey_conflicts: hotkeys::conflict_count(&cfg.hotkeys),
        station_diverging: None,
    }
}

#[cfg(test)]
mod tests;

/// Open Settings on its default tab, in a separate OS window backed by a live-preview draft.
///
/// Args:
///     backend: Application state the window edits through its preview.
///     owner: Window this one belongs to, for placement.
///     owner_display: Display the owner sits on.
///     cx: Application context.
pub fn open(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    cx: &mut App,
) {
    open_on_tab(backend, owner, owner_display, Tab::Cores, cx);
}

/// Open the settings window on a named tab, for a control that sends the trader somewhere precise.
///
/// An ALREADY open window is only brought forward, not re-tabbed: its draft state belongs to
/// whatever the trader was doing in it, and switching the page under them would lose their place.
/// If a draft exists without a usable window, opening is ignored so two windows cannot share one
/// draft.
///
/// Args:
///     backend: Application state the window edits through its preview.
///     owner: Window this one belongs to, for placement.
///     owner_display: Display the owner sits on.
///     tab: Tab to select when the window is created.
///     cx: Application context.
pub fn open_on_tab(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    tab: Tab,
    cx: &mut App,
) {
    if let Some(handle) = backend.read(cx).settings_window
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    if backend.read(cx).preview.is_some() {
        return;
    }
    backend.update(cx, |b, _| {
        b.preview = Some(apply::reverted(&b.config));
    });
    // Restore geometry saved by `SettingsView`, as the Strategies and Assets windows do.
    let saved = backend.read(cx).layout.settings_window;
    let bounds = saved.map_or(
        Bounds {
            origin: point(px(160.0), px(120.0)),
            size: size(px(860.0), px(620.0)),
        },
        |g| Bounds {
            origin: point(px(g.x as f32), px(g.y as f32)),
            size: size(px(g.w as f32), px(g.h as f32)),
        },
    );
    // Select a display from the saved position when supported, or from the owner. Without a
    // display ID, GPUI creates the window on the primary display and may discard off-screen bounds.
    let display_id = crate::window::windowing::saved_or_owner_display_id(
        saved.and_then(|g| g.display_uuid),
        saved.map(|g| point(px(g.x as f32), px(g.y as f32))),
        owner,
        owner_display,
        cx,
    );
    let mut opts = crate::window::windowing::tool_window_options(
        t!("settings.window_title").to_string(),
        crate::window::windowing::restored_window_bounds(
            saved,
            crate::window::windowing::reachable_window_bounds(bounds, display_id, None, cx),
        ),
        Some(size(px(620.0), px(420.0))),
        owner,
    );
    opts.display_id = display_id;
    let b = backend.clone();
    match cx.open_window(opts, move |window, cx| {
        crate::window::windowing::configure_shell_clear_color(window, cx);
        let view = cx.new(|cx| SettingsView::new(b, tab, window, cx));
        // Arm the first-run repaint chain here rather than in `new`: `pulse::arm` needs the built
        // view, and arming from `render` would let the window keep itself awake through its own
        // repaints.
        view.update(cx, |this, cx| this.arm_conn_hint(cx));
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque))
    }) {
        Ok(handle) => {
            backend.update(cx, |b, _| b.settings_window = Some(handle));
            crate::window::windowing::activate_new_window(handle.into(), cx);
        }
        Err(_) => {
            backend.update(cx, |b, cx| {
                b.preview = None;
                b.settings_window = None;
                cx.notify();
            });
        }
    }
}
