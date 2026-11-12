use super::*;

impl Runtime {
    pub fn posix_write_buffer(&self) -> (Word, Word) {
        if self.posix_direct_shm != 0 && self.posix_direct_shm_size != 0 {
            (self.posix_direct_shm, self.posix_direct_shm_size.min(crate::abi::ALTER_DEFAULT_SHM_BYTES))
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
                nanami_client::posix::posix_pwrite_direct(self.posix_port, fd, input, len, offset)
            }
            (true, None) => {
                nanami_client::posix::posix_write_direct(self.posix_port, fd, input, len)
            }
            (false, Some(offset)) => {
                nanami_client::posix::posix_pwrite(self.posix_port, fd, input, len, offset)
            }
            (false, None) => nanami_client::posix::posix_write(self.posix_port, fd, input, len),
        }
    }

    pub fn posix_read_buffer_size(&self) -> Word {
        if self.posix_direct_shm != 0 && self.posix_direct_shm_size != 0 {
            self.posix_direct_shm_size.min(crate::abi::ALTER_DEFAULT_SHM_BYTES)
        } else {
            self.posix_shm_size
        }
    }

    /// Only file snapshots/fresh private mmap population may use the entire
    /// delegated window. Ordinary read/write calls retain their old quantum.
    pub fn posix_image_read_buffer_size(&self) -> Word {
        if self.posix_direct_shm != 0 && self.posix_direct_shm_size != 0 {
            self.posix_direct_shm_size
        } else { self.posix_shm_size }
    }

    pub fn pread_versioned_image(
        &self, fd: Word, size: Word, identity: Word,
    ) -> Result<Option<Word>, RequestError> {
        if self.posix_direct_shm == 0 || size == 0 || size > self.posix_direct_shm_size
            || identity >> 32 == 0
        {
            return Ok(None);
        }
        let direct = self.file_reader.versioned(fd, size, identity);
        let result = direct.and_then(Result::ok).map(Ok).unwrap_or_else(||
            nanami_client::posix::posix_pread_versioned(self.posix_port, fd, size, identity));
        match result {
            Ok((bytes, verified)) if bytes == size && verified == identity => Ok(Some(self.posix_direct_shm)),
            Ok(_) => Err(RequestError::Protocol),
            // A failed positioned read did not advance the open description.
            // Retry through the ordinary read and final fstat path.
            Err(_) => Ok(None),
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
            if let Some(result) = self.file_reader.read(fd, out_offset, len) {
                match result {
                    Ok(bytes) => return Ok((bytes, self.posix_direct_shm + out_offset)),
                    Err(error) if !cursor_request_rejected(&error) => return Err(error),
                    Err(_) => {},
                }
            }
            match nanami_client::posix::posix_read_direct(self.posix_port, fd, out_offset, len) {
                Ok(bytes) => return Ok((bytes, self.posix_direct_shm + out_offset)),
                Err(error) if self.file_reader.cursor_for(fd)
                    && !cursor_request_rejected(&error) => return Err(error),
                // The delegated path is an optimization. A transient IPC or backing-store
                // failure must not turn an otherwise valid Linux read into EIO.
                Err(_) => {}
            }
        }
        let bytes = nanami_client::posix::posix_read(self.posix_port, fd, out_offset, len)?;
        Ok((bytes, self.posix_shm + out_offset))
    }

    pub fn seek_posix(&self, fd: Word, offset: Word, whence: Word) -> Result<Word, RequestError> {
        if let Some(result) = self.file_reader.seek(fd, offset, whence) {
            match result {
                Ok(position) => return Ok(position),
                Err(error) if !cursor_request_rejected(&error) => return Err(error),
                Err(_) => {},
            }
        }
        nanami_client::posix::posix_seek(self.posix_port, fd, offset, whence)
    }

    pub fn pread_posix(
        &self,
        fd: Word,
        out: Word,
        len: Word,
        offset: Word,
    ) -> Result<(Word, Word), RequestError> {
        let mut fallback_len = len;
        if self.posix_direct_shm != 0
            && out
                .checked_add(len)
                .filter(|end| *end <= self.posix_direct_shm_size)
                .is_some()
        {
            let result = self.file_reader.pread(fd, out, len, offset)
                .and_then(Result::ok).map(Ok).unwrap_or_else(||
                    nanami_client::posix::posix_pread_direct(self.posix_port, fd, out, len, offset));
            if let Ok(bytes) = result {
                return Ok((bytes, self.posix_direct_shm + out));
            }
            // A large file-population request must fit the smaller control
            // window on fallback. The caller resumes successful short reads;
            // the same positioned offset makes a failed direct read safe to
            // retry without advancing the descriptor or using partial bytes.
            fallback_len = self.posix_shm_size.checked_sub(out)
                .map(|capacity| len.min(capacity))
                .filter(|&bytes| bytes != 0 || len == 0)
                .ok_or(RequestError::InvalidArgument)?;
        }
        let bytes = nanami_client::posix::posix_pread(self.posix_port, fd, out, fallback_len, offset)?;
        Ok((bytes, self.posix_shm + out))
    }
}

/// These replies reject a cursor operation without committing it. Transport,
/// protocol and fatal proxy errors can hide a committed VFS read/seek and must
/// never trigger an automatic replay through another route.
pub fn cursor_request_rejected(error: &RequestError) -> bool {
    matches!(error, RequestError::Unsupported | RequestError::Status(
        nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT
        | nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR
        | nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION))
}
