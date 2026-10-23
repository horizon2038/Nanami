extern crate alloc;
extern crate self as nun;
pub type Word = usize;
pub use a9n_abi::CapabilityError;

#[path = "../../nanami/src/nanami_core/alpha/process_policy.rs"]
mod policy;
use policy::{ComponentSpec, ProcessPolicies, ProcessPolicy};

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
