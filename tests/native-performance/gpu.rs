#![allow(dead_code, unused_attributes)]
extern crate self as nanami_runtime;
pub type Word = usize;
pub use nanami_uapi::RequestError;
pub const OS_RESPONSE_OK: Word = 0;
pub const OS_RESPONSE_PERMISSION_DENIED: Word = 2;
#[path = "../../nanami/servers/libs/nanami-client/src/gpu.rs"]
mod client;
#[path = "gpu/mock.rs"]
mod mock;
pub fn request_shared_memory(peer: Word, bytes: Word) -> Result<(Word, Word), RequestError> {
    mock::shared(peer, bytes)
}
pub fn request_mapping_release(address: Word, bytes: Word) -> Result<(), RequestError> {
    assert!(mock::release_if_owned(address, bytes));
    Ok(())
}
fn call_port(
    _: Word,
    code: Word,
    a: Word,
    b: Word,
    c: Word,
    d: Word,
    words: u8,
) -> Result<(Word, Word, Word), RequestError> {
    mock::call(code, [a, b, c, d], words)
}
use client::*;
fn surface() -> Surface {
    create(
        20,
        Layout::new(80, 40, 384).unwrap(),
        info(20).unwrap().accelerated,
    )
    .unwrap()
}
fn rect(x: Word, y: Word, width: Word, height: Word) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

#[test]
fn production_client_and_provider_fill_copy_and_present_with_padded_rows() {
    mock::reset();
    assert_eq!(
        info(20).unwrap(),
        Info {
            supported: CAP_ALL,
            accelerated: 0
        }
    );
    let source = surface();
    let target = surface();
    fill(&source, rect(2, 3, 7, 9), 0xaabbccdd).unwrap();
    copy(&source, &target, rect(2, 3, 7, 9), 11, 12).unwrap();
    present(&target, rect(11, 12, 7, 9)).unwrap();
    let pixels = unsafe {
        std::slice::from_raw_parts(target.address as *const u32, target.layout.bytes / 4)
    };
    for y in 0..40 {
        for x in 0..96 {
            let expected = if (12..21).contains(&y) && (11..18).contains(&x) {
                0xaabbccdd
            } else {
                0
            };
            assert_eq!(pixels[y * 96 + x], expected);
        }
    }
    destroy(&source).unwrap();
    destroy(&target).unwrap();
    assert!(fill(&target, rect(0, 0, 1, 1), 1).is_err());
}

#[test]
fn handles_are_owner_scoped_and_destroyed_identifiers_never_alias_new_buffers() {
    mock::reset();
    let first = surface();
    mock::caller(43);
    assert_eq!(
        fill(&first, rect(0, 0, 1, 1), 1),
        Err(RequestError::Status(2))
    );
    assert_eq!(destroy(&first), Err(RequestError::Status(2)));
    mock::caller(42);
    destroy(&first).unwrap();
    let second = surface();
    assert_ne!(first.id, second.id);
    assert!(present(&first, rect(0, 0, 1, 1)).is_err());
    destroy(&second).unwrap();
}

#[test]
fn allocation_pressure_retires_dead_clients_without_reusing_stale_handles() {
    mock::reset();
    let old: Vec<_> = (0..16).map(|_| surface()).collect();
    mock::FAKE.with(|fake| {
        fake.borrow_mut().dead_owners.insert(42);
    });
    mock::caller(43);
    let new = surface();
    assert!(old.iter().all(|surface| surface.id != new.id));
    assert!(fill(&old[0], rect(0, 0, 1, 1), 0).is_err());
    destroy(&new).unwrap();
}

#[test]
fn malformed_rectangles_formats_and_overlapping_copy_have_no_side_effects() {
    mock::reset();
    let buffer = surface();
    for invalid in [
        rect(80, 0, 1, 1),
        rect(0, 40, 1, 1),
        rect(0, 0, 0, 1),
        rect(79, 0, 2, 1),
    ] {
        assert!(fill(&buffer, invalid, 3).is_err());
    }
    assert!(copy(&buffer, &buffer, rect(0, 0, 4, 4), 1, 1).is_err());
    assert_eq!(
        mock::call(CREATE, [10 | (10 << 32), 64, 99, 0], 4)
            .unwrap()
            .0,
        4
    );
    mock::FAKE.with(|fake| assert_eq!((fake.borrow().fills, fake.borrow().copies), (0, 0)));
    destroy(&buffer).unwrap();
}

#[test]
fn buffer_limits_and_uncertain_destroy_retire_handles_without_reuse() {
    mock::reset();
    let buffers: Vec<_> = (0..16).map(|_| surface()).collect();
    assert!(create(20, buffers[0].layout, 0).is_err());
    destroy(&buffers[0]).unwrap();
    let replacement = surface();
    assert_ne!(replacement.id, buffers[0].id);
    mock::FAKE.with(|fake| fake.borrow_mut().fail_release = true);
    assert!(destroy(&replacement).is_err());
    assert!(fill(&replacement, rect(0, 0, 1, 1), 1).is_err());
    mock::FAKE.with(|fake| fake.borrow_mut().fail_release = false);
    for buffer in &buffers[1..] {
        destroy(buffer).unwrap();
    }
}

#[test]
fn layout_and_capability_decoders_reject_overflow_reserved_bits_and_false_acceleration() {
    for dimensions in [
        (0, 1, 64),
        (1, 0, 64),
        (8193, 1, 0),
        (1, 8193, 0),
        (1, 1, 63),
        (8192, 1, 0),
        (1, 1, usize::MAX),
    ] {
        assert!(Layout::new(dimensions.0, dimensions.1, dimensions.2).is_none());
    }
    assert!(Info::decode(1 | (1 << 16), 2 | (1 << 16)).is_none());
    assert!(Info::decode(2 | (7 << 16), 1 << 16).is_none());
    assert!(Info::decode(1 | (0xff << 16), 1 << 16).is_none());
}
