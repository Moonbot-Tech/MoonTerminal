//! Пользовательские фигуры чарта (слой рисования, как «карандаш» Moonbot):
//! горизонталь, отрезок, параллельный канал. Фигуры ЛОКАЛЬНЫ (living в терминале,
//! персист в `figures.json`); в ядро уезжают только фигуры с галкой «Alert»
//! (этап 2-3 алертов, upsert blob `TChartObject`). Ключ набора — (CoreId, market):
//! как в Moonbot, рисунок принадлежит чарту конкретного бота.
//!
//! Модель здесь (moon-core), чтобы билдер геометрии в moon-chart и UI-слой видели
//! одни типы, не завися друг от друга.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::config::{paths, write_file_atomic};
use crate::session::CoreId;

/// Узел фигуры: точка (время, цена). Время — unix ms.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FigNode {
    pub time_ms: f64,
    pub price: f64,
}

/// Вид фигуры. Соответствие типам blob `TChartObject` Moonbot: HLine=1, Segment=2,
/// Triangle=4, Channel=5 (Fibo=3 пока не поддержан). Треугольник = 3 вершины;
/// канал Moonbot = ДВЕ ГОРИЗОНТАЛЬНЫЕ цены (ценовой коридор), без времени.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FigureKind {
    /// Горизонтальная линия на цене (бесконечная по времени).
    HLine { price: f64 },
    /// Отрезок между двумя узлами.
    Segment { a: FigNode, b: FigNode },
    /// Треугольник по трём вершинам (как △ в Moonbot, тип 4).
    Triangle { a: FigNode, b: FigNode, c: FigNode },
    /// Горизонтальный канал: две цены (как канал Moonbot, тип 5).
    Channel { price1: f64, price2: f64 },
}

impl FigureKind {
    /// Человекочитаемое имя вида (для списка алертов/тултипов).
    pub fn label(&self) -> &'static str {
        match self {
            FigureKind::HLine { .. } => "Горизонталь",
            FigureKind::Segment { .. } => "Линия",
            FigureKind::Triangle { .. } => "Треугольник",
            FigureKind::Channel { .. } => "Канал",
        }
    }

    /// Опорная цена фигуры (для колонки Price списка алертов и сортировок).
    pub fn anchor_price(&self) -> f64 {
        match self {
            FigureKind::HLine { price } => *price,
            FigureKind::Channel { price1, .. } => *price1,
            FigureKind::Segment { a, .. } | FigureKind::Triangle { a, .. } => a.price,
        }
    }
}

/// Одна фигура чарта.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Figure {
    /// Локальный id (монотонный в пределах стора). Для алертов этот же id станет
    /// `obj_uid` при upsert в ядро.
    pub id: u64,
    pub kind: FigureKind,
    /// RGBA-цвет линии.
    pub color: [u8; 4],
    /// Толщина, px (до масштабирования ppp).
    pub thickness: f32,
    /// Вид линии (Solid/Dash/Dot/DashDot/DashDotDot), в blob @13.
    #[serde(default)]
    pub line_kind: LineKind,
    /// Unix ms создания (колонка Time списка алертов).
    pub created_ms: i64,
    /// Галка «Alert»: фигура отправлена ядру как chart-алерт.
    pub alert: bool,
    /// Привязанная стратегия (id вида «Alerts»); 0 = без стратегии. Уходит в blob (@32).
    #[serde(default)]
    pub strategy_id: u64,
    /// Фигура ПРИШЛА ИЗ ЯДРА (алерт, нарисованный в Moonbot): декодирована из
    /// серверного blob'а, а не нарисована у нас. Такие НЕ персистятся (управляет
    /// сервер) и удаляются, когда сервер их снимает; но их можно выделять/двигать —
    /// правка ре-апсертится в ядро. Не сериализуется (загруженные всегда локальные).
    #[serde(default, skip)]
    pub from_server: bool,
}

/// Инструмент режима рисования (какую фигуру ставит карандаш). Глобален для
/// приложения (выбор в попапе карандаша), живёт в Backend UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FigureTool {
    HLine,
    Segment,
    Triangle,
    Channel,
}

impl FigureTool {
    /// Все инструменты по порядку (для циклического переключения хоткеем «смена фигуры»).
    pub const ALL: [FigureTool; 4] = [
        FigureTool::HLine,
        FigureTool::Segment,
        FigureTool::Triangle,
        FigureTool::Channel,
    ];

    /// Следующий инструмент по кругу — хоткей `switch_figure` листает HLine→Segment→…→HLine.
    pub fn next(self) -> FigureTool {
        let i = Self::ALL.iter().position(|&t| t == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// Стиль линии (соответствует «Kind» Moonbot и Delphi `TPenStyle` в blob @13):
/// Solid=0, Dash=1, Dot=2, DashDot=3, DashDotDot=4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LineKind {
    Solid,
    #[default]
    Dash,
    Dot,
    DashDot,
    DashDotDot,
}

impl LineKind {
    pub const ALL: [LineKind; 5] = [
        LineKind::Solid,
        LineKind::Dash,
        LineKind::Dot,
        LineKind::DashDot,
        LineKind::DashDotDot,
    ];

    /// TPenStyle (значение @13 в blob).
    pub fn to_pen(self) -> u32 {
        match self {
            LineKind::Solid => 0,
            LineKind::Dash => 1,
            LineKind::Dot => 2,
            LineKind::DashDot => 3,
            LineKind::DashDotDot => 4,
        }
    }

    pub fn from_pen(v: u32) -> Self {
        match v {
            0 => LineKind::Solid,
            2 => LineKind::Dot,
            3 => LineKind::DashDot,
            4 => LineKind::DashDotDot,
            _ => LineKind::Dash,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LineKind::Solid => "Solid",
            LineKind::Dash => "Dash",
            LineKind::Dot => "Dot",
            LineKind::DashDot => "DashDot",
            LineKind::DashDotDot => "DashDotDot",
        }
    }

    /// Сплошная ли (для рендера горизонталей: LineInstance.style 0/1).
    pub fn is_solid(self) -> bool {
        self == LineKind::Solid
    }
}

/// Текущий стиль рисования (цвет/толщина/вид линии) — применяется к НОВЫМ фигурам,
/// правится в попапе карандаша. Живёт в Backend UI.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawStyle {
    /// RGBA; `a` — «Opacity» из попапа.
    pub color: [u8; 4],
    pub thickness: f32,
    pub kind: LineKind,
}

impl Default for DrawStyle {
    fn default() -> Self {
        // Голубой (отличать от ордер-линий), 1 px, пунктир (Dash).
        Self {
            color: [64, 196, 255, 255],
            thickness: 1.0,
            kind: LineKind::Dash,
        }
    }
}

/// Ключ набора фигур: чарт конкретного ядра и монеты.
pub type FigureKey = (CoreId, String);

/// Стор фигур всех чартов + персист. Живёт в Backend UI; правки идут через
/// методы стора, каждая бампает `rev` (гейт перерисовки) и ставит `dirty`
/// (дебаунс-сейв коорд-тиком, как config/docks).
#[derive(Debug, Default)]
pub struct FigureStore {
    /// Все фигуры чартов: локальные (`from_server=false`, персистятся) + серверные
    /// алерты из Moonbot (`from_server=true`, управляет сервер). И те, и другие
    /// одинаково выделяются/двигаются/удаляются.
    by_key: HashMap<FigureKey, Vec<Figure>>,
    next_id: u64,
    /// Растёт при любой правке — data_state чарта перечитывает набор по ней.
    rev: u64,
    /// Есть несохранённые правки (для дебаунс-сейва).
    pub dirty: bool,
}

impl FigureStore {
    pub fn rev(&self) -> u64 {
        self.rev
    }

    pub fn figures(&self, core: CoreId, market: &str) -> &[Figure] {
        self.by_key
            .get(&(core, market.to_string()))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Реконсиляция серверных алерт-фигур (созданных в ядре/Moonbot) в общий набор.
    /// `server` — свежий полный набор `from_server`-фигур из декодированных blob'ов.
    /// Локальные (`from_server=false`) не трогаем; наши уже заармленные (id совпал с
    /// локальным) — не дублируем. Зовётся только при изменении серверного набора
    /// (гейт по activity), поэтому rev бампаем безусловно.
    pub fn set_server_figures(&mut self, server: HashMap<FigureKey, Vec<Figure>>) {
        for figs in self.by_key.values_mut() {
            figs.retain(|f| !f.from_server);
        }
        for (key, figs) in server {
            let entry = self.by_key.entry(key).or_default();
            for f in figs {
                if entry.iter().any(|e| e.id == f.id && !e.from_server) {
                    continue;
                }
                entry.push(f);
            }
        }
        self.by_key.retain(|_, v| !v.is_empty());
        self.rev = self.rev.wrapping_add(1);
    }

    /// Все фигуры-алерты для окна «Алерты»: `(core, market, &Figure)`.
    pub fn all_alerts(&self) -> impl Iterator<Item = (CoreId, &str, &Figure)> + '_ {
        self.by_key.iter().flat_map(|((c, m), v)| {
            v.iter()
                .filter(|f| f.alert)
                .map(move |f| (*c, m.as_str(), f))
        })
    }

    pub fn get(&self, core: CoreId, market: &str, id: u64) -> Option<&Figure> {
        self.figures(core, market).iter().find(|f| f.id == id)
    }

    /// Добавляет ЛОКАЛЬНУЮ фигуру, возвращает её id.
    pub fn add(&mut self, core: CoreId, market: &str, mut fig: Figure) -> u64 {
        self.next_id += 1;
        fig.id = self.next_id;
        fig.from_server = false;
        let id = fig.id;
        self.by_key
            .entry((core, market.to_string()))
            .or_default()
            .push(fig);
        self.bump();
        id
    }

    /// Правка фигуры на месте (драг узла/тела). `edit` возвращает true, если что-то поменяла.
    pub fn edit(
        &mut self,
        core: CoreId,
        market: &str,
        id: u64,
        edit: impl FnOnce(&mut Figure) -> bool,
    ) -> bool {
        let Some(fig) = self
            .by_key
            .get_mut(&(core, market.to_string()))
            .and_then(|v| v.iter_mut().find(|f| f.id == id))
        else {
            return false;
        };
        if edit(fig) {
            self.bump();
            true
        } else {
            false
        }
    }

    pub fn remove(&mut self, core: CoreId, market: &str, id: u64) -> Option<Figure> {
        let list = self.by_key.get_mut(&(core, market.to_string()))?;
        let idx = list.iter().position(|f| f.id == id)?;
        let fig = list.remove(idx);
        if list.is_empty() {
            self.by_key.remove(&(core, market.to_string()));
        }
        self.bump();
        Some(fig)
    }

    /// Удалить все фигуры чарта (Clear All инструмента).
    pub fn clear(&mut self, core: CoreId, market: &str) -> usize {
        let n = self
            .by_key
            .remove(&(core, market.to_string()))
            .map(|v| v.len())
            .unwrap_or(0);
        if n > 0 {
            self.bump();
        }
        n
    }

    fn bump(&mut self) {
        self.rev = self.rev.wrapping_add(1);
        self.dirty = true;
    }

    // ── Персист ──────────────────────────────────────────────────────────────

    /// Загрузка из `figures.json` (нет/битый → пусто).
    pub fn load() -> Self {
        let path = paths::figures_path();
        let by_key: Vec<PersistEntry> = match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                log::warn!("figures.json битый ({e}) → без фигур");
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };
        let mut store = Self::default();
        for e in by_key {
            let max_id = e.figures.iter().map(|f| f.id).max().unwrap_or(0);
            store.next_id = store.next_id.max(max_id);
            store.by_key.insert((e.core, e.market), e.figures);
        }
        store
    }

    /// Сохранение в `figures.json` (не фатально). Сбрасывает `dirty`. Серверные
    /// (`from_server`) алерты НЕ персистятся — ими управляет ядро.
    pub fn save(&mut self) {
        let list: Vec<PersistEntry> = self
            .by_key
            .iter()
            .filter_map(|((core, market), figures)| {
                let figures: Vec<Figure> =
                    figures.iter().filter(|f| !f.from_server).cloned().collect();
                (!figures.is_empty()).then(|| PersistEntry {
                    core: *core,
                    market: market.clone(),
                    figures,
                })
            })
            .collect();
        match serde_json::to_string_pretty(&list) {
            Ok(s) => {
                if let Err(e) =
                    write_file_atomic(&paths::figures_path(), s.as_bytes(), "figures.json")
                {
                    log::warn!("не записал figures.json: {e}");
                }
            }
            Err(e) => log::warn!("не сериализовал figures.json: {e}"),
        }
        self.dirty = false;
    }
}

/// Элемент сериализации: HashMap с tuple-ключом в JSON не живёт — плоский список.
#[derive(Serialize, Deserialize)]
struct PersistEntry {
    core: CoreId,
    market: String,
    figures: Vec<Figure>,
}
