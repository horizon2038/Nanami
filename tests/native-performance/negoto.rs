#![allow(dead_code)]
mod abi {
    pub const ALTER_PATH_MAX: usize = 256;
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/context.rs"]
mod context;
use context::RootfsContext;
#[path = "../../nanami/servers/core-services/system-manager/src/startup_manifest.rs"]
mod startup_manifest;

#[test]
fn rootfs_startup_preserves_arguments_instead_of_silently_ignoring_them() {
    let entry = startup_manifest::parse_system_list_line(
        "  negoto-service 20 /bin/negoto  serve  # daemon",
    )
    .unwrap();
    assert_eq!(entry.name, "negoto-service");
    assert_eq!(entry.priority, 20);
    assert_eq!(entry.path, "/bin/negoto");
    assert_eq!(entry.arguments.trim(), "serve");
    assert!(
        startup_manifest::parse_system_list_line("shell 16 /bin/shell")
            .unwrap()
            .arguments
            .is_empty()
    );
    assert!(startup_manifest::parse_system_list_line("x 32 /bin/x").is_none());
    assert!(startup_manifest::parse_system_list_line("# empty").is_none());
}

#[test]
fn backing_roots_never_accept_dot_components_or_the_native_root() {
    for path in [
        b"/".as_slice(),
        b"relative",
        b"/a/..",
        b"/a/../b",
        b"/a/./b",
        b"/a//b",
        b"/a/",
        b"/a\0b",
    ] {
        assert!(
            RootfsContext::new(path, b"/bin/busybox", false).is_none(),
            "{path:?}"
        );
    }
    assert!(RootfsContext::new(b"/bundle/rootfs", b"/bin/../busybox", false).is_none());
}

#[test]
fn a_host_looking_guest_path_does_not_bypass_container_root() {
    let context = RootfsContext::new(b"/bundle/rootfs", b"/bin/busybox", true).unwrap();
    let mut bytes = [0; 256];
    let len = context
        .host_path(b"/bundle/rootfs/secret", &mut bytes)
        .unwrap();
    assert_eq!(&bytes[..len], b"/bundle/rootfs/bundle/rootfs/secret");
    let len = context.host_path(b"/", &mut bytes).unwrap();
    assert_eq!(&bytes[..len], b"/bundle/rootfs");
    assert!(context.readonly);
    assert!(context.prepared);
    assert!(context.host_path(b"/../../etc", &mut bytes).is_none());
}

#[test]
fn executable_and_path_buffers_are_checked_without_partial_updates() {
    let mut context = RootfsContext::new(b"/bundle/rootfs", b"/bin/busybox", false).unwrap();
    let mut bytes = [0; 8];
    assert!(context.host_path(b"/bin/busybox", &mut bytes).is_none());
    assert_eq!(bytes, [0; 8]);
    assert!(context.set_executable(b"../native").is_none());
    assert_eq!(
        &context.executable[..context.executable_len],
        b"/bin/busybox"
    );
}
