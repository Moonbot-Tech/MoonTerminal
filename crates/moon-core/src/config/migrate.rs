//! One-time migrations from legacy config formats to the current runtime.
//! They return `AppConfig`; the caller (`AppConfig::load`) immediately calls `save()`,
//! which assigns stable uids and writes new servers.enc + settings.toml files.

use serde::Deserialize;

use super::crypto;
use super::paths;
use super::secrets::Secret;
use super::servers::{self, FeedFlags};
use super::{AppConfig, GroupConfig, ServerConfig};

#[cfg(test)]
mod tests;

/// Migrate the legacy combined encrypted `config.enc` format.
///
/// The legacy format has no durable counter, so construction still names `uid_floor`; see
/// [`super::uid_counter::UidCounter`] for the invariant and best-effort boundary.
/// Reads through `decrypt_standalone`: `config.enc` is a file being retired, so its key material
/// must NOT become this process's material for `servers.enc`.
pub fn from_legacy_enc(uid_floor: Option<u64>) -> anyhow::Result<AppConfig> {
    let plain = crypto::decrypt_standalone(&std::fs::read(paths::legacy_enc_path())?)?;
    config_from_legacy_enc(&plain, uid_floor)
}

/// Parse decrypted legacy `config.enc` bytes into a complete runtime configuration.
///
/// Returns an error for malformed UTF-8 or TOML. Every server group receives a concrete
/// [`GroupConfig`] while existing legacy group settings remain unchanged.
fn config_from_legacy_enc(plain: &[u8], uid_floor: Option<u64>) -> anyhow::Result<AppConfig> {
    // Ignore host/port from the legacy format because the endpoint comes from the key.
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
            default_alert_strategy: 0,
            use_core_manual_config: false,
            // Left unset: the key this legacy entry carries seeds it on the next merge.
            transport: None,
        })
        .collect();
    let mut config = AppConfig {
        servers,
        groups: old.groups,
        language: super::Language::default(),
        ..AppConfig::blank(uid_floor)
    };
    super::ensure_server_group_configs(&config.servers, &mut config.groups);
    Ok(config)
}

/// Migrate the oldest plaintext `config.toml` format containing one server.
///
/// Construction names `uid_floor` for the same reason as [`from_legacy_enc`].
pub fn from_legacy_toml(uid_floor: Option<u64>) -> anyhow::Result<AppConfig> {
    let text = std::fs::read_to_string(paths::legacy_toml_path())?;
    config_from_legacy_toml(&text, uid_floor)
}

/// Parse the oldest plaintext `config.toml` into a complete runtime configuration.
///
/// Returns an error for malformed TOML and materializes the default server group's local manual
/// controls before the caller persists the migrated config.
fn config_from_legacy_toml(text: &str, uid_floor: Option<u64>) -> anyhow::Result<AppConfig> {
    // Ignore host/port because the endpoint comes from the key.
    #[derive(Deserialize)]
    struct Legacy {
        #[serde(default)]
        key: String,
        #[serde(default)]
        market: String,
    }

    let l: Legacy = toml::from_str(text)?;
    let market = if l.market.is_empty() {
        servers::default_market()
    } else {
        l.market
    };
    let mut config = AppConfig {
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
            default_alert_strategy: 0,
            use_core_manual_config: false,
            // Left unset: the key this legacy entry carries seeds it on the next merge.
            transport: None,
        }],
        groups: Vec::new(),
        language: super::Language::default(),
        ..AppConfig::blank(uid_floor)
    };
    super::ensure_server_group_configs(&config.servers, &mut config.groups);
    Ok(config)
}
