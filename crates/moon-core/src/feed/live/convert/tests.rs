use super::*;

/// A future edit that wires machine-wide `system_cpu_percent` into the
/// process-CPU field, swaps process/system, or feeds `used_memory_mb` from
/// `free_physical_memory_mb` — the scope-vs-average confusion the panel must
/// never render — reddens here. Oracle independent of the converter: distinct
/// hand-chosen values per field can only land in the correctly-named one.
#[test]
fn kernel_health_maps_each_field_by_scope() {
    let h = moonproto::state::KernelHealth {
        process_cpu_percent: 12,
        system_cpu_percent: 77,
        used_memory_mb: Some(345),
        free_physical_memory_mb: Some(678),
        logical_cpu_count: Some(16),
    };
    let sys = sys_status_from_proto(h, 1_780_000_000_000);
    assert_eq!(sys.process_cpu_percent, Some(12));
    assert_eq!(sys.system_cpu_percent, Some(77));
    assert_eq!(sys.used_memory_mb, Some(345));
    assert_eq!(sys.free_physical_memory_mb, Some(678));
    assert_eq!(sys.logical_cpu_count, Some(16));
    assert_eq!(sys.updated_ms, 1_780_000_000_000);
}

/// Memory / CPU-count Options stay `None` until the lower-rate profile arrives —
/// the converter must not fabricate a value on a CPU-only ping.
#[test]
fn kernel_health_preserves_absent_memory() {
    let h = moonproto::state::KernelHealth {
        process_cpu_percent: 5,
        system_cpu_percent: 9,
        used_memory_mb: None,
        free_physical_memory_mb: None,
        logical_cpu_count: None,
    };
    let sys = sys_status_from_proto(h, 42);
    assert_eq!(sys.process_cpu_percent, Some(5));
    assert_eq!(sys.system_cpu_percent, Some(9));
    assert_eq!(sys.used_memory_mb, None);
    assert_eq!(sys.free_physical_memory_mb, None);
    assert_eq!(sys.logical_cpu_count, None);
}
