//! Системные метрики ядра (CPU/память), вытащенные из СТРОК серверного лога.
//!
//! moonproto НЕ несёт типизированных полей CPU/памяти по ядру — но ядро (MoonBot core)
//! периодически печатает их INFO-строками, которые приходят к нам через `FeedMsg::ServerLog`.
//! Здесь мы парсим эти строки и держим последний снимок на ядро (`CoreData::sys`). Два вида:
//!
//! - CPU:   `CPU auto report [Avg] stored to ...log Moment: 97.8% Avg: 33.7%`
//! - Память:`[Memory] UsedMem App: 641  Sys: 669  FreeMem Phys: 45 Page: 773`
//!
//! Значения — как их печатает ядро: CPU в процентах, память в мегабайтах.

/// Последний снимок системных метрик ядра. Поля `None`, пока соответствующая строка лога
/// не встречена (старый core может их не печатать). Время — из строки лога (unix ms).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CoreSysStatus {
    /// Мгновенная загрузка CPU, % (`Moment:`).
    pub cpu_moment: Option<f32>,
    /// Средняя загрузка CPU, % (`Avg:`).
    pub cpu_avg: Option<f32>,
    /// Время последней CPU-строки, unix ms (0 — не было).
    pub cpu_ms: i64,
    /// Память приложения, МБ (`UsedMem App:`).
    pub mem_app_mb: Option<u32>,
    /// Память системы, МБ (`Sys:`).
    pub mem_sys_mb: Option<u32>,
    /// Свободная физическая память, МБ (`FreeMem Phys:`).
    pub free_phys_mb: Option<u32>,
    /// Свободный файл подкачки, МБ (`Page:`).
    pub free_page_mb: Option<u32>,
    /// Время последней memory-строки, unix ms (0 — не было).
    pub mem_ms: i64,
}

impl CoreSysStatus {
    /// Есть ли хоть какая-то метрика (иначе строку в таблице показываем прочерками).
    pub fn is_empty(&self) -> bool {
        self.cpu_ms == 0 && self.mem_ms == 0
    }

    /// Разобрать одну строку серверного лога. Обновляет поля и время; возвращает `true`,
    /// если что-то распозналось (для bump'а `sys_rev` в сторе). `ts_ms` — время строки.
    pub fn parse_line(&mut self, msg: &str, ts_ms: i64) -> bool {
        // CPU-строка: содержит "Moment:" и "Avg:" (маркер "CPU auto report" — как гейт).
        if msg.contains("CPU auto report") {
            let moment = num_after(msg, "Moment:").map(|v| v as f32);
            let avg = num_after(msg, "Avg:").map(|v| v as f32);
            if moment.is_some() || avg.is_some() {
                if moment.is_some() {
                    self.cpu_moment = moment;
                }
                if avg.is_some() {
                    self.cpu_avg = avg;
                }
                self.cpu_ms = ts_ms;
                return true;
            }
            return false;
        }
        // Память: "[Memory] UsedMem App: A  Sys: B  FreeMem Phys: C Page: D".
        if msg.contains("[Memory]") && msg.contains("UsedMem") {
            let app = num_after(msg, "App:").map(|v| v as u32);
            let sys = num_after(msg, "Sys:").map(|v| v as u32);
            let phys = num_after(msg, "Phys:").map(|v| v as u32);
            let page = num_after(msg, "Page:").map(|v| v as u32);
            if app.is_some() || sys.is_some() || phys.is_some() || page.is_some() {
                if app.is_some() {
                    self.mem_app_mb = app;
                }
                if sys.is_some() {
                    self.mem_sys_mb = sys;
                }
                if phys.is_some() {
                    self.free_phys_mb = phys;
                }
                if page.is_some() {
                    self.free_page_mb = page;
                }
                self.mem_ms = ts_ms;
                return true;
            }
        }
        false
    }
}

/// Число, идущее сразу за меткой `label` в `hay` (первое вхождение). Пропускает пробелы
/// после метки, читает `[-]?[0-9.]+`, останавливается на любом другом символе (`%` и т.п.).
/// `None`, если метки нет или число не собралось.
fn num_after(hay: &str, label: &str) -> Option<f64> {
    let start = hay.find(label)? + label.len();
    let rest = &hay[start..];
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i] == b' ' {
        i += 1;
    }
    let num_start = i;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_digit() || c == b'.' || c == b'-' {
            i += 1;
        } else {
            break;
        }
    }
    if i == num_start {
        return None;
    }
    rest[num_start..i].parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cpu_line() {
        let mut s = CoreSysStatus::default();
        let line = "CPU auto report [Avg] stored to C:\\x\\log Moment: 97.8% Avg: 33.7%";
        assert!(s.parse_line(line, 1000));
        assert_eq!(s.cpu_moment, Some(97.8));
        assert_eq!(s.cpu_avg, Some(33.7));
        assert_eq!(s.cpu_ms, 1000);
        assert_eq!(s.mem_ms, 0);
    }

    #[test]
    fn parses_memory_line() {
        let mut s = CoreSysStatus::default();
        let line = "[Memory] UsedMem App: 641  Sys: 669  FreeMem Phys: 45 Page: 773 ";
        assert!(s.parse_line(line, 2000));
        assert_eq!(s.mem_app_mb, Some(641));
        assert_eq!(s.mem_sys_mb, Some(669));
        assert_eq!(s.free_phys_mb, Some(45));
        assert_eq!(s.free_page_mb, Some(773));
        assert_eq!(s.mem_ms, 2000);
    }

    #[test]
    fn ignores_unrelated_lines() {
        let mut s = CoreSysStatus::default();
        assert!(!s.parse_line("Srv: Sent 451 strategies to clients", 3000));
        assert!(s.is_empty());
    }
}
