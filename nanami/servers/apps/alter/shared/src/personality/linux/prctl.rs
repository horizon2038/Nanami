//! Task metadata, without changing executable identity or granting privileges.
use super::*;

pub(super) fn setresid(args: [Word; 6]) -> Result<Word, i32> {
    // Native POSIX and OCI currently expose only uid/gid 0. Do not pretend a
    // privilege drop succeeded while VFS would still execute with root rights.
    if args[..3]
        .iter()
        .any(|id| !matches!(*id as u32, 0 | u32::MAX))
    {
        return Err(EOPNOTSUPP);
    }
    Ok(0)
}

pub(super) fn prctl(runtime: &mut Runtime, pid: Word, args: [Word; 6]) -> Result<Word, i32> {
    match args[0] as u32 {
        15 => {
            // PR_SET_NAME: 15 bytes plus terminating NUL, per thread.
            let mut name = [0; 16];
            let mut copied = 0;
            while copied < 15 {
                let address = args[1].checked_add(copied).ok_or(EFAULT)?;
                let chunk = (15 - copied).min(LINUX_PAGE_SIZE - address % LINUX_PAGE_SIZE);
                read_target_memory(runtime, pid, address, chunk)?;
                let source =
                    unsafe { core::slice::from_raw_parts(runtime.posix_shm as *const u8, chunk) };
                let bytes = source.iter().position(|byte| *byte == 0).unwrap_or(chunk);
                name[copied..copied + bytes].copy_from_slice(&source[..bytes]);
                if bytes != chunk {
                    break;
                }
                copied += bytes;
            }
            runtime.managed_process_mut(pid).ok_or(ESRCH)?.comm = name;
            Ok(0)
        }
        16 => {
            // PR_GET_NAME always copies TASK_COMM_LEN bytes.
            let name = runtime.managed_process(pid).ok_or(ESRCH)?.comm;
            copyout_from(runtime, pid, args[1], name.as_ptr() as Word, name.len())?;
            Ok(0)
        }
        38 | 39 => {
            // PR_SET/GET_NO_NEW_PRIVS, inherited and irreversible.
            if args[2..5].iter().any(|arg| *arg != 0) || (args[0] == 39 && args[1] != 0) {
                return Err(EINVAL);
            }
            let process = runtime.managed_process_mut(pid).ok_or(ESRCH)?;
            if args[0] == 39 {
                return Ok(process.no_new_privs as Word);
            }
            if args[1] != 1 {
                return Err(EINVAL);
            }
            process.no_new_privs = true;
            Ok(0)
        }
        21 => Ok(0), // PR_GET_SECCOMP: no filter installed; SET is not accepted.
        _ => Err(EINVAL),
    }
}
