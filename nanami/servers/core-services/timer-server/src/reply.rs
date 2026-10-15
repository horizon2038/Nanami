//! Diagnostic IPC-path comparison; the ordinary path is unchanged.

use nanami_runtime::{ipc, RequestError, Word};

use super::init_trace::Trace;

pub(super) fn receive_after_reply(
    trace: &mut Trace,
    port: Word,
    status: Word,
    detail0: Word,
    detail1: Word,
) -> Result<ipc::ServiceEvent, RequestError> {
    if !trace.reply_enter(status) {
        return ipc::service_reply_receive_event(port, status, detail0, detail1);
    }

    // No retries, timer changes or acknowledgement changes. Do not consume a
    // deferred notification here; the existing Receive helper delivers it.
    let result = ipc::service_reply(port, status, detail0, detail1);
    nanami_runtime::println!(
        "[timer.init] stage=reply-split.returned ok={}",
        result.is_ok()
    );
    result?;

    // This marks entry to the helper, not necessarily a blocking kernel
    // Receive: a notification saved during a nested RPC can return first.
    nanami_runtime::println!("[timer.init] stage=receive-enter path=split");
    ipc::service_receive_event(port)
}
