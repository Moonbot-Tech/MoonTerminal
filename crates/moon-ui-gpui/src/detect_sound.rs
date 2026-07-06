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
        crate::diag::bump(&crate::diag::DETECT_SCAN);
        for (core, data) in self.session.store().cores() {
            let cur_max = data.detects.iter().map(|d| d.seq).max().unwrap_or(0);
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
            let sound = data.detects.iter().rev().take_while(|d| d.seq > last).find_map(|d| {
                if let Some(n) = &d.sound_name {
                    Some(n.clone())
                } else if d.is_alert {
                    Some(default_sound.clone())
                } else {
                    None
                }
            });
            self.last_detect_seq.insert(core, cur_max);
            if let Some(name) = sound {
                crate::sound::play(&name);
            }
        }
    }
}
