#![no_std]
#![no_main]
#![feature(alloc_error_handler)]
extern crate alloc;
mod model;
mod view;
use alloc::string::ToString;
use model::{Monitor, Sample};
use nanami_client::timer::*;
use nanami_runtime::{ipc, RequestError};
use nanami_ui::{
    window::{Event, Window},
    Font, Key, Keyboard,
};

nanami_runtime::nanami_entry!(main);
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    nanami_runtime::println!("[performance-monitor] panic: {}", info);
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
    let mut window = Window::new("Performance", 180, 120, 760, 600).map_err(failed)?;
    let result = run(&mut window, &font);
    window.close();
    result.map_err(failed)
}
fn run(window: &mut Window, font: &Font) -> Result<(), RequestError> {
    nanami_client::registry::connect_timer_service(24)?;
    let timer = ipc::process_slot_descriptor(24);
    let mut version = [0; 160];
    let kernel = nanami_runtime::request_kernel_version(&mut version)
        .unwrap_or("Unavailable")
        .to_string();
    let os = nanami_runtime::request_nanami_version(&mut version)
        .unwrap_or("Unavailable")
        .to_string();
    let platform = nanami_runtime::request_nanami_info_platform().ok();
    let system = view::System {
        kernel,
        os,
        architecture: platform
            .map(|p| p.architecture_name().to_string())
            .unwrap_or_else(|| "Unavailable".into()),
        platform: platform
            .map(|p| p.platform_name().to_string())
            .unwrap_or_else(|| "Unavailable".into()),
    };
    let mut model = Monitor::new();
    let mut keyboard = Keyboard::default();
    let mut pointer = (0, 0);
    let mut next = 0usize;
    let mut dirty = true;
    let mut timer_wake = true;
    let mut rearm = true;
    nanami_runtime::println!("[performance-monitor] online");
    let result = (|| loop {
        for _ in 0..256 {
            let Some(event) = window.event()? else {
                break;
            };
            match event {
                Event::Close => return Ok(()),
                Event::Key(code, pressed) => {
                    if let Some(key) = keyboard.event(code, pressed) {
                        if matches!(key, Key::Control('w' | 'q')) {
                            return Ok(());
                        }
                        let before = (model.paused, model.interval);
                        dirty |= model.key(key);
                        rearm |= before != (model.paused, model.interval);
                    }
                }
                Event::Move(x, y) => {
                    pointer = (x, y);
                }
                Event::Button(1, true) => {
                    let before = (model.paused, model.interval);
                    dirty |= model.click(pointer.0, pointer.1);
                    rearm |= before != (model.paused, model.interval);
                }
                Event::Resize(w, h) => {
                    nanami_runtime::println!(
                        "[performance-monitor] resized width={} height={}",
                        w,
                        h
                    );
                    dirty = true;
                }
                _ => {}
            }
        }
        if model.refresh || rearm || timer_wake {
            let (now, hz) = timer_service_monotonic_ticks(timer)?;
            if hz == 0 {
                return Err(RequestError::Protocol);
            }
            if model.refresh || (!model.paused && !rearm && now >= next) {
                let memory = nanami_runtime::request_nanami_info_memory()
                    .ok()
                    .map(|m| (m.total_bytes as u64, m.free_bytes as u64));
                let processes = nanami_runtime::request_nanami_info_process()
                    .ok()
                    .map(|p| (p.running as u64, p.exited as u64));
                let cores = nanami_runtime::request_nanami_info_smp()
                    .ok()
                    .map(|s| (s.online_cores, s.active_core_mask as u64));
                model.history.push(Sample {
                    at_ms: ((now as u128 * 1000) / hz as u128).min(u64::MAX as u128) as u64,
                    memory,
                    processes,
                    cores,
                });
                model.refresh = false;
                dirty = true;
                rearm = true;
            }
            if rearm {
                next = now.saturating_add(
                    ((hz as u128 * model.interval_ms() as u128) / 1000)
                        .max(1)
                        .min(usize::MAX as u128) as usize,
                );
                timer_service_set_alarm_ticks(
                    timer,
                    nanami_runtime::PROCESS_SLOT_NOTIFICATION,
                    if model.paused { None } else { Some(next) },
                )?;
                rearm = false;
            }
        }
        if dirty {
            window.paint(|canvas| view::draw(canvas, font, &model, &system))?;
            dirty = false;
        }
        timer_wake = window.wait()? & TIMER_NOTIFICATION_IDENTIFIER_BIT != 0;
    })();
    let _ = timer_service_set_alarm_ticks(timer, nanami_runtime::PROCESS_SLOT_NOTIFICATION, None);
    result
}
fn failed(error: RequestError) -> nanami_runtime::NanamiError {
    nanami_runtime::println!("[performance-monitor] error: {}", error);
    error.into()
}
