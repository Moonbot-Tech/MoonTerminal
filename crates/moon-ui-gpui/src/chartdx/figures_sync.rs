//! Фигуры чарта (слой рисования) в own-pass: доступ `ChartDataState` к общему
//! стору фигур Backend'а + интерактив панели (превью рисования/hover/выделение).
//! Геометрия добавляется в userdata-слои после `build_order_geometry`
//! (см. `data_state::sync_orders_from_session`).

use std::cell::RefCell;
use std::rc::Rc;

use moon_core::figures::{Figure, FigureStore};
use moon_core::session::CoreId;

use super::ChartDataState;

/// Интерактивное состояние фигур одной панели чарта. Данные фигур живут в общем
/// сторе; здесь — только то, что видит конкретная панель (превью рисуемой фигуры,
/// hover, выделение). Любая смена бампает `rev` → пересборка userdata.
#[derive(Default, Clone, PartialEq)]
pub(crate) struct FigureVisual {
    /// Режим рисования (карандаш). Фигуры видны/интерактивны ТОЛЬКО когда он включён.
    pub draw_mode: bool,
    /// Чарт (ядро+монета), к которому относится интерактив. Панель-движок может
    /// держать СТЕК панелей разных монет — превью/подсветка применяются только
    /// к панели с этим ключом.
    pub key: Option<(CoreId, String)>,
    /// Фигура в процессе рисования (превью за курсором, пунктиром).
    pub draft: Option<Figure>,
    /// Наведённая фигура (подсветка).
    pub hovered: Option<u64>,
    /// Выделенная фигура (подсветка + узелки).
    pub selected: Option<u64>,
}

impl ChartDataState {
    /// Подключить общий стор фигур (один Rc на Backend; зовётся при создании панели).
    pub(super) fn set_figures_store(&mut self, store: Rc<RefCell<FigureStore>>) {
        self.figures = Some(store);
        self.figure_visual_rev = self.figure_visual_rev.wrapping_add(1);
    }

    /// Интерактив фигур (превью/hover/выделение). Смена инвалидирует userdata панелей.
    pub(super) fn set_figure_visual(&mut self, visual: FigureVisual) -> bool {
        if self.figure_visual == visual {
            return false;
        }
        self.figure_visual = visual;
        self.figure_visual_rev = self.figure_visual_rev.wrapping_add(1);
        let mut st = self.render.borrow_mut();
        for pr in &mut st.panes {
            pr.last_figures_sig = u64::MAX;
            pr.gpu_prepare_dirty = true;
        }
        st.needs_present = true;
        true
    }

    /// Сигнатура фигур для гейта пересборки userdata: rev стора + rev интерактива.
    /// u64::MAX не возвращаем (это «принудительно грязно» у панели).
    pub(super) fn figures_sig(&self) -> u64 {
        let store_rev = self
            .figures
            .as_ref()
            .map(|f| f.borrow().rev())
            .unwrap_or(0);
        let sig = store_rev
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(self.figure_visual_rev);
        if sig == u64::MAX { 0 } else { sig }
    }

    /// Добавить геометрию фигур панели (ядро+монета) в userdata-буферы. Фигуры видны
    /// ТОЛЬКО в режиме рисования (карандаш) — иначе слой не строим вовсе.
    pub(super) fn append_figure_geometry(
        &self,
        core: CoreId,
        market: &str,
        epoch_ms: f64,
        hlines: &mut Vec<moon_chart::layers::LineInstance>,
        segs: &mut Vec<moon_chart::layers::SegInstance>,
        markers: &mut Vec<moon_chart::layers::MarkerInstance>,
    ) {
        if !self.figure_visual.draw_mode {
            return;
        }
        let Some(store) = self.figures.as_ref() else {
            return;
        };
        let store = store.borrow();
        // Локальные + серверные (from_server) — единый набор, все интерактивны.
        let figures = store.figures(core, market);
        // Интерактив (превью/hover/выделение) — только для панели своего ключа:
        // в стеке панелей разных монет draft рисуется лишь там, где курсор.
        let v = &self.figure_visual;
        let mine = v
            .key
            .as_ref()
            .is_some_and(|(c, m)| *c == core && m == market);
        let draft = if mine { v.draft.as_ref() } else { None };
        if figures.is_empty() && draft.is_none() {
            return;
        }
        moon_chart::build_figure_geometry(
            figures,
            draft,
            if mine { v.hovered } else { None },
            if mine { v.selected } else { None },
            epoch_ms,
            hlines,
            segs,
            markers,
        );
    }
}
