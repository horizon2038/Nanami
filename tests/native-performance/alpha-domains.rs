extern crate alloc;
extern crate self as nun;
pub use a9n_abi::{CapabilityError, Word};
#[path = "../../nanami/src/nanami_core/alpha/domain_manifest.rs"]
mod manifest;
#[path = "../../nanami/src/nanami_core/alpha/process_policy.rs"]
mod process_policy;
use manifest::ChildSpec;

#[test]
fn real_import_configurations_validate_on_single_and_multi_core_targets() {
    for configuration in [
        include_str!("../alpha/imports/boot-x86_64.conf"),
        include_str!("../alpha/imports/boot-aarch64.conf"),
        include_str!("../alpha/recursive-imports/boot-x86_64.conf"),
        include_str!("../alpha/recursive-imports/boot-aarch64.conf"),
        include_str!("../alpha/recursive-imports/relay.conf"),
    ] {
        for cores in [1, 4] {
            process_policy::ProcessPolicies::parse(configuration, cores).unwrap();
            assert_eq!(manifest::imports(configuration).unwrap().len(), 1);
        }
    }
}

#[test]
fn recursive_fixture_requires_registered_parent_service_before_launch() {
    for configuration in [
        include_str!("../alpha/recursive-imports/boot-x86_64.conf"),
        include_str!("../alpha/recursive-imports/boot-aarch64.conf"),
    ] {
        let child = configuration
            .lines()
            .find_map(|line| ChildSpec::parse(line).unwrap())
            .unwrap();
        assert_eq!(child.label, "relay");
        assert_eq!(child.radix, 30);
        assert_eq!(child.after, Some("terminal-service"));
        let imports = manifest::imports(configuration).unwrap();
        assert_eq!(imports[0].child, child.label);
        assert_eq!(imports[0].name, "middle-door-5");
        assert_eq!(imports[0].sessions, 1);
    }
    let relay = include_str!("../alpha/recursive-imports/relay.conf");
    let grandchild = relay
        .lines()
        .find_map(|line| ChildSpec::parse(line).unwrap())
        .unwrap();
    assert_eq!(grandchild.radix, 29);
    let imports = manifest::imports(relay).unwrap();
    assert_eq!(imports[0].source, "middle-door-5");
    assert_eq!(imports[0].name, "child-door-9");
    assert_eq!(imports[0].sessions, 2);
}

#[test]
fn arbitrary_names_and_module_paths_come_only_from_configuration() {
    let spec =
        ChildSpec::parse(" @alpha laboratory-7 29 ./some/elf - ./arbitrary/config # comment")
            .unwrap()
            .unwrap();
    assert_eq!(
        (
            spec.label,
            spec.radix,
            spec.image,
            spec.archive,
            spec.configuration
        ),
        ("laboratory-7", 29, "./some/elf", "-", "./arbitrary/config")
    );
    let spec = ChildSpec::parse("@alpha x 30 ./different/image ./private/archive ./configuration")
        .unwrap()
        .unwrap();
    assert_eq!(spec.archive, "./private/archive");
    assert_eq!(spec.radix, 30);
    let spec = ChildSpec::parse("@alpha x 29 ./elf - ./config after=unfamiliar-endpoint")
        .unwrap()
        .unwrap();
    assert_eq!(spec.after, Some("unfamiliar-endpoint"));
}

#[test]
fn ordinary_process_entries_and_comments_are_not_child_launches() {
    for line in [
        "",
        "  # none",
        "unfamiliar-service 17 ./different-binary",
        "example 31 ./sample @alpha",
    ] {
        assert!(ChildSpec::parse(line).unwrap().is_none());
    }
}

#[test]
fn malformed_or_unsupported_child_budgets_are_rejected() {
    for line in [
        "@alpha",
        "@alpha x",
        "@alpha x 29 ./elf -",
        "@alpha x 29 ./elf - ./config extra",
        "@alpha x 28 ./elf - ./config",
        "@alpha x -1 ./elf - ./config",
        "@alpha x +29 ./elf - ./config",
        "@alpha x 64 ./elf - ./config",
        "@alpha x 999999999999999999999999 ./elf - ./config",
        "@alpha abcdefghijklmnopqrstuvwxyz0123456789 29 ./elf - ./config",
        "@alpha x 29 ./elf - ./config after=",
        "@alpha x 29 ./elf - ./config other=x",
        "@alpha x 29 ./elf - ./config after=x extra",
    ] {
        assert!(ChildSpec::parse(line).is_err(), "accepted {line}");
    }
}

#[test]
fn explicit_imports_support_aliases_and_require_an_unambiguous_child() {
    let configuration = "@alpha B 29 ./elf - ./config\n@import B child-name parent-name 2\n";
    let imports = manifest::imports(configuration).unwrap();
    assert_eq!(imports.len(), 1);
    assert_eq!(
        (
            imports[0].child,
            imports[0].name,
            imports[0].source,
            imports[0].sessions
        ),
        ("B", "child-name", "parent-name", 2)
    );
    for extra in [
        "@import B child-name other-source 1",
        "@import C another source 1",
        "@alpha B 29 ./elf - ./other-config",
    ] {
        assert!(manifest::imports(&format!("{configuration}{extra}\n")).is_err());
    }
    for line in [
        "@import",
        "@import B",
        "@import B name source",
        "@import B name source 0",
        "@import B name source 65",
        "@import B name source +1",
        "@import B name source -1",
        "@import B name source 1 extra",
        "@import B name source 184467440737095516160",
        "@import B name abcdefghijklmnopqrstuvwxyz 1",
        "@import B na\0me source 1",
    ] {
        assert!(
            manifest::ImportSpec::parse(line).is_err(),
            "accepted {line}"
        );
    }
    let mut configuration = String::from("@alpha B 29 ./elf - ./config\n");
    for index in 0..16 {
        configuration.push_str(&format!("@import B name{index} source 1\n"));
    }
    assert_eq!(manifest::imports(&configuration).unwrap().len(), 16);
    configuration.push_str("@import B overflow source 1\n");
    assert!(manifest::imports(&configuration).is_err());
}
