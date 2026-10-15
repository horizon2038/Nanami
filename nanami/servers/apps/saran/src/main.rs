#![no_std]
#![no_main]
#![feature(alloc_error_handler)]
extern crate alloc;
mod app;
mod bitmap;
mod document;
mod view;
use nanami_runtime::{ipc, RequestError};
use nanami_ui::{
    files::Storage,
    window::{Event, Window},
    Font, Keyboard,
};

nanami_runtime::nanami_entry!(main);
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    nanami_runtime::println!("[Saran] panic: {}", info);
    let _ = nanami_runtime::request_exit();
    loop {
        core::hint::spin_loop();
    }
}
#[alloc_error_handler]
fn oom(_: core::alloc::Layout) -> ! {
    panic!("out of memory");
}
fn main() -> nanami_runtime::NanamiResult {
    ipc::init_ipc_tls().map_err(failed)?;
    nanami_runtime::heap::init_heap(9 * 1024 * 1024).map_err(failed)?;
    let font = Font::new();
    let mut files = Storage::connect(bitmap::MAX_FILE).map_err(failed)?;
    let mut paint = app::Paint::new(912, 620);
    if let Some(argument) = nanami_runtime::process_arg(1) {
        match core::str::from_utf8(argument) {
            Ok(path) => {
                paint.open(path, &mut files);
            }
            Err(_) => {
                paint.message("File path is not UTF-8".into(), true);
            }
        }
    }
    let mut window = Window::new("Saran", 90, 78, 912, 620).map_err(failed)?;
    let result = run(&mut window, &font, &mut paint, &mut files);
    window.close();
    result.map_err(failed)
}
fn run(
    window: &mut Window,
    font: &Font,
    paint: &mut app::Paint,
    files: &mut Storage,
) -> Result<(), RequestError> {
    let (w, h) = window.size();
    paint.resize_window(w, h);
    let mut keyboard = Keyboard::default();
    let mut pointer = (0, 0);
    let mut dirty = true;
    let mut title = alloc::string::String::new();
    nanami_runtime::println!(
        "[Saran] online canvas={}x{}",
        paint.document.image.width,
        paint.document.image.height
    );
    loop {
        for _ in 0..256 {
            let Some(event) = window.event()? else {
                break;
            };
            match event {
                Event::Key(code, pressed) => {
                    if let Some(key) = keyboard.event(code, pressed) {
                        paint.key(key, keyboard.shift(), files);
                        dirty = true;
                    }
                }
                Event::Move(x, y) => {
                    pointer = (x, y);
                    dirty |= paint.pointer_move(x, y);
                }
                Event::Button(button, pressed) => {
                    paint.pointer_button(pointer.0, pointer.1, button, pressed, files);
                    dirty = true;
                }
                Event::Wheel(delta) => {
                    paint.wheel(delta, keyboard.control(), keyboard.shift());
                    dirty = true;
                }
                Event::Close => {
                    paint.request(app::Pending::Close);
                    dirty = true;
                }
                Event::Resize(w, h) => {
                    paint.resize_window(w, h);
                    dirty = true;
                    nanami_runtime::println!(
                        "[Saran] resized width={} height={} canvas={}x{}",
                        w,
                        h,
                        paint.document.image.width,
                        paint.document.image.height
                    );
                }
            }
            if paint.closed {
                return Ok(());
            }
        }
        if dirty {
            let next = paint.title();
            if next != title {
                window.title(&next)?;
                title = next;
            }
            window.paint(|canvas| view::draw(canvas, font, paint))?;
            dirty = false;
        }
        window.wait()?;
    }
}
fn failed(error: RequestError) -> nanami_runtime::NanamiError {
    nanami_runtime::println!("[Saran] error: {}", error);
    error.into()
}
