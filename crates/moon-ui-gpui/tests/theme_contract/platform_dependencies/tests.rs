//! Synthetic regressions for gate inheritance, lexical exclusions, and dependency aliases.

use super::{Contract, Path, dependencies, fs, tokens};

/// Removing local-name resolution must reproduce the real `mod block;` false positive.
#[test]
fn file_modules_shadow_dependencies_without_hiding_undeclared_uses() {
    let dir = std::env::temp_dir().join(format!("moon-platform-local-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("main.rs"),
        "mod block; fn local() { block::Thing; } mod child;",
    )
    .unwrap();
    fs::write(dir.join("block.rs"), "pub struct Thing;").unwrap();
    fs::write(dir.join("child.rs"), "fn inherited() { block::Thing; }").unwrap();
    let manifest = "[target.'cfg(target_os = \"macos\")'.dependencies]\nblock = \"1\"\n";
    let mut check = contract(manifest);
    check.file(&dir.join("main.rs"), [true; 4], None);
    fs::remove_dir_all(&dir).unwrap();
    assert!(check.failures.is_empty(), "{:?}", check.failures);

    let mut check = contract(manifest);
    check.walk(
        &tokens("#[cfg(windows)] fn missing() { block::Thing; }"),
        Path::new("missing.rs"),
        Path::new("."),
        [true; 4],
    );
    assert_eq!(check.failures.len(), 1, "{:?}", check.failures);
    assert!(check.failures[0].contains("block is unavailable"));
    assert!(check.failures[0].contains("target_os = \"windows\""));
}

/// Hoisted modules and import aliases must shadow crates only within their containing scope.
#[test]
fn scoped_items_and_use_trees_shadow_dependency_names() {
    let manifest = "[target.'cfg(target_os = \"macos\")'.dependencies]\nblock = \"1\"\n";
    for source in [
        "fn local() { block::Thing; } mod block {}",
        "mod block {} use block; fn local() { block::Thing; }",
        "use crate::local as block; fn local() { block::Thing; }",
        "use crate::block; fn local() { block::Thing; }",
        "use super::{local as block}; fn local() { block::Thing; }",
        "use crate::{outer::{self as block}}; fn local() { block::Thing; }",
        "use crate::outer::{block}; fn local() { block::Thing; }",
        "use crate::block::{self}; fn local() { block::Thing; }",
        "use crate::local as block; use block::Thing;",
        "fn local() { self::block::Thing; super::block::Thing; crate::block::Thing; }",
        "struct block; fn local() { block::Thing; }",
        "fn captures() -> impl Sized + use<> { 1 }",
    ] {
        let mut check = contract(manifest);
        check.walk(
            &tokens(source),
            Path::new("local.rs"),
            Path::new("."),
            [true; 4],
        );
        assert!(check.failures.is_empty(), "{source}: {:?}", check.failures);
    }
    for source in [
        "mod sibling { mod block {} } fn missing() { block::Thing; }",
        "fn local() { use crate::block; block::Thing; } fn missing() { block::Thing; }",
        "mod block {} fn missing() { ::block::Thing; }",
        "use block::Thing;",
        "use block;",
        "use block as alias;",
        "use block::{self, Thing};",
        "use block as block;",
        "use {block::{self, Thing}};",
    ] {
        let mut check = contract(manifest);
        check.walk(
            &tokens(source),
            Path::new("missing.rs"),
            Path::new("."),
            [true; 4],
        );
        assert_eq!(check.failures.len(), 1, "{source}: {:?}", check.failures);
        assert!(check.failures[0].contains("block is unavailable"));
    }
}

/// Removing the macOS manifest entry must still identify the screenshot module's external use.
#[test]
fn missing_macos_raw_window_handle_is_not_hidden_by_local_names() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let (before, macos) = manifest
        .split_once("[target.'cfg(target_os = \"macos\")'.dependencies]")
        .unwrap();
    let mutated = format!(
        "{before}[target.'cfg(target_os = \"macos\")'.dependencies]{}",
        macos.replacen("raw-window-handle = \"0.6\"", "", 1)
    );
    assert_ne!(manifest, mutated, "mutation must remove a real dependency");
    let mut check = contract(&mutated);
    check.file(&root.join("src/main.rs"), [true; 4], None);
    assert!(
        check.failures.iter().any(|failure| failure
            .replace('\\', "/")
            .contains("shot/mac.rs: raw_window_handle is unavailable")),
        "{:?}",
        check.failures
    );
    assert!(
        check
            .failures
            .iter()
            .all(|failure| !failure.contains("block is unavailable")),
        "{:?}",
        check.failures
    );
}

/// A trailing comma must not import the crate itself or hide an external use after that import.
#[test]
fn trailing_use_commas_do_not_bind_the_prefix() {
    let mut names = std::collections::BTreeSet::new();
    super::import_names(&tokens("block::{Thing,}"), None, &mut names);
    assert_eq!(names.into_iter().collect::<Vec<_>>(), ["Thing"]);
    let mut check = contract("[target.'cfg(target_os = \"macos\")'.dependencies]\nblock = \"1\"\n");
    check.walk(
        &tokens("#[cfg(target_os = \"macos\")] use block::{Thing,}; #[cfg(windows)] fn missing() { block::Thing; }"),
        Path::new("trailing.rs"), Path::new("."), [true; 4],
    );
    assert_eq!(check.failures.len(), 1, "{:?}", check.failures);
    assert!(check.failures[0].contains("block is unavailable"));
}

/// OS-gated modules and imports may shadow a dependency only where their declarations exist.
#[test]
fn local_bindings_retain_their_declaration_targets() {
    let manifest = "[target.'cfg(target_os = \"macos\")'.dependencies]\nblock = \"1\"\n";
    for source in [
        "#[cfg(windows)] mod block {} fn missing() { block::Thing; }",
        "#[cfg(windows)] use crate::local as block; fn missing() { block::Thing; }",
        "#[cfg(windows)] struct block; fn missing() { block::Thing; }",
        "#[cfg(windows)] mod block {} mod child { fn missing() { block::Thing; } }",
    ] {
        let mut check = contract(manifest);
        check.walk(
            &tokens(source),
            Path::new("gated.rs"),
            Path::new("."),
            [true; 4],
        );
        assert_eq!(check.failures.len(), 1, "{source}: {:?}", check.failures);
        assert!(check.failures[0].contains("block is unavailable"));
    }
    let mut check = contract(manifest);
    check.walk(
        &tokens("#[cfg(windows)] mod block {} #[cfg(windows)] fn local() { block::Thing; }"),
        Path::new("gated.rs"),
        Path::new("."),
        [true; 4],
    );
    assert!(check.failures.is_empty(), "{:?}", check.failures);
}

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
