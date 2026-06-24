//! Свойства группы (= окна): иконка и активность. Привязаны к имени группы.

use serde::{Deserialize, Serialize};

use super::servers::default_true;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GroupConfig {
    /// Имя группы (ключ; совпадает с `ServerConfig.group`).
    pub name: String,
    /// Активна ли группа. Неактивная — все её ядра не подключаются, окна нет.
    #[serde(default = "default_true")]
    pub active: bool,
    /// Id иконки в assets/icons (taskbar + шапка окна группы).
    #[serde(default)]
    pub icon: u32,
}

impl GroupConfig {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            active: true,
            icon: 0,
        }
    }
}
