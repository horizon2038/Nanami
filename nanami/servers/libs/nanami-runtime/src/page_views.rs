use super::{call_os_port, RequestError, Word};
use nanami_uapi::os::{OsPageViewEntry, OS_PAGE_VIEW_VECTOR_MAX, OS_REQUEST_PROCESS_READ_VIEWS,
    OS_REQUEST_PROCESS_WRITE_VIEWS, OS_REQUEST_MAPPING_RELEASE_VIEWS, OS_RESPONSE_OK};

/// A bit acknowledges exactly one completed release. On an error, keep all
/// requested VAs unready: the remote side may already have released any subset.
pub fn request_mapping_release_views(locals: &[Word]) -> Result<Word, RequestError> {
    if locals.is_empty() || locals.len() > OS_PAGE_VIEW_VECTOR_MAX
        || locals.iter().enumerate().any(|(index, &local)| local == 0 || local & 4095 != 0
            || local.checked_add(4096).is_none() || locals[..index].contains(&local)) {
        return Err(RequestError::InvalidArgument);
    }
    let (status, acknowledged, count) = call_os_port(OS_REQUEST_MAPPING_RELEASE_VIEWS,
        locals.as_ptr() as Word, locals.len(), 0, 0, 5)?;
    if status != OS_RESPONSE_OK { return Err(RequestError::Status(status)); }
    if count != locals.len() || acknowledged >> locals.len() != 0 {
        return Err(RequestError::Protocol);
    }
    Ok(acknowledged)
}

/// Prepare only the supplied pages. On OK, a nonzero failure status belongs
/// to entry `completed`; later entries were not attempted. On a transport or
/// malformed-response error, conservatively retain every requested local VA.
pub fn request_process_read_views(pid: Word, entries: &[OsPageViewEntry])
    -> Result<(Word, Word), RequestError>
{
    request_process_page_views(OS_REQUEST_PROCESS_READ_VIEWS, pid, entries)
}

pub fn request_process_write_views(pid: Word, entries: &[OsPageViewEntry])
    -> Result<(Word, Word), RequestError>
{
    request_process_page_views(OS_REQUEST_PROCESS_WRITE_VIEWS, pid, entries)
}

fn request_process_page_views(code: Word, pid: Word, entries: &[OsPageViewEntry])
    -> Result<(Word, Word), RequestError>
{
    if pid == 0 || entries.is_empty() || entries.len() > OS_PAGE_VIEW_VECTOR_MAX
        || entries.iter().any(|entry| entry.target == 0 || entry.local == 0
            || (entry.target | entry.local) & 4095 != 0
            || entry.target.checked_add(4096).is_none() || entry.local.checked_add(4096).is_none())
    {
        return Err(RequestError::InvalidArgument);
    }
    let (status, completed, failure) = call_os_port(code, pid, entries.as_ptr() as Word, entries.len(), 0, 5)?;
    if status != OS_RESPONSE_OK { return Err(RequestError::Status(status)); }
    if completed > entries.len() || (completed == entries.len()) != (failure == 0) {
        return Err(RequestError::Protocol);
    }
    Ok((completed, failure))
}

