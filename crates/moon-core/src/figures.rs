//! Пользовательские фигуры чарта (слой рисования, как «карандаш» MoonBot):
//! горизонталь, отрезок, параллельный канал. Фигуры ЛОКАЛЬНЫ (living в терминале,
//! персист в `figures.json`); в ядро уезжают только фигуры с галкой «Alert»
//! (этап 2-3 алертов, upsert blob `TChartObject`). Ключ набора — (CoreId, market):
//! как в MoonBot, рисунок принадлежит чарту конкретного бота.
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

/// Вид фигуры. MVP: горизонталь / отрезок / канал; Fibo и треугольник — следом.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FigureKind {
    /// Горизонтальная линия на цене (бесконечная по времени).
    HLine { price: f64 },
    /// Отрезок между двумя узлами.
    Segment { a: FigNode, b: FigNode },
    /// Параллельный канал: базовый отрезок + смещение цены второй линии.
    Channel { a: FigNode, b: FigNode, dprice: f64 },
}

impl FigureKind {
    /// Человекочитаемое имя вида (для списка алертов/тултипов).
    pub fn label(&self) -> &'static str {
        match self {
            FigureKind::HLine { .. } => "Горизонталь",
            FigureKind::Segment { .. } => "Линия",
            FigureKind::Channel { .. } => "Канал",
        }
    }

    /// Опорная цена фигуры (для колонки Price списка алертов и сортировок).
    pub fn anchor_price(&self) -> f64 {
        match self {
            FigureKind::HLine { price } => *price,
            FigureKind::Segment { a, .. } | FigureKind::Channel { a, .. } => a.price,
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
    /// Пунктир (Kind=Dash MoonBot).
    pub dashed: bool,
    /// Unix ms создания (колонка Time списка алертов).
    pub created_ms: i64,
    /// Галка «Alert»: фигура отправлена ядру как chart-алерт (этап 2+; пока
    /// только персистится).
    pub alert: bool,
}

/// Инструмент режима рисования (какую фигуру ставит карандаш). Глобален для
/// приложения (тоггл хоткеем), живёт в Backend UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FigureTool {
    HLine,
    Segment,
    Channel,
}

/// Ключ набора фигур: чарт конкретного ядра и монеты.
pub type FigureKey = (CoreId, String);

/// Стор фигур всех чартов + персист. Живёт в Backend UI; правки идут через
/// методы стора, каждая бампает `rev` (гейт перерисовки) и ставит `dirty`
/// (дебаунс-сейв коорд-тиком, как config/docks).
#[derive(Debug, Default)]
pub struct FigureStore {
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

    pub fn get(&self, core: CoreId, market: &str, id: u64) -> Option<&Figure> {
        self.figures(core, market).iter().find(|f| f.id == id)
    }

    /// Добавляет фигуру, возвращает её id.
    pub fn add(&mut self, core: CoreId, market: &str, mut fig: Figure) -> u64 {
        self.next_id += 1;
        fig.id = self.next_id;
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

    /// Сохранение в `figures.json` (не фатально). Сбрасывает `dirty`.
    pub fn save(&mut self) {
        let list: Vec<PersistEntry> = self
            .by_key
            .iter()
            .map(|((core, market), figures)| PersistEntry {
                core: *core,
                market: market.clone(),
                figures: figures.clone(),
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
