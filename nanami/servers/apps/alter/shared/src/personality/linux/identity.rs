//! Identity is constant for the current POSIX connection; do not query it on
//! every Linux getter. File-access permission checks still belong to POSIX/VFS.
use crate::state::{IdentityField, Runtime};
use nanami_client::posix;
use nanami_runtime::{RequestError, Word};

pub(super) fn read(runtime: &mut Runtime, field: IdentityField) -> Result<Word, RequestError> {
    let port = runtime.posix_port;
    runtime
        .identity_cache
        .get_or_query(port, field, || match field {
            IdentityField::RealUser => posix::posix_getuid(port),
            IdentityField::EffectiveUser => posix::posix_geteuid(port),
            IdentityField::RealGroup => posix::posix_getgid(port),
            IdentityField::EffectiveGroup => posix::posix_getegid(port),
        })
}
