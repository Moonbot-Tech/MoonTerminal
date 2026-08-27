use super::*;

/// The shipped default is the developer's own Main tab, transcribed from `charts.json` on
/// 2026-08-22. Pinned because it is a DECISION, not a guess: it is what every fresh profile opens
/// on AND what the popup's Reset returns to, so a silent edit here changes both.
#[test]
fn the_default_is_the_shipped_working_layout() {
    let cfg = ChartLabelsCfg::default();
    let rows: Vec<_> = cfg.rows[..cfg.used_rows()]
        .iter()
        .map(|r| {
            (
                r.preset,
                r.zone,
                r.align,
                r.flow,
                r.placement,
                r.gap,
                r.parts[..r.used_parts()]
                    .iter()
                    .map(|p| p.field)
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    use ChartLabelField as F;
    use LabelAlign as A;
    use LabelFlow as Fl;
    use LabelPreset as P;
    use LabelZone as Z;
    assert_eq!(
        rows,
        vec![
            // The instrument as a block in the control strip.
            (
                Some(P::Instrument),
                Z::ZoneTop,
                A::Right,
                Fl::Column,
                Fl::Column,
                0,
                vec![F::Coin, F::Core, F::Venue]
            ),
            // The badge on the plot's top-right, with the coin's deltas standing BESIDE it.
            (
                Some(P::Scale),
                Z::ChartTop,
                A::Right,
                Fl::Row,
                Fl::Column,
                0,
                vec![F::ScaleBadge]
            ),
            (
                Some(P::CoinDeltas),
                Z::ChartTop,
                A::Right,
                Fl::Column,
                Fl::Row,
                24,
                vec![F::Delta1h, F::Delta24h]
            ),
            // What traded over the last minute, and the same measured around the pointer.
            (
                Some(P::Volumes),
                Z::ChartTop,
                A::Right,
                Fl::Column,
                Fl::Column,
                6,
                vec![F::WindowSpanName, F::WindowBuyVolume, F::WindowSellVolume]
            ),
            (
                Some(P::CursorVolumes),
                Z::ChartTop,
                A::Right,
                Fl::Column,
                Fl::Column,
                6,
                vec![
                    F::WindowSpanName,
                    F::WindowBuyVolume,
                    F::WindowSellVolume,
                    F::WindowLiquidations
                ]
            ),
            // What is open, the session counters under it, funding under those.
            (
                Some(P::Position),
                Z::ChartTop,
                A::Left,
                Fl::Row,
                Fl::Row,
                0,
                vec![F::OpenOrders, F::OpenPnlMoney, F::OpenPnlPct, F::Exposure]
            ),
            (
                Some(P::Session),
                Z::ChartTop,
                A::Left,
                Fl::Row,
                Fl::Column,
                4,
                vec![F::SessionPnl, F::SessionProfit]
            ),
            (
                Some(P::Funding),
                Z::ChartTop,
                A::Left,
                Fl::Row,
                Fl::Column,
                8,
                vec![F::Funding, F::FundingIn]
            ),
            // The venue roster down the plot's left edge, and the detect line centred over it.
            (
                Some(P::Arbitrage),
                Z::ChartTop,
                A::Left,
                Fl::Column,
                Fl::Column,
                8,
                vec![F::ArbColumn]
            ),
            (
                Some(P::Detect),
                Z::ChartTop,
                A::Center,
                Fl::Column,
                Fl::Column,
                0,
                vec![F::DetectStrategy, F::DetectMsg, F::OrderStrategy]
            ),
        ]
    );
    assert_eq!(
        cfg.rows[1].parts[0].style.size_mult,
        Some(1.7),
        "the badge is set one step above its own default"
    );
    assert_eq!(
        cfg.rows[3].parts[0].window,
        LabelWindow::M1,
        "the live volume block opens on the minute, which the cheap sources serve"
    );
    assert_eq!(
        (
            cfg.rows[4].parts[1].span,
            cfg.rows[4].parts[1].anchor,
            cfg.rows[4].parts[1].bar
        ),
        (LabelSpan::Seconds(10), SpanAnchor::Cursor, false),
        "the measuring block reads ten seconds around the pointer, without bars"
    );
    assert_eq!(
        cfg.rows[7].parts[1].style.caption,
        Some(false),
        "the funding countdown prints bare, beside the rate that names itself"
    );
    assert_eq!(
        (
            cfg.rows[8].parts[0].style.color,
            cfg.rows[8].parts[0].style.color_min_pct
        ),
        (Some(LabelColor::BySign), Some(0.5)),
        "the roster colours the spread by its sign, and only where the spread is worth acting on"
    );
    assert!(
        cfg.rows[..cfg.used_rows()].iter().all(|r| r.name.is_empty()),
        "every shipped module is named by its PRESET, so the popup speaks the reader's language"
    );
}

/// The figures inherited from the overlay carry their captions: a bare "2" over the candles names
/// nothing, and the badge they replaced read "Ордера: 2".
#[test]
fn the_order_figures_are_captioned_by_default() {
    for field in [
        ChartLabelField::OpenOrders,
        ChartLabelField::Exposure,
        ChartLabelField::PosSize,
    ] {
        assert!(field.default_style().caption, "{field:?} must name itself");
        assert!(
            field.caption_key().is_some(),
            "{field:?} has no short caption"
        );
    }
}

/// The coin is set one size up and the comparison delta larger still: those two sizes are the
/// caption's whole visual hierarchy.
#[test]
fn default_styles_keep_the_captions_size_hierarchy() {
    let coin = ChartLabelField::Coin.default_style();
    let badge = ChartLabelField::ScaleBadge.default_style();
    let delta = ChartLabelField::CompareDelta.default_style();
    let core = ChartLabelField::Core.default_style();
    assert!(
        coin.size_mult > core.size_mult,
        "the coin leads the core name"
    );
    assert!(
        delta.size_mult > badge.size_mult,
        "the comparison delta stays dominant over the scale badge"
    );
    assert_eq!(
        delta.color,
        LabelColor::BySign,
        "a signed figure colors by sign"
    );
    assert_eq!(core.color, LabelColor::Theme);
}

#[test]
fn a_partial_style_override_leaves_the_rest_on_the_field_default() {
    let mut part = ChartLabelPart::new(ChartLabelField::Coin);
    part.style.color = Some(LabelColor::Fixed(0x00ff00));
    let resolved = part.resolved_style();
    assert_eq!(resolved.color, LabelColor::Fixed(0x00ff00));
    assert_eq!(
        resolved.size_mult,
        ChartLabelField::Coin.default_style().size_mult,
        "an untouched size still follows the field"
    );
}

#[test]
fn an_out_of_range_size_is_clamped_rather_than_drawn() {
    let mut part = ChartLabelPart::new(ChartLabelField::Core);
    part.style.size_mult = Some(99.0);
    assert_eq!(part.resolved_style().size_mult, LABEL_SIZE_MULT_MAX);
    part.style.size_mult = Some(0.001);
    assert_eq!(part.resolved_style().size_mult, LABEL_SIZE_MULT_MIN);
}

/// Captions inside a row are contiguous from the front, because "the leading N are the used ones"
/// is what the popup, the draw order and the run pool all read.
#[test]
fn sanitize_closes_a_hole_between_captions() {
    let mut cfg = ChartLabelsCfg::empty();
    let mut row = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    row.parts[0] = ChartLabelPart::new(ChartLabelField::Delta1h);
    // A hand-edited file can state a hole; the popup cannot.
    row.parts[2] = ChartLabelPart::new(ChartLabelField::Delta24h);
    cfg.rows[0] = row;
    cfg.sanitize();
    assert_eq!(cfg.rows[0].used_parts(), 2);
    assert_eq!(cfg.rows[0].parts[1].field, ChartLabelField::Delta24h);
}

/// The same for rows, which is what makes removing a row's LAST caption remove the row: it becomes
/// blank, and a blank row is not a row.
#[test]
fn sanitize_drops_a_blank_row_and_keeps_the_order() {
    // UNNAMED modules: a named one survives losing its captions, which the shipped roster relies on
    // and `a_named_row_survives_without_captions` pins separately.
    let mut cfg = ChartLabelsCfg::default();
    for row in &mut cfg.rows {
        row.name.clear();
    }
    // The badge module, whose single caption is what makes it blank once removed.
    cfg.rows[1].remove_part(0);
    cfg.sanitize();
    let fields: Vec<_> = cfg.rows[..cfg.used_rows()]
        .iter()
        .map(|r| r.parts[0].field)
        .collect();
    assert_eq!(
        fields,
        vec![
            ChartLabelField::Coin,
            ChartLabelField::Delta1h,
            ChartLabelField::WindowSpanName,
            ChartLabelField::WindowSpanName,
            ChartLabelField::OpenOrders,
            ChartLabelField::SessionPnl,
            ChartLabelField::Funding,
            ChartLabelField::ArbColumn,
            ChartLabelField::DetectStrategy
        ],
        "the survivors keep their relative order with no hole between them"
    );
}

/// A NAMED row survives losing its last caption: the name is the user's, and dropping the row would
/// throw it away while they are still assembling it.
#[test]
fn a_named_row_survives_without_captions() {
    let mut cfg = ChartLabelsCfg::empty();
    cfg.rows[0] = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    cfg.rows[0].name = "Дельты".to_string();
    cfg.sanitize();
    assert_eq!(cfg.used_rows(), 1);
    assert!(!cfg.rows[0].is_drawn(), "but it prints nothing");
}

#[test]
fn sanitize_cuts_an_overlong_name_on_a_character_boundary() {
    let mut cfg = ChartLabelsCfg::default();
    cfg.rows[0].name = "Ы".repeat(LABEL_ROW_NAME_MAX + 20);
    cfg.sanitize();
    assert_eq!(cfg.rows[0].name.chars().count(), LABEL_ROW_NAME_MAX);
}

/// A repair that leaves work for its own next run is a value that never equals itself — and every
/// comparison downstream (the panel's settings signature, the engine's change check) then reports a
/// change on every notification. The case that proves it: a name whose cut lands on a space.
#[test]
fn sanitize_is_idempotent_on_a_name_cut_at_a_space() {
    let mut once = ChartLabelsCfg::default();
    once.rows[0].name = format!("{}{}", "a".repeat(LABEL_ROW_NAME_MAX - 1), " хвост");
    once.sanitize();
    let mut twice = once.clone();
    twice.sanitize();
    assert_eq!(once, twice, "one pass has to finish the job");
    assert!(!once.rows[0].name.ends_with(' '));
}

/// The row prints its name only when it HAS one: a toggle with nothing behind it would print an
/// empty plate over the candles.
#[test]
fn an_unnamed_row_prints_no_name() {
    let mut row = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    row.show_name = true;
    assert!(!row.prints_name());
    row.name = "Позиция".to_string();
    assert!(row.prints_name());
    assert!(
        row.is_drawn(),
        "a named row draws even with no captions yet"
    );
}

#[test]
fn removing_a_caption_closes_the_gap() {
    let mut cfg = ChartLabelsCfg::default();
    // The open-order module; its index moved when the two volume blocks joined the shipped set.
    cfg.rows[5].remove_part(0);
    let fields: Vec<_> = cfg.rows[5].parts[..cfg.rows[5].used_parts()]
        .iter()
        .map(|p| p.field)
        .collect();
    assert_eq!(
        fields,
        vec![
            ChartLabelField::OpenPnlMoney,
            ChartLabelField::OpenPnlPct,
            ChartLabelField::Exposure,
        ]
    );
}

#[test]
fn moving_refuses_at_the_ends_and_swaps_in_the_middle() {
    let mut cfg = ChartLabelsCfg::default();
    assert!(!cfg.move_row(0, true), "the first row cannot move up");
    let used = cfg.used_rows();
    assert!(
        !cfg.move_row(used - 1, false),
        "the last used row cannot move down"
    );
    assert!(cfg.move_row(2, true));
    assert_eq!(cfg.rows[1].parts[0].field, ChartLabelField::Delta1h);
    assert_eq!(cfg.rows[2].parts[0].field, ChartLabelField::ScaleBadge);
    assert!(
        !cfg.move_row(used, true),
        "a blank row is not a movable row"
    );
    assert!(!cfg.move_row(CHART_LABEL_ROWS + 5, false));
}

#[test]
fn moving_a_caption_refuses_at_the_ends_of_its_row() {
    let mut cfg = ChartLabelsCfg::default();
    let row = &mut cfg.rows[5];
    assert!(!row.move_part(0, true));
    assert!(!row.move_part(row.used_parts() - 1, false));
    assert!(row.move_part(1, true));
    assert_eq!(row.parts[0].field, ChartLabelField::OpenPnlMoney);
    assert_eq!(row.parts[1].field, ChartLabelField::OpenOrders);
}

#[test]
fn push_fills_the_first_free_row_and_reports_a_full_list() {
    let mut cfg = ChartLabelsCfg::default();
    let used = cfg.used_rows();
    let ix = cfg
        .push_row(
            ChartLabelField::Delta1h,
            LabelZone::ChartTop,
            LabelAlign::Left,
        )
        .expect("there is room");
    assert_eq!(ix, used);
    assert_eq!(cfg.rows[ix].parts[0].field, ChartLabelField::Delta1h);
    while cfg
        .push_row(
            ChartLabelField::LastPrice,
            LabelZone::ChartTop,
            LabelAlign::Left,
        )
        .is_some()
    {}
    assert!(
        cfg.push_row(ChartLabelField::Core, LabelZone::ChartTop, LabelAlign::Left)
            .is_none(),
        "a full list refuses instead of dropping an existing row"
    );
}

#[test]
fn a_row_refuses_a_ninth_caption() {
    let mut row = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    for _ in 0..CHART_LABEL_PARTS {
        assert!(row.push_part(ChartLabelField::LastPrice));
    }
    assert!(
        !row.push_part(ChartLabelField::Coin),
        "a full row refuses instead of overwriting a caption"
    );
}

/// A preset must fit the row it creates, or it would silently lose its tail on every use.
#[test]
fn every_preset_fits_a_row_and_is_named() {
    for preset in LabelPreset::ALL {
        assert!(
            !preset.fields().is_empty() && preset.fields().len() <= CHART_LABEL_PARTS,
            "{preset:?} does not fit a row"
        );
        assert!(
            preset.locale_key().starts_with("chart_labels.preset."),
            "{preset:?} has no locale key"
        );
    }
}

#[test]
fn a_preset_row_carries_its_fields_band_and_name() {
    let mut cfg = ChartLabelsCfg::empty();
    let ix = cfg.push_preset(LabelPreset::Position).expect("there is room");
    let row = &cfg.rows[ix];
    assert_eq!(row.preset, Some(LabelPreset::Position));
    assert_eq!(row.zone, LabelPreset::Position.zone());
    assert_eq!(row.align, LabelPreset::Position.align());
    assert_eq!(row.used_parts(), LabelPreset::Position.fields().len());
}

/// A preset row is named from the DICTIONARY, not from a string frozen at creation time — which is
/// what makes the shipped default readable in a locale the developer does not speak.
#[test]
fn a_preset_names_the_row_until_the_user_names_it_themselves() {
    let mut cfg = ChartLabelsCfg::empty();
    let ix = cfg.push_preset(LabelPreset::Funding).expect("there is room");
    let row = &mut cfg.rows[ix];
    assert!(row.name.is_empty(), "no localized string is stored");
    assert_eq!(row.title_key(), Some(LabelPreset::Funding.locale_key()));

    row.name = "Мой фандинг".to_string();
    assert_eq!(row.title_key(), None, "the user's own name wins");
    row.name.clear();
    assert_eq!(
        row.title_key(),
        Some(LabelPreset::Funding.locale_key()),
        "clearing the name gives the translated one back"
    );
}

/// The name switch follows what the row can PRINT: a preset row has a name without one being typed,
/// and a row with neither has nothing to print.
#[test]
fn a_preset_row_can_print_its_name_with_no_name_typed() {
    let mut row = ChartLabelRow::new(LabelZone::ZoneTop, LabelAlign::Right);
    row.push_part(ChartLabelField::Funding);
    row.show_name = true;
    assert!(!row.prints_name(), "nothing to print without a name");
    row.preset = Some(LabelPreset::Funding);
    assert!(row.prints_name());
}

/// A module the editor was opened on before it existed is only worth a slot once it holds
/// something: a blank one would be swept away by the next `sanitize` anyway.
#[test]
fn a_prepared_row_is_added_only_when_it_holds_something() {
    let mut cfg = ChartLabelsCfg::empty();
    let blank = ChartLabelRow::new(LabelZone::ZoneTop, LabelAlign::Right);
    assert_eq!(cfg.push_prepared(blank), None);
    assert_eq!(cfg.used_rows(), 0);

    let mut row = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    row.push_part(ChartLabelField::LastPrice);
    assert_eq!(cfg.push_prepared(row.clone()), Some(0));
    assert_eq!(cfg.rows[0], row);

    // Every slot taken: the caller is told, rather than silently losing the module.
    let mut full = ChartLabelsCfg::empty();
    for _ in 0..CHART_LABEL_ROWS {
        full.push_prepared(row.clone()).expect("room while filling");
    }
    assert_eq!(full.push_prepared(row), None);
}

#[test]
fn the_basis_selects_which_orders_count() {
    assert!(PnlBasis::All.accepts(true) && PnlBasis::All.accepts(false));
    assert!(PnlBasis::Real.accepts(false) && !PnlBasis::Real.accepts(true));
    assert!(PnlBasis::Emulator.accepts(true) && !PnlBasis::Emulator.accepts(false));
}

/// Only fields that actually read a basis offer the control, so switching a part's field cannot
/// leave a stale basis visible in the popup.
#[test]
fn only_position_fields_use_the_basis() {
    assert!(ChartLabelField::OpenPnlPct.uses_pnl_basis());
    assert!(ChartLabelField::PosSize.uses_pnl_basis());
    assert!(!ChartLabelField::Coin.uses_pnl_basis());
    assert!(!ChartLabelField::Delta1h.uses_pnl_basis());
}

/// Every assignable field must be reachable through a menu section, or it can be configured in a
/// file and never removed through the UI.
#[test]
fn every_field_belongs_to_a_menu_group_and_has_a_locale_key() {
    for field in ChartLabelField::ALL {
        assert!(
            ChartLabelGroup::ALL.contains(&field.group()),
            "{field:?} has no menu section"
        );
        assert!(
            field.locale_key().starts_with("chart_labels.field."),
            "{field:?} has no locale key"
        );
    }
}

/// `any_drawn` gates the expensive sync work, so a hidden caption must not keep it alive.
#[test]
fn a_hidden_caption_does_not_keep_its_sync_work_alive() {
    let mut cfg = ChartLabelsCfg::default();
    assert!(cfg.any_drawn(|f| f == ChartLabelField::OpenPnlPct));
    for row in &mut cfg.rows {
        for part in &mut row.parts {
            part.visible = false;
        }
    }
    assert!(!cfg.any_drawn(|f| f == ChartLabelField::OpenPnlPct));
    assert!(
        cfg.contains(ChartLabelField::OpenPnlPct),
        "but it is still configured, and the add menu says so"
    );
}

#[test]
fn the_configuration_round_trips_through_toml() {
    let mut cfg = ChartLabelsCfg::default();
    cfg.rows[0].name = "Инструмент".to_string();
    cfg.rows[0].show_name = true;
    // A preset row with no name: the pair a file has to state separately.
    assert_eq!(cfg.rows[7].preset, Some(LabelPreset::Funding));
    cfg.rows[0].parts[0].style.color = Some(LabelColor::Fixed(0x112233));
    cfg.rows[0].parts[0].style.size_mult = Some(1.5);
    cfg.rows[3].parts[1].pnl_basis = PnlBasis::Real;
    let text = toml::to_string_pretty(&cfg).expect("serializes");
    let back: ChartLabelsCfg = toml::from_str(&text).expect("parses");
    assert_eq!(back, cfg);
}

/// `charts.json` is the OTHER file this travels in, and it drops every chart tab on a parse error —
/// so the JSON path is pinned separately rather than assumed from the TOML one.
#[test]
fn the_configuration_round_trips_through_json() {
    let mut cfg = ChartLabelsCfg::default();
    cfg.rows[2].name = "Ядро".to_string();
    let text = serde_json::to_string(&cfg).expect("serializes");
    let back: ChartLabelsCfg = serde_json::from_str(&text).expect("parses");
    assert_eq!(back, cfg);
}

/// The whole point of writing a LIST: unused rows and captions never reach the file, so a hundred
/// and twenty-eight tables per tab do not either.
#[test]
fn only_the_used_rows_and_captions_are_written() {
    let cfg = ChartLabelsCfg::default();
    let text = toml::to_string_pretty(&cfg).expect("serializes");
    assert_eq!(
        text.matches("[[rows]]").count(),
        cfg.used_rows(),
        "a blank row is not written"
    );
    let parts: usize = cfg.rows[..cfg.used_rows()]
        .iter()
        .map(|r| r.used_parts())
        .sum();
    assert_eq!(
        text.matches("[[rows.parts]]").count(),
        parts,
        "an unused caption is not written either"
    );
}

/// The reason the file is a list at all: a profile written under a LARGER ceiling still loads, and
/// one written under a smaller one is not rejected. An exact-length array does neither.
#[test]
fn a_list_longer_than_the_ceiling_is_truncated_rather_than_rejected() {
    let mut rows = String::new();
    for _ in 0..CHART_LABEL_ROWS + 4 {
        rows.push_str("[[rows]]\nzone = \"chart_top\"\nalign = \"left\"\n[[rows.parts]]\nfield = \"last_price\"\n");
    }
    let back: ChartLabelsCfg = toml::from_str(&rows).expect("an over-long list still parses");
    assert_eq!(back.used_rows(), CHART_LABEL_ROWS);
}

#[test]
fn a_row_holding_more_captions_than_fit_is_truncated_rather_than_rejected() {
    let mut text = String::from("[[rows]]\nzone = \"chart_top\"\nalign = \"left\"\n");
    for _ in 0..CHART_LABEL_PARTS + 3 {
        text.push_str("[[rows.parts]]\nfield = \"last_price\"\n");
    }
    let back: ChartLabelsCfg = toml::from_str(&text).expect("parses");
    assert_eq!(back.rows[0].used_parts(), CHART_LABEL_PARTS);
}

/// A file written before this feature existed carries no captions at all, and must land on the
/// caption the chart already drew rather than on an empty corner.
#[test]
fn an_absent_table_loads_as_the_default_caption() {
    let back: ChartLabelsCfg = toml::from_str("").expect("parses an empty document");
    assert_eq!(back, ChartLabelsCfg::default());
}

/// "Print nothing" is a choice a user can reach by removing every row, and it must survive a
/// save — not be read back as "said nothing, give them the default".
#[test]
fn an_explicitly_empty_list_stays_empty() {
    let text = toml::to_string_pretty(&ChartLabelsCfg::empty()).expect("serializes");
    let back: ChartLabelsCfg = toml::from_str(&text).expect("parses");
    assert_eq!(back.used_rows(), 0);
}

/// THE migration: a profile saved under the flat slot shape keeps its captions, their styles and
/// their rows. `inline` was the row boundary, so a chain collapses into one row and takes the head's
/// band — which is exactly what the old `sanitize` guaranteed on screen.
#[test]
fn a_legacy_slot_list_migrates_into_rows() {
    let legacy = r#"{"slots":[
        {"field":"coin","zone":"zone_top","align":"right","inline":false,"visible":true,"style":{},"pnl_basis":"all"},
        {"field":"open_orders","zone":"chart_top","align":"left","inline":false,"visible":true,"style":{"color":{"mode":"fixed","rgb":9279918}},"pnl_basis":"all"},
        {"field":"exposure","zone":"chart_bottom","align":"center","inline":true,"visible":true,"style":{},"pnl_basis":"real"},
        {"field":"open_pnl_pct","zone":"zone_bottom","align":"right","inline":true,"visible":false,"style":{"size_mult":1.25},"pnl_basis":"all"},
        {"field":"none","zone":"zone_top","align":"center","inline":false,"visible":true,"style":{},"pnl_basis":"all"}
    ]}"#;
    let cfg: ChartLabelsCfg = serde_json::from_str(legacy).expect("the old shape still loads");
    assert_eq!(cfg.used_rows(), 2, "one chain is one row");
    assert_eq!(cfg.rows[0].parts[0].field, ChartLabelField::Coin);
    assert_eq!(cfg.rows[0].zone, LabelZone::ZoneTop);
    let row = &cfg.rows[1];
    assert_eq!(
        row.parts[..row.used_parts()]
            .iter()
            .map(|p| p.field)
            .collect::<Vec<_>>(),
        vec![
            ChartLabelField::OpenOrders,
            ChartLabelField::Exposure,
            ChartLabelField::OpenPnlPct
        ],
        "the chain kept its print order"
    );
    assert_eq!(
        (row.zone, row.align),
        (LabelZone::ChartTop, LabelAlign::Left),
        "the joined captions took the head's band, as they were drawn"
    );
    assert_eq!(
        row.parts[0].style.color,
        Some(LabelColor::Fixed(0x8d99ae)),
        "and every style travelled with its caption"
    );
    assert_eq!(row.parts[1].pnl_basis, PnlBasis::Real);
    assert!(!row.parts[2].visible, "a hidden caption stayed hidden");
    assert_eq!(row.parts[2].style.size_mult, Some(1.25));
}

/// A legacy chain longer than a row holds continues in a fresh row in the same band instead of
/// losing its tail — the old shape allowed sixteen chained captions.
#[test]
fn an_overlong_legacy_chain_continues_in_a_second_row() {
    let mut slots = vec![r#"{"field":"coin","zone":"chart_top","align":"left"}"#.to_string()];
    for _ in 0..CHART_LABEL_PARTS + 2 {
        slots.push(
            r#"{"field":"last_price","zone":"zone_top","align":"right","inline":true}"#.to_string(),
        );
    }
    let legacy = format!("{{\"slots\":[{}]}}", slots.join(","));
    let cfg: ChartLabelsCfg = serde_json::from_str(&legacy).expect("loads");
    assert_eq!(cfg.used_rows(), 2);
    assert_eq!(cfg.rows[0].used_parts(), CHART_LABEL_PARTS);
    assert_eq!(cfg.rows[1].used_parts(), 3);
    assert_eq!(
        (cfg.rows[1].zone, cfg.rows[1].align),
        (LabelZone::ChartTop, LabelAlign::Left),
        "the continuation stays on the band the chain was drawn in"
    );
}

/// The control strip is its own BAND, not an alignment of the plot's: one lies over the book, the
/// other over the candles. Collapsing them is what made "right" mean different edges on two panes.
#[test]
fn the_control_strip_is_a_band_of_its_own() {
    assert_ne!(LabelZone::ZoneTop, LabelZone::ChartTop);
    assert!(LabelZone::ZoneTop.is_control_zone() && LabelZone::ZoneBottom.is_control_zone());
    assert!(!LabelZone::ChartTop.is_control_zone());
    assert!(LabelZone::ZoneTop.is_top() && !LabelZone::ZoneBottom.is_top());
    assert_eq!(
        ChartLabelsCfg::default().rows[0].zone,
        LabelZone::ZoneTop,
        "the default caption lives in the control strip, where it has always been drawn"
    );
}

/// Legacy files named the alignment inside the zone. They must still load, landing in the matching
/// band rather than being rejected — the alignment then falls back to the row's own.
#[test]
fn the_legacy_zone_spellings_still_load() {
    let legacy = r#"{"slots":[{"field":"coin","zone":"top_right","inline":false}]}"#;
    let cfg: ChartLabelsCfg = serde_json::from_str(legacy).expect("an old spelling parses");
    assert_eq!(
        cfg.rows[0].zone,
        LabelZone::ChartTop,
        "top_right lands in the plot's top band"
    );
}

/// The real thing: the developer's OWN saved Main-tab configuration, copied verbatim out of
/// `charts.json` on 2026-08-21 — a file that had filled all sixteen slots, which is what started
/// this work. A migration is only proven by the file it has to survive, and a fixture written by
/// hand proves the hand, not the file.
#[test]
fn the_developers_own_saved_config_migrates_whole() {
    let legacy = include_str!("fixtures/legacy_slots_full.json");
    let cfg: ChartLabelsCfg = serde_json::from_str(legacy).expect("the real file loads");
    let rows: Vec<(LabelZone, LabelAlign, Vec<ChartLabelField>)> = cfg.rows[..cfg.used_rows()]
        .iter()
        .map(|r| {
            (
                r.zone,
                r.align,
                r.parts[..r.used_parts()].iter().map(|p| p.field).collect(),
            )
        })
        .collect();
    use ChartLabelField as F;
    use LabelAlign as A;
    use LabelZone as Z;
    assert_eq!(
        rows,
        vec![
            (Z::ZoneTop, A::Right, vec![F::Coin]),
            (Z::ChartTop, A::Right, vec![F::ScaleBadge]),
            (Z::ZoneTop, A::Right, vec![F::Core]),
            (
                Z::ChartTop,
                A::Left,
                vec![F::OpenOrders, F::Exposure, F::OpenPnlMoney, F::OpenPnlPct]
            ),
            (Z::ChartTop, A::Center, vec![F::Delta1h]),
            (Z::ChartTop, A::Center, vec![F::Delta24h]),
            (Z::ChartTop, A::Center, vec![F::Funding]),
            (Z::ChartTop, A::Center, vec![F::FundingIn]),
            (Z::ChartTop, A::Center, vec![F::OrderStrategy]),
            (Z::ChartTop, A::Center, vec![F::Venue]),
            (Z::ChartTop, A::Center, vec![F::Quote]),
            (Z::ChartTop, A::Center, vec![F::ExchangeDelta1h]),
            (Z::ChartTop, A::Center, vec![F::ExchangeDelta24h]),
        ],
        "every caption kept its band, its alignment and the row it was drawn on"
    );
    assert_eq!(
        cfg.rows[3].parts[0].style.color,
        Some(LabelColor::Fixed(0x8d99ae)),
        "the muted order count survived with its colour"
    );
    // Sixteen captions became thirteen rows, and there is room to keep going — which is the point
    // of the change this fixture guards.
    assert!(cfg.first_free_row().is_some());
}

/// A hidden legacy caption must not become the row a CHAINED one joins: the old `sanitize` resolved
/// that row from the last DRAWN slot, so joining the hidden one relocates a visible caption into a
/// band the user never saw it in.
#[test]
fn a_hidden_legacy_slot_does_not_anchor_a_chain() {
    let legacy = r#"{"slots":[
        {"field":"coin","zone":"chart_top","align":"left"},
        {"field":"core","zone":"zone_bottom","align":"right","visible":false},
        {"field":"last_price","zone":"zone_top","align":"center","inline":true}
    ]}"#;
    let cfg: ChartLabelsCfg = serde_json::from_str(legacy).expect("loads");
    assert_eq!(
        cfg.rows[0].parts[..cfg.rows[0].used_parts()]
            .iter()
            .map(|p| p.field)
            .collect::<Vec<_>>(),
        vec![ChartLabelField::Coin, ChartLabelField::LastPrice],
        "the chained caption joined the last VISIBLE row, as it was drawn"
    );
    assert_eq!(
        (cfg.rows[1].zone, cfg.rows[1].parts[0].field),
        (LabelZone::ZoneBottom, ChartLabelField::Core),
        "and the hidden caption kept its own row, to come back where it was"
    );
    assert!(!cfg.rows[1].parts[0].visible);
}

/// `f32::clamp` passes NaN through, so a hand-edited `nan` would survive `sanitize` and make the
/// configuration unequal to ITSELF — which turns every settings comparison downstream into a
/// permanent false "changed".
#[test]
fn a_non_finite_size_is_dropped_rather_than_clamped() {
    let mut cfg = ChartLabelsCfg::default();
    cfg.rows[0].parts[0].style.size_mult = Some(f32::NAN);
    cfg.sanitize();
    assert_eq!(cfg.rows[0].parts[0].style.size_mult, None);
    let copy = cfg.clone();
    assert_eq!(cfg, copy, "and the value equals itself again");
    let mut part = ChartLabelPart::new(ChartLabelField::Coin);
    part.style.size_mult = Some(f32::NAN);
    assert!(
        part.resolved_style().size_mult.is_finite(),
        "and nothing non-finite reaches the shaper"
    );
}

/// A whitespace-only name is not a name: it would keep a caption-less row alive and print an empty
/// plated caption while the popup's list shows the row as unnamed.
#[test]
fn a_whitespace_only_name_is_no_name() {
    let mut cfg = ChartLabelsCfg::empty();
    cfg.rows[0] = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    cfg.rows[0].name = "   ".to_string();
    cfg.rows[0].show_name = true;
    cfg.sanitize();
    assert_eq!(
        cfg.used_rows(),
        0,
        "a blank row with a blank name is no row"
    );
}

/// One switch for a whole family: a hidden row keeps its captions and its place, and costs the sync
/// paths nothing while it is off.
#[test]
fn a_hidden_row_draws_nothing_but_keeps_everything() {
    let mut cfg = ChartLabelsCfg::default();
    // The open-order module: its figures are the ones whose absence is measurable through the gate
    // the sync paths read.
    cfg.rows[5].visible = false;
    cfg.sanitize();
    assert!(!cfg.rows[5].is_drawn());
    assert_eq!(cfg.used_rows(), 10, "it is still a row");
    assert!(
        !cfg.any_drawn(|f| f == ChartLabelField::OpenPnlPct),
        "and its captions stop costing the order walk"
    );
    assert!(
        cfg.contains(ChartLabelField::OpenPnlPct),
        "but stay configured"
    );
}

/// A file written before the switch existed drew its rows; absence must not hide them.
#[test]
fn a_row_without_the_visible_flag_is_drawn() {
    let text = "[[rows]]
zone = \"chart_top\"
[[rows.parts]]
field = \"last_price\"
";
    let cfg: ChartLabelsCfg = toml::from_str(text).expect("parses");
    assert!(cfg.rows[0].visible);
    let written = toml::to_string_pretty(&cfg).expect("serializes");
    assert!(
        !written.contains("visible"),
        "and a drawn row does not spend a line saying so"
    );
}

/// Both axes default to the shape the chart drew before either existed, and neither costs a line in
/// a file that never touches them.
#[test]
fn the_flow_axes_default_to_the_old_shape_and_stay_silent() {
    let row = ChartLabelRow::default();
    assert_eq!(row.flow, LabelFlow::Row, "captions run across a line");
    assert_eq!(
        row.placement,
        LabelFlow::Column,
        "and each module starts a line of its own"
    );
    // Again a bare module: the shipped roster states both axes deliberately.
    let mut cfg = ChartLabelsCfg::empty();
    cfg.rows[0] = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    cfg.rows[0].push_part(ChartLabelField::LastPrice);
    let written = toml::to_string_pretty(&cfg).expect("serializes");
    assert!(!written.contains("flow"), "a default axis is not written");
    assert!(!written.contains("placement"));
}

/// A file that states them keeps them, in both formats.
#[test]
fn the_flow_axes_round_trip() {
    let mut cfg = ChartLabelsCfg::default();
    cfg.rows[0].flow = LabelFlow::Column;
    cfg.rows[1].placement = LabelFlow::Row;
    for text in [
        toml::to_string_pretty(&cfg).expect("toml"),
        serde_json::to_string(&cfg).expect("json"),
    ] {
        let back: ChartLabelsCfg = if text.starts_with('{') {
            serde_json::from_str(&text).expect("json parses")
        } else {
            toml::from_str(&text).expect("toml parses")
        };
        assert_eq!(back, cfg);
    }
}

/// A file written before the axes existed draws the way it always did.
#[test]
fn a_row_without_the_axes_keeps_the_old_shape() {
    let legacy = r#"{"slots":[
        {"field":"coin","zone":"chart_top","align":"left"},
        {"field":"core","zone":"chart_top","align":"left","inline":true}
    ]}"#;
    let cfg: ChartLabelsCfg = serde_json::from_str(legacy).expect("loads");
    assert_eq!(cfg.rows[0].flow, LabelFlow::Row);
    assert_eq!(cfg.rows[0].placement, LabelFlow::Column);
}

/// The gap is one number for four directions, and a file states it only when it is asked for.
#[test]
fn the_gap_defaults_to_nothing_and_stays_silent() {
    assert_eq!(ChartLabelRow::default().gap, 0);
    // Asked of a bare module, not of the shipped roster: that roster is a working layout and spaces
    // two of its modules on purpose.
    let mut cfg = ChartLabelsCfg::empty();
    cfg.rows[0] = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    cfg.rows[0].push_part(ChartLabelField::LastPrice);
    let written = toml::to_string_pretty(&cfg).expect("serializes");
    assert!(
        !written.contains("gap"),
        "a module with no gap does not say so"
    );
}

#[test]
fn the_gap_round_trips_and_is_capped() {
    let mut cfg = ChartLabelsCfg::default();
    cfg.rows[1].gap = 12;
    // A hand-edited file can ask for a gap that would push everything after it off the pane.
    cfg.rows[2].gap = 255;
    cfg.sanitize();
    assert_eq!(cfg.rows[2].gap, LABEL_GAP_MAX);
    let text = toml::to_string_pretty(&cfg).expect("serializes");
    let back: ChartLabelsCfg = toml::from_str(&text).expect("parses");
    assert_eq!(back, cfg);
    assert_eq!(back.rows[1].gap, 12);
}

/// Every window is now readable over every field, the split ones included.
///
/// The buy/sell figures used to stop at five minutes, which is as far as the protocol's own rolling
/// buckets reach; the retained mini-candles carry a split of their own and removed that ceiling. A
/// window the retained history does not actually cover is reported as INCOMPLETE by the readout,
/// not repaired away here — silently moving a caption from the day to the minute would answer a
/// different question than the one on screen.
#[test]
fn a_split_figure_keeps_the_long_window_it_was_given() {
    let mut cfg = ChartLabelsCfg::empty();
    let mut row = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    row.push_part(ChartLabelField::WindowBuyShare);
    row.parts[0].window = LabelWindow::H24;
    row.push_part(ChartLabelField::WindowDelta);
    row.parts[1].window = LabelWindow::H24;
    cfg.rows[0] = row;

    cfg.sanitize();

    assert_eq!(cfg.rows[0].parts[0].window, LabelWindow::H24);
    assert_eq!(cfg.rows[0].parts[1].window, LabelWindow::H24);
}

/// A custom span is repaired into what the history can be asked for, and dropped from a caption
/// that reads no period at all.
///
/// Breakage: a zero span reaches the ring as "the last nothing" and a caption that changed field
/// would keep obeying a period nothing beside it names — the block's heading would say one thing
/// and the figure under it would cover another.
#[test]
fn a_custom_span_is_clamped_and_dropped_where_it_means_nothing() {
    let mut cfg = ChartLabelsCfg::empty();
    let mut row = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    row.push_part(ChartLabelField::WindowBuyVolume);
    row.parts[0].span = LabelSpan::Minutes(0);
    row.push_part(ChartLabelField::WindowSellVolume);
    row.parts[1].span = LabelSpan::Trades(u32::MAX);
    row.push_part(ChartLabelField::Coin);
    row.parts[2].span = LabelSpan::Minutes(15);
    cfg.rows[0] = row;

    cfg.sanitize();

    assert_eq!(cfg.rows[0].parts[0].span, LabelSpan::Minutes(1));
    assert_eq!(
        cfg.rows[0].parts[1].span,
        LabelSpan::Trades(LABEL_SPAN_TRADES_MAX)
    );
    assert_eq!(
        cfg.rows[0].parts[2].span,
        LabelSpan::Window,
        "a caption that reads no period keeps none"
    );
}

/// Two captions over one period are ONE history read, and a trade count ignores the window.
///
/// Breakage: the sync path turns every entry here into a read of the retained rings. A module
/// printing the buying, the selling and their total would pay three times for one answer, on every
/// pane, several times a second.
#[test]
fn the_volume_spans_are_deduplicated() {
    let mut cfg = ChartLabelsCfg::empty();
    let mut row = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    row.push_part(ChartLabelField::WindowBuyVolume);
    row.parts[0].window = LabelWindow::M5;
    row.push_part(ChartLabelField::WindowSellVolume);
    row.parts[1].window = LabelWindow::M5;
    // The heading prints a SETTING and reads nothing, so it must not order a read of its own.
    row.push_part(ChartLabelField::WindowSpanName);
    row.parts[2].window = LabelWindow::H24;
    // Two counts of the same size differ only by an unused window; they are one read.
    row.push_part(ChartLabelField::WindowTrades);
    row.parts[3].span = LabelSpan::Trades(500);
    row.parts[3].window = LabelWindow::M1;
    row.push_part(ChartLabelField::WindowVolume);
    row.parts[4].span = LabelSpan::Trades(500);
    row.parts[4].window = LabelWindow::H72;
    cfg.rows[0] = row;
    cfg.sanitize();

    let spans = cfg.volume_spans();
    assert_eq!(spans.len(), 2, "one entry per distinct period");
    assert_eq!(spans[0].span, LabelSpan::Window);
    assert_eq!(spans[0].window, LabelWindow::M5);
    assert_eq!(spans[1].span, LabelSpan::Trades(500));
    assert!(
        spans.iter().all(|key| !key.liquidations),
        "nothing here prints the liquidation figure, so nothing may order that ring read"
    );
}

/// One period printing both a traded figure and the liquidation one is ONE entry, flagged.
///
/// Breakage: listing the period twice would read the trades twice; dropping the flag would read the
/// liquidation ring for every caption block on the chart, including the ones that never print `L`.
#[test]
fn a_liquidation_caption_flags_its_period_without_splitting_it() {
    let mut cfg = ChartLabelsCfg::empty();
    let mut row = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    row.push_part(ChartLabelField::WindowBuyVolume);
    row.parts[0].window = LabelWindow::M5;
    row.push_part(ChartLabelField::WindowLiquidations);
    row.parts[1].window = LabelWindow::M5;
    cfg.rows[0] = row;
    cfg.sanitize();

    let spans = cfg.volume_spans();
    assert_eq!(spans.len(), 1);
    assert!(spans[0].liquidations);
}

/// The measuring anchor is a period of its own: the same minute at the live edge and around the
/// pointer are two different windows.
#[test]
fn the_anchor_separates_two_otherwise_identical_periods() {
    let mut cfg = ChartLabelsCfg::empty();
    let mut row = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    row.push_part(ChartLabelField::WindowBuyVolume);
    row.parts[0].window = LabelWindow::M1;
    row.push_part(ChartLabelField::WindowSellVolume);
    row.parts[1].window = LabelWindow::M1;
    row.parts[1].anchor = SpanAnchor::Cursor;
    cfg.rows[0] = row;
    cfg.sanitize();

    assert_eq!(cfg.volume_spans().len(), 2);
    assert!(cfg.any_cursor_anchored());
}

/// A hidden module orders no history read.
///
/// Breakage: the gate is what keeps a chart printing no volume from walking retained rows on every
/// market revision — and a module switched off is exactly the case a reader expects to cost nothing.
#[test]
fn a_hidden_module_asks_for_no_volume() {
    let mut cfg = ChartLabelsCfg::empty();
    let mut row = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    row.push_part(ChartLabelField::WindowBuyVolume);
    row.visible = false;
    cfg.rows[0] = row;
    cfg.sanitize();

    assert!(cfg.volume_spans().is_empty());
}

/// The colour threshold is a magnitude, so a negative or non-finite one is treated as ABSENT — the
/// default, "colour everything" — rather than clamped into something the user never asked for.
#[test]
fn a_broken_colour_threshold_falls_back_to_colouring_everything() {
    let mut part = ChartLabelPart::new(ChartLabelField::Delta1h);
    part.style.color_min_pct = Some(f32::NAN);
    assert_eq!(part.resolved_style().color_min_pct, 0.0);

    part.style.color_min_pct = Some(-1.0);
    assert_eq!(part.resolved_style().color_min_pct, 0.0);

    part.style.color_min_pct = Some(0.5);
    assert_eq!(part.resolved_style().color_min_pct, 0.5);
}

/// Colouring the value alone is the DEFAULT: "Фандинг: +3.90%" is a label and a figure, and only
/// the figure has a sign.
#[test]
fn a_caption_colours_its_value_alone_by_default() {
    assert!(
        ChartLabelPart::new(ChartLabelField::Funding)
            .resolved_style()
            .value_only
    );
    let mut part = ChartLabelPart::new(ChartLabelField::Funding);
    part.style.value_only = Some(false);
    assert!(!part.resolved_style().value_only);
}

/// The backing plate belongs to the MODULE, and a file states it there. A file written before it
/// moved says nothing, and every module then draws one — which is what those files looked like,
/// since every caption carried its own plate and they all defaulted to on.
#[test]
fn the_plate_is_a_module_setting_and_defaults_to_drawn() {
    let row = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    assert!(
        row.plate,
        "a module draws its backing unless told otherwise"
    );

    let mut cfg = ChartLabelsCfg::empty();
    let mut named = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    named.push_part(ChartLabelField::Funding);
    named.plate = false;
    cfg.rows[0] = named;

    let text = toml::to_string_pretty(&cfg).expect("serializes");
    assert!(
        text.contains("plate"),
        "a module that turned it OFF states so: {text}"
    );
    let back: ChartLabelsCfg = toml::from_str(&text).expect("parses");
    assert!(!back.rows[0].plate);

    // The other direction: a file that never heard of the setting.
    let old = r#"
        [[rows]]
        zone = "chart_top"
        align = "left"
        [[rows.parts]]
        field = "funding"
    "#;
    let read: ChartLabelsCfg = toml::from_str(old).expect("parses");
    assert!(read.rows[0].plate, "absent means drawn");
}

/// A preset name this build does not know costs the row its NAME and nothing else.
///
/// The list grows, and a profile written by a newer build gets opened by an older one every time a
/// user steps back a version. This configuration is deserialized as ONE value inside `layout.toml`,
/// so a strict read would take every caption — and every window position in that file — with it.
#[test]
fn an_unknown_preset_costs_only_the_row_its_name() {
    let mut cfg = ChartLabelsCfg::empty();
    let mut row = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    row.preset = Some(LabelPreset::Detect);
    row.push_part(ChartLabelField::DetectMsg);
    cfg.rows[0] = row;
    let toml = toml::to_string(&cfg).expect("the set serializes");
    let forged = toml.replace("preset = \"detect\"", "preset = \"from_the_future\"");
    assert!(forged.contains("from_the_future"), "{forged}");

    let read: ChartLabelsCfg = toml::from_str(&forged).expect("an unknown preset still loads");
    assert_eq!(read.used_rows(), 1, "the row survives");
    assert_eq!(
        read.rows[0].preset, None,
        "it just loses the name it cannot resolve"
    );
    assert_eq!(read.rows[0].parts[0].field, ChartLabelField::DetectMsg);
}

/// A field name this build cannot resolve empties that ONE caption and leaves the rest standing.
///
/// The catalogue shrinks as well as grows — `exch_pos_price` was retired once the core turned out
/// to leave the entry price at zero — and a profile written before that still names it. A strict
/// read would fail the whole value: `layout.toml` would fall back to its default and `charts.json`
/// would not parse at all, costing the user every chart tab.
#[test]
fn an_unknown_field_costs_only_its_own_caption() {
    let mut cfg = ChartLabelsCfg::empty();
    let mut row = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    row.push_part(ChartLabelField::Coin);
    row.push_part(ChartLabelField::LiqPrice);
    cfg.rows[0] = row;
    let toml = toml::to_string(&cfg).expect("the set serializes");
    let forged = toml.replace("field = \"liq_price\"", "field = \"exch_pos_price\"");
    assert!(forged.contains("exch_pos_price"), "{forged}");

    let read: ChartLabelsCfg = toml::from_str(&forged).expect("a retired field still loads");
    assert_eq!(read.used_rows(), 1, "the module survives");
    assert_eq!(
        read.rows[0].used_parts(),
        1,
        "the retired caption is dropped rather than failing the whole read"
    );
    assert_eq!(
        read.rows[0].parts[0].field,
        ChartLabelField::Coin,
        "the sibling survives"
    );
}

/// `Авто` has no length of its own and resolves to the chart's; a fixed choice ignores the chart.
/// Resolution is TOTAL — every input answers with a named period — which is what lets the caption's
/// prefix print a period rather than the word `Авто`, and what removes the "unnameable timeframe"
/// case the prefix would otherwise have had to spell out by hand.
#[test]
fn a_label_timeframe_resolves_auto_against_the_chart() {
    assert_eq!(LabelTf::Auto.resolve_ms(5 * 60_000), 5 * 60_000);
    assert_eq!(LabelTf::H4.resolve_ms(5 * 60_000), 4 * 3_600_000);
    assert_eq!(LabelTf::D1.resolve_ms(0), 24 * 3_600_000);
    // A length outside the set — a zero included — still answers with a NAMED period rather than
    // with `Авто`. No producer emits one: `CandleViewCfg::tf_ms` already answers within this set.
    // What this pins is TOTALITY, which is what leaves the prefix with no case to spell out.
    assert_eq!(LabelTf::Auto.resolved(4 * 3_600_000), LabelTf::H4);
    assert_eq!(
        LabelTf::H1.resolved(4 * 3_600_000),
        LabelTf::H1,
        "a fixed choice ignores the chart"
    );
    assert_eq!(LabelTf::Auto.resolved(7 * 60_000), LabelTf::M5);
    assert_eq!(LabelTf::Auto.resolved(0), LabelTf::M5, "total, and never the setting");
}

/// The caption's timeframes are the chart's timeframes. Two hand-written tables of the same six
/// periods exist; this is what stops them drifting apart silently when a new one is added.
#[test]
fn the_label_timeframes_are_exactly_the_charts_own() {
    let offered: Vec<u32> = LabelTf::ALL
        .into_iter()
        .filter(|tf| *tf != LabelTf::Auto)
        .map(|tf| (tf.resolve_ms(0) / 60_000) as u32)
        .collect();
    assert_eq!(
        offered,
        crate::market::candles::CANDLE_TF_CHOICES_MIN.to_vec(),
        "the caption offers a period the chart cannot be set to, or misses one it can"
    );
}

/// The remaining time is the candle grid's, floored on the Unix epoch: never zero, never longer
/// than the period, and the same answer on every coin.
#[test]
fn a_label_timeframe_reports_the_remainder_of_its_own_bucket() {
    let hour = 3_600_000;
    assert_eq!(
        LabelTf::H1.remaining_ms(0, 0),
        hour,
        "a boundary opens a full candle"
    );
    assert_eq!(LabelTf::H1.remaining_ms(0, hour - 1), 1);
    assert_eq!(LabelTf::H1.remaining_ms(0, 90 * 60_000), 30 * 60_000);
    assert_eq!(
        LabelTf::Auto.remaining_ms(5 * 60_000, 7 * 60_000),
        3 * 60_000,
        "auto follows the chart's own bucket"
    );
}

/// A timeframe left on a caption whose field was changed to one that counts nothing down is
/// dropped, exactly like the window and the span beside it: a parameter nothing reads is one a
/// later field change would suddenly obey.
#[test]
fn sanitize_drops_a_timeframe_from_a_caption_that_counts_nothing() {
    let mut cfg = ChartLabelsCfg::empty();
    let mut row = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    row.push_part(ChartLabelField::TfCloseIn);
    row.push_part(ChartLabelField::LastPrice);
    row.parts[0].tf = LabelTf::H4;
    row.parts[1].tf = LabelTf::H4;
    cfg.rows[0] = row;
    cfg.sanitize();
    assert_eq!(
        cfg.rows[0].parts[0].tf,
        LabelTf::H4,
        "the countdown keeps it"
    );
    assert_eq!(
        cfg.rows[0].parts[1].tf,
        LabelTf::Auto,
        "the price caption does not"
    );
}

/// The clock only advances while something counts down with it, and only a countdown inside its
/// last hour buys the second-by-second step. This is the whole cost control for these captions.
#[test]
fn the_countdown_clock_picks_the_coarsest_step_that_works() {
    let tf_5m = 5 * 60_000;
    let one_of = |field: ChartLabelField, tf: LabelTf| {
        let mut cfg = ChartLabelsCfg::empty();
        let mut row = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
        row.push_part(field);
        row.parts[0].tf = tf;
        cfg.rows[0] = row;
        cfg
    };
    // Nothing drawn: no clock at all, so a chart printing no countdown pays nothing.
    let quiet = one_of(ChartLabelField::LastPrice, LabelTf::Auto);
    assert_eq!(quiet.countdown_clock_ms(tf_5m, 1_234_567), None);

    // A day away: the printed figure is hours and minutes, which a minute step already tracks.
    let daily = one_of(ChartLabelField::TfCloseIn, LabelTf::D1);
    let noon = 12 * 3_600_000;
    assert_eq!(
        daily.countdown_clock_ms(tf_5m, noon + 1_500),
        Some(noon),
        "the part-minute is quantized away"
    );
    let clock = daily
        .countdown_clock_ms(tf_5m, noon + 61_500)
        .expect("a countdown is drawn");
    assert_eq!(clock % 60_000, 0, "far out it steps by the minute: {clock}");

    // Inside the last hour of that same day candle: seconds, because the caption now prints them.
    let late = 23 * 3_600_000 + 30 * 60_000 + 1_500;
    assert_eq!(daily.countdown_clock_ms(tf_5m, late), Some(late - 500));

    // A five-minute candle is never more than five minutes out, so it is always on the fine step.
    let five = one_of(ChartLabelField::TfCloseIn, LabelTf::Auto);
    assert_eq!(five.countdown_clock_ms(tf_5m, 1_234_567), Some(1_234_000));

    // Funding alone keeps the minute step it always had.
    let funding = one_of(ChartLabelField::FundingIn, LabelTf::Auto);
    assert_eq!(
        funding.countdown_clock_ms(tf_5m, 1_234_567),
        Some(1_200_000)
    );
}

/// A default timeframe is not written, like every other caption parameter: the file states what was
/// CHANGED. A chosen one survives the round trip.
#[test]
fn a_label_timeframe_round_trips_only_when_it_was_chosen() {
    let mut cfg = ChartLabelsCfg::empty();
    let mut row = ChartLabelRow::new(LabelZone::ChartTop, LabelAlign::Left);
    row.push_part(ChartLabelField::TfCloseIn);
    cfg.rows[0] = row;
    let bare = serde_json::to_string(&cfg).expect("writes");
    assert!(!bare.contains("\"tf\""), "the default is silent: {bare}");

    cfg.rows[0].parts[0].tf = LabelTf::H4;
    let chosen = serde_json::to_string(&cfg).expect("writes");
    assert!(chosen.contains("\"tf\""), "a choice is stated: {chosen}");
    let back: ChartLabelsCfg = serde_json::from_str(&chosen).expect("reads");
    assert_eq!(back.rows[0].parts[0].tf, LabelTf::H4);
}
