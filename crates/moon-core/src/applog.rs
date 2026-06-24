//! Лог приложения: (1) файловый лог сырых команд/отчётов ядра в `logs/commands.log`
//! рядом с exe — для диагностики report-SQL (INSERT/UPDATE), которые шлёт ядро;
//! (2) общий in-memory кольцевой буфер для вкладки «Лог» нижнего дока.
//!
//! Буфер наполняют два источника: [`command`] (сырые report-SQL ядра) и
//! [`TeeLogger`] — обёртка над `env_logger`, дублирующая каждую напечатанную
//! `log::`-запись в буфер (и дальше отдающая её env_logger'у в консоль/файл).
//! Потокобезопасно (несколько feed-потоков + UI-поток читает снимок).

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::config::paths;
use crate::util::now_unix_ms_i64 as now_ms_i64;

static LOG: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();

/// Максимум строк в кольцевом буфере (старые вытесняются).
const RING_CAP: usize = 5000;

/// Писать ли лог в файлы logs/<дата>_<источник>.log. Дефолт on, чтобы ранние
/// записи (до загрузки конфига) не терялись; приложение уточняет из конфига.
static FILE_LOG: AtomicBool = AtomicBool::new(true);
/// Срок хранения файлов лога (дней). 0 = хранить всё. См. [`purge_old`].
static RETENTION_DAYS: AtomicU32 = AtomicU32::new(14);

/// Применить настройки файлового лога из конфига (вызывается из App после загрузки
/// и после сохранения настроек).
pub fn set_file_logging(enabled: bool, retention_days: u32) {
    FILE_LOG.store(enabled, Ordering::Relaxed);
    RETENTION_DAYS.store(retention_days, Ordering::Relaxed);
}

/// Пишем ли сейчас лог в файлы.
pub fn file_logging_enabled() -> bool {
    FILE_LOG.load(Ordering::Relaxed)
}

/// Одна строка лога для вкладки «Лог».
#[derive(Clone)]
pub struct LogLine {
    /// Время UTC, `YYYY-MM-DD HH:MM:SS.mmm`.
    pub ts: String,
    pub level: log::Level,
    /// Источник: target лог-записи или `core.cmd` для сырых команд ядра.
    /// Пустой для строк лога ядра (источник ясен из выбранного сервера).
    pub target: String,
    pub msg: String,
}

impl LogLine {
    /// Строка лога ядра (по unix-времени из ServerLog). Уровня у ядра нет → Info,
    /// target пустой (источник — выбранный сервер в селекторе).
    pub fn core(unix_ms: i64, msg: String) -> Self {
        Self {
            ts: ts_from_unix_ms(unix_ms),
            level: log::Level::Info,
            target: String::new(),
            msg,
        }
    }

    /// Эвристика «строка про ошибку» — для быстрого фильтра «только ошибки».
    /// Уровень Warn/Error ИЛИ текст содержит error-подобные слова (лог ядра без
    /// уровней — фильтруем по содержимому).
    pub fn is_errorish(&self) -> bool {
        if matches!(self.level, log::Level::Error | log::Level::Warn) {
            return true;
        }
        let m = self.msg.to_lowercase();
        [
            "error",
            "ошиб",
            "fail",
            "warn",
            "panic",
            "exception",
            "critical",
        ]
        .iter()
        .any(|k| m.contains(k))
    }
}

static RING: OnceLock<Mutex<VecDeque<LogLine>>> = OnceLock::new();
/// Монотонный счётчик добавлений — дёшево ловит «появились новые строки» (host
/// форсит кадр, когда активна вкладка «Лог»). См. [`revision`].
static REVISION: AtomicU64 = AtomicU64::new(0);

fn ring() -> &'static Mutex<VecDeque<LogLine>> {
    RING.get_or_init(|| Mutex::new(VecDeque::with_capacity(RING_CAP)))
}

/// Глобальный файловый писатель локального лога приложения (`logs/<дата>_app.log`).
static APP_WRITER: OnceLock<Mutex<DatedWriter>> = OnceLock::new();
fn app_writer() -> &'static Mutex<DatedWriter> {
    APP_WRITER.get_or_init(|| Mutex::new(DatedWriter::new("app")))
}

/// Добавить строку в кольцевой буфер (с вытеснением самой старой) и, если включён
/// файловый лог, дописать её в `logs/<дата>_app.log`.
fn push(line: LogLine) {
    if file_logging_enabled() {
        if let Ok(mut w) = app_writer().lock() {
            let date = line.ts.get(0..10).unwrap_or("");
            let hms = line.ts.get(11..).unwrap_or(line.ts.as_str());
            w.write(date, hms, level_code(line.level), &line.target, &line.msg);
            w.flush(); // лог приложения низкочастотный — флашим сразу (виден на диске)
        }
    }
    if let Ok(mut r) = ring().lock() {
        if r.len() >= RING_CAP {
            r.pop_front();
        }
        r.push_back(line);
    }
    REVISION.fetch_add(1, Ordering::Relaxed);
}

/// Ревизия буфера (число добавлений). Сравнивай для детекта новых строк.
pub fn revision() -> u64 {
    REVISION.load(Ordering::Relaxed)
}

/// Снимок последних `max` строк (по порядку старые→новые) для вкладки «Лог».
pub fn snapshot(max: usize) -> Vec<LogLine> {
    ring()
        .lock()
        .map(|r| {
            let start = r.len().saturating_sub(max);
            r.iter().skip(start).cloned().collect()
        })
        .unwrap_or_default()
}

fn handle() -> Option<&'static Mutex<std::fs::File>> {
    LOG.get_or_init(|| {
        let dir = paths::logs_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("applog: не удалось создать {}: {e}", dir.display());
            return None;
        }
        let path = dir.join("commands.log");
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(f) => {
                eprintln!("applog: команды пишутся в {}", path.display());
                Some(Mutex::new(f))
            }
            Err(e) => {
                eprintln!("applog: не удалось открыть {}: {e}", path.display());
                None
            }
        }
    })
    .as_ref()
}

fn ts() -> String {
    ts_from_unix_ms(now_ms_i64())
}

/// unix-ms → ("YYYY-MM-DD", "HH:MM:SS.mmm") в UTC (дата для имени файла, время в строке).
pub fn split_unix_ms(ms: i64) -> (String, String) {
    let secs = ms.div_euclid(1000);
    let frac = ms.rem_euclid(1000);
    let full = crate::db::fmt_unix_secs(secs); // "YYYY-MM-DD HH:MM:SS"
    let date = full.get(0..10).unwrap_or("").to_string();
    let time = full.get(11..).unwrap_or("");
    (date, format!("{time}.{frac:03}"))
}

/// unix-ms → "YYYY-MM-DD HH:MM:SS.mmm" (UTC).
pub fn ts_from_unix_ms(ms: i64) -> String {
    let (d, t) = split_unix_ms(ms);
    format!("{d} {t}")
}

/// Код уровня для записи в файл (TSV).
fn level_code(l: log::Level) -> &'static str {
    match l {
        log::Level::Error => "ERR",
        log::Level::Warn => "WARN",
        log::Level::Info => "INFO",
        log::Level::Debug => "DBG",
        log::Level::Trace => "TRC",
    }
}

fn level_from_code(s: &str) -> log::Level {
    match s {
        "ERR" => log::Level::Error,
        "WARN" => log::Level::Warn,
        "DBG" => log::Level::Debug,
        "TRC" => log::Level::Trace,
        _ => log::Level::Info,
    }
}

/// Метку источника приводим к безопасному имени файла (буквы/цифры/-/_).
pub fn sanitize_label(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() {
        "core".to_string()
    } else {
        s
    }
}

/// Файловый писатель лога одного источника с дневной ротацией: пишет в
/// `logs/<дата>_<label>.log`, при смене суток сам переоткрывает файл. Формат строки —
/// TSV: `время \t уровень \t источник \t сообщение`. Используется и локальным логом
/// приложения, и потоком каждого ядра (там — со своим экземпляром на поток).
pub struct DatedWriter {
    label: String,
    date: String,
    file: Option<BufWriter<File>>,
}

impl DatedWriter {
    pub fn new(label: &str) -> Self {
        Self {
            label: sanitize_label(label),
            date: String::new(),
            file: None,
        }
    }

    /// Дописать строку. Если файловый лог выключен — ничего не пишет (и закрывает
    /// открытый файл). `date`=YYYY-MM-DD (имя файла), `hms`=HH:MM:SS.mmm (в строку).
    pub fn write(&mut self, date: &str, hms: &str, level: &str, target: &str, msg: &str) {
        if !file_logging_enabled() {
            self.file = None;
            return;
        }
        if self.file.is_none() || self.date != date {
            self.open(date);
        }
        if let Some(f) = self.file.as_mut() {
            // msg без переводов строк — одна запись = одна строка файла.
            let msg = msg.replace(['\n', '\r'], " ");
            let _ = writeln!(f, "{hms}\t{level}\t{target}\t{msg}");
        }
    }

    fn open(&mut self, date: &str) {
        let dir = paths::logs_dir();
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("{date}_{}.log", self.label));
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()
            .map(BufWriter::new);
        self.date = date.to_string();
    }

    pub fn flush(&mut self) {
        if let Some(f) = self.file.as_mut() {
            let _ = f.flush();
        }
    }
}

impl Drop for DatedWriter {
    fn drop(&mut self) {
        self.flush();
    }
}

/// Удалить файлы лога старше срока хранения. Имя `<YYYY-MM-DD>_<label>.log` — возраст
/// берём из даты в имени (надёжнее mtime). 0 дней = не удалять. Файлы без даты в
/// имени (например `commands.log`) не трогаем.
pub fn purge_old() {
    let days = RETENTION_DAYS.load(Ordering::Relaxed);
    if days == 0 {
        return;
    }
    let cutoff = now_ms_i64().div_euclid(1000) - (days as i64) * 86_400;
    let dir = paths::logs_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut removed = 0u32;
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".log") {
            continue;
        }
        let Some(date) = name.get(0..10) else {
            continue;
        };
        let Some(file_secs) = crate::db::parse_ymd(date) else {
            continue; // нет даты в начале имени → не наш ротируемый файл
        };
        if file_secs < cutoff && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    if removed > 0 {
        log::info!("лог: удалено старых файлов: {removed} (хранение {days} дн.)");
    }
}

/// Список файлов лога для источника (по метке), новейшие сверху. Метка: "app" для
/// локального лога, имя ядра — для лога ядра.
pub fn list_files(label: &str) -> Vec<String> {
    let suffix = format!("_{}.log", sanitize_label(label));
    let dir = paths::logs_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut v: Vec<String> = rd
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.ends_with(&suffix).then_some(n)
        })
        .collect();
    v.sort(); // дата в начале имени → лексикографически = по возрастанию даты
    v.reverse(); // новейшие сверху
    v
}

/// Прочитать историю из файла лога (последние `max` строк). Парсит TSV-формат;
/// чужие/битые строки кладём целиком в msg.
pub fn read_file(filename: &str, max: usize) -> Vec<LogLine> {
    let path = paths::logs_dir().join(filename);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let date = filename.get(0..10).unwrap_or("");
    let mut out: Vec<LogLine> = text.lines().map(|l| parse_file_line(date, l)).collect();
    if out.len() > max {
        let drop = out.len() - max;
        out.drain(0..drop);
    }
    out
}

fn parse_file_line(date: &str, l: &str) -> LogLine {
    let mut it = l.splitn(4, '\t');
    match (it.next(), it.next(), it.next(), it.next()) {
        (Some(hms), Some(lv), Some(tg), Some(m)) => LogLine {
            ts: format!("{date} {hms}"),
            level: level_from_code(lv),
            target: tg.to_string(),
            msg: m.to_string(),
        },
        _ => LogLine {
            ts: date.to_string(),
            level: log::Level::Info,
            target: String::new(),
            msg: l.to_string(),
        },
    }
}

/// Пишет строку команды в `logs/commands.log` (с UTC-временем) и в in-memory
/// буфер вкладки «Лог».
pub fn command(line: &str) {
    let ts = ts();
    if file_logging_enabled() {
        if let Some(m) = handle() {
            if let Ok(mut f) = m.lock() {
                let _ = writeln!(f, "[{ts}] {line}");
            }
        }
    }
    push(LogLine {
        ts,
        level: log::Level::Info,
        target: "core.cmd".to_string(),
        msg: line.to_string(),
    });
}

/// Логгер-обёртка: дублирует напечатанные записи в кольцевой буфер и делегирует
/// форматирование/вывод внутреннему `env_logger`. Ставится глобально в `main`.
pub struct TeeLogger {
    inner: env_logger::Logger,
}

impl TeeLogger {
    pub fn new(inner: env_logger::Logger) -> Self {
        Self { inner }
    }
}

impl log::Log for TeeLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.inner.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        // В буфер кладём только то, что реально проходит фильтр env_logger (то же,
        // что попадёт в консоль) — иначе буфер забьётся trace-шумом зависимостей.
        if self.inner.matches(record) {
            push(LogLine {
                ts: ts(),
                level: record.level(),
                target: record.target().to_string(),
                msg: record.args().to_string(),
            });
        }
        self.inner.log(record);
    }

    fn flush(&self) {
        self.inner.flush();
    }
}
