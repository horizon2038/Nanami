//! Real sysinfo encoder and copyout, with deterministic accounting providers.
#![allow(dead_code)]
extern crate self as nanami_runtime;
use std::cell::RefCell;
type Word = usize;
const EFAULT: i32 = 14;
const EIO: i32 = 5;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/sysinfo.rs"]
mod sysinfo;
struct Runtime {
    monotonic_ticks: Word,
    monotonic_tick_hz: Word,
    writable: bool,
    copy_fails: bool,
}
impl Runtime {
    fn new() -> Self {
        PROVIDER.with(|p| *p.borrow_mut() = Provider::default());
        Self {
            monotonic_ticks: 12001,
            monotonic_tick_hz: 1000,
            writable: true,
            copy_fails: false,
        }
    }
    fn memory_access_allowed(&self, _: Word, pointer: Word, bytes: Word, rights: Word) -> bool {
        self.writable && pointer >= 4096 && pointer.checked_add(bytes).is_some() && rights == 2
    }
}
#[derive(Clone, Copy)]
pub struct NanamiMemoryInfo {
    pub total_bytes: Word,
    pub free_bytes: Word,
}
#[derive(Clone, Copy)]
pub struct NanamiProcessInfo {
    pub running: Word,
    pub exited: Word,
}
struct Provider {
    calls: usize,
    memory: Result<NanamiMemoryInfo, i32>,
    processes: Result<NanamiProcessInfo, i32>,
}
impl Default for Provider {
    fn default() -> Self {
        Self {
            calls: 0,
            memory: Ok(NanamiMemoryInfo {
                total_bytes: 8 << 30,
                free_bytes: 3 << 30,
            }),
            processes: Ok(NanamiProcessInfo {
                running: 42,
                exited: 7,
            }),
        }
    }
}
thread_local! { static PROVIDER: RefCell<Provider> = RefCell::new(Provider::default()); }
pub fn request_nanami_info_memory() -> Result<NanamiMemoryInfo, i32> {
    PROVIDER.with(|p| {
        let mut p = p.borrow_mut();
        p.calls += 1;
        p.memory
    })
}
pub fn request_nanami_info_process() -> Result<NanamiProcessInfo, i32> {
    PROVIDER.with(|p| {
        let mut p = p.borrow_mut();
        p.calls += 1;
        p.processes
    })
}
fn map_request_error(error: i32) -> i32 {
    error
}
mod clocks {
    use super::*;
    pub fn refresh_clock(runtime: &mut Runtime) -> Result<(), i32> {
        if runtime.monotonic_tick_hz == 0 {
            return Err(EIO);
        }
        Ok(())
    }
}
fn copyout_from(
    runtime: &Runtime,
    _: Word,
    output: Word,
    input: Word,
    bytes: Word,
) -> Result<(), i32> {
    if runtime.copy_fails {
        return Err(EFAULT);
    }
    unsafe {
        std::ptr::copy_nonoverlapping(input as *const u8, output as *mut u8, bytes);
    }
    Ok(())
}
fn field(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}
#[test]
fn real_accounting_has_the_112_byte_linux_layout_and_zero_padding() {
    let mut runtime = Runtime::new();
    let mut output = [0xa5u8; 120];
    assert_eq!(
        sysinfo::sysinfo(&mut runtime, 1, output.as_mut_ptr() as Word),
        Ok(0)
    );
    assert_eq!(field(&output, 0), 13);
    assert_eq!(field(&output, 32), 8 << 30);
    assert_eq!(field(&output, 40), 3 << 30);
    assert_eq!(u16::from_le_bytes(output[80..82].try_into().unwrap()), 42);
    assert_eq!(&output[104..108], &1u32.to_le_bytes());
    for range in [8..32, 48..80, 82..104, 108..112] {
        assert!(output[range].iter().all(|b| *b == 0));
    }
    assert_eq!(&output[112..], &[0xa5; 8]);
}
#[test]
fn uptime_rounding_zero_and_large_process_counts_are_bounded() {
    let mut runtime = Runtime::new();
    PROVIDER.with(|p| {
        p.borrow_mut().processes = Ok(NanamiProcessInfo {
            running: 70000,
            exited: 0,
        })
    });
    let mut bytes = [0u8; 112];
    for (ticks, seconds) in [
        (0, 0),
        (12000, 12),
        (12001, 13),
        (Word::MAX, Word::MAX / 1000 + 1),
    ] {
        runtime.monotonic_ticks = ticks;
        assert_eq!(
            sysinfo::sysinfo(&mut runtime, 1, bytes.as_mut_ptr() as Word),
            Ok(0)
        );
        assert_eq!(field(&bytes, 0), seconds as u64);
        assert_eq!(
            u16::from_le_bytes(bytes[80..82].try_into().unwrap()),
            u16::MAX
        );
    }
}
#[test]
fn invalid_or_readonly_outputs_fail_before_accounting_and_transport_errors_propagate() {
    let mut runtime = Runtime::new();
    assert_eq!(sysinfo::sysinfo(&mut runtime, 1, 0), Err(EFAULT));
    assert_eq!(
        sysinfo::sysinfo(&mut runtime, 1, Word::MAX - 100),
        Err(EFAULT)
    );
    let mut output = [0xa5u8; 112];
    runtime.writable = false;
    assert_eq!(
        sysinfo::sysinfo(&mut runtime, 1, output.as_mut_ptr() as Word),
        Err(EFAULT)
    );
    PROVIDER.with(|p| assert_eq!(p.borrow().calls, 0));
    runtime.writable = true;
    PROVIDER.with(|p| p.borrow_mut().memory = Err(EIO));
    assert_eq!(
        sysinfo::sysinfo(&mut runtime, 1, output.as_mut_ptr() as Word),
        Err(EIO)
    );
    assert_eq!(output, [0xa5; 112]);
    let mut runtime = Runtime::new();
    runtime.copy_fails = true;
    assert_eq!(
        sysinfo::sysinfo(&mut runtime, 1, output.as_mut_ptr() as Word),
        Err(EFAULT)
    );
    runtime.monotonic_tick_hz = 0;
    assert_eq!(
        sysinfo::sysinfo(&mut runtime, 1, output.as_mut_ptr() as Word),
        Err(EIO)
    );
}
