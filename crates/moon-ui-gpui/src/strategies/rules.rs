//! Правила зависимостей полей стратегий: какое поле редактируемо/раздел активен в
//! зависимости от значений ДРУГИХ полей. Источник — `assets/param_deps.toml`
//! (`"Поле" = "A=VAL;B<>VAL"`). На старте пробуем внешний файл из dev-пути, иначе
//! берём вшитый фолбэк. Hot-reload внешнего файла включается только явным env
//! `MOON_STRATEGY_RULES_HOT_RELOAD`; production не опрашивает filesystem каждую секунду.
//! Парсим имя→условия один раз.
//! Порт egui `src/strategies/rules.rs` (точь-в-точь).
//!
//! Логика разделов выводится отсюда же + соглашение имён `Ignore*` (см. mod.rs):
//! отдельного конфига разделов нет.

use std::collections::HashMap;
use std::time::SystemTime;

/// Внешний путь (относительно cwd) для hot-reload в dev (`cargo run` → корень воркспейса).
const EXTERNAL: &str = "assets/param_deps.toml";
/// Фолбэк, вшитый в бинарь (release-запуск без assets рядом).
const BUNDLED: &str = include_str!("../../../../assets/param_deps.toml");

/// Значения полей выбранной стратегии: имя(lowercase) → значение(как есть).
pub type Values = HashMap<String, String>;

/// Оператор условия.
#[derive(Clone, Copy)]
enum Op {
    Eq,
    Ne,
    Gt,
    Lt,
    Ge,
    Le,
}

/// Одно условие: `field` (op) `value`.
#[derive(Clone)]
struct Cond {
    field: String,
    op: Op,
    value: String,
}

pub struct Rules {
    /// Имя поля(lowercase) → список условий (через ; — И).
    deps: HashMap<String, Vec<Cond>>,
    /// mtime внешнего файла — для hot-reload.
    mtime: Option<SystemTime>,
}

impl Rules {
    /// Грузит правила: внешний файл, если есть, иначе вшитый фолбэк.
    pub fn load() -> Self {
        let mut r = Rules {
            deps: HashMap::new(),
            mtime: None,
        };
        match std::fs::read_to_string(EXTERNAL) {
            Ok(content) => {
                r.mtime = file_mtime();
                r.parse_into(&content);
            }
            Err(_) => r.parse_into(BUNDLED),
        }
        r
    }

    /// Перечитывает внешний файл, если он изменился. true — перечитали (нужен кадр).
    pub fn reload_if_changed(&mut self) -> bool {
        let m = file_mtime();
        if m.is_some() && m != self.mtime {
            if let Ok(content) = std::fs::read_to_string(EXTERNAL) {
                self.mtime = m;
                self.deps.clear();
                self.parse_into(&content);
                return true;
            }
        }
        false
    }

    /// Построчный разбор (терпимый к ручной правке): `"Поле" = "условие"`. Терпит
    /// дубликаты (побеждает последний), комментарии `#`, заголовок `[deps]`, кавычки.
    /// Один битый ключ не валит весь файл (в отличие от строгого TOML).
    fn parse_into(&mut self, content: &str) {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
                continue;
            }
            // Разделитель — ПЕРВЫЙ '=' (внутри значения '=' уже за кавычками).
            let Some(eq) = line.find('=') else { continue };
            let key = line[..eq].trim().trim_matches('"').trim().to_lowercase();
            let expr = line[eq + 1..].trim().trim_matches('"').trim();
            if key.is_empty() {
                continue;
            }
            self.deps.insert(key, parse_conds(expr));
        }
        log::info!(
            "strategy param rules: {} полей с зависимостями",
            self.deps.len()
        );
    }

    /// Поле активно (редактируемо), если все его условия истинны на текущих
    /// значениях. Нет правила — активно. Условие на поле, которого НЕТ в values
    /// (т.е. нет у этого вида стратегии вовсе), неприменимо — не блокирует. Поля
    /// схемы кладутся в values с дефолтом/пустым (см. `selected_values`), так что
    /// «нет в values» = «нет у вида», а несохранённое поле сравнивается по дефолту.
    pub fn field_active(&self, name: &str, values: &Values) -> bool {
        match self.deps.get(&name.to_lowercase()) {
            None => true,
            Some(conds) => conds.iter().all(|c| match values.get(&c.field) {
                None => true,
                Some(v) => cond_true(c, v),
            }),
        }
    }
}

/// Истинно ли условие `c` на значении `v`. `=`/`<>` — булево/строковое сравнение,
/// `>`/`<`/`>=`/`<=` — числовое (нечисловое значение → условие НЕ выполнено).
fn cond_true(c: &Cond, v: &str) -> bool {
    match c.op {
        Op::Eq => value_eq(v, &c.value),
        Op::Ne => !value_eq(v, &c.value),
        _ => match (v.trim().parse::<f64>(), c.value.trim().parse::<f64>()) {
            (Ok(a), Ok(e)) => match c.op {
                Op::Gt => a > e,
                Op::Lt => a < e,
                Op::Ge => a >= e,
                Op::Le => a <= e,
                _ => true,
            },
            _ => false,
        },
    }
}

/// Булева трактовка значения: ядро отдаёт да/нет как `1/0`, `Yes/No`, `true/false`.
/// None — не булево (число/строка).
fn as_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "yes" | "true" | "1" | "on" => Some(true),
        "no" | "false" | "0" | "off" | "" => Some(false),
        _ => None,
    }
}

/// Сравнение значения условия: если ОБЕ стороны булевы (включая `0/1`), сравниваем
/// как булевы (чтобы `IgnoreVolume=NO` совпало с сырым `"0"`), иначе — как строки.
fn value_eq(actual: &str, expected: &str) -> bool {
    match (as_bool(actual), as_bool(expected)) {
        (Some(a), Some(e)) => a == e,
        _ => actual.eq_ignore_ascii_case(expected),
    }
}

/// mtime внешнего файла правил (None — файла нет).
fn file_mtime() -> Option<SystemTime> {
    std::fs::metadata(EXTERNAL)
        .ok()
        .and_then(|m| m.modified().ok())
}

/// Разбирает `A=VAL;B<>VAL;C>1` в условия. Операторы проверяем от длинных к
/// коротким (`<>`,`>=`,`<=` раньше `>`,`<`,`=`). Поля/значения — в lowercase.
fn parse_conds(expr: &str) -> Vec<Cond> {
    const OPS: [(&str, Op); 6] = [
        ("<>", Op::Ne),
        (">=", Op::Ge),
        ("<=", Op::Le),
        (">", Op::Gt),
        ("<", Op::Lt),
        ("=", Op::Eq),
    ];
    expr.split(';')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            OPS.iter().find_map(|&(s, op)| {
                part.find(s).map(|i| Cond {
                    field: part[..i].trim().to_lowercase(),
                    op,
                    value: part[i + s.len()..].trim().to_lowercase(),
                })
            })
        })
        .collect()
}
