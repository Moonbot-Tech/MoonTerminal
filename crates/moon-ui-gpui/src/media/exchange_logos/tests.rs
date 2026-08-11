use std::sync::{
    Arc, Barrier, OnceLock,
    atomic::{AtomicUsize, Ordering},
};

use moon_core::venue::Brand;

use super::{EMBEDDED, RASTER_PX, load, prewarm_once};

/// Catches replacing `exchange_logos.rs:prewarm_once` with an ordinary cache check: concurrent
/// Shell entries could all begin filesystem reads and SVG decode before any one task fills it.
#[test]
fn concurrent_logo_prewarm_runs_one_blocking_initializer() {
    const CALLERS: usize = 8;
    let gate = Arc::new(OnceLock::new());
    let start = Arc::new(Barrier::new(CALLERS));
    let calls = Arc::new(AtomicUsize::new(0));
    let workers = (0..CALLERS)
        .map(|_| {
            let gate = gate.clone();
            let start = start.clone();
            let calls = calls.clone();
            std::thread::spawn(move || {
                start.wait();
                prewarm_once(&gate, || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    std::thread::yield_now();
                });
            })
        })
        .collect::<Vec<_>>();

    for worker in workers {
        worker.join().expect("prewarm worker must not panic");
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "all concurrent callers must share one blocking initializer"
    );
}

/// Every brand the directory can name must ship a file that actually rasterizes.
///
/// Breakage: adding a brand to `moon_core::venue` without its SVG, or renaming an asset, leaves the
/// resolver pointing at a stem that silently produces no icon at runtime — which reads as "this
/// exchange has no logo" instead of as a bug.
#[test]
fn every_brand_ships_a_rasterizable_file() {
    for brand in Brand::ALL {
        let slug = brand.slug();
        assert!(
            EMBEDDED.get_file(format!("{slug}.svg")).is_some(),
            "assets/exchanges/{slug}.svg is missing from the embedded set"
        );
        let texture = load(slug).unwrap_or_else(|| panic!("{slug}.svg must rasterize"));
        assert_eq!(
            texture.size(0).width.0 as u32,
            RASTER_PX,
            "{slug}.svg must rasterize to the shared square size"
        );
    }
}
