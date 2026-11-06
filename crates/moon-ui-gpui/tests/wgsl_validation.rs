//! Compile the native chart shader sources even on hosts whose renderer is DX11 or Metal.

/// Removing a helper used by an entry point (as happened to the candle volume shader), or
/// introducing invalid WGSL, must fail here instead of leaving Linux with an invalid pipeline.
#[test]
fn native_chart_shaders_parse_and_validate() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/chartdx/shaders");
    let mut files: Vec<_> = std::fs::read_dir(&directory)
        .expect("chart shader directory exists")
        .map(|entry| entry.expect("shader entry is readable").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "wgsl"))
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "no WGSL shaders found in {}",
        directory.display()
    );
    let mut errors = Vec::new();
    for path in files {
        let source = std::fs::read_to_string(&path).expect("shader source is readable");
        match naga::front::wgsl::parse_str(&source) {
            Ok(module) => {
                let mut validator = naga::valid::Validator::new(
                    naga::valid::ValidationFlags::all(),
                    naga::valid::Capabilities::empty(),
                );
                if let Err(error) = validator.validate(&module) {
                    errors.push(format!(
                        "{}: {}",
                        path.display(),
                        error.emit_to_string(&source)
                    ));
                }
            }
            Err(error) => errors.push(format!(
                "{}: {}",
                path.display(),
                error.emit_to_string(&source)
            )),
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}
