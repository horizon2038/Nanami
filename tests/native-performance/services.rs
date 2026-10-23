extern crate self as nanami_uapi;
extern crate self as nun;
pub type Word = usize;
pub type CapabilityDescriptor = usize;
pub use a9n_abi::CapabilityError;
#[path = "../../nanami/servers/uapi/src/registry.rs"]
pub mod registry;
#[path = "../../nanami/src/nanami_core/communication/services.rs"]
mod services;
use services::Services;

fn listed_name(services: &Services, ordinal: usize) -> registry::ServiceInfo {
    let (owner_pid, name_len) = services.info_chunk(ordinal, 0).unwrap();
    let mut info = registry::ServiceInfo {
        owner_pid,
        name_len,
        name: [0; registry::SERVICE_NAME_BYTES],
    };
    for chunk in 1..=2 {
        let (a, b) = services.info_chunk(ordinal, chunk).unwrap();
        for (offset, byte) in a
            .to_le_bytes()
            .into_iter()
            .chain(b.to_le_bytes())
            .enumerate()
        {
            let index = (chunk - 1) * 16 + offset;
            if index < info.name.len() {
                info.name[index] = byte;
            }
        }
    }
    info
}

#[test]
fn arbitrary_names_are_returned_without_closed_kind_numbers() {
    let mut services = Services::new();
    for (index, name) in [
        "unfamiliar-endpoint",
        "abcdefghijklmnopqrstuvwx",
        "名前は外部で決定",
    ]
    .iter()
    .enumerate()
    {
        services.register(index + 1, name, index + 100).unwrap();
        assert_eq!(services.resolve(name), Some((index + 100, index + 1)));
        let info = listed_name(&services, index);
        assert_eq!(info.name(), *name);
        assert_eq!(info.owner_pid, index + 1);
    }
    assert!(services.info_chunk(3, 0).is_none());
    for chunk in [3, usize::MAX] {
        assert!(services.info_chunk(0, chunk).is_none());
    }
}

#[test]
fn registration_cannot_steal_or_replace_an_existing_binding() {
    let mut services = Services::new();
    services.register(7, "arbitrary", 100).unwrap();
    assert!(services.register(8, "arbitrary", 200).is_err());
    assert!(services.register(7, "arbitrary", 200).is_err());
    services.register(7, "arbitrary", 100).unwrap();
    assert_eq!(services.resolve("arbitrary"), Some((100, 7)));
    assert!(services.info_chunk(1, 0).is_none());
}

#[test]
fn independent_domains_have_no_shared_or_fallback_namespace() {
    let mut a = Services::new();
    let mut b = Services::new();
    a.register(7, "same-name", 100).unwrap();
    assert_eq!(b.resolve("same-name"), None);
    b.register(7, "same-name", 200).unwrap();
    assert_eq!(a.resolve("same-name"), Some((100, 7)));
    assert_eq!(b.resolve("same-name"), Some((200, 7)));
}

#[test]
fn invalid_names_and_full_tables_do_not_corrupt_existing_records() {
    let mut services = Services::new();
    for name in ["", "abcdefghijklmnopqrstuvwxyz", "nul\0inside"] {
        assert!(services.register(1, name, 100).is_err());
    }
    assert!(services.register(1, "zero-port", 0).is_err());
    for index in 0..64 {
        services
            .register(index + 1, &format!("fixture-{index}"), index + 100)
            .unwrap();
    }
    assert!(services.register(99, "over-capacity", 999).is_err());
    for index in 0..64 {
        assert_eq!(
            listed_name(&services, index).name(),
            format!("fixture-{index}")
        );
    }
}

#[test]
fn managed_names_share_the_namespace_without_legacy_registration_authority() {
    let mut names = Services::new();
    names.register(7, "legacy", 100).unwrap();
    assert!(names.publish("legacy", 1, 200).is_err());
    names.publish("unfamiliar-managed", 1, 201).unwrap();
    assert_eq!(names.managed_id("unfamiliar-managed"), Some(1));
    assert_eq!(names.managed_id("legacy"), None);
    assert!(names.register(8, "unfamiliar-managed", 300).is_err());
    assert_eq!(listed_name(&names, 1).owner_pid, 0);
    assert_eq!(listed_name(&names, 1).name(), "unfamiliar-managed");
}
