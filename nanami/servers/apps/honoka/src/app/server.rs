use libnanami::ipc::ServiceRequest;
use libnanami::{RequestError, Word};

use crate::compositor::Compositor;

pub fn handle_request(request: ServiceRequest, compositor: &mut Compositor) -> (Word, Word, Word) {
    match request.code {
        nanami_services::gfx::honoka::HONOKA_REQUEST_CREATE_WINDOW => {
            match compositor.create_window(
                request.identifier,
                request.arg0 as i32,
                request.arg1 as i32,
                request.arg2 as i32,
                request.arg3 as i32,
            ) {
                Ok(window_id) => (libnanami::OS_RESPONSE_OK, window_id, 0),
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        nanami_services::gfx::honoka::HONOKA_REQUEST_CREATE_WINDOW_WITH_TITLE => {
            let x = (request.arg0 & 0xffff_ffff) as u32 as i32;
            let y = ((request.arg0 >> 32) & 0xffff_ffff) as u32 as i32;
            let width = (request.arg1 & 0xffff_ffff) as u32 as i32;
            let height = ((request.arg1 >> 32) & 0xffff_ffff) as u32 as i32;
            match compositor.create_window_with_title(
                request.identifier,
                x,
                y,
                width,
                height,
                request.arg2,
                request.arg3,
            ) {
                Ok(window_id) => (libnanami::OS_RESPONSE_OK, window_id, 0),
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        nanami_services::gfx::honoka::HONOKA_REQUEST_ATTACH_LOGICAL_FRAMEBUFFER => {
            match compositor.attach_logical_framebuffer(request.identifier, request.arg0) {
                Ok((peer_vaddr, size_bytes)) => (libnanami::OS_RESPONSE_OK, peer_vaddr, size_bytes),
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        nanami_services::gfx::honoka::HONOKA_REQUEST_GET_WINDOW_CONTENT_SIZE => {
            match compositor.window_content_size(request.identifier, request.arg0) {
                Ok((width, height)) => (
                    libnanami::OS_RESPONSE_OK,
                    (width & 0xffff_ffff) | ((height & 0xffff_ffff) << 32),
                    0,
                ),
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        nanami_services::gfx::honoka::HONOKA_REQUEST_MOVE_WINDOW => {
            match compositor.move_window(
                request.identifier,
                request.arg0,
                request.arg1 as i32,
                request.arg2 as i32,
            ) {
                Ok(()) => (libnanami::OS_RESPONSE_OK, 0, 0),
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        nanami_services::gfx::honoka::HONOKA_REQUEST_SET_WINDOW_VISIBLE => {
            match compositor.set_window_visible(request.identifier, request.arg0, request.arg1 != 0)
            {
                Ok(()) => (libnanami::OS_RESPONSE_OK, 0, 0),
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        nanami_services::gfx::honoka::HONOKA_REQUEST_ATTACH_INPUT_QUEUE => {
            match compositor.attach_input_queue(request.identifier, request.arg0) {
                Ok((peer_vaddr, size_bytes)) => (libnanami::OS_RESPONSE_OK, peer_vaddr, size_bytes),
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        nanami_services::gfx::honoka::HONOKA_REQUEST_ATTACH_INPUT_NOTIFICATION => {
            match compositor.attach_input_notification(request.identifier, request.arg0) {
                Ok(()) => (libnanami::OS_RESPONSE_OK, 0, 0),
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        nanami_services::gfx::honoka::HONOKA_REQUEST_SET_WINDOW_TITLE => {
            match compositor.set_window_title(
                request.identifier,
                request.arg0,
                request.arg1,
                request.arg2,
                request.arg3,
            ) {
                Ok(()) => (libnanami::OS_RESPONSE_OK, 0, 0),
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        nanami_services::gfx::honoka::HONOKA_REQUEST_INVALIDATE_LOGICAL_FRAMEBUFFER => {
            let x = (request.arg1 & 0xffff_ffff) as u32 as i32;
            let y = ((request.arg1 >> 32) & 0xffff_ffff) as u32 as i32;
            let width = (request.arg2 & 0xffff_ffff) as u32 as i32;
            let height = ((request.arg2 >> 32) & 0xffff_ffff) as u32 as i32;
            match compositor.invalidate_logical_framebuffer(
                request.identifier,
                request.arg0,
                x,
                y,
                width,
                height,
            ) {
                Ok(()) => (libnanami::OS_RESPONSE_OK, 0, 0),
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn map_request_error_to_status(err: RequestError) -> Word {
    match err {
        RequestError::InvalidArgument => libnanami::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Unsupported => libnanami::OS_RESPONSE_ILLEGAL_OPERATION,
        RequestError::Transport | RequestError::Protocol => libnanami::OS_RESPONSE_FATAL,
        RequestError::Status(code) => code,
    }
}
