//! Production session/group policy and tty ioctl marshaling, with guest-memory mocks.
#![allow(dead_code)]
type Word = usize;
mod state {
    pub const LINUX_FD_MAX: usize = 64;
    pub use crate::control::JobControlError;
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/constants.rs"]
mod constants;
use constants::*;
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/job_control.rs"]
mod control;
use control::{JobControlError, ProcessJobControl, TerminalControl};
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/job_control.rs"]
mod ioctl_policy;

#[derive(Clone, Copy)]
struct Rootfs {
    leader: Word,
}
#[derive(Clone, Copy)]
struct ManagedProcess {
    pid: Word,
    parent_pid: Word,
    job: ProcessJobControl,
    rootfs: Rootfs,
    terminal_id: Word,
    exited: bool,
}
struct Runtime {
    managed: Vec<ManagedProcess>,
    terminal_controls: [TerminalControl; 8],
    posix_shm: Word,
    scratch: Box<[u8; 8]>,
    fail_memory: bool,
}
impl Runtime {
    fn new() -> Self {
        let mut scratch = Box::new([0; 8]);
        let posix_shm = scratch.as_mut_ptr() as Word;
        Self {
            managed: Vec::new(),
            terminal_controls: [TerminalControl::EMPTY; 8],
            posix_shm,
            scratch,
            fail_memory: false,
        }
    }
    fn managed_process(&self, pid: Word) -> Option<&ManagedProcess> {
        self.managed.iter().find(|process| process.pid == pid)
    }
    fn managed_process_mut(&mut self, pid: Word) -> Option<&mut ManagedProcess> {
        self.managed.iter_mut().find(|process| process.pid == pid)
    }
    fn add(&mut self, pid: Word, parent: Word, terminal: Word) {
        let leader = self
            .managed_process(parent)
            .map_or(pid, |parent| parent.rootfs.leader);
        self.managed.push(ManagedProcess {
            pid,
            parent_pid: parent,
            terminal_id: terminal,
            exited: false,
            rootfs: Rootfs { leader },
            job: ProcessJobControl::EMPTY,
        });
        self.initialize_job_control(pid, parent);
    }
}
fn read_target_memory(
    runtime: &mut Runtime,
    _: Word,
    address: Word,
    bytes: Word,
) -> Result<(), i32> {
    if runtime.fail_memory || address == 0 {
        return Err(EFAULT);
    }
    assert_eq!(bytes, 4);
    unsafe {
        core::ptr::copy_nonoverlapping(address as *const u8, runtime.posix_shm as *mut u8, bytes);
    }
    Ok(())
}
fn write_u32_to_target(
    runtime: &mut Runtime,
    _: Word,
    address: Word,
    value: u32,
) -> Result<(), i32> {
    if runtime.fail_memory || address == 0 {
        return Err(EFAULT);
    }
    unsafe {
        core::ptr::write_unaligned(address as *mut u32, value);
    }
    Ok(())
}
fn ioctl(runtime: &mut Runtime, pid: Word, request: Word, argument: Word) -> Result<Word, i32> {
    ioctl_policy::terminal_ioctl(runtime, pid, request, argument).unwrap()
}

#[test]
fn groups_inherit_and_can_be_changed_without_confusing_pids_with_pgids() {
    let mut runtime = Runtime::new();
    runtime.add(10, 0, 1);
    runtime.add(11, 10, 1);
    runtime.add(12, 10, 1);
    assert_eq!(runtime.process_group(11, 0), Ok(10));
    assert_eq!(runtime.process_session(11, 0), Ok(10));
    assert_eq!(runtime.set_process_group(10, 11, 0), Ok(()));
    assert_eq!(runtime.set_process_group(10, 12, 11), Ok(()));
    assert_eq!(runtime.process_group(10, 12), Ok(11));
    runtime.managed_process_mut(12).unwrap().job.executed = true;
    assert_eq!(
        runtime.set_process_group(10, 12, 12),
        Err(JobControlError::ExecutedChild)
    );
    assert_eq!(runtime.set_process_group(12, 0, 0), Ok(()));
}

#[test]
fn invalid_or_cross_session_group_changes_do_not_mutate_state() {
    let mut runtime = Runtime::new();
    runtime.add(10, 0, 1);
    runtime.add(11, 10, 1);
    runtime.add(12, 11, 1);
    runtime.add(20, 0, 2);
    assert_eq!(
        runtime.set_process_group(10, 10, 10),
        Err(JobControlError::Permission)
    );
    assert_eq!(
        runtime.set_process_group(10, 11, Word::MAX),
        Err(JobControlError::InvalidArgument)
    );
    assert_eq!(
        runtime.set_process_group(10, 11, 99),
        Err(JobControlError::Permission)
    );
    assert_eq!(
        runtime.set_process_group(10, 12, 12),
        Err(JobControlError::NoProcess)
    );
    assert_eq!(
        runtime.process_group(10, 20),
        Err(JobControlError::NoProcess)
    );
    assert_eq!(runtime.process_group(11, 0), Ok(10));
    assert_eq!(runtime.create_session(11), Ok(11));
    assert_eq!(
        runtime.set_process_group(10, 11, 10),
        Err(JobControlError::Permission)
    );
}

#[test]
fn terminal_foreground_is_shared_and_uses_exactly_a_linux_pid_t() {
    let mut runtime = Runtime::new();
    runtime.add(10, 0, 1);
    runtime.add(11, 10, 1);
    runtime.set_process_group(10, 11, 11).unwrap();
    let mut output = [0xaaaaaaaa_u32; 2];
    assert_eq!(
        ioctl(
            &mut runtime,
            11,
            LINUX_TIOCGPGRP,
            output.as_mut_ptr() as Word
        ),
        Ok(0)
    );
    assert_eq!(output, [10, 0xaaaaaaaa]);
    let group = 11_i32;
    assert_eq!(
        ioctl(
            &mut runtime,
            10,
            LINUX_TIOCSPGRP,
            &group as *const _ as Word
        ),
        Ok(0)
    );
    ioctl(
        &mut runtime,
        11,
        LINUX_TIOCGPGRP,
        output.as_mut_ptr() as Word,
    )
    .unwrap();
    assert_eq!(output, [11, 0xaaaaaaaa]);
    ioctl(
        &mut runtime,
        11,
        LINUX_TIOCGSID,
        output.as_mut_ptr() as Word,
    )
    .unwrap();
    assert_eq!(output, [10, 0xaaaaaaaa]);
}

#[test]
fn invalid_groups_and_faulty_buffers_do_not_change_the_terminal() {
    let mut runtime = Runtime::new();
    runtime.add(10, 0, 1);
    runtime.add(20, 0, 2);
    for (group, error) in [(-1_i32, EINVAL), (0, EPERM), (20, EPERM), (99, EPERM)] {
        assert_eq!(
            ioctl(
                &mut runtime,
                10,
                LINUX_TIOCSPGRP,
                &group as *const _ as Word
            ),
            Err(error)
        );
    }
    assert_eq!(ioctl(&mut runtime, 10, LINUX_TIOCGPGRP, 0), Err(EFAULT));
    let group = 10_i32;
    runtime.fail_memory = true;
    assert_eq!(
        ioctl(
            &mut runtime,
            10,
            LINUX_TIOCSPGRP,
            &group as *const _ as Word
        ),
        Err(EFAULT)
    );
    assert_eq!(runtime.controlling_terminal(10, 1).unwrap().foreground, 10);
}

#[test]
fn setsid_detaches_and_controlling_terminals_cannot_be_stolen() {
    let mut runtime = Runtime::new();
    runtime.add(10, 0, 1);
    runtime.add(11, 10, 1);
    assert_eq!(runtime.create_session(10), Err(JobControlError::Permission));
    assert_eq!(runtime.create_session(11), Ok(11));
    let mut output = 0u32;
    assert_eq!(
        ioctl(
            &mut runtime,
            11,
            LINUX_TIOCGPGRP,
            &mut output as *mut _ as Word
        ),
        Err(ENOTTY)
    );
    assert_eq!(ioctl(&mut runtime, 11, LINUX_TIOCSCTTY, 1), Err(EPERM));
    assert_eq!(ioctl(&mut runtime, 10, LINUX_TIOCNOTTY, 0), Ok(0));
    assert_eq!(ioctl(&mut runtime, 11, LINUX_TIOCSCTTY, 0), Ok(0));
    assert_eq!(ioctl(&mut runtime, 11, LINUX_TIOCSCTTY, 0), Ok(0));
    assert_eq!(runtime.controlling_terminal(11, 1).unwrap().session, 11);
    assert_eq!(
        ioctl(
            &mut runtime,
            10,
            LINUX_TIOCGSID,
            &mut output as *mut _ as Word
        ),
        Err(ENOTTY)
    );
    runtime.release_session_terminal(11);
    assert!(runtime.controlling_terminal(11, 1).is_err());
    assert!(ioctl_policy::terminal_ioctl(&mut runtime, 10, Word::MAX, 0).is_none());
}
