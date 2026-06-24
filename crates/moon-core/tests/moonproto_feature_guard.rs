use std::path::PathBuf;
use std::process::Command;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn workspace_dir() -> PathBuf {
    manifest_dir()
        .parent()
        .and_then(|p| p.parent())
        .expect("moon-core must live under workspace/crates")
        .to_path_buf()
}

#[test]
fn moonproto_dependency_does_not_request_diagnostics() {
    let manifest = manifest_dir().join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest).expect("read moon-core Cargo.toml");
    let moonproto_lines: Vec<&str> = text
        .lines()
        .filter(|line| line.trim_start().starts_with("moonproto ="))
        .collect();

    assert!(
        !moonproto_lines.is_empty(),
        "moon-core must keep an explicit moonproto dependency"
    );

    for line in moonproto_lines {
        assert!(
            !line.contains("diagnostics") && !line.contains("diagnostic-trace"),
            "terminal default dependency must not enable MoonProto diagnostics features: {line}"
        );
    }
}

#[test]
fn resolved_default_moonproto_features_do_not_include_diagnostics() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = Command::new(cargo)
        .current_dir(workspace_dir())
        .args([
            "tree",
            "-p",
            "moon-core",
            "-e",
            "features",
            "--prefix",
            "none",
        ])
        .output()
        .expect("run cargo tree feature guard");

    assert!(
        output.status.success(),
        "cargo tree feature guard failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let forbidden = [
        "moonproto feature \"diagnostics\"",
        "moonproto feature \"diagnostic-trace\"",
    ];

    for feature in forbidden {
        assert!(
            !stdout.lines().any(|line| line.trim() == feature),
            "terminal default dependency graph must not enable {feature}\n{stdout}"
        );
    }
}

#[test]
fn moonproto_diagnostics_requires_explicit_feature() {
    let manifest = manifest_dir().join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest).expect("read moon-core Cargo.toml");
    let expected = r#"moonproto-diagnostics = ["moonproto/diagnostics"]"#;
    assert!(
        text.lines().any(|line| line.trim() == expected),
        "debug fill exception must stay explicit and reviewable: expected `{expected}`"
    );
}
