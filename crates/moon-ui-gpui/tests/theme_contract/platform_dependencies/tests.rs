//! Synthetic regressions for gate inheritance, lexical exclusions, and dependency aliases.

use super::{Contract, Path, dependencies, fs, tokens};

/// Build a contract from synthetic manifest data, independently of the repository manifest.
fn contract(manifest: &str) -> Contract {
    Contract {
        dependencies: dependencies(manifest),
        failures: Vec::new(),
        files: 0,
    }
}

/// Dropping inheritance or treating unknown cfg as false must not hide an unavailable import.
#[test]
fn item_and_inline_module_gates_preserve_missing_dependencies() {
    let mut check = contract("[target.'cfg(windows)'.dependencies]\nonly-win = \"1\"\n");
    check.walk(
        &tokens(
            r#"
        #[cfg(windows)] fn allowed() { only_win::run(); }
        #[cfg(target_os = "macos")] mod nested {
            fn missing() { only_win::run(); }
        }
        #[cfg(not(feature = "unknown"))] fn conservative() { only_win::run(); }
        #[cfg(target_os = "unclassified")] fn unknown_os() { only_win::run(); }
    "#,
        ),
        Path::new("synthetic.rs"),
        Path::new("."),
        [true; 4],
    );
    assert_eq!(check.failures.len(), 3, "{:?}", check.failures);
    assert!(check.failures[0].contains("target_os = \"macos\""));
    assert!(check.failures[1].contains("[dependencies]"));
}

/// Renamed package keys and shared declarations must not turn aliases into phantom violations.
#[test]
fn renamed_dependencies_and_comment_literals_are_respected() {
    let mut check = contract(
        r#"
        [dependencies]
        shared-name = { package = "original-package", version = "1" }
        [target.'cfg(any(windows, target_os = "macos"))'.dependencies]
        native-alias = { package = "native-package", version = "1" }
    "#,
    );
    check.walk(
        &tokens(
            r##"
        /* outer /* native_alias::bad(); */ native_alias::bad(); */
        // native_alias::bad();
        const TEXT: &str = r#"native_alias::bad();"#;
        fn shared() { shared_name::run(); }
        #[cfg(any(windows, target_os = "macos"))] fn allowed() { native_alias::run(); }
        #[cfg(target_os = "linux")] fn missing() { native_alias::run(); }
    "##,
        ),
        Path::new("synthetic.rs"),
        Path::new("."),
        [true; 4],
    );
    assert_eq!(check.failures.len(), 1, "{:?}", check.failures);
    assert!(check.failures[0].contains("native_alias"));
    assert!(check.failures[0].contains("target_os = \"linux\""));
}

/// A path attribute must carry its parent's macOS gate into a differently named file and children.
#[test]
fn relocated_module_and_descendants_inherit_declaration_gate() {
    let dir = std::env::temp_dir().join(format!("moon-platform-contract-{}", std::process::id()));
    fs::create_dir_all(dir.join("inline")).unwrap();
    fs::write(
        dir.join("main.rs"),
        "#[cfg(target_os = \"macos\")] #[path = \"renamed.rs\"] mod native;\n#[cfg(windows)] mod inline { #[path = \"moved.rs\"] mod other; }",
    )
    .unwrap();
    fs::write(
        dir.join("inline/moved.rs"),
        "fn allowed() { only_win::run(); }",
    )
    .unwrap();
    fs::write(dir.join("renamed.rs"), "mod child;").unwrap();
    fs::write(dir.join("child.rs"), "fn missing() { only_win::run(); }").unwrap();
    let mut check = contract("[target.'cfg(windows)'.dependencies]\nonly-win = \"1\"\n");
    check.file(&dir.join("main.rs"), [true; 4], None);
    fs::remove_dir_all(&dir).unwrap();
    assert_eq!(check.files, 4);
    assert_eq!(check.failures.len(), 1);
    assert!(check.failures[0].contains("child.rs"));
    assert!(check.failures[0].contains("target_os = \"macos\""));
}

/// Local paths and strings mentioning an external name must never be classified as crate uses.
#[test]
fn local_namespaces_and_file_inner_cfg_do_not_create_false_violations() {
    let mut check =
        contract("[target.'cfg(windows)'.dependencies]\nbacktrace = \"1\"\nwindows = \"1\"\n");
    check.walk(
        &tokens(
            r#"
        #![cfg(windows)]
        use super::{windows::local};
        fn first() { std::backtrace::Backtrace::capture(); }
        fn second() { windows::external(); }
    "#,
        ),
        Path::new("synthetic.rs"),
        Path::new("."),
        [true; 4],
    );
    assert!(check.failures.is_empty(), "{:?}", check.failures);
}
