//! Персист чарт-вкладок (порт идеи `detached.rs`, но для чарт-вкладок — у них своя
//! сериализация). Хранит ПО ВКЛАДКЕ (ключ = группа/номер/ядро): масштаб цены и, если вкладка
//! откреплена в своё ОС-окно, геометрию этого окна. Файл `charts.json` рядом с exe.
//!
//! На старте откреп-вкладки восстанавливаются ПУСТЫМИ (только брендовое лого) в том же месте и
//! ждут детект — `ChartTabs::ingest` наполнит их по (номер, ядро), как обычные AddToChart.
//! Положение/зум самого чарта НЕ персистим: при загрузке вкладка пуста (нечего восстанавливать),
//! а появившиеся монеты идут в live-follow.

use moon_core::config::{ChartBucket, ServerConfig, paths};
use moon_core::session::CoreId;
use serde::{Deserialize, Serialize};

/// Геометрия окна откреплённой вкладки.
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct WinGeom {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// Режим раскладки чарт-стека вкладки (per-tab). Два положения:
/// - `Fit`: высота 0 → растяжение (графики делят окно); высота ≥20 → COMPRESS (фикс. высота,
///   без скролла, сжатие при переполнении);
/// - `Scroll`: фикс. высота слота + вертикальный скролл.
/// Высоты у Fit и Scroll РАЗДЕЛЬНЫЕ (`layout_height_fit` / `layout_height_scroll`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum StackLayoutMode {
    Fit,
    Scroll,
}

/// Ориентация стека чартов (per-tab). `Vertical` — графики стопкой сверху-вниз (дефолт),
/// `Horizontal` — колонками слева-направо. В горизонтальном режиме поле «высота слота» попапа
/// раскладки трактуется как ШИРИНА слота (та же логика FIT/COMPRESS/SCROLL, просто мерим по X).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum StackOrientation {
    Vertical,
    Horizontal,
}

impl StackOrientation {
    /// Горизонтальная раскладка? (None в спеке = Vertical).
    pub fn is_horizontal(self) -> bool {
        matches!(self, StackOrientation::Horizontal)
    }
}

/// Состояние одной чарт-вкладки. `num == 0` — Main; `num >= 1` — AddToChart-N.
#[derive(Clone, Serialize, Deserialize)]
pub struct ChartTabSpec {
    pub group: String,
    pub num: u32,
    /// LEGACY-ключ старых charts.json (до именованных связок): Some(ядро)=split, None=общая.
    /// Читается для обратной совместимости; новые записи кладут `bucket`, а `core`=None.
    #[serde(default)]
    pub core: Option<CoreId>,
    /// Канонический ключ вкладки (своё ядро / общая / именованная связка). Отсутствует в
    /// старых файлах → выводим из `core` (см. `bucket()`).
    #[serde(default)]
    pub bucket: Option<ChartBucket>,
    #[serde(default)]
    pub scale: Option<f32>,
    /// Some → вкладка откреплена в своё окно с этой геометрией; None → во вкладочном стрипе.
    #[serde(default)]
    pub detached: Option<WinGeom>,
    /// Режим раскладки стека этой вкладки (Fit/Scroll). None → дефолт (Fit).
    #[serde(default)]
    pub layout_mode: Option<StackLayoutMode>,
    /// Высота слота (px) для режима Fit: 0 = растяжение (обычный Fit), ≥20 = COMPRESS
    /// (фикс. высота без скролла). None → дефолт (0).
    #[serde(default)]
    pub layout_height_fit: Option<u16>,
    /// Высота слота (px) для режима Scroll. None → дефолт.
    #[serde(default)]
    pub layout_height_scroll: Option<u16>,
    /// Показывать ли стакан на графиках этой вкладки. None → дефолт (вкл). Выкл = стакан не
    /// рисуется, подпись не выводится и (Stage 2) рынок не подписывается на стакан, если его не
    /// хочет ни одно другое окно.
    #[serde(default)]
    pub orderbook_enabled: Option<bool>,
    /// Показывать ли заливку зоны управления при раздельных зонах и скрытом стакане. None →
    /// дефолт (вкл). Per-окно/вкладка, как `orderbook_enabled`.
    #[serde(default)]
    pub show_zone: Option<bool>,
    /// Авто-пин графика при выставлении ордера. None → дефолт (выкл). Per-окно/вкладка.
    #[serde(default)]
    pub auto_pin: Option<bool>,
    /// Ориентация стека (Vertical/Horizontal). None → дефолт (Vertical). Per-окно/вкладка.
    #[serde(default)]
    pub layout_orientation: Option<StackOrientation>,
    /// Кастомная (мульти-монетная) вкладка из поиска: явный список тикеров `(core, market)`.
    /// `Some` помечает спек как кастомный — на старте вкладка восстанавливается и заполняется
    /// ИМЕННО этими чартами (а не ждёт детект, как обычные AddToChart). None → обычная вкладка.
    #[serde(default)]
    pub custom_coins: Option<Vec<(CoreId, String)>>,
    /// Имя кастомной вкладки (редактируется в попапе ⚙). None → дефолтная метка «Набор N».
    #[serde(default)]
    pub custom_label: Option<String>,
}

impl ChartTabSpec {
    /// Ключ вкладки: новый `bucket`, иначе выводим из legacy `core`
    /// (Some(ядро)→Core, None→Shared).
    pub fn bucket(&self) -> ChartBucket {
        self.bucket.clone().unwrap_or_else(|| match self.core {
            Some(id) => ChartBucket::Core(id),
            None => ChartBucket::Shared,
        })
    }
}

/// Одноразовый ремап legacy ПОЗИЦИОННЫХ CoreId (`Core(n)` / `core = n`) в стабильные
/// uid. Запускается ровно один раз при апгрейде конфига с версии < `COREID_UID_VERSION`
/// (флаг `AppConfig::chart_core_remap_needed`), пока порядок серверов в `servers.enc` ещё
/// тот же, что был при записи `charts.json`. До v11 `n` означало позицию (1-based) ядра в
/// списке серверов → берём `servers[n-1].uid`. Вне диапазона (вкладка осиротевшего ядра)
/// оставляем как есть — живого ядра с таким id всё равно нет, вкладка останется пустой.
///
/// Идемпотентность держится снаружи (версия схемы): повторно НЕ вызывать на уже
/// перемапленном файле, иначе uid воспримется как позиция и привязка поедет.
pub fn remap_core_ids(specs: &mut [ChartTabSpec], servers: &[ServerConfig]) {
    let pos_to_uid = |n: CoreId| -> Option<CoreId> {
        // n — старый позиционный id (1-based) → сервер на этой позиции.
        n.checked_sub(1)
            .and_then(|i| servers.get(i as usize))
            .map(|s| s.uid)
    };
    let mut remapped = 0usize;
    for spec in specs.iter_mut() {
        if let Some(ChartBucket::Core(n)) = spec.bucket {
            if let Some(uid) = pos_to_uid(n) {
                spec.bucket = Some(ChartBucket::Core(uid));
                remapped += 1;
            }
        }
        if let Some(n) = spec.core {
            if let Some(uid) = pos_to_uid(n) {
                spec.core = Some(uid);
                remapped += 1;
            }
        }
    }
    log::info!(
        "charts.json: ремап позиционных CoreId → uid ({remapped} вкладок, {} серверов)",
        servers.len()
    );
}

/// Загрузить из `charts.json` (нет/битый → пусто).
pub fn load_all() -> Vec<ChartTabSpec> {
    match std::fs::read_to_string(paths::charts_path()) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            log::warn!("charts.json битый ({e}) → без сохранённых вкладок");
            Vec::new()
        }),
        Err(_) => Vec::new(),
    }
}

/// Записать в `charts.json` (не фатально).
pub fn save_all(list: &[ChartTabSpec]) {
    moon_core::detect_diag::line(&format!(
        "[save] charts.json: {} спек, detached(окна)={}",
        list.len(),
        list.iter().filter(|s| s.detached.is_some()).count()
    ));
    match serde_json::to_string_pretty(list) {
        Ok(s) => {
            if let Err(e) = std::fs::write(paths::charts_path(), s) {
                log::warn!("не записал charts.json: {e}");
            }
        }
        Err(e) => log::warn!("не сериализовал charts.json: {e}"),
    }
}
