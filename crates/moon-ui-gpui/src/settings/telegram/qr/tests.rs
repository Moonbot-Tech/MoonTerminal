use super::{GridFit, fit_grid};

/// The layout snaps an element to whole device pixels, so the area it grants is routinely a
/// fraction of a pixel smaller than the side the element asked for, and the code must still be
/// drawn. The numbers are the reported case: Windows at 125 % with the interface at 0.85 gives a
/// factor of 1.0625, and a 45-module code that asked for 180 px is granted 191 device pixels.
#[test]
fn a_grid_fits_an_area_the_layout_snapped_below_the_requested_side() {
    let scale = 1.0625_f32;
    let granted = 191.0 / scale;
    assert!(granted < 180.0, "the case is an area below the request");
    // 191 / 45 leaves four whole pixels a module; the spare 11 put five ahead of the grid.
    assert_eq!(
        fit_grid(granted, scale, 45),
        Some(GridFit {
            module: 4.0,
            inset: 5.0
        })
    );
}

/// An area that holds a whole number of modules keeps every pixel of them. At a factor of 1.1 the
/// 180 device pixels come back from the layout's division as 179.99998, which a bare floor would
/// turn into three-pixel modules.
#[test]
fn float_noise_in_the_granted_area_does_not_cost_a_pixel_per_module() {
    let scale = 1.1_f32;
    let granted = 180.0 / scale;
    assert!(granted * scale < 180.0, "the case is a product below 180");
    assert_eq!(
        fit_grid(granted, scale, 45),
        Some(GridFit {
            module: 4.0,
            inset: 0.0
        })
    );
}

/// An area with less than one device pixel a module cannot carry the code at all: nothing is
/// placed, rather than a grid of zero-sized modules.
#[test]
fn an_area_too_small_for_one_pixel_modules_places_nothing() {
    assert_eq!(fit_grid(40.0, 1.0, 45), None);
}
