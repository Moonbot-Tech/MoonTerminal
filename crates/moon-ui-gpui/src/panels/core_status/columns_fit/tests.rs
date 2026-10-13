//! Narrow-dock column dropping. Dropping ping or the core name leaves an operator unable to
//! see who is slow; dropping a later tier before an earlier one hides the wrong columns.

use super::{DROP_TIERS, apply_drop, drop_level};

/// The tier list is the product order: server CPU and startup go first, then free RAM and
/// exchange ping, then process memory and the server name. Ping, the core name and status
/// are not on it.
#[test]
fn tiers_drop_server_cpu_before_free_ram_before_memory() {
    assert_eq!(DROP_TIERS[0], ["cpu_sys", "startup"]);
    assert_eq!(DROP_TIERS[1], ["free_phys", "ping_exch"]);
    assert_eq!(DROP_TIERS[2], ["mem_used", "server"]);
    let dropped: Vec<&str> = DROP_TIERS
        .iter()
        .flat_map(|tier| tier.iter().copied())
        .collect();
    for kept in ["ping", "core", "status"] {
        assert!(!dropped.contains(&kept), "{kept} is not a droppable column");
    }
}

fn measured() -> Vec<(&'static str, f32)> {
    vec![
        ("cpu_sys", 30.0),
        ("startup", 20.0),
        ("free_phys", 30.0),
        ("ping_exch", 20.0),
        ("mem_used", 30.0),
        ("server", 20.0),
        ("ping", 10.0),
        ("core", 10.0),
        ("status", 10.0),
    ]
}

/// Sum is 180. Each tier is 50 px, and the three columns that never drop are 30 px. The level
/// is the first one whose remaining sum fits, including the exact half-pixel boundary.
#[test]
fn drop_level_follows_tier_order_and_the_half_pixel_boundary() {
    let measured = measured();
    assert_eq!(drop_level(&measured, 179.5), 0);
    assert_eq!(drop_level(&measured, 179.4), 1);
    assert_eq!(drop_level(&measured, 129.5), 1);
    assert_eq!(drop_level(&measured, 129.4), 2);
    assert_eq!(drop_level(&measured, 79.5), 2);
    assert_eq!(drop_level(&measured, 79.4), 3);
    assert_eq!(drop_level(&measured, 0.0), 3);
}

/// A width the measurer did not report adds nothing. Missing tier columns must not force a
/// drop, and a column that is not in any tier still cannot be removed to make the rest fit.
#[test]
fn unmeasured_keys_add_no_width_and_ping_survives_a_tiny_dock() {
    assert_eq!(drop_level(&[("ping", 10.0), ("core", 8.0)], 17.5), 0);
    assert_eq!(drop_level(&[("ping", 10.0)], 9.5), 0);
    assert_eq!(drop_level(&[("ping", 10.0)], 9.4), 3);

    let keys = [
        "server",
        "core",
        "cpu_sys",
        "status",
        "ping",
        "startup",
        "free_phys",
        "ping_exch",
        "mem_used",
    ];
    assert_eq!(apply_drop(&keys, 0), keys.to_vec());
    assert_eq!(
        apply_drop(&keys, 1),
        vec![
            "server",
            "core",
            "status",
            "ping",
            "free_phys",
            "ping_exch",
            "mem_used"
        ]
    );
    assert_eq!(
        apply_drop(&keys, 2),
        vec!["server", "core", "status", "ping", "mem_used"]
    );
    let tiny = apply_drop(&keys, drop_level(&measured(), 0.0));
    assert_eq!(tiny, vec!["core", "status", "ping"]);
}
