use super::{GpuArgs, parse};

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// Both spellings name the GPU and the mode; the mode is kept lower-case, as MoonUI parses it.
#[test]
fn both_spellings_are_read() {
    let parsed = parse(args(&["--gpu", "pci-0000:01:00.0", "--present-mode=FIFO"])).unwrap();
    assert_eq!(parsed.gpu.as_deref(), Some("pci-0000:01:00.0"));
    assert_eq!(parsed.present_mode.as_deref(), Some("fifo"));
    assert!(parsed.explicit());
}

/// `auto` is what leaving the option out means: nothing explicit is handed to the renderer.
#[test]
fn auto_is_the_same_as_leaving_it_out() {
    let parsed = parse(args(&["--gpu=auto", "--present-mode", "AUTO"])).unwrap();
    assert_eq!(parsed, GpuArgs::default());
    assert!(!parsed.explicit());
}

/// A mode the renderer does not know is refused with the list, not taken as automatic.
#[test]
fn an_unknown_mode_is_refused_with_the_choices() {
    let error = parse(args(&["--present-mode", "vsync"])).unwrap_err();
    assert!(error.contains("auto, fifo, mailbox, immediate"), "{error}");
}

/// An option given twice is ambiguous, and the next flag is not a value.
#[test]
fn a_repeated_option_or_a_missing_value_is_refused() {
    assert!(parse(args(&["--gpu", "a", "--gpu", "b"])).is_err());
    assert!(parse(args(&["--gpu", "--present-mode", "fifo"])).is_err());
    assert!(parse(args(&["--present-mode"])).is_err());
}

/// Other options belong to other parts of the terminal and pass through untouched.
#[test]
fn other_options_are_passed_over() {
    let parsed = parse(args(&[
        "--fixture",
        "--debug-script",
        "chart-smoke",
        "--list-gpus",
    ]))
    .unwrap();
    assert!(parsed.list_gpus);
    assert!(!parsed.explicit());
}
