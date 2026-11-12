use super::*;

impl Runtime {
    pub fn with_file_reader<T>(&mut self, file: LinuxFile, operation: impl FnOnce(&mut Self) -> T) -> T {
        let token = file.posix_read_grant();
        let reader = if token != 0 {
            let port = if self.posix_direct_shm != 0 {
                self.rooted_channel.port(self.posix_port).unwrap_or(0)
            } else { 0 };
            // Retain cursor ownership even when the direct port is missing:
            // an uncertain POSIX proxy read must still not be replayed.
            super::super::file_reader::Reader::new(file.posix_fd, token, port)
                .with_cursor(file.posix_read_cursor())
        } else { super::super::file_reader::Reader::EMPTY };
        let previous = core::mem::replace(&mut self.file_reader, reader);
        // Dispatch is synchronous and cannot close/reuse this FD or
        // dispatch another guest syscall. Restore even after a returned error.
        let result = operation(self);
        self.file_reader = previous;
        result
    }

    pub fn with_file_reader_operation<T>(
        &mut self, file: LinuxFile, pid: Word, operation: &str,
        read: impl FnOnce(&mut Self) -> T,
    ) -> T {
        if option_env!("NANAMI_POSITIONED_IO_DIAG") == Some("1") || option_env!("NANAMI_FILE_IO_DIAG") == Some("1") {
            nanami_runtime::println!("[linux.read-scope] begin={} pid={} fd={}", operation, pid, file.posix_fd);
        }
        let result = self.with_file_reader(file, read);
        if option_env!("NANAMI_POSITIONED_IO_DIAG") == Some("1") || option_env!("NANAMI_FILE_IO_DIAG") == Some("1") {
            nanami_runtime::println!("[linux.read-scope] end={} pid={} fd={}", operation, pid, file.posix_fd);
        }
        result
    }

    pub fn with_cursor_file_reader<T>(
        &mut self, file: LinuxFile, pid: Word, operation: &str,
        read: impl FnOnce(&mut Self) -> T,
    ) -> T {
        // Positioned-only grants need no new channel just for a legacy
        // sequential read/seek. Also prevent inheriting an outer cursor grant.
        let selected = if file.posix_read_cursor() { file } else { file.with_posix_read_grant(0) };
        self.with_file_reader_operation(selected, pid, operation, read)
    }
}
