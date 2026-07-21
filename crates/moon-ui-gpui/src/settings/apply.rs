//! Saving a Settings draft and applying presentation, logging, market, session, and window
//! changes at their appropriate boundaries.

use std::collections::HashSet;

use gpui::*;
use moon_ui::Root;

use super::SettingsView;
use moon_core::config::{AppConfig, SnapshotOutcome};

impl SettingsView {
    /// Validate and save the draft, then apply it without closing the Settings window.
    ///
    /// A failed save changes neither the active config nor the draft.
    pub(super) fn save(&mut self, cx: &mut Context<Self>) {
        // Compare the saved candidate with this snapshot to select live updates and required
        // rebuilds.
        let before = self.backend.read(cx).config.clone();
        let res = self.backend.update(cx, |b, _| {
            // Commit the candidate only after validation and I/O succeed. Otherwise the config
            // would change without corresponding session and window reconciliation.
            let mut candidate = b.preview.as_ref().unwrap_or(&b.config).clone();
            // Preserve the preceding on-disk files in `backups/` first: this is a deliberate user
            // save for which rollback must be available.
            let res = candidate.save_with_snapshot();
            if res.is_ok() {
                // Propagate uid normalization from the save back into the draft so the next save
                // cannot roll back `next_uid` and reuse an id from reports.sqlite history.
                if let Some(p) = b.preview.as_mut() {
                    *p = candidate.clone();
                }
                b.config = candidate;
            }
            res
        });
        match res {
            Ok(outcome) => {
                // Snapshot failure does NOT cancel the save, but a normal success message would
                // promise a nonexistent rollback copy precisely when the user relies on one.
                let snapshot_failed = outcome == SnapshotOutcome::Failed;
                let msg = if snapshot_failed {
                    super::StatusMsg::Key("settings.saved_no_backup")
                } else {
                    super::StatusMsg::Key("settings.saved")
                };
                self.status = Some((msg, snapshot_failed));
                self.apply_settings(&before, cx);
            }
            Err(e) => self.status = Some((super::StatusMsg::Text(e.to_string()), true)),
        }
        cx.notify();
    }

    /// Apply saved settings at the narrowest required boundary.
    ///
    /// Presentation, logging, and market-mode changes apply live. Structural server or group
    /// changes reconcile sessions, while chart-topology changes rebuild group windows.
    fn apply_settings(&mut self, before: &AppConfig, cx: &mut Context<Self>) {
        let after = self.backend.read(cx).config.clone();

        // Presentation settings are read during rendering, so update locale/order and redraw
        // without recreating windows or sessions.
        let lang_changed = before.language != after.language;
        let sort_changed = before.core_sort != after.core_sort;
        if lang_changed {
            rust_i18n::set_locale(after.language.code());
        }
        if lang_changed || sort_changed {
            // Notify Backend before redrawing so signature-gated panels recompute their order.
            self.backend.update(cx, |b, bcx| {
                // An order change can replace the canonical first core while the cached core is
                // still live, so bypass the usual liveness early return.
                if sort_changed {
                    b.refresh_header_ticker_default(true);
                }
                bcx.notify();
            });
            cx.refresh_windows();
        }

        // Файловый лог — применяем живо: включили запись или сократили срок → чистим.
        if before.log_to_file != after.log_to_file
            || before.log_retention_days != after.log_retention_days
        {
            moon_core::applog::set_file_logging(after.log_to_file, after.log_retention_days);
            moon_core::applog::purge_old();
        }

        let struct_changed = before.structural_sig() != after.structural_sig();
        let mode_changed = before.market_mode != after.market_mode;
        let split_changed = before.charts_split_by_core != after.charts_split_by_core;
        // Смена чарт-связки (`chart_bundle`) у ядра меняет состав чарт-вкладок, но НЕ требует
        // реконнекта — как split, только пересобираем окна групп (без рестарта сессий).
        let bundle_sig = |c: &AppConfig| {
            let mut v: Vec<(u64, String)> = c
                .servers
                .iter()
                .map(|s| (s.uid, s.chart_bundle.clone()))
                .collect();
            v.sort();
            v
        };
        let bundle_changed = bundle_sig(before) != bundle_sig(&after);
        let ui_theme_changed = before.ui_font_delta != after.ui_font_delta
            || before.ui_theme_mode != after.ui_theme_mode
            || before.ui_scale != after.ui_scale;

        if ui_theme_changed {
            crate::install_moon_theme_for_config(&after, cx);
        }

        if struct_changed {
            // Инкрементальный реконсайл сессий по новому конфигу (НЕ полный рестарт):
            // добавляем новые ядра, гасим удалённые, переподнимаем только изменённые —
            // неизменные ядра не дёргаем. epoch/market_mode сохраняем. chart_market_refs
            // НЕ сбрасываем: пережившие окна сохраняют свои подписки, закрытые освободят их
            // через on_release панелей, новые — зарегистрируют при открытии.
            self.backend.update(cx, |b, _| {
                let reports = b.reports.as_ref().map(|h| &h.tx);
                b.session.reconcile(&b.config, reports);
                b.session.set_market_mode(b.config.market_mode);
            });
            self.reconcile_group_windows(cx);
        } else if mode_changed {
            // Режим рынка — живо: ядра остаются на связи, координатор пере-выберет
            // провайдеров на следующем тике.
            self.backend
                .update(cx, |b, _| b.session.set_market_mode(b.config.market_mode));
        }

        // Сменили «отдельная чарт-вкладка на ядро» (без структурного ребилда, который и
        // так всё пересоздаёт) → пересобираем окна, чтобы чарт-вкладки собрались в новом
        // режиме (egui чистил chart-tabs; в GPUI вкладки живут в окне — пересоздаём окно).
        if !struct_changed && (split_changed || bundle_changed) {
            self.rebuild_group_windows(cx);
        }
    }

    /// Закрыть все окна групп и открыть заново по актуальному конфигу (порт egui
    /// `needs_rebuild`). Геометрия восстановится из сохранённой раскладки.
    ///
    /// Также закрываем ВСЕ откреп-окна чарт-вкладок и снимаем у спек `detached`: при
    /// смене групп их состав/ключи (bucket) меняются — старые окна иначе зависают дублями
    /// и сыплют «window not found» по протухшим хэндлам. Вкладки вернутся в стрип нового
    /// окна группы по детектам (а не повторно откроются off-screen окнами).
    fn rebuild_group_windows(&mut self, cx: &mut Context<Self>) {
        let (handles, chart_handles, cfg, epoch, layout) = self.backend.update(cx, |b, _| {
            let handles: Vec<WindowHandle<Root>> = b.group_windows.values().copied().collect();
            b.group_windows.clear();
            let chart_handles: Vec<WindowHandle<Root>> =
                b.detached_chart_windows.drain(..).map(|(_, h)| h).collect();
            // Вернуть откреп-вкладки в стрип: снять detached у всех спек, чтобы свежие
            // окна групп не открыли их повторно (иначе дубли).
            for s in b.chart_specs.iter_mut() {
                s.detached = None;
            }
            b.chart_specs_dirty = true;
            (
                handles,
                chart_handles,
                b.config.clone(),
                b.epoch,
                b.layout.clone(),
            )
        });
        for h in handles {
            let _ = h.update(cx, |_, window, _| window.remove_window());
        }
        for h in chart_handles {
            let _ = h.update(cx, |_, window, _| window.remove_window());
        }
        for (i, g) in crate::group_window::groups(&cfg).into_iter().enumerate() {
            crate::group_window::spawn_group_window(
                cx,
                &self.backend,
                &cfg,
                g,
                epoch,
                &layout,
                i as f32 * 40.0,
            );
        }
    }

    /// Инкрементальный реконсайл окон групп (вместо разрушительного `rebuild_group_windows`):
    /// закрывает окна ТОЛЬКО исчезнувших групп (и их откреп-чарты), открывает окна ТОЛЬКО
    /// новых групп, а окна сохранившихся групп НЕ трогает — их `ChartTabs` сами подхватят
    /// добавленные/убранные ядра через сигнатуру. Так открытые вкладки и раскладка переживают
    /// добавление/удаление серверов (фикс: раньше любое изменение состава сносило все окна).
    fn reconcile_group_windows(&mut self, cx: &mut Context<Self>) {
        let (close_group, close_detached, spawn_groups, cfg, epoch, layout) =
            self.backend.update(cx, |b, _| {
                let want = crate::group_window::groups(&b.config);
                let want_set: HashSet<&str> = want.iter().map(String::as_str).collect();
                // Окна исчезнувших групп → закрыть.
                let close_group: Vec<WindowHandle<Root>> = b
                    .group_windows
                    .iter()
                    .filter(|(g, _)| !want_set.contains(g.as_str()))
                    .map(|(_, h)| *h)
                    .collect();
                let gone: HashSet<String> = b
                    .group_windows
                    .keys()
                    .filter(|g| !want_set.contains(g.as_str()))
                    .cloned()
                    .collect();
                b.group_windows.retain(|g, _| want_set.contains(g.as_str()));
                // Откреп-чарты исчезнувших групп → закрыть (их группы больше нет).
                let close_detached: Vec<WindowHandle<Root>> = b
                    .detached_chart_windows
                    .iter()
                    .filter(|(g, _)| gone.contains(g))
                    .map(|(_, h)| *h)
                    .collect();
                b.detached_chart_windows.retain(|(g, _)| !gone.contains(g));
                // Новые группы (в want, окна ещё нет) → открыть. Сохранившиеся пропускаем.
                let spawn_groups: Vec<String> = want
                    .iter()
                    .filter(|g| !b.group_windows.contains_key(g.as_str()))
                    .cloned()
                    .collect();
                (
                    close_group,
                    close_detached,
                    spawn_groups,
                    b.config.clone(),
                    b.epoch,
                    b.layout.clone(),
                )
            });
        for h in close_group {
            let _ = h.update(cx, |_, window, _| window.remove_window());
        }
        for h in close_detached {
            let _ = h.update(cx, |_, window, _| window.remove_window());
        }
        for (i, g) in spawn_groups.into_iter().enumerate() {
            crate::group_window::spawn_group_window(
                cx,
                &self.backend,
                &cfg,
                g,
                epoch,
                &layout,
                i as f32 * 40.0,
            );
        }
    }
}
