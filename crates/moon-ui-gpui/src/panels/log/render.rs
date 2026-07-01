//! Лог: сигнатура (гейт пересборки), агрегат живых логов ядер, классификация и
//! рендер одной строки (цвет по тяжести, подсветка монеты и совпадений поиска).
//!
//! Классификация (`classify`) и детект монеты (`find_coin`) считаются ОДИН раз при
//! сборке видимого списка (`apply_filter` → [`LineView`]), не на каждом кадре: парсинг
//! текста дорог, а рендер строки вызывается для каждой видимой строки каждый кадр.

use super::*;
use std::collections::HashSet;
use std::ops::Range;

/// «Тяжесть» строки — определяет цвет всей строки. У лога ядра уровня нет
/// (`LogLine::core` → Info), поэтому Error/Warn для него выводим из текста.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Sev {
    Error,
    Warn,
    Info,
    /// Debug/Trace — приглушаем.
    Dim,
    /// Шум форка GPUI (`window not found` пачками на уровне ERR при закрытии окон,
    /// см. docs-internal/FORK_BUGS.md) — не настоящая ошибка, гасим, чтобы не тонули
    /// реальные ошибки. В фильтр «только ошибки» НЕ попадает.
    Noise,
}

/// Семантическая категория строки (независимо от тяжести) — для короткого бейджа.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Cat {
    None,
    /// Отказ/отклонение ордера, недостаточно средств и т.п.
    Reject,
    /// Проблема связи: разрыв/переподключение/таймаут.
    Conn,
}

/// Результат классификации строки: тяжесть + категория (считаются за один проход,
/// один `to_lowercase`).
#[derive(Clone, Copy)]
pub(super) struct Class {
    pub(super) sev: Sev,
    pub(super) cat: Cat,
}

/// Строка, подготовленная к отрисовке: время/источник/тяжесть/категория/плоский текст
/// и диапазон тикера монеты (байтовый, по `flat`). Всё посчитано заранее.
#[derive(Clone)]
pub(super) struct LineView {
    /// Время (HH:MM:SS.mmm) — хвост `ts`.
    pub(super) time: String,
    pub(super) target: String,
    pub(super) sev: Sev,
    pub(super) cat: Cat,
    /// Сообщение без переводов строк (для однострочного рендера).
    pub(super) flat: String,
    /// Монета строки: диапазон токена в `flat` (подсветка) + база для клик-фильтра
    /// (напр. токен `USDT-SPK` → база `SPK`), если найдена.
    pub(super) coin: Option<(Range<usize>, String)>,
}

impl LineView {
    /// Собрать вид строки из уже посчитанной классификации (детект монеты — здесь, чтобы
    /// не сканировать строки, которые отсеет фильтр «только ошибки»). `cl` берём из
    /// [`classify`], вызванного до фильтра, чтобы не классифицировать дважды. `known` —
    /// базы монет, собранные из всего буфера (для подсветки голых тикеров вроде `SPK`).
    pub(super) fn from_parts(line: &LogLine, cl: Class, known: &HashSet<String>) -> Self {
        let time = line
            .ts
            .rsplit(' ')
            .next()
            .unwrap_or(line.ts.as_str())
            .to_string();
        let flat = line.msg.replace('\n', " ⏎ ");
        let coin = find_coin(&flat, known);
        Self {
            time,
            target: line.target.clone(),
            sev: cl.sev,
            cat: cl.cat,
            flat,
            coin,
        }
    }
}

/// Тяжесть считается ошибкой для фильтра «только ошибки». Шум форка исключён намеренно.
pub(super) fn is_error(sev: Sev) -> bool {
    matches!(sev, Sev::Error | Sev::Warn)
}

const ERROR_KW: [&str; 8] = [
    "error",
    "panic",
    "exception",
    "critical",
    "fail",
    "reject",
    "ошиб",
    "отклон",
];
const WARN_KW: [&str; 5] = ["warn", "timeout", "таймаут", "retry", "предупре"];
const REJECT_KW: [&str; 6] = [
    "reject",
    "отклон",
    "denied",
    "insufficient",
    "недостаточно",
    "rejected",
];
const CONN_KW: [&str; 8] = [
    "disconnect",
    "reconnect",
    "connection",
    "разрыв",
    "timeout",
    "таймаут",
    "соедин",
    "подключ",
];

/// Классификация: тяжесть + категория за один `to_lowercase`. Явный уровень (лог
/// приложения) приоритетнее текста; шум форка — поверх всего (он приходит уровнем ERR).
/// Лог ядра идёт уровнем Info → тяжесть смотрим по тексту.
pub(super) fn classify(line: &LogLine) -> Class {
    let lower = line.msg.to_lowercase();
    if lower.contains("window not found") || lower.contains("недопустимый дескриптор окна") {
        return Class {
            sev: Sev::Noise,
            cat: Cat::None,
        };
    }
    let cat = categorize(&lower);
    let sev = match line.level {
        log::Level::Error => Sev::Error,
        log::Level::Warn => Sev::Warn,
        log::Level::Debug | log::Level::Trace => Sev::Dim,
        log::Level::Info => {
            if ERROR_KW.iter().any(|k| lower.contains(k)) {
                Sev::Error
            } else if WARN_KW.iter().any(|k| lower.contains(k)) {
                Sev::Warn
            } else {
                Sev::Info
            }
        }
    };
    Class { sev, cat }
}

/// Категория по тексту (уже в нижнем регистре). Reject важнее Conn (таймаут при отказе —
/// это отказ).
fn categorize(lower: &str) -> Cat {
    if REJECT_KW.iter().any(|k| lower.contains(k)) {
        Cat::Reject
    } else if CONN_KW.iter().any(|k| lower.contains(k)) {
        Cat::Conn
    } else {
        Cat::None
    }
}

const QUOTES: [&str; 6] = ["USDT", "USDC", "BUSD", "PERP", "USDe", "USD"];

fn is_tick(b: u8) -> bool {
    b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-' || b == b'_'
}

/// Базовая монета рыночного токена — если токен «рыночной формы»:
/// - спот/перп с котируемой валютой: `SPKUSDT`/`SPK-USDT`/`SPK_USDT` → `SPK`;
/// - котировка впереди: `USDT-SPK`/`USDT_SPK` → `SPK`;
/// - поставочный контракт «база+дата»: `BTC_0626`/`ETH-240628` → `BTC`.
/// Иначе `None`. Нужна заглавная буква (чтобы дата `2026-06-30` не считалась тикером).
fn market_base(w: &str) -> Option<String> {
    if w.len() < 4 || !w.bytes().any(|b| b.is_ascii_uppercase()) {
        return None;
    }
    for q in QUOTES {
        // Котировка суффиксом: <BASE><QUOTE>, <BASE>-<QUOTE>, <BASE>_<QUOTE>.
        if w.len() > q.len() && w.ends_with(q) {
            let base = w[..w.len() - q.len()].trim_end_matches(['-', '_']);
            if base.len() >= 3 && base.bytes().any(|b| b.is_ascii_uppercase()) {
                return Some(base.to_string());
            }
        }
        // Котировка префиксом: <QUOTE>-<BASE>, <QUOTE>_<BASE>.
        if w.len() > q.len() + 1 && w.starts_with(q) {
            let after = &w[q.len()..];
            if let Some(base) = after.strip_prefix('-').or_else(|| after.strip_prefix('_')) {
                if base.len() >= 3 && base.bytes().any(|b| b.is_ascii_uppercase()) {
                    return Some(base.to_string());
                }
            }
        }
    }
    // Поставочный/квартальный контракт: база из букв + разделитель + хвост из цифр.
    if let Some(pos) = w.rfind(['_', '-']) {
        let (base, tail) = (&w[..pos], &w[pos + 1..]);
        let base_ok = base.len() >= 3 && base.bytes().any(|b| b.is_ascii_uppercase());
        let tail_ok = (2..=8).contains(&tail.len()) && tail.bytes().all(|b| b.is_ascii_digit());
        if base_ok && tail_ok {
            return Some(base.to_string());
        }
    }
    None
}

/// Набор базовых монет, встреченных в буфере в рыночной форме — чтобы затем подсветить
/// их голые упоминания (`SPK` сам по себе неотличим от `BUY`/`API` по форме).
pub(super) fn collect_coin_bases(lines: &[LogLine]) -> HashSet<String> {
    let mut set = HashSet::new();
    for l in lines {
        let bytes = l.msg.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if !is_tick(bytes[i]) {
                i += 1;
                continue;
            }
            let start = i;
            while i < bytes.len() && is_tick(bytes[i]) {
                i += 1;
            }
            if let Some(base) = market_base(&l.msg[start..i]) {
                set.insert(base);
            }
        }
    }
    set
}

/// Первая монета в сообщении: токен рыночной формы (подсветим весь токен, база — для
/// клик-фильтра) ИЛИ голый тикер, известный по буферу (`known`). Возвращает (диапазон,
/// база). Свой сканер, без regex.
pub(super) fn find_coin(msg: &str, known: &HashSet<String>) -> Option<(Range<usize>, String)> {
    let bytes = msg.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !is_tick(bytes[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && is_tick(bytes[i]) {
            i += 1;
        }
        let word = &msg[start..i];
        if let Some(base) = market_base(word) {
            return Some((start..i, base));
        }
        if word.len() >= 3 && known.contains(word) {
            return Some((start..i, word.to_string()));
        }
    }
    None
}

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

/// Базовый цвет текста строки по тяжести.
fn sev_color(sev: Sev, p: MoonPalette) -> u32 {
    match sev {
        Sev::Error => p.red,
        Sev::Warn => p.amber,
        Sev::Info => p.text_soft,
        Sev::Dim | Sev::Noise => p.text_muted,
    }
}

/// Бейдж уровня (только для Error/Warn).
fn badge(sev: Sev, p: MoonPalette) -> Option<(&'static str, u32)> {
    match sev {
        Sev::Error => Some(("ERR", p.red)),
        Sev::Warn => Some(("WARN", p.amber)),
        _ => None,
    }
}

/// Бейдж категории (отказ / связь).
fn cat_badge(cat: Cat, p: MoonPalette) -> Option<(&'static str, u32)> {
    match cat {
        Cat::Reject => Some(("REJ", p.orange)),
        Cat::Conn => Some(("NET", p.yellow)),
        Cat::None => None,
    }
}

/// Вид сегмента сообщения — определяет стиль. Совпадение поиска важнее монеты.
#[derive(Clone, Copy, PartialEq)]
enum Seg {
    Plain,
    Coin,
    Match,
}

fn seg_at(idx: usize, coin: &Option<Range<usize>>, matches: &[Range<usize>]) -> Seg {
    if matches.iter().any(|r| r.contains(&idx)) {
        Seg::Match
    } else if coin.as_ref().is_some_and(|r| r.contains(&idx)) {
        Seg::Coin
    } else {
        Seg::Plain
    }
}

/// Разбить сообщение на цветные спаны: база по тяжести, тикер монеты — синим (кликабелен →
/// фильтр по монете), совпадения поиска — акцентом (жирным). Пустой запрос без монеты →
/// один спан (быстрый путь).
fn message_spans(
    flat: &str,
    base: u32,
    coin: &Option<Range<usize>>,
    query: &str,
    // (панель, база монеты, источник строки) — для ЛКМ-фильтра и ПКМ-«открыть график».
    coin_click: Option<(WeakEntity<LogPanel>, SharedString, SharedString)>,
    p: MoonPalette,
) -> Vec<AnyElement> {
    let mut matches: Vec<Range<usize>> = Vec::new();
    if !query.is_empty() {
        let lower = flat.to_lowercase();
        let mut from = 0;
        while let Some(pos) = lower[from..].find(query) {
            let s = from + pos;
            matches.push(s..s + query.len());
            from = s + query.len();
        }
    }
    if coin.is_none() && matches.is_empty() {
        return vec![div()
            .flex_none()
            .text_color(rgb(base))
            .child(flat.to_string())
            .into_any_element()];
    }
    let span = |text: &str, seg: Seg| -> AnyElement {
        match seg {
            Seg::Plain => div()
                .flex_none()
                .text_color(rgb(base))
                .child(text.to_string())
                .into_any_element(),
            Seg::Match => div()
                .flex_none()
                .font_bold()
                .text_color(rgb(p.accent))
                .child(text.to_string())
                .into_any_element(),
            Seg::Coin => {
                let mut d = div()
                    .flex_none()
                    .font_bold()
                    .text_color(rgb(p.blue))
                    .child(text.to_string());
                if let Some((weak, ticker, target)) = coin_click.clone() {
                    // ЛКМ — фильтр по этой монете.
                    d = d.cursor_pointer().on_mouse_down(MouseButton::Left, {
                        let weak = weak.clone();
                        let ticker = ticker.clone();
                        move |_ev, _w, app| {
                            if let Some(e) = weak.upgrade() {
                                let ticker = ticker.to_string();
                                e.update(app, |t, cx| t.set_coin_filter(Some(ticker), cx));
                            }
                        }
                    });
                    // ПКМ — открыть график монета+ядро на Main (stop_propagation, чтобы не
                    // сработало копирование строки, висящее на всей строке).
                    d = d.on_mouse_down(MouseButton::Right, move |_ev, _w, app| {
                        if let Some(e) = weak.upgrade() {
                            let (base, target) = (ticker.to_string(), target.to_string());
                            e.update(app, |t, cx| t.open_coin_chart(base, target, cx));
                        }
                        app.stop_propagation();
                    });
                }
                d.into_any_element()
            }
        }
    };
    let mut out: Vec<AnyElement> = Vec::new();
    let mut cur: Option<Seg> = None;
    let mut buf = String::new();
    for (idx, ch) in flat.char_indices() {
        let seg = seg_at(idx, coin, &matches);
        if Some(seg) != cur {
            if let Some(prev) = cur {
                out.push(span(&buf, prev));
                buf.clear();
            }
            cur = Some(seg);
        }
        buf.push(ch);
    }
    if let Some(prev) = cur {
        if !buf.is_empty() {
            out.push(span(&buf, prev));
        }
    }
    out
}

/// Рендер одной строки лога (время · [уровень] · [категория] · источник · сообщение).
/// `query` — уже trim+lowercase запрос поиска (подсветка совпадений). ПКМ по строке
/// копирует её в буфер обмена; клик по тикеру монеты фильтрует по этой монете.
pub(super) fn log_row(
    v: &LineView,
    query: &str,
    weak: &WeakEntity<LogPanel>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let base = sev_color(v.sev, p);
    let mut row = h_flex()
        .w_full()
        .gap_1()
        .items_baseline()
        .text_size(crate::design::t_body(cx))
        .px_1();
    row = row.child(
        div()
            .flex_none()
            .text_color(rgb(p.text_muted))
            .child(v.time.clone()),
    );
    if let Some((tag, col)) = badge(v.sev, p) {
        row = row.child(div().flex_none().font_bold().text_color(rgb(col)).child(tag));
    }
    if let Some((tag, col)) = cat_badge(v.cat, p) {
        row = row.child(div().flex_none().font_bold().text_color(rgb(col)).child(tag));
    }
    if !v.target.is_empty() {
        row = row.child(
            div()
                .flex_none()
                .text_color(rgb(p.text_soft))
                .child(v.target.clone()),
        );
    }
    // Клик по монете фильтрует по её базе (напр. токен `USDT-SPK` → фильтр `SPK`);
    // ПКМ открывает график этой монеты на ядре строки (`target`).
    let coin_range = v.coin.as_ref().map(|(r, _)| r.clone());
    let coin_click = v.coin.as_ref().map(|(_, base)| {
        (
            weak.clone(),
            SharedString::from(base.clone()),
            SharedString::from(v.target.clone()),
        )
    });
    // Копия строки по правому клику (TSV-подобно: время · источник · текст).
    let copy = if v.target.is_empty() {
        format!("{} {}", v.time, v.flat)
    } else {
        format!("{} {} {}", v.time, v.target, v.flat)
    };
    row.child(
        h_flex()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .children(message_spans(&v.flat, base, &coin_range, query, coin_click, p)),
    )
    .on_mouse_down(MouseButton::Right, move |_ev, _w, app| {
        app.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
    })
    .into_any_element()
}
