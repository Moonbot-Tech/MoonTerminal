//! Настройки отображения ленты детектов: размер карточки + видимые поля. Per-group
//! (лента одна на окно-терминала = группу), persist в `layout.toml`
//! (`WindowLayout::detect_view_by_group`, ключ — имя группы). Отсутствие записи →
//! [`DetectViewCfg::default`] (средний размер, как было + мини-чарт).

use serde::{Deserialize, Serialize};

/// Размер карточки детекта.
pub const DETECT_SIZE_MINI: u8 = 0;
pub const DETECT_SIZE_MEDIUM: u8 = 1;
pub const DETECT_SIZE_LARGE: u8 = 2;

/// Что показывать в карточке детекта. Галки — источник истины; каждый размер рисует
/// пересечение включённых полей с тем, что физически помещается (мини — только
/// монета+бейдж+время; средний — строка как раньше + чарт; крупный — квадрат со всем).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DetectViewCfg {
    /// Размер карточки: 0=мини, 1=средний, 2=крупный (квадрат).
    pub size: u8,
    /// Обратный отсчёт времени детекта («Ns»).
    pub show_time: bool,
    /// Имя ядра (бейдж справа-снизу).
    pub show_core: bool,
    /// Бейдж типа детекта (код сработавшей стратегии).
    pub show_badge: bool,
    /// Мини-чарт — замороженный тумбнейл на момент детекта.
    pub show_chart: bool,
    /// Название биржи (Binance/Bybit/…).
    pub show_exchange: bool,
    /// Тип биржи (спот/фьючи/квартальные).
    pub show_exchange_kind: bool,
}

impl Default for DetectViewCfg {
    fn default() -> Self {
        Self {
            size: DETECT_SIZE_MEDIUM,
            show_time: true,
            show_core: true,
            show_badge: true,
            show_chart: true,
            show_exchange: false,
            show_exchange_kind: false,
        }
    }
}

impl DetectViewCfg {
    /// Нормализованный размер (клампится к валидному диапазону).
    pub fn size_clamped(&self) -> u8 {
        self.size.min(DETECT_SIZE_LARGE)
    }
}
