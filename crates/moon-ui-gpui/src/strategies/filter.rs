//! Фильтры дерева стратегий. Все условия действуют одновременно (И).
//! Отделены от остального состояния окна: чистые предикаты без UI.
//! Порт egui `src/strategies/filter.rs` (точь-в-точь).

use moon_core::feed::StrategyRow;

pub struct StrategyFilter {
    /// По названию стратегии (подстрока, без регистра).
    pub search: String,
    /// По виду стратегии (ordinal). None — все виды.
    pub kind: Option<u8>,
    /// По направлению: None — все, Some(true) — SHORT, Some(false) — LONG.
    pub dir: Option<bool>,
    /// Показывать только активные (запущенные). По умолчанию вкл.
    pub only_active: bool,
}

impl Default for StrategyFilter {
    fn default() -> Self {
        Self {
            search: String::new(),
            kind: None,
            dir: None,
            only_active: true,
        }
    }
}

impl StrategyFilter {
    /// Поиск активен → дерево временно раскрываем целиком.
    pub fn searching(&self) -> bool {
        !self.search.trim().is_empty()
    }

    /// Условие для СЧЁТЧИКОВ активных/всего: вид И направление (без имени и без
    /// «только активные»), чтобы цифры на ядрах/папках отражали выбранный тип и L/S.
    pub fn counts(&self, row: &StrategyRow) -> bool {
        self.kind.is_none_or(|k| row.kind_ordinal == k)
            && self.dir.is_none_or(|s| row.is_short == s)
    }

    /// Видимость строки в дереве: имя И вид И направление И («только активные» → checked).
    pub fn matches(&self, row: &StrategyRow) -> bool {
        let q = self.search.trim().to_lowercase();
        let by_name = q.is_empty() || row.name.to_lowercase().contains(&q);
        let by_active = !self.only_active || row.checked;
        self.counts(row) && by_name && by_active
    }
}
