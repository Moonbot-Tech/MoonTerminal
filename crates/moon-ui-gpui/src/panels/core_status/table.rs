//! Таблица «Статус ядер»: колонки и ячейки. Числовые метрики — прочерк, если строки
//! соответствующего вида лога ещё не было (старый core их не печатает).

use super::*;
use moon_ui::{MoonDataCell, MoonDataRow, MoonDataTable, MoonDataTableColumn};

/// Колонки: ядро, статус, CPU (тек./сред.), память (прил./сис.), свободно (физ./подкачка),
/// «обновлено N назад».
fn columns() -> Vec<MoonDataTableColumn> {
    let numeric =
        |key: &'static str, title: String, w: f32| MoonDataTableColumn::new(key, title, w).right();
    vec![
        MoonDataTableColumn::new("core", t!("core_status.col.core").to_string(), 130.0),
        MoonDataTableColumn::new("status", t!("core_status.col.status").to_string(), 120.0),
        numeric("cpu", t!("core_status.col.cpu").to_string(), 90.0),
        numeric("cpu_avg", t!("core_status.col.cpu_avg").to_string(), 90.0),
        numeric("mem_app", t!("core_status.col.mem_app").to_string(), 100.0),
        numeric("mem_sys", t!("core_status.col.mem_sys").to_string(), 100.0),
        numeric(
            "free_phys",
            t!("core_status.col.free_phys").to_string(),
            110.0,
        ),
        numeric(
            "free_page",
            t!("core_status.col.free_page").to_string(),
            110.0,
        ),
        numeric("updated", t!("core_status.col.updated").to_string(), 100.0),
    ]
}

pub(super) fn core_status_table(
    id: &'static str,
    rows: Rc<Vec<CoreStatusRow>>,
    now_ms: i64,
    state: &Entity<MoonDataTableState>,
    cx: &Context<CoreStatusView>,
) -> impl IntoElement {
    let empty = rows.is_empty();
    let row_count = rows.len();
    let table_rows = rows.clone();
    let p = MoonPalette::active(cx);

    crate::panels::common::data_table_host(
        SharedString::from(format!("{id}-host")),
        empty,
        t!("core_status.empty").to_string(),
        p,
        cx,
        MoonDataTable::new(id, row_count, move |ix, _window, _app| {
            core_status_row(&table_rows[ix], now_ms, p)
        })
        .columns(columns())
        .state(state)
        .header_height(design::TABLE_HEAD_H)
        .row_height(design::TABLE_ROW_H),
    )
}

fn core_status_row(r: &CoreStatusRow, now_ms: i64, p: MoonPalette) -> MoonDataRow {
    let sys = &r.sys;
    // Самая свежая из двух строк (CPU/память) — для «обновлено».
    let last_ms = sys.cpu_ms.max(sys.mem_ms);
    MoonDataRow::new([
        MoonDataCell::text(r.name.clone()),
        MoonDataCell::element(status_cell(&r.status, p)),
        MoonDataCell::text(pct(sys.cpu_moment)),
        MoonDataCell::text(pct(sys.cpu_avg)),
        MoonDataCell::text(mb(sys.mem_app_mb)),
        MoonDataCell::text(mb(sys.mem_sys_mb)),
        MoonDataCell::text(mb(sys.free_phys_mb)),
        MoonDataCell::text(mb(sys.free_page_mb)),
        MoonDataCell::text(ago(last_ms, now_ms)),
    ])
}

/// Ячейка статуса: цветной кружок (зелёный=Ready, янтарь=подключение, красный=ошибка,
/// серый=нет связи) + локализованная подпись. Порт `status_dot` из настроек подключений.
fn status_cell(status: &ConnStatus, p: MoonPalette) -> impl IntoElement + 'static {
    let (color, label) = match status {
        ConnStatus::Ready => (p.green, t!("conn.status.ready").to_string()),
        ConnStatus::Connecting => (p.amber, t!("conn.status.connecting").to_string()),
        ConnStatus::Stage(s) => (
            p.amber,
            t!("conn.status.stage", stage = s.clone()).to_string(),
        ),
        ConnStatus::Failed(e) => (p.red, t!("conn.status.failed", err = e.clone()).to_string()),
        ConnStatus::Disconnected => (p.text_soft, t!("conn.status.disconnected").to_string()),
    };
    h_flex()
        .w_full()
        .h_full()
        .items_center()
        .gap_2()
        .child(
            div()
                .w(px(8.0))
                .h(px(8.0))
                .flex_none()
                .rounded_full()
                .bg(rgb(color)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(rgb(p.text_soft))
                .child(label),
        )
}

/// Проценты CPU: `97.8%` (одна десятая). `None` → прочерк.
fn pct(v: Option<f32>) -> String {
    v.map(|v| format!("{v:.1}%"))
        .unwrap_or_else(|| "—".to_string())
}

/// Мегабайты: целое + « МБ». `None` → прочерк.
fn mb(v: Option<u32>) -> String {
    v.map(|v| format!("{v} {}", t!("core_status.mb")))
        .unwrap_or_else(|| "—".to_string())
}

/// «Обновлено» — сколько назад пришла последняя строка (компактно, чтобы не обрезалось в узкой
/// правой колонке): `< 60 с → «Nс»`, `< 60 мин → «Nм»`, иначе `«Nч»`. `0`/будущее → прочерк.
/// Число биндим в локальную переменную (path-аргумент rust-i18n рендерится надёжнее выражения).
fn ago(last_ms: i64, now_ms: i64) -> String {
    if last_ms <= 0 {
        return "—".to_string();
    }
    let secs = ((now_ms - last_ms) / 1000).max(0);
    if secs < 60 {
        let n = secs;
        t!("core_status.ago_s", n = n).to_string()
    } else if secs < 3600 {
        let n = secs / 60;
        t!("core_status.ago_m", n = n).to_string()
    } else {
        let n = secs / 3600;
        t!("core_status.ago_h", n = n).to_string()
    }
}
