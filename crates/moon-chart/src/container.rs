//! Типы-описания вкладок/панелей чарта, общие для UI-оболочки. Сама логика контейнера
//! (open/auto/prune/layout/режим) живёт в own-pass-оболочке (`chartdx::pane` в
//! moon-ui-gpui), которая ре-экспортит эти типы. wgpu-движок панелей (`Pane{chart:Chart}`)
//! удалён вместе с egui-бинарём.

use moon_core::config::ChartBucket;

#[derive(Clone, PartialEq, Eq, Hash)]
pub enum ContainerKind {
    /// Главная вкладка: клики по детектам, фулскрин-центричный.
    Main,
    /// Чарт-вкладка AddToChart=`num`. `bucket` — куда сведены графики ядра внутри
    /// группы (своё ядро / общая вкладка / именованная связка). См. `ChartBucket`.
    Chart { num: u32, bucket: ChartBucket },
}

/// Источник панели — влияет на TTL и поведение.
#[derive(Clone, Copy)]
pub enum PaneSource {
    /// Открыта вручную (клик по детекту) — живёт до закрытия крестиком.
    Manual,
    /// Авто-добавлена по AddToChart — живёт `ttl_ms` от последнего детекта.
    AddToChart { born_ms: f64, ttl_ms: f64 },
}
