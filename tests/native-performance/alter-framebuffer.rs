#![allow(dead_code)]

#[path = "../../nanami/servers/apps/alter/shared/src/common/framebuffer_size.rs"]
pub mod framebuffer_size;
pub mod common {
    pub use crate::framebuffer_size;
}
#[path = "../../nanami/servers/apps/alter/src/cli.rs"]
mod cli;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/framebuffer_info.rs"]
mod framebuffer_info;

type Word = usize;
const LINUX_FB_FIX_SCREENINFO_BYTES: Word = 80;
const LINUX_FB_VAR_SCREENINFO_BYTES: Word = 160;
fn write_u32(base: Word, value: u32) {
    unsafe { (base as *mut u32).write_unaligned(value) }
}
fn write_u64(base: Word, value: u64) {
    unsafe { (base as *mut u64).write_unaligned(value) }
}
fn parse(args: &[&[u8]]) -> Result<cli::Launch, ()> {
    cli::parse_cli(args.len(), |index| args.get(index).copied())
}

#[test]
fn cli_size_is_not_passed_to_the_guest() {
    let launch = parse(&[
        b"alter",
        b"-g",
        b"--fb-size",
        b"640x400",
        b"doom",
        b"-iwad",
        b"doom1.wad",
    ])
    .unwrap();
    assert!(launch.graphics);
    assert_eq!(
        launch.framebuffer_size,
        framebuffer_size::FramebufferSize::new(640, 400)
    );
    assert_eq!((launch.first_arg, launch.argc), (4, 3));
}

#[test]
fn existing_cli_and_os_selection_are_unchanged() {
    for args in [
        &[b"alter".as_slice(), b"-g", b"doom"][..],
        &[b"alter".as_slice(), b"-t", b"-d", b"-os", b"linux", b"doom"][..],
        &[b"alter".as_slice(), b"linux", b"doom"][..],
    ] {
        let launch = parse(args).unwrap();
        assert!(launch.framebuffer_size.is_none());
        assert_eq!(launch.argc, 1);
        assert_eq!(launch.os.service_name(), "alter-linux");
    }
    let launch = parse(&[b"alter", b"-os", b"freebsd", b"hello"]).unwrap();
    assert_eq!(launch.os.service_name(), "alter-freebsd");
    let launch = parse(&[b"alter", b"doom", b"--fb-size", b"640x400"]).unwrap();
    assert_eq!((launch.first_arg, launch.argc), (1, 3));
    assert!(launch.framebuffer_size.is_none());
}

#[test]
fn cli_rejects_missing_duplicate_or_non_graphics_size() {
    for args in [
        &[][..],
        &[b"alter".as_slice()][..],
        &[b"alter".as_slice(), b"-g", b"--fb-size"][..],
        &[b"alter".as_slice(), b"-g", b"--fb-size", b"640x400"][..],
        &[b"alter".as_slice(), b"--fb-size", b"640x400", b"doom"][..],
        &[
            b"alter".as_slice(),
            b"-g",
            b"--fb-size",
            b"640x400",
            b"--fb-size",
            b"800x600",
            b"doom",
        ][..],
    ] {
        assert!(parse(args).is_err());
    }
    assert!(parse(&[b"alter", b"--fb-size", b"640x400", b"--graphics", b"doom"]).is_ok());
}

#[test]
fn dimensions_reject_invalid_text_and_abi_overflow() {
    use framebuffer_size::FramebufferSize as Size;
    for text in [
        "",
        "640",
        "x400",
        "640x",
        "0x400",
        "640x0",
        "-1x400",
        "+1x400",
        "640X400",
        "640x400x2",
        "640x 400",
        "1x2147483648",
        "2147483648x1",
        "65536x65536",
        "184467440737095516160x400",
    ] {
        assert!(Size::parse(text.as_bytes()).is_none(), "{text}");
    }
    assert!(Size::new(usize::MAX, 1).is_none());
    assert!(Size::from_packed(640).is_none());
    assert!(Size::from_packed(400usize << 32).is_none());
    assert!(Size::from_packed((65536usize << 32) | 65536).is_none());
}

#[test]
fn size_round_trip_and_lengths() {
    use framebuffer_size::FramebufferSize as Size;
    assert_eq!(Size::DEFAULT, Size::new(800, 600).unwrap());
    for (width, height) in [(1, 1), (640, 400), (641, 401), (800, 600), (4096, 2160)] {
        let size = Size::new(width, height).unwrap();
        assert_eq!(Size::from_packed(size.packed()), Some(size));
        assert_eq!(size.stride(), width * 4);
        assert_eq!(size.bytes(), width * height * 4);
    }
}

#[test]
fn screeninfo_reports_session_geometry_without_touching_canaries() {
    fn word(bytes: &[u8], offset: usize) -> u32 {
        u32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }
    for (width, height) in [(640, 400), (641, 401), (800, 600), (1912, 1048)] {
        let size = framebuffer_size::FramebufferSize::new(width, height).unwrap();
        let mut fix = [0xa5; 82];
        let mut var = [0xa5; 162];
        framebuffer_info::write_fb_fix_screeninfo(fix.as_mut_ptr() as usize + 1, size);
        framebuffer_info::write_fb_var_screeninfo(var.as_mut_ptr() as usize + 1, size);
        assert_eq!(
            (fix[0], fix[81], var[0], var[161]),
            (0xa5, 0xa5, 0xa5, 0xa5)
        );
        let (fix, var) = (&fix[1..81], &var[1..161]);
        assert_eq!(word(fix, 24) as usize, size.bytes());
        assert_eq!(word(fix, 48) as usize, size.stride());
        assert_eq!(word(fix, 36), 2); // truecolor
        for offset in [0, 8] {
            assert_eq!(word(var, offset) as usize, width);
        }
        for offset in [4, 12] {
            assert_eq!(word(var, offset) as usize, height);
        }
        assert_eq!(word(var, 24), 32);
        for (offset, value) in [
            (32, 16),
            (36, 8),
            (44, 8),
            (48, 8),
            (56, 0),
            (60, 8),
            (68, 24),
            (72, 8),
        ] {
            assert_eq!(word(var, offset), value);
        }
    }
}
