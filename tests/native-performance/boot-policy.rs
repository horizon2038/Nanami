extern crate alloc;
extern crate self as nun;
pub type Word = usize;
pub use a9n_abi::CapabilityError;

#[path = "../../nanami/src/nanami_core/alpha/process_policy.rs"]
mod policy;
use policy::{ComponentSpec, ProcessPolicies, ProcessPolicy};

#[test]
fn placement_preferences_preserve_automatic_order_and_exact_affinity() {
    use core::cell::Cell;
    let policies = ProcessPolicies::parse("@placement 21 2\n@placement 22 2", 4).unwrap();
    let next = Cell::new(1usize);
    let automatic = || {
        let core = next.get();
        next.set(core + 1);
        core
    };
    assert_eq!(policies.select_affinity(21, None, automatic), 2);
    assert_eq!(next.get(), 2);
    assert_eq!(policies.select_affinity(4, None, automatic), 2);
    assert_eq!(next.get(), 3);
    assert_eq!(policies.select_affinity(22, None, automatic), 2);
    assert_eq!(next.get(), 4);
    assert_eq!(policies.select_affinity(21, Some(0), automatic), 0);
    assert_eq!(next.get(), 4);
    assert_eq!(policies.select_affinity(22, Some(3), automatic), 3);
    assert_eq!(next.get(), 4);
}

#[test]
fn unavailable_preferred_cpus_fall_back_without_hiding_invalid_exact_policy() {
    for cores in 1..=4 {
        let policies = ProcessPolicies::parse("@placement 21 2\n@placement 22 2", cores).unwrap();
        let expected = if cores > 2 { 2 } else { 0 };
        assert_eq!(policies.select_affinity(21, None, || 0), expected);
        assert_eq!(policies.select_affinity(22, None, || 0), expected);
        assert_eq!(policies.select_affinity(4, None, || cores - 1), cores - 1);
    }
    assert!(ProcessPolicies::parse("@process /image affinity=2", 1).is_err());
    assert!(ProcessPolicies::parse("@placement 21 2", 0).is_err());
    assert!(ProcessPolicies::parse("@placement 21 2", 65).is_err());
}

#[test]
fn placement_is_a_scheduling_preference_not_image_or_service_authority() {
    let policies = ProcessPolicies::parse(
        "@placement 0 0 # a numeric scheduling class\n\
         @placement 21 2\n\
         @process ./trusted priority=21 affinity=1 platform=1 services=1",
        4,
    )
    .unwrap();
    for image in [
        "rootfs-memory.elf",
        "posix-service",
        "alter-linux",
        "unfamiliar",
    ] {
        assert_eq!(policies.for_image(image), ProcessPolicy::default());
    }
    let trusted = policies.for_image("/trusted");
    assert_eq!(trusted.priority(None).unwrap(), 21);
    assert!(trusted.platform_information);
    assert!(trusted.service_admin);
    assert_eq!(
        policies.select_affinity(21, trusted.affinity, || panic!(
            "exact affinity consumed a slot"
        )),
        1
    );
    assert_eq!(policies.select_affinity(0, None, || 3), 0);
    assert!(ComponentSpec::parse("@unknown 21 2").is_err());
}

#[test]
fn malformed_or_duplicate_placement_is_rejected_even_when_cpu_is_absent() {
    for configuration in [
        "@placement",
        "@placement 21",
        "@placement +21 2",
        "@placement -1 2",
        "@placement 32 2",
        "@placement 21 +2",
        "@placement 21 -2",
        "@placement 21 64",
        "@placement 21 999999999999999999999999",
        "@placement 21 2 extra",
        "@placement 21 platform=1",
        "@placement 21 2\n@placement 21 3",
        "@placement 021 2\n@placement 21 3",
    ] {
        assert!(
            ProcessPolicies::parse(configuration, 1).is_err(),
            "accepted {configuration}"
        );
    }
}

#[test]
fn all_priority_classes_remain_bounded_and_legacy_defaults_stay_automatic() {
    let mut configuration = String::new();
    for priority in 0..32 {
        configuration.push_str(&format!("@placement {priority} {}\n", priority % 4));
    }
    let policies = ProcessPolicies::parse(&configuration, 4).unwrap();
    let legacy = ProcessPolicies::parse("label 7 ./image\n@process ./image priority=7", 4).unwrap();
    for priority in 0..32 {
        assert_eq!(policies.select_affinity(priority, None, || 3), priority % 4);
        assert_eq!(legacy.select_affinity(priority, None, || 1), 1);
    }
    assert_eq!(policies.select_affinity(32, None, || 3), 3);
    let single_core =
        ProcessPolicies::parse(include_str!("../../nanami/servers/boot-list"), 1).unwrap();
    assert_eq!(single_core.select_affinity(21, None, || 0), 0);
    assert_eq!(single_core.select_affinity(22, None, || 0), 0);
    let shipped =
        ProcessPolicies::parse(include_str!("../../nanami/servers/boot-list"), 4).unwrap();
    assert_eq!(shipped.select_affinity(21, None, || 3), 2);
    // Warm VFS requests stay local to the compatibility/namespace brokers.
    assert_eq!(shipped.select_affinity(22, None, || 3), 2);
    // Device drivers, rendering and guest workers keep their automatic CPU.
    assert_eq!(shipped.select_affinity(23, None, || 3), 3);
    assert_eq!(shipped.select_affinity(24, None, || 1), 1);
    assert_eq!(shipped.select_affinity(4, None, || 1), 1);
}

#[test]
fn component_labels_do_not_carry_privileges_and_invalid_entries_are_not_ignored() {
    for label in ["arbitrary", "device-manager", "unfamiliar-endpoint"] {
        let line = format!("{label} 17 ./modules/custom");
        let entry = ComponentSpec::parse(&line).unwrap().unwrap();
        assert_eq!(
            (entry.name, entry.priority, entry.image_path),
            (label, 17, "./modules/custom")
        );
    }
    assert!(ComponentSpec::parse("  # comment").unwrap().is_none());
    for line in [
        "unknown",
        "label -1 ./image",
        "label +1 ./image",
        "label 32 ./image",
        "label 1",
        "label 1 ./image ignored-argv",
        "@typo anything",
    ] {
        assert!(ComponentSpec::parse(line).is_err(), "accepted {line}");
    }
}

#[test]
fn arbitrary_images_receive_only_explicit_defaults() {
    let policies = ProcessPolicies::parse(
        "arbitrary-label 7 ./module/foo\n@process ./module/foo priority=29 affinity=2 platform=1 # comment",
        4,
    ).unwrap();
    let foo = policies.for_image("./module/foo");
    assert_eq!(foo.priority(None).unwrap(), 29);
    assert_eq!(foo.priority(Some(17)).unwrap(), 17);
    assert_eq!(foo.affinity, Some(2));
    assert!(foo.platform_information);
    assert_eq!(policies.for_image("/module/foo"), foo);
    assert_eq!(policies.for_image("module/foo"), foo);
    for name in [
        "foo",
        "./bin/foo",
        "./module/foo.elf",
        "device-manager",
        "acpi-server",
    ] {
        assert_eq!(policies.for_image(name), ProcessPolicy::default());
    }
    assert_eq!(ProcessPolicy::default().priority(None).unwrap(), 4);
}

#[test]
fn malformed_duplicate_or_unenforceable_policy_is_rejected() {
    for line in [
        "@process",
        "@process ./image",
        "@process / priority=1",
        "@process ./image priority=32",
        "@process ./image priority=-1",
        "@process ./image priority=+1",
        "@process ./image priority=",
        "@process ./image affinity=4",
        "@process ./image affinity=999999999999999999999",
        "@process ./image platform=2",
        "@process ./image platform=1 platform=0",
        "@process ./image priority=1 priority=2",
        "@process ./image affinity=1 affinity=2",
        "@process ./image unknown=1",
        "@process ./image priority=1 extra",
        "@process ./image priority=1\n@process /image affinity=0",
    ] {
        assert!(ProcessPolicies::parse(line, 4).is_err(), "accepted {line}");
    }
    assert!(ProcessPolicy::default().priority(Some(32)).is_err());
}

#[test]
fn shipped_policy_preserves_irq_dependencies_without_core_image_rules() {
    let policies =
        ProcessPolicies::parse(include_str!("../../nanami/servers/boot-list"), 1).unwrap();
    assert_eq!(policies.for_image("./bin/acpi-server").affinity, Some(0));
    assert_eq!(
        policies
            .for_image("./bin/acpi-server")
            .priority(None)
            .unwrap(),
        28
    );
    assert_eq!(
        policies
            .for_image("./bin/usb-server")
            .priority(None)
            .unwrap(),
        28
    );
    assert_eq!(
        policies
            .for_image("./bin/hpet-server")
            .priority(None)
            .unwrap(),
        30
    );
    assert_eq!(
        policies
            .for_image("./bin/ahci-server")
            .priority(None)
            .unwrap(),
        23
    );
    assert!(
        policies
            .for_image("./bin/driver-manager")
            .platform_information
    );
    assert!(
        !policies
            .for_image("./other/driver-manager")
            .platform_information
    );
    let arm =
        ProcessPolicies::parse(include_str!("../../nanami/servers/boot-list.aarch64"), 1).unwrap();
    assert_eq!(
        arm.for_image("./bin/virtio-blk-server")
            .priority(None)
            .unwrap(),
        23
    );
}
