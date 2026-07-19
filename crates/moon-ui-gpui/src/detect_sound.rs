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
            // Пикают детекты с `SoundAlert=Yes` (флаг = «играть звук» в Moonbot) И
            // срабатывания алертов (is_alert). Играем ИМЕННО заданный стратегией звук
            // (`sound_name`); `None` = SoundKind=NONE → детект молчит (find_map идёт к
            // следующему). Дефолт — только для алерт-файра БЕЗ стратегии. За тик берём один
            // самый свежий звучащий (новые→старые), чтобы залп не пикал десятки раз.
            let sound = data
                .detects
                .iter()
                .rev()
                .take_while(|d| d.seq > last)
                .find_map(|d| {
                    // Гейт: пикают только is_alert-файры и детекты с SoundAlert=Yes.
                    if !d.is_alert && !d.sound_alert {
                        return None;
                    }
                    match &d.sound_name {
                        Some(name) => Some(name.clone()),
                        // Алерт-файр без снимка стратегии — дефолтный звук; обычный детект с
                        // SoundKind=NONE — молчит (не даём дефолт).
                        None if d.is_alert => Some(default_sound.clone()),
                        None => None,
                    }
                });
            self.last_detect_seq.insert(core, cur_max);
            if let Some(name) = sound {
                moon_core::detect_diag::line(&format!("[sound] core={core} play={name}"));
                crate::sound::play(&name);
            } else {
                moon_core::detect_diag::line(&format!(
                    "[sound] core={core} silent: {} new detects, среди них нет is_alert/SoundAlert",
                    cur_max.saturating_sub(last)
                ));
            }
        }
    }
}
