use super::{
    map_request_error, Runtime, Word, ALTER_LAUNCH_MAX_ARGS, ALTER_LAUNCH_MAX_ENVS, EFAULT,
    ENAMETOOLONG, ENOMEM, LINUX_EXEC_SNAPSHOT_ALLOCATION_BYTES, LINUX_EXEC_SNAPSHOT_BYTES,
    LINUX_EXEC_STRING_MAX, LINUX_PAGE_SIZE,
};

pub(super) struct ExecStringSnapshot {
    pub(super) buffer: Word,
    pub(super) used: usize,
    pub(super) argc: usize,
    pub(super) envc: usize,
    pub(super) argv_offsets: [usize; ALTER_LAUNCH_MAX_ARGS],
    pub(super) argv_lens: [usize; ALTER_LAUNCH_MAX_ARGS],
    pub(super) env_offsets: [usize; ALTER_LAUNCH_MAX_ENVS],
    pub(super) env_lens: [usize; ALTER_LAUNCH_MAX_ENVS],
}

pub(super) struct ExecGuestPage {
    pub(super) buffer: Word,
    pub(super) pid: Word,
    pub(super) page: Word,
    pub(super) valid: bool,
}

impl ExecGuestPage {
    fn new(buffer: Word) -> Self {
        Self {
            buffer,
            pid: 0,
            page: 0,
            valid: false,
        }
    }

    fn ensure(&mut self, pid: Word, address: Word) -> Result<usize, i32> {
        let page = address & !(LINUX_PAGE_SIZE - 1);
        if !self.valid || self.pid != pid || self.page != page {
            libnanami::request_process_memory_read(pid, page, self.buffer, LINUX_PAGE_SIZE)
                .map_err(map_request_error)?;
            self.pid = pid;
            self.page = page;
            self.valid = true;
        }
        Ok((address - page) as usize)
    }

    fn copy(&mut self, pid: Word, address: Word, out: &mut [u8]) -> Result<(), i32> {
        let mut copied = 0usize;
        while copied < out.len() {
            let source = address.checked_add(copied as Word).ok_or(EFAULT)?;
            let page_offset = self.ensure(pid, source)?;
            let chunk =
                ::core::cmp::min(out.len() - copied, LINUX_PAGE_SIZE as usize - page_offset);
            unsafe {
                ::core::ptr::copy_nonoverlapping(
                    (self.buffer as usize + page_offset) as *const u8,
                    out.as_mut_ptr().add(copied),
                    chunk,
                );
            }
            copied += chunk;
        }
        Ok(())
    }

    fn read_word(&mut self, pid: Word, address: Word) -> Result<Word, i32> {
        let mut bytes = [0u8; ::core::mem::size_of::<Word>()];
        self.copy(pid, address, &mut bytes)?;
        Ok(Word::from_ne_bytes(bytes))
    }
}

pub(super) fn snapshot_exec_strings(
    runtime: &mut Runtime,
    pid: Word,
    argv_ptr: Word,
    envp_ptr: Word,
) -> Result<ExecStringSnapshot, i32> {
    if runtime.exec_snapshot_buffer == 0
        || runtime.exec_snapshot_buffer_size < LINUX_EXEC_SNAPSHOT_ALLOCATION_BYTES as Word
    {
        let (buffer, mapped) =
            libnanami::request_heap(LINUX_EXEC_SNAPSHOT_ALLOCATION_BYTES as Word)
                .map_err(map_request_error)?;
        if mapped < LINUX_EXEC_SNAPSHOT_ALLOCATION_BYTES as Word {
            return Err(ENOMEM);
        }
        runtime.exec_snapshot_buffer = buffer;
        runtime.exec_snapshot_buffer_size = mapped;
    }

    let mut guest_page =
        ExecGuestPage::new(runtime.exec_snapshot_buffer + LINUX_EXEC_SNAPSHOT_BYTES as Word);
    let (argv, argc) = read_guest_pointer_array_args(&mut guest_page, pid, argv_ptr)?;
    let (envp, envc) = read_guest_pointer_array_envs(&mut guest_page, pid, envp_ptr)?;

    let mut snapshot = ExecStringSnapshot {
        buffer: runtime.exec_snapshot_buffer,
        used: 0,
        argc,
        envc,
        argv_offsets: [0; ALTER_LAUNCH_MAX_ARGS],
        argv_lens: [0; ALTER_LAUNCH_MAX_ARGS],
        env_offsets: [0; ALTER_LAUNCH_MAX_ENVS],
        env_lens: [0; ALTER_LAUNCH_MAX_ENVS],
    };
    let mut i = 0usize;
    while i < argc {
        let (offset, len) = snapshot_guest_string(
            &mut guest_page,
            pid,
            runtime.exec_snapshot_buffer,
            snapshot.used,
            argv[i],
        )?;
        snapshot.argv_offsets[i] = offset;
        snapshot.argv_lens[i] = len;
        snapshot.used = offset + len;
        i += 1;
    }
    i = 0;
    while i < envc {
        let (offset, len) = snapshot_guest_string(
            &mut guest_page,
            pid,
            runtime.exec_snapshot_buffer,
            snapshot.used,
            envp[i],
        )?;
        snapshot.env_offsets[i] = offset;
        snapshot.env_lens[i] = len;
        snapshot.used = offset + len;
        i += 1;
    }
    Ok(snapshot)
}

pub(super) fn snapshot_guest_string(
    guest_page: &mut ExecGuestPage,
    pid: Word,
    buffer: Word,
    offset: usize,
    user_ptr: Word,
) -> Result<(usize, usize), i32> {
    if user_ptr == 0 {
        return Err(EFAULT);
    }
    let mut copied = 0usize;
    while copied < LINUX_EXEC_STRING_MAX {
        let source = user_ptr.checked_add(copied as Word).ok_or(EFAULT)?;
        let page_offset = guest_page.ensure(pid, source)?;
        let page_bytes = ::core::cmp::min(
            LINUX_EXEC_STRING_MAX - copied,
            LINUX_PAGE_SIZE as usize - page_offset,
        );
        let page = unsafe {
            ::core::slice::from_raw_parts(
                (guest_page.buffer as usize + page_offset) as *const u8,
                page_bytes,
            )
        };
        let chunk = page
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(page_bytes);
        let end = offset
            .checked_add(copied)
            .and_then(|value| value.checked_add(chunk))
            .filter(|end| *end <= LINUX_EXEC_SNAPSHOT_BYTES)
            .ok_or(ENOMEM)?;
        unsafe {
            ::core::ptr::copy_nonoverlapping(
                page.as_ptr(),
                (buffer as usize + offset + copied) as *mut u8,
                chunk,
            );
        }
        copied += chunk;
        if chunk < page_bytes {
            return Ok((offset, end - offset));
        }
    }
    Err(ENAMETOOLONG)
}

pub(super) fn read_guest_pointer_array_args(
    guest_page: &mut ExecGuestPage,
    pid: Word,
    array_ptr: Word,
) -> Result<([Word; ALTER_LAUNCH_MAX_ARGS], usize), i32> {
    let mut out = [0; ALTER_LAUNCH_MAX_ARGS];
    let count = read_guest_pointer_array(guest_page, pid, array_ptr, &mut out)?;
    Ok((out, count))
}

pub(super) fn read_guest_pointer_array_envs(
    guest_page: &mut ExecGuestPage,
    pid: Word,
    array_ptr: Word,
) -> Result<([Word; ALTER_LAUNCH_MAX_ENVS], usize), i32> {
    let mut out = [0; ALTER_LAUNCH_MAX_ENVS];
    let count = read_guest_pointer_array(guest_page, pid, array_ptr, &mut out)?;
    Ok((out, count))
}

pub(super) fn read_guest_pointer_array(
    guest_page: &mut ExecGuestPage,
    pid: Word,
    array_ptr: Word,
    out: &mut [Word],
) -> Result<usize, i32> {
    if array_ptr == 0 {
        return Ok(0);
    }
    let mut count = 0usize;
    while count < out.len() {
        let source = array_ptr
            .checked_add((count as Word).checked_mul(8).ok_or(EFAULT)?)
            .ok_or(EFAULT)?;
        let value = guest_page.read_word(pid, source)?;
        if value == 0 {
            return Ok(count);
        }
        out[count] = value;
        count += 1;
    }
    Ok(count)
}
