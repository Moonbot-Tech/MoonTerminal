//! Одноразовые миграции со старых форматов конфига в текущий рантайм.
//! Возвращают `AppConfig`; вызывающий (`AppConfig::load`) сразу делает `save()`,
//! который проставит стабильные uid и запишет новые servers.enc + settings.toml.

use serde::Deserialize;

use super::crypto;
use super::paths;
use super::secrets::Secret;
use super::servers::{self, FeedFlags};
use super::{AppConfig, GroupConfig, ServerConfig};

/// Старый объединённый зашифрованный config.enc: { servers:[…], groups:[…] }.
pub fn from_legacy_enc() -> anyhow::Result<AppConfig> {
    // host/port из старого формата игнорируем — endpoint берётся из ключа.
    #[derive(Deserialize, Default)]
    struct OldServer {
        #[serde(default)]
        name: String,
        #[serde(default)]
        key: Secret,
        #[serde(default = "servers::default_group")]
        group: String,
        #[serde(default = "servers::default_market")]
        market: String,
        #[serde(default = "servers::default_color")]
        color: [u8; 3],
    }
    #[derive(Deserialize, Default)]
    struct Old {
        #[serde(default)]
        servers: Vec<OldServer>,
        #[serde(default)]
        groups: Vec<GroupConfig>,
    }

    let plain = crypto::decrypt(&std::fs::read(paths::legacy_enc_path())?)?;
    let old: Old = toml::from_str(std::str::from_utf8(&plain)?)?;
    let servers = old
        .servers
        .into_iter()
        .enumerate()
        .map(|(i, s)| ServerConfig {
            id: (i as u64) + 1,
            uid: (i as u64) + 1,
            name: s.name,
            active: true,
            show_window: true,
            feed: FeedFlags::default(),
            key: s.key,
            group: s.group,
            market: s.market,
            color: s.color,
            synthetic: false,
            chart_bundle: String::new(),
            order_sizes: None,
        })
        .collect();
    Ok(AppConfig {
        servers,
        groups: old.groups,
        language: super::Language::default(),
        ..Default::default()
    })
}

/// Совсем старый открытый config.toml (один сервер).
pub fn from_legacy_toml() -> anyhow::Result<AppConfig> {
    // host/port игнорируем — endpoint берётся из ключа.
    #[derive(Deserialize)]
    struct Legacy {
        #[serde(default)]
        key: String,
        #[serde(default)]
        market: String,
    }

    let l: Legacy = toml::from_str(&std::fs::read_to_string(paths::legacy_toml_path())?)?;
    let market = if l.market.is_empty() {
        servers::default_market()
    } else {
        l.market
    };
    Ok(AppConfig {
        servers: vec![ServerConfig {
            id: 1,
            uid: 1,
            name: "default".to_string(),
            active: true,
            show_window: true,
            feed: FeedFlags::default(),
            key: Secret::new(l.key),
            group: servers::default_group(),
            market,
            color: servers::default_color(),
            synthetic: false,
            chart_bundle: String::new(),
            order_sizes: None,
        }],
        groups: Vec::new(),
        language: super::Language::default(),
        ..Default::default()
    })
}
