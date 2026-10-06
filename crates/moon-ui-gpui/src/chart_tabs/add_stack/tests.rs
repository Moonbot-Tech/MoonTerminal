//! Synthetic measurements of render-order ownership costs.

use std::hint::black_box;
use std::rc::Rc;
use std::time::Instant;

/// Measures the cost of copying chart slots versus sharing them across render closures.
#[test]
#[ignore]
fn render_order_share_bench() {
    let started = Instant::now();
    for _ in 0..100_000 {
        let order: Vec<usize> = black_box((0..64).collect());
        black_box(order.clone());
        black_box(order.clone());
        black_box(order.clone());
        black_box(order);
    }
    let vec_elapsed = started.elapsed();

    let started = Instant::now();
    for _ in 0..100_000 {
        let render_order: Vec<usize> = black_box((0..64).collect());
        let order: Rc<[usize]> = render_order.into();
        black_box(order.clone());
        black_box(order.clone());
        black_box(order.clone());
        black_box(order);
    }
    let rc_elapsed = started.elapsed();
    println!(
        "render_order_share_bench default test profile: slots=64 iterations=100000 vec_us={} rc_us={}",
        vec_elapsed.as_micros(),
        rc_elapsed.as_micros()
    );
}
