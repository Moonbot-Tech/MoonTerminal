//! Static contract for the one writer of each core's full safe-share configuration.

use super::support::*;

/// Rejects a second `send_shared_config` call outside `feed/live/shared_config.rs::drive`.
///
/// Adding `MoonClient::settings().send_shared_config` anywhere else makes this test red: two
/// producers can build full snapshots from different bases, silently reverting each other's
/// settings and leaving the user with a confirmed change that disappears.
#[test]
fn shared_config_has_one_live_sequence_writer() {
    let core_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("moon-core")
        .join("src");
    let shared_config = read_core_src("feed/live/shared_config.rs");
    let drive = code_only(braced_body(&shared_config, "pub(super) fn drive("));
    assert_eq!(
        drive.matches("send_shared_config(").count(),
        1,
        "SharedConfigSequence::drive must contain its one full-config send"
    );

    let mut sources = Vec::new();
    rust_sources(&core_root, &mut sources);
    assert!(
        !sources.is_empty(),
        "no sources found under {}",
        core_root.display()
    );

    let mut call_sites = Vec::new();
    for path in sources {
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
            .replace("\r\n", "\n");
        let rel = path
            .strip_prefix(&core_root)
            .unwrap_or_else(|err| {
                panic!(
                    "{} is outside {}: {err}",
                    path.display(),
                    core_root.display()
                )
            })
            .to_string_lossy()
            .replace('\\', "/");
        for (line, source) in text.lines().enumerate() {
            if source
                .split("//")
                .next()
                .unwrap_or(source)
                .contains("send_shared_config(")
            {
                call_sites.push(format!("{rel}:{}", line + 1));
            }
        }
    }

    assert_eq!(
        call_sites,
        ["feed/live/shared_config.rs:271"],
        "send_shared_config must have exactly one call site in feed/live/shared_config.rs"
    );
}
