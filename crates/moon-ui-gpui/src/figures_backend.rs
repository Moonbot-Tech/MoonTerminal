//! Методы `Backend` для слоя фигур/алертов: тоггл галки «Alert» у выделенной
//! фигуры, удаление фигуры с разоружением серверного алерта, re-upsert после драга.
//! Вынесено из `main.rs` (дочерний модуль видит приватные поля `Backend`).

use std::collections::HashMap;

use moon_core::alert_blob;
use moon_core::figures::{Figure, FigureKey};
use moon_core::session::CoreId;

use crate::Backend;

/// Собрать blob для upsert из фигуры (obj_uid = id фигуры).
fn figure_blob(fig: &Figure) -> Vec<u8> {
    alert_blob::encode(
        &fig.kind,
        fig.color,
        fig.thickness,
        fig.line_kind,
        fig.created_ms as f64,
        fig.strategy_id,
        fig.id,
    )
}

impl Backend {
    /// Def Strategy ядра (Backend читает из ServerConfig — персистится в servers.enc).
    pub(crate) fn alert_def_strategy(&self, core: CoreId) -> u64 {
        self.config
            .servers
            .iter()
            .find(|s| s.id == core)
            .map(|s| s.default_alert_strategy)
            .unwrap_or(0)
    }

    /// Задать Def Strategy ядра (пишет в конфиг + дебаунс-сейв).
    pub(crate) fn set_alert_def_strategy(&mut self, core: CoreId, strategy_id: u64) {
        if let Some(s) = self.config.servers.iter_mut().find(|s| s.id == core) {
            if s.default_alert_strategy != strategy_id {
                s.default_alert_strategy = strategy_id;
                self.config_dirty = true;
            }
        }
    }

    /// Тоггл галки «Alert» у выделенной фигуры: армит (upsert) или разоружает
    /// (delete) chart-алерт на ядре. Возвращает `true`, если что-то изменилось.
    pub(crate) fn toggle_selected_figure_alert(&mut self) -> bool {
        let Some((core, market, id)) = self.fig_selected.clone() else {
            return false;
        };
        let def_strategy = self.alert_def_strategy(core);
        let mut upsert_blob = None;
        let mut disarm = false;
        let changed = self.figures.borrow_mut().edit(core, &market, id, |fig| {
            fig.alert = !fig.alert;
            if fig.alert {
                // Новый алерт без стратегии → применяем «Def Strategy».
                if fig.strategy_id == 0 && def_strategy != 0 {
                    fig.strategy_id = def_strategy;
                }
                upsert_blob = Some(figure_blob(fig));
            } else {
                disarm = true;
            }
            true
        });
        if !changed {
            return false;
        }
        if let Some(blob) = upsert_blob {
            let _ = self.session.chart_alert_upsert(core, market, id, blob);
        } else if disarm {
            let _ = self.session.chart_alert_delete(core, market, id);
        }
        true
    }

    /// Удалить фигуру; если была заармлена — разоружить серверный алерт.
    pub(crate) fn remove_figure(&mut self, core: CoreId, market: &str, id: u64) {
        let removed = self.figures.borrow_mut().remove(core, market, id);
        if let Some(fig) = removed {
            if fig.alert {
                let _ = self
                    .session
                    .chart_alert_delete(core, market.to_string(), id);
            }
        }
        if self
            .fig_selected
            .as_ref()
            .is_some_and(|(c, m, i)| *c == core && m == market && *i == id)
        {
            self.fig_selected = None;
        }
    }

    /// Реконсиляция серверных (созданных в ЯДРЕ/Moonbot) chart-алертов в render-стор:
    /// декодируем blob'ы всех ядер в фигуры и кладём в `remote`-набор FigureStore.
    /// Дедуп: алерты, чей `obj_uid` == id НАШЕЙ локальной фигуры (мы их сами заармили),
    /// пропускаем — они уже рисуются как локальные. Зовётся при изменении серверного
    /// набора (`chart_alerts_activity`). Возвращает `activity` для гейта вызывающим.
    pub(crate) fn sync_remote_alerts(&mut self) {
        let mut server: HashMap<FigureKey, Vec<Figure>> = HashMap::new();
        for (core, data) in self.session.store().cores() {
            for ((market, obj_uid), blob) in &data.chart_alerts {
                let Some(d) = alert_blob::decode(blob) else {
                    continue;
                };
                server.entry((core, market.clone())).or_default().push(Figure {
                    id: *obj_uid,
                    kind: d.kind,
                    color: d.color,
                    thickness: d.thickness,
                    line_kind: d.line_kind,
                    created_ms: d.created_ms as i64,
                    alert: true,
                    strategy_id: d.strategy_id,
                    from_server: true,
                });
            }
        }
        self.figures.borrow_mut().set_server_figures(server);
    }

    /// Назначить фигуре-алерту стратегию (id вида «Alerts», 0 = без). Пишет в blob (@32)
    /// и ре-апсертит, если алерт заармлен.
    pub(crate) fn set_figure_strategy(
        &mut self,
        core: CoreId,
        market: &str,
        id: u64,
        strategy_id: u64,
    ) {
        let changed = self.figures.borrow_mut().edit(core, market, id, |f| {
            if f.strategy_id == strategy_id {
                false
            } else {
                f.strategy_id = strategy_id;
                true
            }
        });
        if changed {
            self.reupsert_figure_alert(core, market, id);
        }
    }

    /// Пере-заармить фигуру после правки (драг узла/тела): если алерт вкл — upsert со
    /// свежими координатами. Зовётся на mouse-up драга, не на каждое движение.
    pub(crate) fn reupsert_figure_alert(&mut self, core: CoreId, market: &str, id: u64) {
        let blob = {
            let store = self.figures.borrow();
            store
                .get(core, market, id)
                .filter(|f| f.alert)
                .map(figure_blob)
        };
        if let Some(blob) = blob {
            let _ = self
                .session
                .chart_alert_upsert(core, market.to_string(), id, blob);
        }
    }
}
