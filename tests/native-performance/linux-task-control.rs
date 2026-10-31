//! Production clone3 decoding / prctl; only guest-memory copying and fork are mocked.
#![allow(dead_code)]
type Word = usize;
const EINVAL: i32 = 22;
const EFAULT: i32 = 14;
const ESRCH: i32 = 3;
const ENOSYS: i32 = 38;
const EOPNOTSUPP: i32 = 95;
const LINUX_PAGE_SIZE: usize = 4096;
const LINUX_PROT_WRITE: usize = 2;
const SYS_CLONE: usize = 56;
#[derive(Clone, Copy, Debug)]
struct LinuxSyscallContext {
    number: Word,
    args: [Word; 6],
    program_counter: Word,
}
struct Process {
    comm: [u8; 16],
    no_new_privs: bool,
}
struct Runtime {
    process: Process,
    posix_shm: usize,
    scratch: Box<[u8; 4096]>,
    captured: Option<LinuxSyscallContext>,
}
impl Runtime {
    fn new() -> Self {
        let mut scratch = Box::new([0; 4096]);
        Self {
            process: Process {
                comm: [0; 16],
                no_new_privs: false,
            },
            posix_shm: scratch.as_mut_ptr() as usize,
            scratch,
            captured: None,
        }
    }
    fn managed_process(&self, pid: usize) -> Option<&Process> {
        (pid == 1).then_some(&self.process)
    }
    fn managed_process_mut(&mut self, pid: usize) -> Option<&mut Process> {
        (pid == 1).then_some(&mut self.process)
    }
    fn memory_access_allowed(&self, _: usize, _: usize, _: usize, _: usize) -> bool {
        true
    }
}
fn read_target_memory(
    runtime: &Runtime,
    _: usize,
    pointer: usize,
    bytes: usize,
) -> Result<(), i32> {
    if pointer < 4096 || bytes > 4096 {
        return Err(EFAULT);
    }
    unsafe {
        std::ptr::copy_nonoverlapping(pointer as *const u8, runtime.posix_shm as *mut u8, bytes);
    }
    Ok(())
}
fn read_shm_u8(runtime: &Runtime, offset: usize) -> u8 {
    runtime.scratch[offset]
}
fn copyout_from(
    _: &Runtime,
    _: usize,
    destination: usize,
    source: usize,
    bytes: usize,
) -> Result<(), i32> {
    if destination < 4096 {
        return Err(EFAULT);
    }
    unsafe {
        std::ptr::copy_nonoverlapping(source as *const u8, destination as *mut u8, bytes);
    }
    Ok(())
}
fn sys_fork(runtime: &mut Runtime, _: usize, context: LinuxSyscallContext) -> Result<usize, i32> {
    runtime.captured = Some(context);
    Ok(77)
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/clone3.rs"]
mod clone3;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/prctl.rs"]
mod prctl;

fn clone(runtime: &mut Runtime, words: &[u64; 12], bytes: usize) -> Result<usize, i32> {
    clone3::clone3(
        runtime,
        1,
        LinuxSyscallContext {
            number: 435,
            args: [words.as_ptr() as usize, bytes, 0, 0, 0, 0],
            program_counter: 0x123456,
        },
    )
}

#[test]
fn root_only_credentials_never_fake_a_privilege_drop() {
    assert_eq!(
        prctl::setresid([0, 0xffff_ffff, usize::MAX, 0, 0, 0]),
        Ok(0)
    );
    for index in 0..3 {
        let mut args = [0; 6];
        args[index] = 1000;
        assert_eq!(prctl::setresid(args), Err(EOPNOTSUPP));
    }
}
#[test]
fn comm_truncates_pads_and_preserves_the_old_name_on_fault() {
    let mut runtime = Runtime::new();
    let long = b"0123456789abcdefghi\0";
    assert_eq!(
        prctl::prctl(&mut runtime, 1, [15, long.as_ptr() as usize, 0, 0, 0, 0]),
        Ok(0)
    );
    assert_eq!(&runtime.process.comm, b"0123456789abcde\0");
    assert_eq!(
        prctl::prctl(&mut runtime, 1, [15, 1, 0, 0, 0, 0]),
        Err(EFAULT)
    );
    assert_eq!(&runtime.process.comm, b"0123456789abcde\0");
    let mut short = [0; 16];
    short[0] = b'x';
    assert_eq!(
        prctl::prctl(&mut runtime, 1, [15, short.as_ptr() as usize, 0, 0, 0, 0]),
        Ok(0)
    );
    let mut out = [0x55u8; 17];
    assert_eq!(
        prctl::prctl(&mut runtime, 1, [16, out.as_mut_ptr() as usize, 0, 0, 0, 0]),
        Ok(0)
    );
    assert_eq!(out[0], b'x');
    assert_eq!(&out[1..16], &[0; 15]);
    assert_eq!(out[16], 0x55);
}
#[test]
fn no_new_privs_is_irreversible_and_validates_every_unused_argument() {
    let mut runtime = Runtime::new();
    assert_eq!(prctl::prctl(&mut runtime, 1, [39, 0, 0, 0, 0, 0]), Ok(0));
    assert_eq!(prctl::prctl(&mut runtime, 1, [38, 1, 0, 0, 0, 0]), Ok(0));
    assert_eq!(
        prctl::prctl(&mut runtime, 1, [38, 0, 0, 0, 0, 0]),
        Err(EINVAL)
    );
    assert_eq!(prctl::prctl(&mut runtime, 1, [39, 0, 0, 0, 0, 0]), Ok(1));
    for index in 2..5 {
        let mut args = [38, 1, 0, 0, 0, 0];
        args[index] = 1;
        assert_eq!(prctl::prctl(&mut runtime, 1, args), Err(EINVAL));
    }
    assert_eq!(
        prctl::prctl(&mut runtime, 1, [22, 2, 0, 0, 0, 0]),
        Err(EINVAL)
    );
}
#[test]
fn clone3_accepts_all_struct_versions_and_zero_extended_future_fields() {
    let mut runtime = Runtime::new();
    let mut words = [0; 12];
    words[4] = 17;
    for bytes in [64, 80, 88, 96] {
        assert_eq!(clone(&mut runtime, &words, bytes), Ok(77));
        let saved = runtime.captured.unwrap();
        assert_eq!(saved.number, SYS_CLONE);
        assert_eq!(saved.program_counter, 0x123456);
        assert_eq!(saved.args, [17, 0, 0, 0, 0, 0]);
    }
}
#[test]
fn clone3_translates_stack_tls_and_tid_without_losing_the_resume_pc() {
    let mut runtime = Runtime::new();
    let mut words = [0; 12];
    words[0] = 0x100 | 0x80000 | 0x100000 | 0x1000000;
    words[2] = 0x123;
    words[3] = 0x456;
    words[4] = 17;
    words[5] = 0x10000;
    words[6] = 0x4000;
    words[7] = 0x789;
    assert_eq!(clone(&mut runtime, &words, 88), Ok(77));
    let saved = runtime.captured.unwrap();
    #[cfg(target_arch = "x86_64")]
    assert_eq!(
        saved.args,
        [words[0] as usize | 17, 0x14000, 0x456, 0x123, 0x789, 0]
    );
    #[cfg(target_arch = "aarch64")]
    assert_eq!(
        saved.args,
        [words[0] as usize | 17, 0x14000, 0x456, 0x789, 0x123, 0]
    );
    assert_eq!(saved.program_counter, 0x123456);
}
#[test]
fn clone3_rejects_small_large_and_unknown_nonzero_tails_before_fork() {
    let mut runtime = Runtime::new();
    let mut words = [0; 12];
    assert_eq!(clone(&mut runtime, &words, 63), Err(EINVAL));
    assert_eq!(clone(&mut runtime, &words, 4097), Err(7));
    words[11] = 1;
    assert_eq!(clone(&mut runtime, &words, 96), Err(7));
    assert!(runtime.captured.is_none());
}
#[test]
fn clone3_rejects_signal_bits_bad_stacks_and_unavailable_vm_vfork() {
    let mut runtime = Runtime::new();
    let mut words = [0; 12];
    words[0] = 17;
    assert_eq!(clone(&mut runtime, &words, 88), Err(EINVAL));
    words[0] = 0;
    words[5] = 0x10000;
    assert_eq!(clone(&mut runtime, &words, 88), Err(EINVAL));
    words[6] = u64::MAX;
    assert_eq!(clone(&mut runtime, &words, 88), Err(EINVAL));
    words[0] = 0x4100;
    assert_eq!(clone(&mut runtime, &words, 88), Err(ENOSYS));
    assert!(runtime.captured.is_none());
}
