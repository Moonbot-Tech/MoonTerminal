//! Проигрывание звука при приходе детектов/алертов ядра. «Алерт — это такой же
//! детект»: срабатывание алерта приходит `Event::Detect` (с DETECT_KIND_ALERT), и
//! звук берётся из стратегии-источника так же, как у обычного детекта
//! (`DetectRow.sound_name`). Метод `Backend`, т.к. нужен доступ к стору ядер.

use crate::Backend;

impl Backend {
    /// Проиграть звук для НОВЫХ детектов (по каждому ядру — самый свежий со звуком).
    /// Курсор `last_detect_seq` защищает от повторов и от «залпа» на старте (первый
    /// визит ядра только сидирует курсор, без проигрывания).
    pub(crate) fn play_detect_sounds(&mut self) {
        for (core, data) in self.session.store().cores() {
            // Гейт по ревизии: дренаж будит нас сотнями раз/с, детекты меняются редко.
            // Без изменений — не трогаем список вообще.
            if self.last_detect_rev.get(&core) == Some(&data.detects_rev) {
                continue;
            }
            self.last_detect_rev.insert(core, data.detects_rev);
            crate::diag::bump(&crate::diag::DETECT_SCAN);
            // seq — клиентский монотонный счётчик, детекты пушатся по возрастанию →
            // максимум всегда в хвосте, полный проход не нужен.
            let cur_max = data.detects.back().map(|d| d.seq).unwrap_or(0);
            let last = match self.last_detect_seq.get(&core) {
                Some(v) => *v,
                None => {
                    // Первый визит — сидируем курсор без звука (не проигрываем backlog).
                    self.last_detect_seq.insert(core, cur_max);
                    continue;
                }
            };
            if cur_max <= last {
                continue;
            }
            // Самый свежий из новых детектов, который должен звучать: звук стратегии,
            // либо дефолт для срабатывания алерта БЕЗ стратегии (detects: старые→новые).
            let default_sound = &self.default_alert_sound;
            // Звучат ТОЛЬКО срабатывания алертов (is_alert): звук — из стратегии алерта,
            // иначе дефолтный. Обычные детекты НЕ пикают — их SoundAlert гейтит только
            // кнопку в панели детектов (см. AlertParams), не звук.
            let sound = data.detects.iter().rev().take_while(|d| d.seq > last).find_map(|d| {
                if !d.is_alert {
                    return None;
                }
                Some(
                    d.sound_name
                        .clone()
                        .unwrap_or_else(|| default_sound.clone()),
                )
            });
            self.last_detect_seq.insert(core, cur_max);
            if let Some(name) = sound {
                moon_core::detect_diag::line(&format!("[sound] core={core} play={name}"));
                crate::sound::play(&name);
            } else {
                moon_core::detect_diag::line(&format!(
                    "[sound] core={core} silent: {} new detects, среди них нет алертов",
                    cur_max.saturating_sub(last)
                ));
            }
        }
    }
}
