//! Group (= window) properties: icon and active state. Associated by group name.

use serde::{Deserialize, Serialize};

use super::servers::default_true;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GroupConfig {
    /// Group name (key; matches `ServerConfig.group`).
    pub name: String,
    /// Whether the group is active. An inactive group's cores do not connect, and it has no window.
    #[serde(default = "default_true")]
    pub active: bool,
    /// Icon id in assets/icons (taskbar + group-window header).
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
