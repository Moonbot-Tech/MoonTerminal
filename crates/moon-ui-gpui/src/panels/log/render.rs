//! Лог: сигнатура (гейт пересборки), агрегат живых логов ядер и рендер одной строки.

use super::*;

/// Сигнатура лога: ревизия кольца applog + сумма log_rev ядер группы. Растёт при
/// любой новой строке (локальной или ядра). Не сменилась → пересобирать не нужно.
pub(super) fn log_sig(b: &Backend, group: &str) -> u64 {
    let store = b.session.store();
    let scoped = !group.is_empty();
    let cores: u64 = b
        .session
        .sessions()
        .iter()
        .filter(|s| !scoped || s.group == group)
        .filter_map(|s| store.core(s.id))
        .fold(0u64, |a, c| a.wrapping_mul(31).wrapping_add(c.log_rev));
    applog::revision().wrapping_add(cores)
}

/// Слияние живых логов всех ядер области по времени (ts лексикографичен = хронологичен).
pub(super) fn aggregate(store: &CoreStore, sources: &[LogSourceItem]) -> Vec<LogLine> {
    let mut merged: Vec<LogLine> = Vec::new();
    for item in sources {
        if let LogSource::Core(id) = item.source {
            if let Some(c) = store.core(id) {
                for mut l in c.log_snapshot(AGG_PER_CORE) {
                    l.target = item.display.clone();
                    merged.push(l);
                }
            }
        }
    }
    merged.sort_by(|a, b| a.ts.cmp(&b.ts));
    if merged.len() > VIEW_LIMIT {
        let drop = merged.len() - VIEW_LIMIT;
        merged.drain(0..drop);
    }
    merged
}

/// Бейдж уровня + цвет (палитра).
fn level_tag(level: log::Level, p: MoonPalette) -> Option<(&'static str, u32)> {
    match level {
        log::Level::Error => Some(("ERR", p.red)),
        log::Level::Warn => Some(("WARN", p.amber)),
        _ => None,
    }
}

/// Рендер одной строки лога (время · [уровень] · источник · сообщение).
pub(super) fn log_row(line: &LogLine, p: MoonPalette, cx: &App) -> AnyElement {
    let time = line
        .ts
        .rsplit(' ')
        .next()
        .unwrap_or(line.ts.as_str())
        .to_string();
    let flat = line.msg.replace('\n', " ⏎ ");
    let mut row = h_flex()
        .w_full()
        .gap_1()
        .items_baseline()
        .text_size(crate::design::t_body(cx))
        .px_1();
    row = row.child(div().flex_none().text_color(rgb(p.text_soft)).child(time));
    if let Some((tag, col)) = level_tag(line.level, p) {
        row = row.child(
            div()
                .flex_none()
                .font_bold()
                .text_color(rgb(col))
                .child(tag),
        );
    }
    if !line.target.is_empty() {
        row = row.child(
            div()
                .flex_none()
                .text_color(rgb(p.text_soft))
                .child(line.target.clone()),
        );
    }
    row.child(
        div()
            .flex_1()
            .min_w_0()
            .text_color(rgb(p.text_soft))
            .child(flat),
    )
    .into_any_element()
}
