//! The actual default spawn policy, independent of hardware and IPC mocks.
extern crate self as nun;
pub type Word = usize;

#[path = "../../nanami/src/nanami_core/alpha/process_priority.rs"]
mod policy;
use policy::process_priority_for_image as priority;

#[test]
fn acpi_irq_provider_matches_usb_priority_and_stays_below_timer() {
    assert_eq!(priority("./bin/acpi-server"), 28);
    assert_eq!(priority("acpi-server"), priority("usb-server"));
    assert!(priority("acpi-server") > priority("ahci-server"));
    assert!(priority("acpi-server") > priority("ext2-server"));
    assert!(priority("acpi-server") < priority("hpet-server"));
}

#[test]
fn unrelated_default_priorities_are_preserved() {
    for (image, expected) in [
        ("timer-server", 30),
        ("hpet-server", 30),
        ("driver-manager", 30),
        ("input-server", 28),
        ("ps2-server", 28),
        ("usb-server", 28),
        ("fb-server", 24),
        ("honoka", 24),
        ("block-device-server", 23),
        ("virtio-blk-server", 23),
        ("ahci-server", 23),
        ("virtio-net", 23),
        ("ext2-server", 22),
        ("net-server", 22),
        ("rtc-server", 22),
        ("http-server", 21),
        ("honoka-client", 18),
        ("eg-test", 18),
        ("image-viewer", 18),
        ("performance-monitor", 18),
        ("shell", 16),
        ("cpp-hello", 8),
        ("rust-hello", 8),
        ("unrecognized", 4),
        ("", 4),
    ] {
        assert_eq!(priority(image), expected, "{image}");
    }
}

#[test]
fn path_and_elf_normalization_preserve_the_policy() {
    for path in [
        "acpi-server",
        "acpi-server.elf",
        "./bin/acpi-server",
        "/bin/acpi-server.elf",
    ] {
        assert_eq!(priority(path), 28, "{path}");
    }
    assert_eq!(priority("/bin/ahci-server.elf"), 23);
    assert_eq!(priority("/acpi-server/unrecognized"), 4);
    assert_eq!(priority("acpi-server.ELF"), 4);
    assert_eq!(priority("/bin/"), 4);
}
