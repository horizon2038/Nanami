use super::*;

impl Runtime {
    pub fn posix_write_buffer(&self) -> (Word, Word) {
        if self.posix_direct_shm != 0 && self.posix_direct_shm_size != 0 {
            (self.posix_direct_shm, self.posix_direct_shm_size)
        } else {
            (
                self.posix_shm,
                self.posix_shm_size
                    .saturating_sub(crate::abi::ALTER_IO_OFFSET),
            )
        }
    }

    pub fn write_posix(
        &self,
        fd: Word,
        input: Word,
        len: Word,
        offset: Option<Word>,
    ) -> Result<Word, RequestError> {
        let (_, capacity) = self.posix_write_buffer();
        if input
            .checked_add(len)
            .filter(|end| *end <= capacity)
            .is_none()
        {
            return Err(RequestError::InvalidArgument);
        }
        let direct = self.posix_direct_shm != 0 && self.posix_direct_shm_size != 0;
        // Do not retry a failed write through another route: it may have committed data.
        match (direct, offset) {
            (true, Some(offset)) => {
                nanami_services::posix::posix_pwrite_direct(self.posix_port, fd, input, len, offset)
            }
            (true, None) => {
                nanami_services::posix::posix_write_direct(self.posix_port, fd, input, len)
            }
            (false, Some(offset)) => {
                nanami_services::posix::posix_pwrite(self.posix_port, fd, input, len, offset)
            }
            (false, None) => nanami_services::posix::posix_write(self.posix_port, fd, input, len),
        }
    }

    pub fn posix_read_buffer_size(&self) -> Word {
        if self.posix_direct_shm != 0 && self.posix_direct_shm_size != 0 {
            self.posix_direct_shm_size
        } else {
            self.posix_shm_size
        }
    }

    pub fn read_posix(
        &self,
        fd: Word,
        out_offset: Word,
        len: Word,
    ) -> Result<(Word, Word), RequestError> {
        if self.posix_direct_shm != 0
            && out_offset
                .checked_add(len)
                .filter(|end| *end <= self.posix_direct_shm_size)
                .is_some()
        {
            match nanami_services::posix::posix_read_direct(self.posix_port, fd, out_offset, len) {
                Ok(bytes) => return Ok((bytes, self.posix_direct_shm + out_offset)),
                // The delegated path is an optimization. A transient IPC or backing-store
                // failure must not turn an otherwise valid Linux read into EIO.
                Err(_) => {}
            }
        }
        let bytes = nanami_services::posix::posix_read(self.posix_port, fd, out_offset, len)?;
        Ok((bytes, self.posix_shm + out_offset))
    }

    pub fn pread_posix(
        &self,
        fd: Word,
        out: Word,
        len: Word,
        offset: Word,
    ) -> Result<(Word, Word), RequestError> {
        if self.posix_direct_shm != 0
            && out
                .checked_add(len)
                .filter(|end| *end <= self.posix_direct_shm_size)
                .is_some()
        {
            if let Ok(bytes) =
                nanami_services::posix::posix_pread_direct(self.posix_port, fd, out, len, offset)
            {
                return Ok((bytes, self.posix_direct_shm + out));
            }
        }
        let bytes = nanami_services::posix::posix_pread(self.posix_port, fd, out, len, offset)?;
        Ok((bytes, self.posix_shm + out))
    }
}
