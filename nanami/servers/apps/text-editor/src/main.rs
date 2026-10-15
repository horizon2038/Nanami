#![no_std]
#![no_main]
#![feature(alloc_error_handler)]

extern crate alloc;

mod app;
mod document;
mod input;
mod layout;
mod render;
mod storage;

use app::{Editor, Pending};
use nanami_client::{gfx::honoka::*, input::*};
use nanami_runtime::{ipc, NanamiError, RequestError, Word};

const SLOT_HONOKA: Word = 22;
const SLOT_VFS: Word = 24;

nanami_runtime::nanami_entry!(nanami_main);

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    nanami_runtime::println!("[text-editor] panic: {}", info);
    let _ = nanami_runtime::request_exit();
    loop {
        core::hint::spin_loop();
    }
}

#[alloc_error_handler]
fn alloc_error(layout: core::alloc::Layout) -> ! {
    nanami_runtime::println!("[text-editor] out of memory: {} bytes", layout.size());
    let _ = nanami_runtime::request_exit();
    loop {
        core::hint::spin_loop();
    }
}

fn nanami_main() -> nanami_runtime::NanamiResult {
    ipc::init_ipc_tls().map_err(failed)?;
    nanami_runtime::heap::init_heap(9 * 1024 * 1024).map_err(failed)?;
    let notification = ipc::process_slot_descriptor(nanami_runtime::PROCESS_SLOT_NOTIFICATION);
    ipc::bind_current_thread_notification(notification).map_err(failed)?;
    nanami_client::registry::connect_honoka_service_with_pid(SLOT_HONOKA).map_err(failed)?;
    let port = ipc::process_slot_descriptor(SLOT_HONOKA);
    let mut files = storage::Storage::connect(SLOT_VFS).map_err(failed)?;
    let mut renderer = render::Renderer::new();
    let mut editor = Editor::new(840, 536);
    if let Some(argument) = nanami_runtime::process_arg(1) {
        match core::str::from_utf8(argument) {
            Ok(path) => {
                editor.open(path, &mut files);
            }
            Err(_) => {
                editor.message(alloc::string::String::from("File path is not UTF-8"), true);
            }
        }
    }
    let id = honoka_create_window_with_title(port, 220, 120, 840, 536, b"Text editor")
        .map_err(failed)?;
    let result = run(
        port,
        id,
        notification,
        &mut editor,
        &mut renderer,
        &mut files,
    );
    // Also release compositor resources when a later IPC setup/receive fails.
    let _ = honoka_destroy_window(port, id);
    result
}

fn run(
    port: Word,
    id: Word,
    notification: Word,
    editor: &mut Editor,
    renderer: &mut render::Renderer,
    files: &mut storage::Storage,
) -> nanami_runtime::NanamiResult {
    let mut surface = WindowSurface::attach(port, id).map_err(failed)?;
    let (input_base, _) = honoka_attach_input_queue(port, id).map_err(failed)?;
    honoka_attach_input_notification(port, id).map_err(failed)?;
    let mut queue = WindowEventQueue::new(input_base);
    let mut keyboard = input::Keyboard::default();
    let mut pointer = (0, 0);
    let mut last_title = alloc::string::String::new();
    let mut changed = true;
    editor.resize(surface.width, surface.height);
    nanami_runtime::println!("[text-editor] online window={}", id);
    loop {
        for _ in 0..256 {
            let Some(packed) = queue.pop() else {
                break;
            };
            let (kind, code, value0, value1, _) = unpack_input_event(packed);
            match kind {
                INPUT_EVENT_KIND_KEY => {
                    if let Some(key) = keyboard.event(code as usize, value0 != 0) {
                        editor.key(key, keyboard.shifted(), files);
                        changed = true;
                    }
                }
                INPUT_EVENT_KIND_MOUSE_MOVE => {
                    pointer = (value0.max(0) as usize, value1.max(0) as usize);
                    let old_cursor = editor.document.cursor;
                    editor.pointer_move(pointer.0, pointer.1);
                    changed |= editor.document.cursor != old_cursor;
                }
                INPUT_EVENT_KIND_MOUSE_BUTTON if code == 1 => {
                    editor.pointer_button(
                        pointer.0,
                        pointer.1,
                        value0 != 0,
                        keyboard.shifted(),
                        files,
                    );
                    changed = true;
                }
                INPUT_EVENT_KIND_MOUSE_WHEEL => {
                    editor.wheel(value0);
                    changed = true;
                }
                INPUT_EVENT_KIND_WINDOW_CLOSE => {
                    editor.request(Pending::Close);
                    changed = true;
                }
                INPUT_EVENT_KIND_WINDOW_RESIZE => {
                    if surface
                        .resize(port, id, value0 as u16 as usize, value1 as u16 as usize)
                        .map_err(failed)?
                    {
                        editor.resize(surface.width, surface.height);
                        nanami_runtime::println!(
                            "[text-editor] resized width={} height={}",
                            surface.width,
                            surface.height
                        );
                        changed = true;
                    }
                }
                _ => {}
            }
            if editor.closed {
                return Ok(());
            }
        }
        if changed {
            let title = editor.title(HONOKA_WINDOW_TITLE_BYTES);
            if title != last_title {
                honoka_set_window_title(port, id, title.as_bytes()).map_err(failed)?;
                last_title = title;
            }
            let pixels = unsafe {
                core::slice::from_raw_parts_mut(
                    surface.pixels() as *mut u32,
                    surface.pixel_bytes() / 4,
                )
            };
            renderer.draw(pixels, editor);
            // Input uses the shared queue; invalidate once per drained batch.
            // There is no periodic repaint timer or idle polling.
            honoka_invalidate_logical_framebuffer(
                port,
                id,
                0,
                0,
                surface.width as Word,
                surface.height as Word,
            )
            .map_err(failed)?;
            changed = false;
        }
        if queue.is_empty() {
            ipc::notification_wait(notification).map_err(failed)?;
        }
    }
}

fn failed(error: RequestError) -> NanamiError {
    nanami_runtime::println!("[text-editor] IPC failed: {:?}", error);
    error.into()
}
