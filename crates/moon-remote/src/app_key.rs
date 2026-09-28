//! The terminal's own SSH key: ed25519, one per terminal installation, created on first use.
//!
//! Only its public half ever leaves this machine — into the administrator's `authorized_keys`.
//! The private half is sealed in `remote/app_key.enc` by this machine's key (the same slot scheme
//! as `servers.enc`), in a file of its own so this tool and a running terminal never race on one
//! file. Losing it locks the terminal out of its servers — password logins are off there — and
//! the way back is the provider's console.

use anyhow::Context;
use russh::keys::PrivateKey;
use russh::keys::ssh_key::LineEnding;
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use zeroize::Zeroizing;

/// Load the app key, or create and seal a new one.
pub fn load_or_create() -> anyhow::Result<PrivateKey> {
    let path = moon_core::config::paths::remote_dir().join("app_key.enc");
    match std::fs::read(&path) {
        Ok(sealed) => {
            let plain = Zeroizing::new(
                moon_core::config::crypto::decrypt_standalone(&sealed)
                    .with_context(|| format!("open {}", path.display()))?,
            );
            PrivateKey::from_openssh(&*plain).context("parse the app key")
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let key = generate()?;
            let text = key
                .to_openssh(LineEnding::LF)
                .context("encode the app key")?;
            let sealed = moon_core::config::crypto::encrypt_standalone(text.as_bytes())?;
            let tmp = path.with_extension("enc.tmp");
            std::fs::write(&tmp, sealed).with_context(|| format!("write {}", tmp.display()))?;
            std::fs::rename(&tmp, &path).with_context(|| format!("replace {}", path.display()))?;
            Ok(key)
        }
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

/// The line for `authorized_keys`.
pub fn authorized_line(key: &PrivateKey) -> anyhow::Result<String> {
    key.public_key()
        .to_openssh()
        .context("encode the app public key")
}

fn generate() -> anyhow::Result<PrivateKey> {
    let mut seed = Zeroizing::new([0u8; 32]);
    getrandom::getrandom(&mut *seed).map_err(|e| anyhow::anyhow!("getrandom: {e}"))?;
    let pair = KeypairData::from(Ed25519Keypair::from_seed(&seed));
    let machine = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "host".to_owned());
    PrivateKey::new(pair, format!("moonterminal@{machine}")).context("build the app key")
}
