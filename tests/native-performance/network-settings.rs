extern crate alloc;
extern crate self as nanami_client;
pub type Word = usize;
pub mod net {
    pub use nanami_uapi::net::*;
}
pub mod uapi {
    pub use nanami_uapi::pci;
}
use net::*;
#[path = "../../nanami/servers/apps/http-server/src/framing.rs"]
mod framing;
#[path = "../../nanami/servers/apps/network-manager/src/model.rs"]
mod model;
#[path = "../../nanami/servers/apps/http-server/src/options.rs"]
mod options;
#[path = "../../nanami/servers/apps/net-server/src/app/settings_record.rs"]
mod record;
#[path = "../../nanami/servers/core-services/intel-net/src/routing.rs"]
mod routing;
#[test]
fn http_framing_is_bounded_and_connection_tokens_are_exact() {
    let a = b"GET / HTTP/1.1\r\nHost: test\r\nConnection: x-close\r\n\r\n";
    for n in 0..a.len() {
        assert!(matches!(framing::request(&a[..n]), Ok(None)));
    }
    assert!(framing::request(a).unwrap().unwrap().keep_alive);
    assert!(
        !framing::request(
            b"GET / HTTP/1.1\r\nHost: t\r\nConnection: keep-alive\r\nConnection: close\r\n\r\n"
        )
        .unwrap()
        .unwrap()
        .keep_alive
    );
    assert!(
        framing::request(b"HEAD / HTTP/1.0\r\nConnection: Keep-Alive\r\n\r\n")
            .unwrap()
            .unwrap()
            .head
    );
    for req in [
        &b"GET / HTTP/1.1\r\n\r\n"[..],
        b"GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n",
        b"GET / HTTP/1.0\r\nContent-Length: 1\r\n\r\nx",
        b"GET / HTTP/1.0\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
        b"GET / HTTP/1.0\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n",
        b"GET / HTTP/1.0\r\nBad Header: value\r\n\r\n",
    ] {
        assert!(framing::request(req).is_err());
    }
    assert!(framing::request(&vec![b'x'; framing::MAX_HEADER]).is_err());
    let pipeline = b"GET / HTTP/1.0\r\n\r\nHEAD / HTTP/1.0\r\n\r\n";
    let first = framing::request(pipeline).unwrap().unwrap();
    assert!(!first.head);
    assert!(
        framing::request(&pipeline[first.bytes..])
            .unwrap()
            .unwrap()
            .head
    );
}
fn manual() -> Ipv4Settings {
    Ipv4Settings {
        mode: IPV4_MODE_MANUAL,
        address: [192, 168, 10, 101],
        netmask: [255, 255, 255, 0],
        gateway: [192, 168, 10, 1],
        dns: [192, 168, 10, 1],
    }
}
#[test]
fn ipv4_parsing_and_masks_are_strict_and_include_host_routes() {
    assert_eq!(parse_ipv4("192.168.10.101"), Some(manual().address));
    for text in [
        "",
        "1.2.3",
        "1.2.3.4.5",
        "1..3.4",
        "1.2.3.256",
        "-1.2.3.4",
        "1.2.3.4 ",
        "0x1.2.3.4",
    ] {
        assert_eq!(parse_ipv4(text), None, "{text}");
    }
    for bits in 1..=32 {
        assert!(valid_netmask((u32::MAX << (32 - bits)).to_be_bytes()));
    }
    for mask in [
        [0; 4],
        [255, 0, 255, 0],
        [0, 255, 255, 255],
        [255, 255, 255, 1],
    ] {
        assert!(!valid_netmask(mask));
    }
}
#[test]
fn invalid_manual_routes_are_rejected_before_application_but_drafts_can_be_incomplete() {
    assert!(Ipv4Settings::DEFAULT.validate_apply().is_ok());
    let mut settings = Ipv4Settings {
        mode: IPV4_MODE_MANUAL,
        ..Ipv4Settings::DEFAULT
    };
    assert!(settings.validate_draft().is_ok());
    assert!(settings.validate_apply().is_err());
    settings = manual();
    assert!(settings.validate_apply().is_ok());
    for address in [
        [192, 168, 10, 0],
        [192, 168, 10, 255],
        [127, 0, 0, 1],
        [224, 0, 0, 1],
        [0, 1, 2, 3],
    ] {
        assert!(Ipv4Settings {
            address,
            ..settings
        }
        .validate_apply()
        .is_err());
    }
    for gateway in [
        [192, 168, 11, 1],
        settings.address,
        [192, 168, 10, 0],
        [192, 168, 10, 255],
    ] {
        assert!(Ipv4Settings {
            gateway,
            ..settings
        }
        .validate_apply()
        .is_err());
    }
    assert!(Ipv4Settings {
        gateway: [0; 4],
        dns: [0; 4],
        ..settings
    }
    .validate_apply()
    .is_ok());
    assert!(Ipv4Settings {
        mode: 2,
        ..settings
    }
    .validate_draft()
    .is_err());
}
#[test]
fn interrupted_record_writes_preserve_the_previous_complete_settings() {
    let old = record::encode(Ipv4Settings::DEFAULT, 2);
    let new = record::encode(manual(), 3);
    // The inactive record may itself contain an older complete generation.
    let inactive = record::encode(Ipv4Settings::DEFAULT, 1);
    for written in 0..=64 {
        let mut file = [0; 128];
        file[..64].copy_from_slice(&old);
        file[64..].copy_from_slice(&inactive);
        file[64..64 + written].copy_from_slice(&new[..written]);
        let (settings, revision) = record::latest(&file).unwrap();
        assert!((2..=3).contains(&revision));
        assert_eq!(
            settings,
            if revision == 3 {
                manual()
            } else {
                Ipv4Settings::DEFAULT
            }
        );
    }
    let mut file = [0; 128];
    file[..64].copy_from_slice(&old);
    file[64..].copy_from_slice(&new);
    file[64 + 24] ^= 1;
    assert_eq!(record::latest(&file), Some((Ipv4Settings::DEFAULT, 2)));
    assert_eq!(record::latest(&[0; 128]), None);
    assert_eq!(record::latest(&[0; 129]), None);
}
#[test]
fn gui_edits_use_the_same_validation_and_do_not_change_the_original_snapshot() {
    use nanami_ui::Key;
    let initial = manual();
    let mut model = model::Model::new(initial, 9);
    model.fields[0].selected = true;
    for ch in "192.168.10.42".chars() {
        model.key(Key::Character(ch));
    }
    assert_eq!(model.settings().unwrap().address, [192, 168, 10, 42]);
    assert_eq!(model.revision, 9);
    assert!(model.edited);
    model.fields[1].text = "255.0.255.0".into();
    assert!(model.settings().is_err());
    model.fields[1].text = "255.255.255.0".into();
    model.mode = IPV4_MODE_DHCP;
    assert!(!model.key(Key::Character('1')));
    assert!(model.settings().is_ok());
    assert_eq!(initial, manual());
}
#[test]
fn http_options_validate_ports_paths_and_missing_values() {
    let parse = |args: &[&'static str]| options::Options::parse(|i| args.get(i).copied());
    let defaults = parse(&["http-server"]).unwrap();
    assert_eq!(defaults.port, 80);
    let options = parse(&[
        "http-server",
        "--port",
        "8080",
        "--file",
        "/www/custom.html",
    ])
    .unwrap();
    assert_eq!(options.port, 8080);
    assert_eq!(options.file, "/www/custom.html");
    for args in [
        vec!["http-server", "--port", "0"],
        vec!["http-server", "--port", "65536"],
        vec!["http-server", "--port", "+80"],
        vec!["http-server", "--file"],
        vec!["http-server", "--bad", "x"],
    ] {
        assert!(parse(&args).is_err());
    }
}
#[test]
fn pci_bridge_swizzling_uses_each_child_slot_and_supports_nonzero_root_buses() {
    use nanami_uapi::pci::Address;
    let leaf = Address::new(4, 3, 0);
    let bridges = [
        routing::Bridge {
            address: Address::new(2, 5, 0),
            primary: 2,
            secondary: 4,
            subordinate: 4,
        },
        routing::Bridge {
            address: Address::new(0, 28, 0),
            primary: 0,
            secondary: 2,
            subordinate: 4,
        },
    ];
    let mut queried = Vec::new();
    let irq = routing::resolve(
        leaf,
        1,
        |i| bridges.get(i).copied(),
        |address, pin| {
            queried.push((address, pin));
            if address.bus == 0 && address.device == 28 && pin == 1 {
                Some(19)
            } else {
                None
            }
        },
    );
    assert_eq!(irq, Some(19));
    assert_eq!(
        queried,
        [(leaf, 1), (bridges[0].address, 4), (bridges[1].address, 1)]
    );
    assert_eq!(
        routing::resolve(
            bridges[0].address,
            2,
            |_| None,
            |address, pin| if address.bus == 2 && pin == 2 {
                Some(17)
            } else {
                None
            }
        ),
        Some(17)
    );
    assert_eq!(routing::resolve(leaf, 0, |_| None, |_, _| Some(19)), None);
    assert_eq!(routing::resolve(leaf, 1, |_| None, |_, _| None), None);
}
