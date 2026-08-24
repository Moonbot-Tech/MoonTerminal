//! Settings window, ported from egui's `src/settings/*` and `window/settings_window.rs`.
//! The separate OS window gives draft-backed tabs a live `Backend.preview`; closing the window
//! discards unsaved changes, while the Storage tab applies its separately persisted settings
//! immediately.
//! Save writes to disk through `AppConfig::save`; the shared daily scheduler independently owns
//! recovery copies in `backups/settings/`.
//!
//! The window is split into tabs like the egui original. This module owns the `SettingsView`
//! state and `open`; tab state and `impl SettingsView` blocks live in submodules. [`render`] owns
//! the tab bar, header, body, and Save footer, [`apply`] owns persistence and activation, and
//! [`common`] provides the shared UI and draft-binding helpers re-exported below.

mod apply;
mod badges;
mod common;
mod connections;
mod general;
mod hotkeys;
mod import_preview;
mod interface;
mod lines;
mod render;
mod security;
mod share;
mod storage;

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use gpui::*;
use moon_ui::{
    IndexPath, MoonBackgroundPolicy, MoonCheckbox, MoonInputState, MoonSelectEvent, MoonSelectItem,
    MoonSelectState, MoonSliderState, Root,
};
use rust_i18n::t;

use crate::Backend;
use crate::media::icons::IconSet;
use moon_core::config::{AppConfig, CoreSortMode, Language};
use moon_core::db::valuation::ValuationMode;
use moon_core::market::MarketDataMode;

use badges::BadgesEd;
use common::{
    collapse_block, color_row, draft_color, draft_slider, section, separator, slider_row,
};
use connections::ConnRow;
use interface::Iface;
use lines::Lines;

const SETTINGS_HEADER_H: f32 = 30.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Connections,
    General,
    Hotkeys,
    Interface,
    Lines,
    Badges,
    Storage,
}

impl Tab {
    const ALL: [Tab; 7] = [
        Tab::Connections,
        Tab::General,
        Tab::Hotkeys,
        Tab::Interface,
        Tab::Lines,
        Tab::Badges,
        Tab::Storage,
    ];
    /// Returns the stable, deliberately untranslated tab ID used by `MoonButton::new` and keys.
    fn id(self) -> &'static str {
        match self {
            Tab::Connections => "Подключения",
            Tab::General => "Общие",
            Tab::Hotkeys => "Хоткеи",
            Tab::Interface => "Интерфейс",
            Tab::Lines => "Линии",
            Tab::Badges => "Бейджи",
            Tab::Storage => "Хранилище",
        }
    }
    /// Returns the localized tab label from the `tab.*` namespace.
    fn title(self) -> String {
        match self {
            Tab::Connections => t!("tab.connections"),
            Tab::General => t!("tab.general"),
            Tab::Hotkeys => t!("tab.hotkeys"),
            Tab::Interface => t!("tab.interface"),
            Tab::Lines => t!("tab.lines"),
            Tab::Badges => t!("tab.badges"),
            Tab::Storage => t!("tab.storage"),
        }
        .to_string()
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
    /// UI-font slider for the personal `ui_font_delta` setting in `settings.toml`.
    ui_font: Entity<MoonSliderState>,
    /// Numeric UI-font input synchronized bidirectionally with the `ui_font` slider.
    ui_font_input: Entity<MoonInputState>,
    /// Language selector for the General tab.
    lang: Entity<MoonSelectState<Language>>,
    /// Quote-money conversion selector for the General tab.
    valuation: Entity<MoonSelectState<ValuationMode>>,
    /// Data-source selector for the Connections tab.
    mode: Entity<MoonSelectState<MarketDataMode>>,
    /// Order selector shared by every core list on the Connections tab.
    core_sort: Entity<MoonSelectState<CoreSortMode>>,
    /// Expanded line-style sections on the Lines tab, ported from `CollapsingHeader`.
    open_lines: HashSet<&'static str>,
    /// Active Hotkeys sub-tab, matching Moonbot's hotkey pages.
    hotkeys_group: hotkeys::HotkeyGroup,
    /// Storage-tab state: `storage.toml` configuration plus a background size/count snapshot.
    storage: storage::StorageEd,
    /// Group-icon cache for the Connections tab.
    icons: IconSet,
    /// Group whose icon picker is open, or `None` when closed; ported from egui's `picking`.
    picking: Option<String>,
    /// Signature of data consumed by Settings: draft/configuration fields plus session statuses.
    last_sig: u64,
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
                    if let Some(p) = b.preview.as_mut() {
                        if apply(p, v) {
                            bcx.notify();
                            changed = true;
                        }
                    }
                    changed
                });
                if changed {
                    cx.notify();
                }
            }))
    }

    /// Build Settings window state and synchronized selectors from the current draft.
    fn new(backend: Entity<Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let iface = interface::build(&backend, window, cx);
        let lines = lines::build(&backend, window, cx);
        let badges = badges::build(&backend, window, cx);
        let conn = connections::build_conn(&backend, window, cx);
        let security_ed = security::build(&backend, window, cx);

        // Build the General tab's personal `settings.toml` UI-font control: a labeled slider and
        // bidirectionally synchronized numeric input. Edits reinstall the MoonUI theme live so
        // the entire UI font scale updates. `general::build_font` owns the complete control.
        let (ui_font, ui_font_input) = general::build_font(&backend, window, cx);

        // Persist the Settings window position and size in layout so it reopens in the same place.
        // The debounced persistence loop drains `layout_dirty`, as it does for Strategies/Assets.
        cx.observe_window_bounds(window, |this, window, cx| {
            let Some(geom) = crate::window::windowing::window_geom_rect(window, cx) else {
                return;
            };
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
        let (cur_lang, cur_mode, cur_core_sort, cur_valuation) = {
            let b = backend.read(cx);
            let d = b.preview.as_ref().unwrap_or(&b.config);
            (
                d.language,
                d.market_mode,
                d.core_sort,
                d.report_valuation_mode,
            )
        };
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

        let initial_sig = settings_sig(backend.read(cx));
        cx.observe(&backend, |this, backend, cx| {
            let sig = settings_sig(backend.read(cx));
            if sig != this.last_sig {
                this.last_sig = sig;
                cx.notify();
            }
        })
        .detach();

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
            active: Tab::Connections,
            status: None,
            iface,
            lines,
            badges,
            conn,
            ui_font,
            ui_font_input,
            lang,
            valuation,
            mode,
            core_sort,
            open_lines: HashSet::new(),
            hotkeys_group: hotkeys::HotkeyGroup::Presets,
            storage: storage::build(),
            icons: IconSet::discover(),
            picking: None,
            last_sig: initial_sig,
            idle_last_secs: std::cell::Cell::new(0),
            import: None,
            security: security_ed,
        }
    }
}

/// Hash settings whose changes require refreshing the open window's editor state.
fn settings_sig(b: &Backend) -> u64 {
    let cfg = b.preview.as_ref().unwrap_or(&b.config);
    let mut h = DefaultHasher::new();

    cfg.language.code().hash(&mut h);
    cfg.market_mode.code().hash(&mut h);
    cfg.core_sort.hash(&mut h);
    cfg.report_valuation_mode.hash(&mut h);
    cfg.charts_split_by_core.hash(&mut h);
    cfg.charts_stack_scroll.hash(&mut h);
    cfg.charts_stack_compress.hash(&mut h);
    cfg.chart_stack_height.hash(&mut h);
    cfg.log_to_file.hash(&mut h);
    cfg.log_retention_days.hash(&mut h);
    cfg.ui_font_delta.to_bits().hash(&mut h);
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
        s.show_window.hash(&mut h);
        s.feed.orders.hash(&mut h);
        s.feed.detects.hash(&mut h);
        s.feed.reports.hash(&mut h);
        s.feed.balance.hash(&mut h);
        s.feed.strategies.hash(&mut h);
        s.feed.log.hash(&mut h);
        s.feed.alerts.hash(&mut h);
        s.feed.arb.hash(&mut h);
        // The key input owns its local repaint while typing; only empty/non-empty
        // affects surrounding settings layout.
        s.key.is_empty().hash(&mut h);
        s.group.hash(&mut h);
        s.market.hash(&mut h);
        s.color.hash(&mut h);
        s.synthetic.hash(&mut h);
    }

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
    }

    h.finish()
}

/// Opens Settings in a separate OS window backed by a live-preview configuration draft.
///
/// Reuses and activates an existing Settings window. If a draft already exists without a usable
/// window, opening is ignored so two windows cannot share one draft.
pub fn open(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    cx: &mut App,
) {
    if let Some(handle) = backend.read(cx).settings_window {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    if backend.read(cx).preview.is_some() {
        return;
    }
    backend.update(cx, |b, _| {
        let mut preview = b.config.clone();
        connections::sync_groups_from_servers(&preview.servers, &mut preview.groups);
        b.preview = Some(preview);
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
        WindowBounds::Windowed(bounds),
        Some(size(px(620.0), px(420.0))),
        owner,
    );
    opts.display_id = display_id;
    let b = backend.clone();
    match cx.open_window(opts, move |window, cx| {
        crate::window::windowing::configure_shell_clear_color(window, cx);
        let view = cx.new(|cx| SettingsView::new(b, window, cx));
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
