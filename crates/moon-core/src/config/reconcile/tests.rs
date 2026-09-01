use super::super::schema::{
    SCHEMA_VERSION, ServersFile, SettingsFile, UiThemeMode, default_ui_font_delta, default_ui_scale,
};
use super::{Merged, merge, split};
use crate::config::{CoreGroup, DEFAULT_ORDER_SIZES_USD, GroupConfig, Language};
use crate::market::MarketDataMode;

/// Merge a settings file carrying nothing but the two scaling knobs.
fn merged_with(ui_scale: f32, ui_font_delta: f32) -> Merged {
    merge(
        ServersFile::default(),
        SettingsFile {
            ui_scale,
            ui_font_delta,
            ..Default::default()
        },
        None,
    )
}

/// Pins the `repair_ui_scale` CALL inside [`merge`], not the repair itself — a pure repair
/// function nobody invokes is exactly how this regresses. The plausible edit: someone reads
/// `ui_scale` back as a plain passthrough, matching its unrepaired neighbours on either side.
///
/// A stored `ui_scale = 0.0` is not hypothetical — it is what every `settings.toml` written
/// before the loader applied schema defaults contains. `MoonThemeTokens::ui` floors the factor
/// at `0.25`, so honouring the zero renders the whole interface at a quarter size: text still
/// paints, every hit rectangle shrinks past the point where clicks land, and the Settings
/// screen the user would repair it from is itself unusable. Loading has to fix it.
#[test]
fn a_degenerate_stored_ui_scale_is_repaired_on_load() {
    for broken in [0.0_f32, -1.0, f32::NAN, f32::INFINITY] {
        assert_eq!(
            merged_with(broken, 0.0).ui_scale,
            default_ui_scale(),
            "a scale of {broken} cannot mean anything; loading must repair it, not pass it on"
        );
    }
}

/// The other half of the contract, and the half that is easy to break while "hardening" the
/// first: repair must not become a range clamp.
///
/// `ui_scale` has no settings-UI control, so hand-editing `settings.toml` is the only way to
/// set it — and the loaded value is written straight back by the next `save()`. A clamp would
/// therefore not just ignore an unusual choice, it would DESTROY it on disk, with nothing in
/// the UI to restore it from. `0.25` is MoonUI's own floor in `MoonThemeTokens::ui` and `6.0`
/// is far past any preset; both are usable, so both must survive untouched.
#[test]
fn an_unusual_but_usable_scale_survives_the_load() {
    for kept in [0.25_f32, 0.4, 6.0, 10.0] {
        assert_eq!(
            merged_with(kept, 0.0).ui_scale,
            kept,
            "a usable scale of {kept} must load verbatim; repair is not a clamp"
        );
    }
}

/// `ui_font_delta` splits the other way from `ui_scale`: `0.0` means "no adjustment" and is a
/// real choice, while a non-finite value is not — TOML parses `inf`/`nan`, and MoonUI adds
/// this delta directly into text metrics, where an infinity spreads into layout dimensions.
#[test]
fn a_non_finite_font_delta_is_repaired_while_zero_is_kept() {
    for broken in [f32::INFINITY, f32::NEG_INFINITY, f32::NAN] {
        assert_eq!(
            merged_with(1.0, broken).ui_font_delta,
            default_ui_font_delta(),
            "a font delta of {broken} reaches MoonUI text metrics; it must be repaired"
        );
    }
    assert_eq!(
        merged_with(1.0, 0.0).ui_font_delta,
        0.0,
        "zero font delta is 'no adjustment', a legitimate choice — it must NOT be repaired"
    );
}

#[test]
/// Regression target: restoring the removed `ServerMeta::order_sizes` assignment in
/// `config::reconcile::merge` reinterprets a legacy 0.01 BTC preset as $0.01 in the new toolbar.
fn legacy_base_coin_sizes_reset_to_group_usd_defaults() {
    let servers: ServersFile = toml::from_str(
        r#"
        [[servers]]
        uid = 1
        name = "btc-core"
        "#,
    )
    .expect("legacy servers file must parse");
    let settings: SettingsFile = toml::from_str(
        r#"
        version = 15

        [[groups]]
        name = "desk"
        active = true
        icon = 0

        [[servers]]
        uid = 1
        name = "btc-core"
        group = "desk"
        order_sizes = [0.01, 0.025, 0.05, 0.1, 0.25, 0.5]
        order_size_sel = 5
        "#,
    )
    .expect("legacy settings file must parse");

    let merged = merge(servers, settings, None);

    assert!(merged.dirty, "schema v15 must be written back as v17");
    assert_eq!(
        merged.groups[0].trade.order_sizes_usd,
        DEFAULT_ORDER_SIZES_USD
    );
    assert_eq!(merged.groups[0].trade.order_size_sel, 2);
}

/// Regression target: removing the missing-group materialization loop in `config::reconcile::merge`
/// leaves a migrated server without the local TP/SL generation promised by its toolbar.
#[test]
fn a_server_group_without_metadata_gets_complete_local_defaults() {
    let servers: ServersFile = toml::from_str(
        r#"
        [[servers]]
        uid = 1
        name = "desk-core"
        "#,
    )
    .expect("servers file must parse");
    let mut settings: SettingsFile = toml::from_str(
        r#"
        version = 17
        next_uid = 2

        [[servers]]
        uid = 1
        name = "desk-core"
        group = "desk"
        "#,
    )
    .expect("settings file without a matching group row must parse");
    settings.version = SCHEMA_VERSION;

    let merged = merge(servers, settings, None);

    assert!(
        merged.dirty,
        "materialized group metadata must be persisted"
    );
    assert_eq!(merged.groups, vec![GroupConfig::new("desk")]);
}

/// Regression target: replacing the repair loop in `config::reconcile::merge` with
/// `groups.iter_mut().any(repair)` stops after the first changed group and leaves later exits corrupt.
#[test]
fn every_group_is_repaired_even_after_an_earlier_change() {
    let mut first = GroupConfig::new("first");
    first.trade.order_sizes_usd[0] = f64::NAN;
    let mut second = GroupConfig::new("second");
    second.trade.exit.stop_loss_pct = f32::NAN;
    let settings = SettingsFile {
        version: SCHEMA_VERSION,
        groups: vec![first, second],
        ..Default::default()
    };

    let merged = merge(ServersFile::default(), settings, None);

    assert!(merged.dirty, "repaired group settings must be persisted");
    assert_eq!(
        merged.groups[0].trade.order_sizes_usd[0],
        DEFAULT_ORDER_SIZES_USD[0]
    );
    assert_eq!(merged.groups[1].trade.exit, Default::default());
}

/// Regression target: a file written while the flag meant "read the values back out of the core"
/// carries it with no `trade` block. Left unseeded, the display route (which needs a set) and the
/// write route (which needed only the flag) disagreed about that core: the toolbar showed the
/// group's numbers with the switch off while a click forked a per-core set behind it.
#[test]
fn an_enabled_core_without_a_generation_is_seeded_from_its_group() {
    let servers: ServersFile = toml::from_str(
        r#"
        [[servers]]
        uid = 1
        name = "desk-core"
        "#,
    )
    .expect("servers file must parse");
    let mut group = GroupConfig::new("desk");
    group.trade.order_sizes_usd[0] = 77.0;
    let settings: SettingsFile = toml::from_str(&format!(
        r#"
        version = {SCHEMA_VERSION}
        next_uid = 2

        [[servers]]
        uid = 1
        name = "desk-core"
        group = "desk"
        use_core_manual_config = true
        "#
    ))
    .map(|mut s: SettingsFile| {
        s.groups = vec![group.clone()];
        s
    })
    .expect("a file using the flag's previous name must parse");

    let merged = merge(servers, settings, None);

    assert!(merged.dirty, "a seeded core generation must be persisted");
    assert_eq!(
        merged.servers[0].trade.as_ref().map(|t| t.order_sizes_usd),
        Some(group.trade.order_sizes_usd),
        "the seed must come from the core's own group, not from the defaults"
    );
}

/// Regression target: skipping the per-core repair loop lets a hand-edited `trade` block size a
/// real order from a non-finite preset — the same failure the group loop above exists to prevent,
/// on values that reach the same order path.
#[test]
fn a_cores_own_generation_is_repaired_like_a_groups() {
    let servers: ServersFile = toml::from_str(
        r#"
        [[servers]]
        uid = 1
        name = "desk-core"
        "#,
    )
    .expect("servers file must parse");
    let settings: SettingsFile = toml::from_str(
        r#"
        version = 17
        next_uid = 2

        [[servers]]
        uid = 1
        name = "desk-core"
        group = "desk"
        own_trade_config = true

        [servers.trade]
        order_sizes_usd = [50.0, nan, 250.0, 500.0, 1000.0, 2500.0]
        order_size_sel = 9
        "#,
    )
    .expect("settings file with a per-core generation must parse");

    let merged = merge(servers, settings, None);

    assert!(merged.dirty, "a repaired core generation must be persisted");
    let trade = merged.servers[0]
        .trade
        .as_ref()
        .expect("the core keeps its own generation");
    assert_eq!(trade.order_sizes_usd[1], DEFAULT_ORDER_SIZES_USD[1]);
    assert_eq!(trade.order_size_sel, 5);
}

/// Named breakage (`config::reconcile::merge`): replacing
/// `dirty |= sanitize_core_groups(&mut core_groups);` with a bare `let core_groups =
/// meta.core_groups.clone();` would stop repairing a hand-edited `settings.toml` at load time.
/// Consequence: a duplicate-name pair (`Scalpers` / `scalpers`) never converges -- the same
/// broken pair reloads every launch instead of being repaired once and written back.
#[test]
fn a_core_group_list_needing_repair_marks_the_merge_dirty() {
    let settings = SettingsFile {
        version: SCHEMA_VERSION,
        next_uid: 1,
        core_groups: vec![
            CoreGroup {
                name: "Scalpers".to_string(),
                cores: vec![1],
            },
            CoreGroup {
                name: "scalpers".to_string(),
                cores: vec![2],
            },
        ],
        ..Default::default()
    };

    let merged = merge(ServersFile::default(), settings, None);

    assert!(
        merged.dirty,
        "a duplicate-name core group list must be repaired and the merge marked dirty, so the \
         fix is written back instead of re-derived on every launch"
    );
    assert_eq!(
        merged.core_groups.len(),
        2,
        "sanitize renames a collision, it does not drop it"
    );
    assert_ne!(
        merged.core_groups[0].name, merged.core_groups[1].name,
        "the two groups must no longer collide after repair"
    );
}

/// A clean core-group list survives `merge` -> `split` unchanged: nothing in the round trip may
/// reorder, rename or drop a member, and an already-clean list must not itself mark the merge
/// dirty (or every launch would rewrite `settings.toml` for nothing).
#[test]
fn a_clean_core_group_list_round_trips_through_merge_and_split() {
    let groups = vec![
        CoreGroup {
            name: "Scalpers".to_string(),
            cores: vec![1, 2],
        },
        CoreGroup {
            name: "Swing".to_string(),
            cores: vec![3],
        },
    ];
    let settings = SettingsFile {
        version: SCHEMA_VERSION,
        next_uid: 1,
        core_groups: groups.clone(),
        ..Default::default()
    };

    let merged = merge(ServersFile::default(), settings, None);
    assert!(
        !merged.dirty,
        "an already-clean core group list must not itself mark the merge dirty"
    );
    assert_eq!(merged.core_groups, groups);

    let (_, split_settings) = split(
        &merged.servers,
        &merged.groups,
        &merged.core_groups,
        merged.language,
        merged.market_mode,
        merged.charts_split_by_core,
        merged.charts_stack_scroll,
        merged.charts_stack_compress,
        merged.chart_stack_height,
        merged.separate_control_zones,
        merged.main_idle_close_secs,
        merged.log_to_file,
        merged.log_retention_days,
        merged.ui_font_delta,
        merged.ui_theme_mode,
        merged.ui_scale,
        merged.chart_memory_percent,
        merged.core_sort,
        merged.report_valuation_mode,
        merged.next_uid.get(),
    );

    assert_eq!(
        split_settings.core_groups, groups,
        "split must carry the merged groups through unchanged"
    );
}

/// Merge a one-server pair of files: `servers.enc` carries the key, `settings.toml` the metadata.
fn merged_server(key: &str, meta_toml: &str) -> crate::config::ServerConfig {
    let entry: crate::config::schema::ServerEntry =
        toml::from_str(&format!("uid = 7\nname = \"alpha\"\nkey = \"{key}\""))
            .expect("server entry fixture must parse");
    let meta: crate::config::schema::ServerMeta =
        toml::from_str(meta_toml).expect("server meta fixture must parse");
    let merged = merge(
        ServersFile {
            servers: vec![entry],
        },
        SettingsFile {
            servers: vec![meta],
            ..Default::default()
        },
        None,
    );
    merged
        .servers
        .into_iter()
        .next()
        .expect("the merged config keeps its only server")
}

/// The stored transport mode is the user's choice and must outrank the key it was seeded from.
/// MoonBot moves a core's own V0/V1/V2 switch without issuing a new key, so re-reading the key on
/// every load would silently undo the switch the user made here, which is the whole point of the
/// control.
#[test]
fn a_stored_transport_outranks_the_key() {
    let server = merged_server("", "uid = 7\nname = \"alpha\"\ntransport = \"v2\"");
    assert_eq!(server.transport, Some(crate::config::TransportVersion::V2));
}

/// Nothing stored and nothing readable in the key leaves the choice unset, and the connection
/// then follows the key exactly as it did before this field existed. A default of `V0` here would
/// pin every legacy core to V0 the first time its config was rewritten.
#[test]
fn an_unreadable_key_leaves_the_transport_unset() {
    for key in ["", "not-a-key"] {
        let server = merged_server(key, "uid = 7\nname = \"alpha\"");
        assert_eq!(
            server.transport, None,
            "key {key:?} names no mode, so nothing may be pinned"
        );
    }
}

/// `split` must carry the mode back into `settings.toml`; without it the choice would live for
/// one session and the next load would fall back to the key. It also pins the on-disk spelling:
/// `settings.toml` is hand-edited, and a renamed variant would read as "unset" on the next load.
#[test]
fn the_transport_survives_a_split() {
    let server = merged_server("", "uid = 7\nname = \"alpha\"\ntransport = \"v1\"");
    let (_, meta) = split(
        std::slice::from_ref(&server),
        &[],
        &[],
        Language::default(),
        MarketDataMode::default(),
        true,
        false,
        false,
        360,
        true,
        0,
        true,
        14,
        default_ui_font_delta(),
        UiThemeMode::default(),
        default_ui_scale(),
        100,
        crate::config::CoreSortMode::default(),
        crate::db::valuation::ValuationMode::default(),
        8,
    );

    assert_eq!(
        meta.servers.first().and_then(|m| m.transport),
        Some(crate::config::TransportVersion::V1),
        "a saved config must keep the mode the user chose"
    );
    let text = toml::to_string(&meta).expect("settings must serialize");
    assert!(
        text.contains("transport = \"v1\""),
        "the mode must persist as its MoonBot name, got: {text}"
    );
}

/// Regression target: adding a per-core field to `ServerConfig` without wiring BOTH halves of
/// `merge`/`split`. The manual-strategy mode is the only record of which strategy a core fires, and
/// a missing `split` line would drop it on every save while the running session looked correct.
#[test]
fn the_manual_strategy_mode_survives_a_split() {
    let server = merged_server(
        "",
        "uid = 7\nname = \"alpha\"\n[manual_strategy]\non = true\nstrategy = \"Beta\"
id = 2981",
    );
    assert_eq!(
        server.manual_strategy,
        Some(crate::config::ManualStratState {
            on: true,
            strategy: "Beta".to_string(),
            id: 2981,
            ..Default::default()
        }),
        "merge must carry the stored mode into the running config"
    );
    assert!(
        server
            .manual_strategy
            .as_ref()
            .is_some_and(|manual| manual.mb_logic),
        "a file written before the switch existed follows Moonbot's own stop rule, as Moonbot does"
    );
    let (_, meta) = split(
        std::slice::from_ref(&server),
        &[],
        &[],
        Language::default(),
        MarketDataMode::default(),
        true,
        false,
        false,
        360,
        true,
        0,
        true,
        14,
        default_ui_font_delta(),
        UiThemeMode::default(),
        default_ui_scale(),
        100,
        crate::config::CoreSortMode::default(),
        crate::db::valuation::ValuationMode::default(),
        8,
    );

    assert_eq!(
        meta.servers.first().and_then(|m| m.manual_strategy.clone()),
        server.manual_strategy,
        "a saved config must keep the strategy the core is set to"
    );
    let text = toml::to_string(&meta).expect("settings must serialize");
    assert!(
        text.contains("strategy = \"Beta\""),
        "the selection must persist by name, got: {text}"
    );
}
