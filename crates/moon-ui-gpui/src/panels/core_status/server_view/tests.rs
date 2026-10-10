//! Regression tests for Core Status tree items and reported-build layout.

use std::net::{IpAddr, Ipv4Addr};

use moon_core::feed::{ConnStatus, CoreEndpoint, CoreTimeOffsetStatus};
use moon_core::session::{CoreStartupStatus, CoreSysStatus};

use super::tree_items;
use crate::panels::core_status::model::{CoreStatusRow, aggregate_servers};

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    App, Bounds, Context, InteractiveElement, IntoElement, ParentElement, Pixels, Render, Styled,
    Window, div, px,
};
use moon_ui::{
    MoonDataCell, MoonDataRow, MoonDataTable, MoonDataTableColumn, MoonDataTableWidthPolicy,
    MoonRect, MoonTableAlign, MoonTheme, h_flex, v_flex,
};

use super::version_mark;
use crate::design;
use crate::panels::core_status::by_ip_widths::ByIpWidths;
use crate::panels::core_status::presentation::build_parts;

/// Render the production mark under the Flat cell's centred chrome or the By-IP slot's flex host.
struct BuildLayoutProbe {
    /// Select the content-sized Flat mark rather than the growing By-IP mark.
    end: bool,
    /// Flat table width and alignment, including the historical persisted-width reproduction.
    flat: Option<(f32, MoonTableAlign)>,
    /// Capture both production children, including the invisible reserved badge slot.
    bounds: Rc<RefCell<Vec<Vec<Bounds<Pixels>>>>>,
}

impl Render for BuildLayoutProbe {
    /// Lay out tagged and bare synthetic builds in equal-width rows without backend state.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        if let Some((width, align)) = self.flat {
            let bounds = self.bounds.clone();
            let mut column = MoonDataTableColumn::new("version", "Build", width).no_grow();
            column.align = align;
            let table = MoonDataTable::new("build-layout-table", 2, move |index, _, cx| {
                MoonDataRow::new([MoonDataCell::element(build_probe_cell(
                    index,
                    true,
                    bounds.clone(),
                    cx,
                ))])
            })
            .bounds(MoonRect::new(0.0, 0.0, width, 160.0))
            .columns([column])
            .width_policy(MoonDataTableWidthPolicy::Preserve);
            return div()
                .debug_selector(|| "flat-column".into())
                .w(px(width))
                .child(table)
                .into_any_element();
        }
        let mut rows = v_flex();
        for index in 0..2 {
            rows = rows.child(
                div()
                    .w(px(ByIpWidths::BASE.version))
                    .flex_none()
                    .child(build_probe_cell(index, self.end, self.bounds.clone(), cx)),
            );
        }
        rows.into_any_element()
    }
}

/// Match the production cell's mark and reserved update slot without constructing a live backend.
fn build_probe_cell(
    index: usize,
    end: bool,
    bounds: Rc<RefCell<Vec<Vec<Bounds<Pixels>>>>>,
    cx: &App,
) -> gpui::Div {
    let suffix = (index == 0).then_some("R3");
    let selector = if index == 0 {
        "tagged-cell"
    } else {
        "bare-cell"
    };
    let mark = version_mark(
        build_parts(Some(771), suffix),
        MoonTheme::active_tokens(cx).palette,
        cx,
        end,
    )
    .on_children_prepainted(move |children, _, _| bounds.borrow_mut()[index] = children);
    let update_slot = div()
        .flex_none()
        .w(design::ui_px(cx, crate::controls::core_update::SLOT_W));
    let cell = if end {
        div()
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .gap_1()
    } else {
        h_flex().w_full().items_center().gap(px(super::CELL_GAP_W))
    };
    cell.debug_selector(move || selector.into())
        .child(mark)
        .child(update_slot)
}

/// Restoring the shrinking number or growing Flat mark squeezes a build into an ellipsis;
/// removing the invisible badge reservation shifts bare builds relative to tagged rows.
#[gpui::test]
fn build_number_and_letter_fit_both_default_cells_without_shifting(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(
                moon_core::config::UiThemeMode::Dark,
                1.0,
            ),
            cx,
        );
    });
    let measured = cx.update(|cx| design::mono_body_text_width(cx, "7.71", 400.0));
    for flat in [
        Some((128.0, MoonTableAlign::Center)),
        Some((96.0, MoonTableAlign::Center)),
        Some((96.0, MoonTableAlign::Right)),
        None,
    ] {
        let end = flat.is_some();
        let bounds = Rc::new(RefCell::new(vec![Vec::new(), Vec::new()]));
        let captured = bounds.clone();
        let window = cx.add_window(move |_, _| BuildLayoutProbe {
            end,
            flat,
            bounds: captured,
        });
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        for _ in 0..3 {
            visual.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
        let cells = if end {
            // The inner w_full element may have a content-sized flex box; containment is
            // against the actual fixed-width table column, not that inner box.
            [visual
                .debug_bounds("flat-column")
                .expect("Flat column laid out"); 2]
        } else {
            [
                visual
                    .debug_bounds("tagged-cell")
                    .expect("tagged cell laid out"),
                visual
                    .debug_bounds("bare-cell")
                    .expect("bare cell laid out"),
            ]
        };
        let bounds = bounds.borrow();
        for (index, cell) in cells.iter().enumerate() {
            let children = &bounds[index];
            assert_eq!(
                children.len(),
                2,
                "number and reserved letter slot, end={end}"
            );
            let (number, letter) = (children[0], children[1]);
            assert!(
                f32::from(number.size.width) + 0.01 >= measured,
                "number width {:?} is narrower than measured {measured}px, end={end}, row={index}",
                number.size.width,
            );
            assert!(
                letter.origin.x >= number.right(),
                "letter overlaps number, end={end}"
            );
            assert!(
                number.origin.x >= cell.origin.x,
                "number starts outside cell, end={end}, flat={flat:?}, number={number:?}, cell={cell:?}"
            );
            assert!(
                letter.right() <= cell.right(),
                "letter extends past cell, end={end}, flat={flat:?}, letter={letter:?}, cell={cell:?}"
            );
        }
        assert!(
            (f32::from(bounds[0][0].origin.x - cells[0].origin.x)
                - f32::from(bounds[1][0].origin.x - cells[1].origin.x))
            .abs()
                <= 0.01,
            "tagged and bare number x positions differ, end={end}",
        );
    }
}

/// Build one ready core snapshot at an address.
fn row(id: u64, address: IpAddr, port: u16) -> CoreStatusRow {
    CoreStatusRow {
        fault: None,
        mode_suggestion: None,
        id,
        name: format!("Core {id}"),
        status: ConnStatus::Ready,
        sys: CoreSysStatus::default(),
        endpoint: Some(CoreEndpoint { address, port }),
        ping_warn: false,
        exch_warn: false,
        ping_sev: crate::backend::core_warn::LatencySeverity::Normal,
        exch_sev: crate::backend::core_warn::LatencySeverity::Normal,
        api_key: crate::panels::core_status::model::ApiKeyState::Unknown,
        api_warn: false,
        api_notice: false,
        api_quota: None,
        api_quota_warn: false,
        startup: CoreStartupStatus::default(),
        time_offset: CoreTimeOffsetStatus::default(),
        server_version: None,
        server_version_suffix: None,
        version_behind: None,
        update: None,
    }
}

/// `server_view.rs:tree_items` must give each server root a folder of its core children, so the
/// row expands to per-core detail.
#[test]
fn server_root_folds_its_core_children() {
    let address = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 17));
    let groups = aggregate_servers(&[row(51, address, 3000)], None);

    let items = tree_items(&groups);

    assert_eq!(items.len(), 1);
    assert!(items[0].is_folder());
    assert_eq!(items[0].children.len(), 1);
    assert_eq!(items[0].children[0].id.as_ref(), "core:51");
}

/// `server_view.rs:tree_items` must follow the aggregate's address order, keeping stable server and
/// core ids so the virtual tree does not reshuffle.
#[test]
fn tree_items_follow_address_order() {
    let low = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10));
    let high = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 20));
    let groups = aggregate_servers(&[row(82, high, 3000), row(81, low, 3000)], None);

    let items = tree_items(&groups);

    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id.to_string(), format!("server:{low}"));
    assert_eq!(items[0].children[0].id.as_ref(), "core:81");
    assert_eq!(items[1].id.to_string(), format!("server:{high}"));
    assert_eq!(items[1].children[0].id.as_ref(), "core:82");
}
