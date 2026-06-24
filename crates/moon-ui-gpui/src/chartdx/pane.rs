//! Контейнер одной панели чарта — ЛОГИКА (open/auto/prune/layout), без GPU. Порт смыслов
//! `moon_chart::container`, но панель хранит только `ChartView` (математика вида), а не
//! wgpu-движок: рисуем своим own-pass (`super::combo`/…). GPU-состояние слоёв живёт отдельно
//! в `RenderState` (см. `mod.rs`), синхронизируется с этими панелями по индексу.

use moon_chart::view::{ChartView, Rect};
use moon_core::session::CoreId;

// Виды/источник панели — переиспользуем общие типы, но НЕ режимы раскладки:
// в терминале один `ChartEngine` владеет максимум одним рынком.
pub use moon_chart::container::{ContainerKind, PaneSource};

/// Применить масштаб цены к виду: None = Авто, Some(доля) = процент от цены.
fn apply_scale(view: &mut ChartView, pct: Option<f32>) {
    match pct {
        None => view.set_auto(),
        Some(p) => view.set_scale_percent(p),
    }
}

/// Одна панель: ядро/рынок/источник + вид (координаты). GPU-слои — в `RenderState` по индексу.
#[derive(Clone)]
pub struct Pane {
    pub core: CoreId,
    pub market: String,
    pub source: PaneSource,
    pub view: ChartView,
    /// П.2: пользователь «приколол» AddToChart-панель → TTL не закрывает её. На Manual-панели
    /// не влияет (они и так живут вечно). Сессионный флаг (панели сами по себе не персистятся).
    pub pinned: bool,
}

#[derive(Clone)]
pub struct Container {
    /// Идентичность вкладки (Main / Chart{num}); используется при persist раскладки (позже).
    #[allow(dead_code)]
    pub kind: ContainerKind,
    pane: Option<Pane>,
    /// Текущий масштаб цены контейнера (None=Авто): новые панели создаются сразу с ним.
    scale: Option<f32>,
}

impl Container {
    pub fn new(kind: ContainerKind) -> Self {
        Self {
            kind,
            pane: None,
            scale: None,
        }
    }

    fn new_view(&self, epoch_ms: f64) -> ChartView {
        let mut view = ChartView::new(epoch_ms);
        apply_scale(&mut view, self.scale);
        view
    }

    fn find(&self, core: CoreId, market: &str) -> Option<usize> {
        self.pane
            .as_ref()
            .is_some_and(|p| p.core == core && p.market == market)
            .then_some(0)
    }

    pub fn view_mut(&mut self, idx: usize) -> Option<&mut ChartView> {
        if idx == 0 {
            self.pane.as_mut().map(|p| &mut p.view)
        } else {
            None
        }
    }

    pub fn target(&self, idx: usize) -> Option<(CoreId, String)> {
        if idx == 0 {
            self.pane.as_ref().map(|p| (p.core, p.market.clone()))
        } else {
            None
        }
    }

    pub fn target_ref(&self, idx: usize) -> Option<(CoreId, &str)> {
        if idx == 0 {
            self.pane.as_ref().map(|p| (p.core, p.market.as_str()))
        } else {
            None
        }
    }

    pub fn pane(&self, idx: usize) -> Option<&Pane> {
        if idx == 0 { self.pane.as_ref() } else { None }
    }

    pub fn pane_mut(&mut self, idx: usize) -> Option<&mut Pane> {
        if idx == 0 { self.pane.as_mut() } else { None }
    }

    pub fn panes(&self) -> &[Pane] {
        match &self.pane {
            Some(pane) => std::slice::from_ref(pane),
            None => &[],
        }
    }

    pub fn panes_mut(&mut self) -> &mut [Pane] {
        match &mut self.pane {
            Some(pane) => std::slice::from_mut(pane),
            None => &mut [],
        }
    }

    pub fn pane_count(&self) -> usize {
        usize::from(self.pane.is_some())
    }

    pub fn is_empty(&self) -> bool {
        self.pane.is_none()
    }

    /// Задать масштаб цены контейнера: применить ко ВСЕМ панелям и запомнить для будущих.
    pub fn set_scale(&mut self, pct: Option<f32>) {
        self.scale = pct;
        for p in self.panes_mut() {
            apply_scale(&mut p.view, pct);
        }
    }

    /// Ручное открытие монеты. Инвариант терминала: один `ChartEngine` = один рынок.
    /// Стек нескольких графиков живёт снаружи как список отдельных `ChartPanel`.
    pub fn open_manual(&mut self, core: CoreId, market: &str, epoch_ms: f64) {
        if self.find(core, market).is_some() {
            if let Some(p) = self.pane.as_mut() {
                p.source = PaneSource::Manual;
            }
            return;
        }
        let view = self.new_view(epoch_ms);
        self.pane = Some(Pane {
            core,
            market: market.to_string(),
            source: PaneSource::Manual,
            view,
            pinned: false,
        });
    }

    /// AddToChart-детект для одного графика: продлить TTL или заменить рынок в этом
    /// `ChartPanel`. Несколько графиков держит внешний `AddChartStack`, не внутренний tiled canvas.
    pub fn push_auto(
        &mut self,
        core: CoreId,
        market: &str,
        now_ms: f64,
        ttl_ms: f64,
        epoch_ms: f64,
    ) {
        match self.find(core, market) {
            Some(_) => {
                if let Some(p) = self.pane.as_mut() {
                    p.source = PaneSource::AddToChart {
                        born_ms: now_ms,
                        ttl_ms,
                    };
                }
            }
            None => {
                let view = self.new_view(epoch_ms);
                self.pane = Some(Pane {
                    core,
                    market: market.to_string(),
                    source: PaneSource::AddToChart {
                        born_ms: now_ms,
                        ttl_ms,
                    },
                    view,
                    pinned: false,
                });
            }
        }
    }

    /// Удалить истёкшие AddToChart-панели. Возвращает удалённые рынки для owner/refcount.
    pub fn prune_ttl(&mut self, now_ms: f64) -> Vec<(CoreId, String)> {
        let remove = self.pane.as_ref().is_some_and(|p| match p.source {
            PaneSource::AddToChart { born_ms, ttl_ms } => !p.pinned && now_ms - born_ms >= ttl_ms,
            PaneSource::Manual => false,
        });
        if remove {
            if let Some(p) = self.pane.take() {
                return vec![(p.core, p.market)];
            }
        }
        Vec::new()
    }

    pub fn has_ttl_panes(&self) -> bool {
        self.panes()
            .iter()
            .any(|p| matches!(p.source, PaneSource::AddToChart { .. }) && !p.pinned)
    }

    pub fn next_ttl_deadline_ms(&self) -> Option<f64> {
        self.panes()
            .iter()
            .filter_map(|p| match p.source {
                // Приколотые панели дедлайна не имеют (П.2).
                PaneSource::AddToChart { born_ms, ttl_ms } if !p.pinned => Some(born_ms + ttl_ms),
                _ => None,
            })
            .min_by(|a, b| a.total_cmp(b))
    }

    /// Можно ли приколоть панель idx (только AddToChart с TTL; Manual/Main — нет смысла). П.2
    pub fn is_pinnable(&self, idx: usize) -> bool {
        self.pane(idx)
            .is_some_and(|p| matches!(p.source, PaneSource::AddToChart { .. }))
    }

    pub fn is_pinned(&self, idx: usize) -> bool {
        self.pane(idx).is_some_and(|p| p.pinned)
    }

    /// Переключить пин панели idx. Возвращает новое состояние (или None — индекс вне диапазона).
    pub fn toggle_pin(&mut self, idx: usize) -> Option<bool> {
        let p = self.pane_mut(idx)?;
        p.pinned = !p.pinned;
        Some(p.pinned)
    }

    /// Удалить панель (закрытие крестиком в UI). Возвращает её (core, market) — для решения
    /// об отписке от стакана. None — индекс вне диапазона.
    pub fn remove_pane(&mut self, idx: usize) -> Option<(CoreId, String)> {
        if idx != 0 {
            return None;
        }
        let p = self.pane.take()?;
        Some((p.core, p.market))
    }

    /// Использует ли ещё какая-то панель этот (core, market) — чтобы не отписаться от стакана,
    /// который нужен другой панели этого же чарта.
    pub fn uses_market(&self, core: CoreId, market: &str) -> bool {
        self.panes()
            .iter()
            .any(|p| p.core == core && p.market == market)
    }

    /// Закрыть ВСЕ панели (кнопка «закрыть все графики» в выносном окне). Возвращает их
    /// (core, market) — для отписки от стаканов.
    pub fn clear_panes(&mut self) -> Vec<(CoreId, String)> {
        self.pane
            .take()
            .map(|p| vec![(p.core, p.market)])
            .unwrap_or_default()
    }

    /// Раскладка видимой панели: (индекс панели, прямоугольник) в координатах `content` (физ. px).
    pub fn layout(&self, content: Rect) -> Vec<(usize, Rect)> {
        if self.pane.is_none() {
            return Vec::new();
        }
        vec![(0, content)]
    }
}
