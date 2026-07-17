//! Форматирование чисел для UI/feed.

/// Компактное число: точность `decimals`, хвостовые нули и точка срезаются
/// ("1.500000" → "1.5", "2.000000" → "2"). Нули режутся ТОЛЬКО в дробной
/// части: при `decimals=0` строка без точки, и слепой трим калечил целые
/// ("330" → "33", "1000" → "1").
pub fn compact(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    if !s.contains('.') {
        return s;
    }
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() {
        "0".to_string()
    } else {
        s.to_string()
    }
}

/// Компактное число с SI-суффиксом (K/M/B/T): 1_500 → «1.5K», 2_300_000 → «2.3M».
/// Значения меньше 1000 идут через [`adaptive`] (без суффикса). Хвостовые нули срезаются.
pub fn compact_si(v: f64) -> String {
    let a = v.abs();
    if a < 1000.0 {
        return adaptive(v);
    }
    const UNITS: [(f64, &str); 4] = [(1e12, "T"), (1e9, "B"), (1e6, "M"), (1e3, "K")];
    for (scale, suffix) in UNITS {
        if a >= scale {
            let n = v / scale;
            let s = if n.abs() >= 100.0 {
                format!("{n:.0}")
            } else if n.abs() >= 10.0 {
                format!("{n:.1}")
            } else {
                format!("{n:.2}")
            };
            let s = s.trim_end_matches('0').trim_end_matches('.');
            return format!("{s}{suffix}");
        }
    }
    adaptive(v)
}

/// Точность `decimals`, хвостовые нули срезаются, но МИНИМУМ один знак после точки
/// остаётся ("45.20" → "45.2", "45.00" → "45.0", "10000.000" → "10000.0").
fn trim_keep_one(v: f64, decimals: usize) -> String {
    let mut s = format!("{v:.decimals$}");
    if let Some(dot) = s.find('.') {
        let min_len = dot + 2; // точка + один знак
        while s.len() > min_len && s.ends_with('0') {
            s.pop();
        }
    }
    s
}

/// Количество для таблиц активов: знаков по величине (крупнее — меньше), максимум
/// тысячные, минимум десятые. "0.16206"→"0.162", "7.7972"→"7.797", "35483"→"35483.0".
pub fn qty(v: f64) -> String {
    let a = v.abs();
    let decimals = if a >= 100.0 {
        1
    } else if a >= 10.0 {
        2
    } else {
        3
    };
    trim_keep_one(v, decimals)
}

/// Долларовая сумма (без символа): максимум сотые, минимум десятые.
/// "45.238"→"45.24", "45.2"→"45.2", "10176"→"10176.0".
pub fn usd(v: f64) -> String {
    trim_keep_one(v, 2)
}

/// Адаптивное число под размер/цену: точность подбирается по величине, а не фиксирована.
/// Крупные значения — без дробной части (5000000.0001 → "5000000", 5000 → "5000");
/// мелкие — с достаточным числом знаков, чтобы значащие цифры были видны
/// (0.0000001 → "0.0000001"). `sig` — желаемое число значащих цифр (для дробной части).
pub fn adaptive(v: f64) -> String {
    let a = v.abs();
    if a == 0.0 {
        return "0".to_string();
    }
    // Тысячи и больше — без дробной части.
    if a >= 1000.0 {
        return compact(v, 0);
    }
    const SIG: i32 = 5;
    // Экспонента старшего разряда: для a<1 отрицательна (0.0001 → -4).
    let exp = a.log10().floor() as i32;
    // Знаков после запятой = столько, чтобы набрать SIG значащих цифр (с запасом
    // на ведущие нули у мелких чисел). Ограничиваем сверху на разумный максимум.
    let decimals = (SIG - 1 - exp).clamp(0, 18) as usize;
    compact(v, decimals)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_keeps_integer_zeros() {
        // Регрессия: слепой трим нулей калечил целые (330 → «33», 1000 → «1»).
        assert_eq!(compact(330.0, 0), "330");
        assert_eq!(compact(1000.0, 0), "1000");
        assert_eq!(compact(0.0, 0), "0");
        assert_eq!(compact(-500.0, 0), "-500");
        // Дробная часть по-прежнему чистится.
        assert_eq!(compact(1.5, 6), "1.5");
        assert_eq!(compact(2.0, 6), "2");
        assert_eq!(compact(45.20, 2), "45.2");
    }

    #[test]
    fn adaptive_thousands_intact() {
        assert_eq!(adaptive(25000.0), "25000");
        assert_eq!(adaptive(1000.0), "1000");
    }
}
